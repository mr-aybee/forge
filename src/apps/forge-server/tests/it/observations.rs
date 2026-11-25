//! Observations API (D-085): current values, history, `health.view`
//! authorization and input validation.
#![allow(clippy::unwrap_used)]

use crate::common::*;
use forge_agent::enroll::{HostFacts, enroll};
use forge_agent::{AgentConfig, StatePaths};
use forge_core::observation::*;
use forge_core::{EntityKind, EntityRef, ServerId, Timestamp};
use reqwest::StatusCode;
use serde_json::json;

/// Creating users needs an MFA-verified session (#13 inventory).
async fn step_up(c: &mut Client) {
    use forge_identity::totp;
    let (_, t) = c.post("/api/v1/auth/totp", json!({})).await;
    let secret = totp::base32_decode(t["secret"].as_str().unwrap()).unwrap();
    let code = || {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        format!("{:06}", totp::code_at(&secret, i64::try_from(now).unwrap()))
    };
    let (s, _) =
        c.post("/api/v1/auth/totp/confirm", json!({"password": PASSWORD, "secret": t["secret"], "code": code()})).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, r) = c.post("/api/v1/auth/step-up", json!({"code": code()})).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    c.csrf = r["csrf_token"].as_str().map(str::to_owned);
}

async fn enroll_node(h: &Harness, admin: &Client) -> (ServerId, tempfile::TempDir) {
    let (s, body) = admin.post("/api/v1/enrollment-tokens", json!({"ttl_minutes": 10})).await;
    assert_eq!(s, StatusCode::CREATED, "{body}");
    let dir = tempfile::tempdir().unwrap();
    let facts = HostFacts { hostname: "obs-1".into(), os: "linux".into(), arch: "x86_64".into() };
    let id = enroll(
        &StatePaths::new(dir.path()),
        AgentConfig::new(h.agent_url.clone()),
        body["token"].as_str().unwrap(),
        facts,
    )
    .await
    .unwrap();

    (id.server_id.parse().unwrap(), dir)
}

fn obs(node: ServerId, subject: &str, state: HealthState, at: Timestamp) -> Observation {
    Observation {
        key: ObservationKey {
            resource: EntityRef::new(EntityKind::Server, node),
            server: None,
            subject: Subject::new(subject).unwrap(),
        },
        value: ObservedValue::Health(state),
        reason: (state == HealthState::Failing).then_some(ReasonCode::Timeout),
        detail: (state == HealthState::Failing).then(|| "connect timed out after 2s".to_owned()),
        source: ObservationSource::Probe { policy: "p1".into(), check: "web".into() },
        executor: None,
        generation: Some("g1".into()),
        observed_at: at,
        fresh_for_ms: 60_000,
    }
}

#[tokio::test]
async fn observations_are_listed_with_history_and_need_health_view() {
    let h = start().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let (node, _dir) = enroll_node(&h, &admin).await;
    let state = &h.app.state;
    let now = state.clock.now();
    let report = vec![
        obs(node, "health.readiness.web", HealthState::Passing, now),
        obs(node, "health.liveness.web", HealthState::Passing, now),
    ];
    let r = state.health.record_from_agent(node, report).await.unwrap();
    assert_eq!(r.transitions.len(), 2);
    let later = Timestamp(now.0 + 1);
    state
        .health
        .record_from_agent(node, vec![obs(node, "health.readiness.web", HealthState::Failing, later)])
        .await
        .unwrap();

    // Current values, effective on the server clock.
    let (s, b) = admin.get(&format!("/api/v1/observations?resource=server:{node}")).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    let list = b["observations"].as_array().unwrap();
    assert_eq!(list.len(), 2, "{b}");
    let ready = list.iter().find(|o| o["key"]["subject"] == "health.readiness.web").unwrap();
    assert_eq!(ready["effective"], json!({"type": "health", "state": "failing"}));
    assert_eq!(ready["reason"], "timeout");
    assert_eq!(ready["stale"], false);
    assert_eq!(ready["executor"], node.to_string(), "the reporting node is the executor");

    // Prefix filter and limit.
    let (_, b) = admin.get(&format!("/api/v1/observations?resource=server:{node}&subject_prefix=health.live")).await;
    assert_eq!(b["observations"].as_array().unwrap().len(), 1);
    let (_, b) = admin.get(&format!("/api/v1/observations?resource=server:{node}&limit=1")).await;
    assert_eq!(b["observations"].as_array().unwrap().len(), 1);

    // History of one check, newest first.
    let (s, b) =
        admin.get(&format!("/api/v1/observations/history?resource=server:{node}&subject=health.readiness.web")).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    let states: Vec<&str> =
        b["entries"].as_array().unwrap().iter().map(|e| e["value"]["state"].as_str().unwrap()).collect();
    assert_eq!(states, ["failing", "passing"]);

    // Input validation.
    for bad in [
        "/api/v1/observations?resource=job:x".to_owned(),
        "/api/v1/observations?resource=nonsense".to_owned(),
        format!("/api/v1/observations/history?resource=server:{node}&subject=Bad Subject"),
    ] {
        assert_eq!(admin.get(&bad).await.0, StatusCode::BAD_REQUEST, "{bad}");
    }

    // A user without a role on the node can't read them.
    step_up(&mut admin).await;
    let (s, b) = admin
        .post("/api/v1/users", json!({"email": "nobody@example.com", "display_name": "N", "password": PASSWORD}))
        .await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    let nobody = Client::login(&h.base, "nobody@example.com", PASSWORD).await;
    assert_eq!(nobody.get(&format!("/api/v1/observations?resource=server:{node}")).await.0, StatusCode::FORBIDDEN);

    // An auditor (read-only role) can.
    let auditor = user_with_role(&h, &admin, "aud@example.com", "auditor", "platform:root").await;
    assert_eq!(auditor.get(&format!("/api/v1/observations?resource=server:{node}")).await.0, StatusCode::OK);
}
