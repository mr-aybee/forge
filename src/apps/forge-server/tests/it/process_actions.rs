//! Process actions (D-080): `POST /servers/{id}/processes/{pid}/signal` and
//! `…/priority` need `process.manage` with an MFA-verified session, accept
//! only what the node's OS supports, and never reach an offline node.
#![allow(clippy::unwrap_used)]

use crate::common::*;
use forge_agent::enroll::{HostFacts, enroll_with};
use forge_agent::{AgentConfig, StatePaths};
use forge_server::App;
use forge_server::state::Services;
use reqwest::StatusCode;
use serde_json::json;
use tokio::net::TcpListener;

/// A server with `[security] require_mfa` as given (D-064).
async fn start_mfa(require_mfa: bool) -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let mut config = test_config(dir.path());
    let api = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let agent = TcpListener::bind("127.0.0.1:0").await.unwrap();
    config.public_url = format!("http://{}", api.local_addr().unwrap());
    config.security.require_mfa = require_mfa;
    let state = Services::build(config).await.unwrap();
    let init =
        forge_server::bootstrap::init_admin(&state, None, Some(ADMIN_EMAIL), "Admin", PASSWORD, false).await.unwrap();
    let app = App::start_with(state, api, agent).unwrap();
    Harness {
        base: format!("http://{}", app.api_addr),
        agent_url: format!("https://127.0.0.1:{}", app.agent_addr.port()),
        default_org: init.workspace_id.to_string(),
        app,
        dir,
    }
}

/// D-064: with MFA off, an API key counts as verified like a browser
/// session, so scripts and the CLI can run guarded actions; with MFA
/// required it never does (no second factor behind a key).
#[tokio::test]
async fn api_keys_follow_the_installation_mfa_setting() {
    for require_mfa in [true, false] {
        let h = start_mfa(require_mfa).await;
        let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
        let (node, _dir) = enroll_node(&h, &admin, "linux").await;
        let (s, created) = admin.post("/api/v1/api-keys", json!({"name": "ci", "expires_days": 1})).await;
        assert_eq!(s, StatusCode::CREATED, "{created}");
        let mut ci = Client::anonymous(&h.base);
        ci.bearer = created["secret"].as_str().map(str::to_owned);
        let url = format!("/api/v1/servers/{node}/processes/4242/signal");
        let (s, e) = ci.post(&url, json!({"start_time": 1, "signal": "term"})).await;
        if require_mfa {
            assert_eq!((s, e["title"].as_str()), (StatusCode::FORBIDDEN, Some("step_up_required")), "{e}");
        } else {
            // Past the guardrail; the enrolled test node is not online.
            assert_eq!(s, StatusCode::CONFLICT, "{e}");
            assert!(e.to_string().contains("offline"), "{e}");
        }
    }
}

async fn enroll_node(h: &Harness, admin: &Client, os: &str) -> (String, tempfile::TempDir) {
    let (s, body) = admin.post("/api/v1/enrollment-tokens", json!({"ttl_minutes": 10})).await;
    assert_eq!(s, StatusCode::CREATED, "{body}");
    let dir = tempfile::tempdir().unwrap();
    let facts = HostFacts { hostname: format!("proc-{os}"), os: os.into(), arch: "x86_64".into() };
    let id = enroll_with(
        &StatePaths::new(dir.path()),
        AgentConfig::new(h.agent_url.clone()),
        body["token"].as_str().unwrap(),
        facts,
        None,
    )
    .await
    .unwrap();
    (id.server_id, dir)
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
async fn process_actions_need_rights_mfa_and_valid_input() {
    let h = start().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let (linux, _d1) = enroll_node(&h, &admin, "linux").await;
    let (windows, _d2) = enroll_node(&h, &admin, "windows").await;
    let sig = |id: &str| format!("/api/v1/servers/{id}/processes/4242/signal");
    let pri = |id: &str| format!("/api/v1/servers/{id}/processes/4242/priority");
    let term = json!({"start_time": 1_700_000_000u64, "signal": "term"});

    // A viewer cannot; an admin needs an MFA-verified session.
    let ws = &h.default_org;
    let viewer = user_with_role(&h, &admin, "v@example.com", "customer", &format!("workspace:{ws}")).await;
    assert_eq!(viewer.post(&sig(&linux), term.clone()).await.0, StatusCode::FORBIDDEN);
    let (s, e) = admin.post(&sig(&linux), term.clone()).await;
    assert_eq!((s, e["title"].as_str()), (StatusCode::FORBIDDEN, Some("step_up_required")));
    step_up(&mut admin).await;

    // Linux: four signals and nice -20..19; no priority classes.
    for bad in [json!({"start_time": 1, "signal": "stop"}), json!({"start_time": 1, "signal": ""})] {
        assert_eq!(admin.post(&sig(&linux), bad).await.0, StatusCode::BAD_REQUEST);
    }
    assert_eq!(admin.post(&pri(&linux), json!({"start_time": 1, "nice": 20})).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(admin.post(&pri(&linux), json!({"start_time": 1, "class": "high"})).await.0, StatusCode::BAD_REQUEST);
    let zero = format!("/api/v1/servers/{linux}/processes/0/signal");
    assert_eq!(admin.post(&zero, term.clone()).await.0, StatusCode::BAD_REQUEST);
    // Unknown fields are refused.
    let extra = json!({"start_time": 1, "signal": "kill", "pid": 1});
    assert_eq!(admin.post(&sig(&linux), extra).await.0, StatusCode::UNPROCESSABLE_ENTITY);

    // Windows: End task / End process only; classes, never realtime; no nice.
    assert_eq!(admin.post(&sig(&windows), json!({"start_time": 1, "signal": "hup"})).await.0, StatusCode::BAD_REQUEST);
    for bad in [json!({"start_time": 1, "class": "realtime"}), json!({"start_time": 1, "nice": 5})] {
        assert_eq!(admin.post(&pri(&windows), bad).await.0, StatusCode::BAD_REQUEST);
    }

    // Valid requests reach the node only while it is online.
    for (url, body) in [
        (sig(&linux), json!({"start_time": 1, "signal": "kill", "tree": true})),
        (pri(&linux), json!({"start_time": 1, "nice": -5})),
        (sig(&windows), json!({"start_time": 1, "signal": "term"})),
        (pri(&windows), json!({"start_time": 1, "class": "below_normal"})),
    ] {
        let (s, e) = admin.post(&url, body).await;
        assert_eq!(s, StatusCode::CONFLICT, "{url}: {e}");
        assert!(e.to_string().contains("offline"), "{e}");
    }
}
