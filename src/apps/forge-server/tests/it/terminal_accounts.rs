//! Terminal logins (D-078): accounts saved for a node (installer, onboarding,
//! console) and accounts the node reports, listed together.
#![allow(clippy::unwrap_used)]

use crate::common::*;
use forge_agent::enroll::{HostFacts, enroll_with};
use forge_agent::{AgentConfig, StatePaths};
use forge_core::tags::FactsRepository;
use reqwest::{Method, StatusCode};
use serde_json::json;

async fn enroll_node(h: &Harness, admin: &Client, account: Option<&str>) -> (String, tempfile::TempDir) {
    let (s, body) = admin.post("/api/v1/enrollment-tokens", json!({"ttl_minutes": 10})).await;
    assert_eq!(s, StatusCode::CREATED, "{body}");
    let dir = tempfile::tempdir().unwrap();
    let facts = HostFacts { hostname: "term-1".into(), os: "linux".into(), arch: "x86_64".into() };
    let id = enroll_with(
        &StatePaths::new(dir.path()),
        AgentConfig::new(h.agent_url.clone()),
        body["token"].as_str().unwrap(),
        facts,
        account,
    )
    .await
    .unwrap();
    (id.server_id, dir)
}

/// Saved logins come back sorted by name when saved at the same time.
fn sorted(v: &serde_json::Value) -> Vec<String> {
    let mut l: Vec<String> = serde_json::from_value(v.clone()).unwrap();
    l.sort();
    l
}

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

#[tokio::test]
async fn logins_list_saved_and_discovered_accounts() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    // The installer created `forge-ops`; enrolling saves it for the node.
    let (id, _dir) = enroll_node(&h, &admin, Some("forge-ops")).await;
    let url = format!("/api/v1/servers/{id}/logins");
    let (s, body) = admin.get(&url).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(body["saved"], json!(["forge-ops"]));
    assert_eq!(body["logins"], json!(["forge-ops"]), "older clients still read `logins`");
    assert_eq!(body["discovered"], json!([]));

    // The node reports its accounts in its facts; saved ones are not repeated
    // and invalid names are dropped.
    let store = &h.app.state.store;
    let facts = json!({ "login_accounts": ["root", "forge-ops", "alice", "-bad"] });
    store.put_facts(id.parse().unwrap(), &facts, h.app.state.clock.now()).await.unwrap();
    let (_, body) = admin.get(&url).await;
    assert_eq!(body["saved"], json!(["forge-ops"]));
    assert_eq!(body["discovered"], json!(["root", "alice"]));

    // Saving a discovered account moves it to `saved`.
    let put = json!({ "logins": ["forge-ops", "alice"] });
    assert_eq!(admin.call(Method::PUT, &url, Some(put)).await.0, StatusCode::OK);
    let (_, body) = admin.get(&url).await;
    assert_eq!(sorted(&body["saved"]), ["alice", "forge-ops"]);
    assert_eq!(body["discovered"], json!(["root"]));
}

#[tokio::test]
async fn enrolling_without_an_account_saves_nothing() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let (id, _dir) = enroll_node(&h, &admin, None).await;
    let (_, body) = admin.get(&format!("/api/v1/servers/{id}/logins")).await;
    assert_eq!(body["saved"], json!([]));
}

#[tokio::test]
async fn onboarding_saves_the_ssh_user_once_and_audits_it() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let (id, _dir) = enroll_node(&h, &admin, Some("forge-ops")).await;
    let state = &h.app.state;
    let who = forge_core::Principal::User(forge_core::UserId::new());
    let ws = h.default_org.parse().unwrap();
    let server = id.parse().unwrap();
    let save = || {
        forge_server::jobs::node_bootstrap::save_onboarding_login(
            &state.store,
            state.audit.as_ref(),
            &who,
            ws,
            server,
            "deploy",
            state.clock.now(),
        )
    };
    assert!(save().await.unwrap());
    // A retried step adds nothing.
    assert!(!save().await.unwrap());
    let (_, body) = admin.get(&format!("/api/v1/servers/{id}/logins")).await;
    assert_eq!(sorted(&body["saved"]), ["deploy", "forge-ops"]);
    let (_, audit) = admin.get("/api/v1/audit?limit=100").await;
    let text = audit.to_string();
    assert!(text.contains("server.logins.update") && text.contains("onboarding"), "{text}");
}

#[tokio::test]
async fn creating_a_terminal_account_checks_rights_and_names() {
    let h = start().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let (id, _dir) = enroll_node(&h, &admin, None).await;
    let url = format!("/api/v1/servers/{id}/terminal-account");
    let ws = &h.default_org;
    let viewer = user_with_role(&h, &admin, "v@example.com", "customer", &format!("workspace:{ws}")).await;
    assert_eq!(viewer.post(&url, json!({"name": "forge-ops"})).await.0, StatusCode::FORBIDDEN);
    // server.command needs an MFA-verified session.
    let (s, e) = admin.post(&url, json!({"name": "forge-ops"})).await;
    assert_eq!((s, e["title"].as_str()), (StatusCode::FORBIDDEN, Some("step_up_required")));
    step_up(&mut admin).await;
    for bad in ["root", "forge-agent", "Ops", "a.b", ""] {
        let (s, body) = admin.post(&url, json!({"name": bad, "sudo": true})).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{bad}: {body}");
    }
    assert_eq!(admin.post(&url, json!({"name": "forge-ops", "extra": 1})).await.0, StatusCode::UNPROCESSABLE_ENTITY);
}
