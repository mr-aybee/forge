//! Agent certificate renewal: only the node's current certificate renews,
//! only once renewal is due, and the replaced certificate is revoked.
#![allow(clippy::unwrap_used)]

use std::time::Duration;

use forge_agent::enroll::{HostFacts, enroll};
use forge_agent::link::{Endpoint, Link};
use forge_agent::{AgentConfig, StatePaths, tls};
use forge_core::ServerId;
use forge_core::ports::{EnrollmentRepository, ServerRepository};
use forge_pki::{AGENT_CERT_VALIDITY, AgentCsr};
use forge_proto::messages::{Heartbeat, RenewRequest, RenewResponse};
use prost::Message;
use serde_json::json;

use crate::common::*;

struct Creds {
    key_pem: String,
    cert_pem: String,
    serial: String,
}

/// A certificate for `server` from the control plane's own CA, issued
/// `age` ago (so renewal can be due) and still valid.
fn issue(h: &Harness, server: &str, age: Duration) -> Creds {
    let csr = AgentCsr::generate("renewal-test").unwrap();
    let issued_at = h.app.state.clock.now().minus(age);
    let c = h.app.state.ca.issue_agent_certificate(&csr.csr_pem, server, issued_at, AGENT_CERT_VALIDITY).unwrap();

    Creds { key_pem: csr.key_pem, cert_pem: c.cert_pem, serial: c.serial_hex }
}

fn link(h: &Harness, c: &Creds) -> Link {
    let tls = tls::mutual(h.app.state.ca.cert_pem(), &c.cert_pem, &c.key_pem).unwrap();
    Link::new(Endpoint::parse(&h.agent_url).unwrap(), tls, Duration::from_secs(10))
}

async fn renew(h: &Harness, c: &Creds) -> anyhow::Result<RenewResponse> {
    let csr = AgentCsr::generate("renewal-test").unwrap();
    let body = RenewRequest { csr_pem: csr.csr_pem }.encode_to_vec();
    let raw = link(h, c).request(http::Method::POST, forge_proto::paths::RENEW, body).await?;

    Ok(RenewResponse::decode(raw.as_slice())?)
}

#[tokio::test]
async fn only_the_current_due_certificate_renews_and_the_old_one_is_revoked() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let (s, body) = admin.post("/api/v1/enrollment-tokens", json!({"ttl_minutes": 10})).await;
    assert_eq!(s, reqwest::StatusCode::CREATED, "{body}");
    let dir = tempfile::tempdir().unwrap();
    let paths = StatePaths::new(dir.path());
    let facts = HostFacts { hostname: "renew-01".into(), os: "linux".into(), arch: "x86_64".into() };
    let id =
        enroll(&paths, AgentConfig::new(h.agent_url.clone()), body["token"].as_str().unwrap(), facts).await.unwrap();
    let server_id: ServerId = id.server_id.parse().unwrap();
    let store = &h.app.state.store;

    // The fresh enrollment certificate is current but not due yet.
    let fresh = Creds {
        key_pem: std::fs::read_to_string(paths.key()).unwrap(),
        cert_pem: std::fs::read_to_string(paths.cert()).unwrap(),
        serial: store.get_server(server_id).await.unwrap().cert_serial.unwrap(),
    };
    let err = renew(&h, &fresh).await.unwrap_err();
    assert!(err.to_string().contains("403"), "{err}");

    // An older certificate of the same node (due, unrevoked, not current) is refused.
    let stale = issue(&h, &id.server_id, Duration::from_secs(20 * 3600));
    let err = renew(&h, &stale).await.unwrap_err();
    assert!(err.to_string().contains("403"), "{err}");

    // The current certificate, once due, renews.
    let due = issue(&h, &id.server_id, Duration::from_secs(20 * 3600));
    let not_after = h.app.state.clock.now().plus(Duration::from_secs(4 * 3600));
    store.set_certificate(server_id, &due.serial, not_after).await.unwrap();
    let resp = renew(&h, &due).await.unwrap();
    assert!(resp.cert_pem.contains("BEGIN CERTIFICATE"));

    // The new serial is current and the replaced one is revoked: it can
    // neither renew again nor heartbeat.
    let now_current = store.get_server(server_id).await.unwrap().cert_serial.unwrap();
    assert_ne!(now_current, due.serial);
    assert!(store.is_certificate_revoked(&due.serial).await.unwrap());
    let err = renew(&h, &due).await.unwrap_err();
    assert!(err.to_string().contains("403"), "{err}");
    let hb = Heartbeat { agent_version: "test".into(), ..Default::default() }.encode_to_vec();
    let err = link(&h, &due).request(http::Method::POST, forge_proto::paths::HEARTBEAT, hb).await.unwrap_err();
    assert!(err.to_string().contains("403"), "{err}");

    // Denials and the renewal are audited.
    let (_, entries) = admin.get("/api/v1/audit?limit=200").await;
    let renewals: Vec<_> = entries.as_array().unwrap().iter().filter(|e| e["action"] == "server.cert.renew").collect();
    assert!(renewals.iter().any(|e| e["outcome"] == "denied"), "{entries}");
    assert!(renewals.iter().any(|e| e["outcome"] == "success"), "{entries}");
}
