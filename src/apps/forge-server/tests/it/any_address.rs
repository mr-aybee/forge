//! Any-address console (D-167): links, same-origin checks, passkeys and
//! allowed hosts follow the address the client used, not one fixed name.
#![allow(clippy::unwrap_used)]

use crate::common::*;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

/// The port of the base URL (`http://127.0.0.1:<port>`).
fn port(base: &str) -> u16 {
    url::Url::parse(base).unwrap().port().unwrap()
}

/// A request to the harness with another `Host` header.
async fn send_as(
    c: &Client,
    method: Method,
    path: &str,
    host: &str,
    extra: &[(&str, &str)],
    body: Option<Value>,
) -> (StatusCode, String) {
    let mut req = c.http.request(method, format!("{}{path}", c.base)).header("host", host);

    if let Some(csrf) = &c.csrf {
        req = req.header("x-csrf-token", csrf);
    }

    for (k, v) in extra {
        req = req.header(*k, *v);
    }

    if let Some(b) = body {
        req = req.json(&b);
    }

    let r = req.send().await.unwrap();
    let s = r.status();

    (s, r.text().await.unwrap())
}

/// A WebSocket upgrade of the live-events socket (what a browser sends).
async fn events_as(c: &Client, host: &str, origin: Option<&str>) -> StatusCode {
    let mut h = vec![
        ("connection", "upgrade"),
        ("upgrade", "websocket"),
        ("sec-websocket-version", "13"),
        ("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ=="),
    ];

    if let Some(o) = origin {
        h.push(("origin", o));
    }

    send_as(c, Method::GET, "/api/v1/events", host, &h, None).await.0
}

#[tokio::test]
async fn install_script_names_the_agent_on_the_host_the_node_used() {
    let h = start().await;
    let anon = Client::anonymous(&h.base);
    let agent_port = forge_server::config::ServerConfig::default().agent_listen.port();
    let (s, body) = send_as(&anon, Method::GET, "/install.sh", "192.0.2.5:3880", &[], None).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert!(body.contains(&format!("AGENT_URL='https://192.0.2.5:{agent_port}'")), "{body}");
    assert!(body.contains("FORGE_URL='http://192.0.2.5:3880'"), "{body}");
    let (s, body) = send_as(&anon, Method::GET, "/install.ps1", "lab-a.example:3880", &[], None).await;
    assert_eq!(s, StatusCode::OK);
    assert!(body.contains(&format!("https://lab-a.example:{agent_port}")), "{body}");

    // A malformed Host is refused before anything renders.
    let (s, _) = send_as(&anon, Method::GET, "/install.sh", "a..b", &[], None).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _) = send_as(&anon, Method::GET, "/install.sh", "user@192.0.2.5", &[], None).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // Forwarded headers from an untrusted peer change nothing.
    let (_, body) = send_as(
        &anon,
        Method::GET,
        "/install.sh",
        "192.0.2.5:3880",
        &[("x-forwarded-host", "evil.test"), ("x-forwarded-proto", "https")],
        None,
    )
    .await;
    assert!(body.contains("FORGE_URL='http://192.0.2.5:3880'") && !body.contains("evil.test"), "{body}");

    // The signed-in onboarding commands use the address the person is on.
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let (s, v) = send_as(&admin, Method::GET, "/api/v1/onboarding", "192.0.2.9:3880", &[], None).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let v: Value = serde_json::from_str(&v).unwrap();
    assert_eq!(v["agent_url"], format!("https://192.0.2.9:{agent_port}"), "{v}");
    assert_eq!(v["install_url"], "http://192.0.2.9:3880/install.sh", "{v}");
    h.app.stop().await;
}

#[tokio::test]
async fn sockets_accept_their_own_origin_under_any_name_and_refuse_others() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let p = port(&h.base);
    let other = format!("localhost:{p}");

    // A name that is not the configured public_url, with its own Origin.
    assert_eq!(events_as(&admin, &other, Some(&format!("http://{other}"))).await, StatusCode::SWITCHING_PROTOCOLS);

    // No Origin (a CLI): allowed, as before.
    assert_eq!(events_as(&admin, &other, None).await, StatusCode::SWITCHING_PROTOCOLS);

    // Another site's page: refused.
    assert_eq!(events_as(&admin, &other, Some("http://evil.test")).await, StatusCode::FORBIDDEN);

    // The scheme is part of the origin.
    assert_eq!(events_as(&admin, &other, Some(&format!("https://{other}"))).await, StatusCode::FORBIDDEN);

    // The pinned address stays accepted on any host (proxies not listed in
    // trusted_proxies).
    assert_eq!(events_as(&admin, &other, Some(&h.base)).await, StatusCode::SWITCHING_PROTOCOLS);
    h.app.stop().await;
}

#[tokio::test]
async fn passkey_options_name_the_host_of_the_request() {
    let h = start().await;
    let anon = Client::anonymous(&h.base);
    let p = port(&h.base);
    let (s, v) = send_as(&anon, Method::POST, "/api/v1/auth/passkey/begin", &format!("localhost:{p}"), &[], None).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let v: Value = serde_json::from_str(&v).unwrap();
    assert_eq!(v["rpId"], "localhost", "{v}");
    let (_, v) = send_as(&anon, Method::POST, "/api/v1/auth/passkey/begin", "192.0.2.5:3880", &[], None).await;
    let v: Value = serde_json::from_str(&v).unwrap();
    assert_eq!(v["rpId"], "192.0.2.5", "{v}");
    h.app.stop().await;
}

#[tokio::test]
async fn allowed_hosts_refuse_other_names_with_421_and_are_audited() {
    let h = start().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let p = port(&h.base);

    let (s, v) = admin.get("/api/v1/server/settings").await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["allowed_hosts"]["hosts"], json!(["*"]), "{v}");
    assert!(v["public_address"]["last_admin"].is_null(), "loopback is never recorded: {v}");

    // MFA first, then validated patterns.
    let (s, _) = admin.put("/api/v1/server/allowed-hosts", json!({ "hosts": ["127.0.0.1"] })).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    step_up(&mut admin).await;

    for bad in [json!([]), json!(["*.com"]), json!(["a b"]), json!(["foo.*.example.com"])] {
        let (s, body) = admin.put("/api/v1/server/allowed-hosts", json!({ "hosts": bad })).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{bad}: {body}");
    }

    let (s, v) = admin.put("/api/v1/server/allowed-hosts", json!({ "hosts": ["127.0.0.1", "*.example.com"] })).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["hosts"], json!(["127.0.0.1", "*.example.com"]));

    let anon = Client::anonymous(&h.base);
    let (s, body) = send_as(&anon, Method::GET, "/api/version", &format!("localhost:{p}"), &[], None).await;
    assert_eq!(s, StatusCode::MISDIRECTED_REQUEST);
    assert!(body.is_empty(), "no detail: {body}");
    let (s, _) = send_as(&anon, Method::GET, "/api/version", "example.com.evil.test", &[], None).await;
    assert_eq!(s, StatusCode::MISDIRECTED_REQUEST);
    let (s, _) = send_as(&anon, Method::GET, "/api/version", "forge.example.com", &[], None).await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = anon.get("/api/version").await;
    assert_eq!(s, StatusCode::OK, "127.0.0.1 is allowed");

    // The liveness probe answers on any host.
    let (s, _) = send_as(&anon, Method::GET, "/api/health", &format!("localhost:{p}"), &[], None).await;
    assert_eq!(s, StatusCode::OK);

    let (_, log) = admin.get("/api/v1/audit?action=server.allowed_hosts.update").await;
    let log = log.as_array().unwrap();
    assert!(log.iter().any(|e| e["outcome"] == "success" && e["details"]["to"][0] == "127.0.0.1"), "{log:?}");
    assert!(log.iter().any(|e| e["outcome"] == "denied"), "{log:?}");

    let (s, v) = admin.call(Method::DELETE, "/api/v1/server/allowed-hosts", None).await;
    assert_eq!((s, v["any"].as_bool()), (StatusCode::OK, Some(true)), "{v}");
    let (s, _) = send_as(&anon, Method::GET, "/api/version", &format!("localhost:{p}"), &[], None).await;
    assert_eq!(s, StatusCode::OK);
    h.app.stop().await;
}

#[tokio::test]
async fn last_admin_address_is_recorded_only_on_trusted_hosts_and_audited() {
    let h = start_configured(|c| c.agent_hostnames.push("forge.test".into())).await;
    let p = port(&h.base);
    let anon = Client::anonymous(&h.base);

    // Signing in on a name nobody configured, or on loopback, records nothing.
    for host in [format!("evil.test:{p}"), format!("localhost:{p}"), format!("127.0.0.1:{p}")] {
        let body = login_body(&h.base, ADMIN_EMAIL, PASSWORD).await;
        let (s, b) = send_as(&anon, Method::POST, "/api/v1/auth/login", &host, &[], Some(body)).await;
        assert_eq!(s, StatusCode::OK, "signing in on {host} works: {b}");
    }

    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let (_, v) = admin.get("/api/v1/server/settings").await;
    assert!(v["public_address"]["last_admin"].is_null(), "{v}");

    // A configured name is recorded and audited.
    let body = login_body(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let (s, b) = send_as(&anon, Method::POST, "/api/v1/auth/login", &format!("forge.test:{p}"), &[], Some(body)).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    let recorded = format!("http://forge.test:{p}");
    let (_, v) = admin.get("/api/v1/server/settings").await;
    assert_eq!(v["public_address"]["last_admin"].as_str(), Some(recorded.as_str()), "{v}");
    let (_, log) = admin.get("/api/v1/audit?action=server.last_admin_address.update").await;
    let items = log.as_array().unwrap();
    assert_eq!(items.len(), 1, "one change, never evil.test or loopback: {log}");
    assert!(!log.to_string().contains("evil.test"), "{log}");
    assert_eq!(items[0]["details"]["to"], recorded.as_str(), "{log}");
    h.app.stop().await;
}

#[tokio::test]
async fn install_scripts_are_never_cached() {
    let h = start().await;

    for path in ["/install.sh", "/install.ps1"] {
        let r = reqwest::get(format!("{}{path}", h.base)).await.unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(r.headers()["cache-control"], "no-store", "{path}");
        assert_eq!(r.headers()["vary"], "Host, X-Forwarded-Host, X-Forwarded-Proto", "{path}");
    }

    h.app.stop().await;
}
