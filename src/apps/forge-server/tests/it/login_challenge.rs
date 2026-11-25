//! Login challenge (D-064): every password sign-in carries a solved,
//! single-use proof of work; failures make the next challenge harder.
#![allow(clippy::unwrap_used)]

use crate::common::*;
use reqwest::StatusCode;
use serde_json::{Value, json};

async fn challenge(h: &Harness, login: &str) -> Value {
    let (s, c) = Client::anonymous(&h.base).get(&format!("/api/v1/auth/challenge?login={login}")).await;
    assert_eq!(s, StatusCode::OK, "{c}");
    c
}

fn solve(c: &Value) -> String {
    let salt = forge_secrets::pow::salt_from_hex(c["salt"].as_str().unwrap()).unwrap();
    forge_secrets::pow::solve(&salt, u8::try_from(c["difficulty"].as_u64().unwrap()).unwrap())
}

#[tokio::test]
async fn sign_in_needs_a_solved_single_use_challenge() {
    let h = start().await;
    let anon = Client::anonymous(&h.base);
    let (_, caps) = anon.get("/api/capabilities").await;
    assert!(caps.to_string().contains("auth.login_challenge"));

    // Missing.
    let (s, e) = anon.post("/api/v1/auth/login", json!({ "login": ADMIN_EMAIL, "password": PASSWORD })).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{e}");
    assert_eq!(e["title"], "challenge_required");

    // Wrong nonce (and the challenge is then used up).
    let c = challenge(&h, ADMIN_EMAIL).await;
    assert_eq!(c["algorithm"], "sha256-leading-zero-bits");
    assert!(c["expires_at"].as_i64().unwrap() > 0);
    let body = json!({ "login": ADMIN_EMAIL, "password": PASSWORD, "challenge_id": c["id"], "nonce": "nope" });
    let (s, e) = anon.post("/api/v1/auth/login", body).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(e["title"], "challenge_failed");

    // Right, then reused.
    let c = challenge(&h, ADMIN_EMAIL).await;
    let body = json!({ "login": ADMIN_EMAIL, "password": PASSWORD, "challenge_id": c["id"], "nonce": solve(&c) });
    let (s, _) = anon.post("/api/v1/auth/login", body.clone()).await;
    assert_eq!(s, StatusCode::OK);
    let (s, e) = anon.post("/api/v1/auth/login", body).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "reused: {e}");
    assert_eq!(e["title"], "challenge_failed");

    // A fabricated id is refused.
    let body = json!({ "login": ADMIN_EMAIL, "password": PASSWORD, "challenge_id": "made-up", "nonce": "1" });
    assert_eq!(anon.post("/api/v1/auth/login", body).await.0, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn failures_make_the_next_challenge_harder() {
    let h = start().await;
    let anon = Client::anonymous(&h.base);
    let base_bits = challenge(&h, ADMIN_EMAIL).await["difficulty"].as_u64().unwrap();
    // An easy challenge fetched before the failures …
    let early = challenge(&h, ADMIN_EMAIL).await;
    for _ in 0..2 {
        let (s, _) = anon.post("/api/v1/auth/login", login_body(&h.base, ADMIN_EMAIL, "wrong password!!").await).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
    }
    let c = challenge(&h, ADMIN_EMAIL).await;
    assert_eq!(c["difficulty"].as_u64().unwrap(), base_bits + 2);
    // … no longer counts for this name.
    let body =
        json!({ "login": ADMIN_EMAIL, "password": PASSWORD, "challenge_id": early["id"], "nonce": solve(&early) });
    let (s, e) = anon.post("/api/v1/auth/login", body).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{e}");
    // Another name is not affected; success resets it.
    assert_eq!(challenge(&h, "someone-else").await["difficulty"].as_u64().unwrap(), base_bits);
    Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    assert_eq!(challenge(&h, ADMIN_EMAIL).await["difficulty"].as_u64().unwrap(), base_bits);
}

#[tokio::test]
async fn the_challenge_can_be_turned_off() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = test_config(dir.path());
    config.security.login_challenge = false;
    let api = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let agent = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    config.public_url = Some(format!("http://{}", api.local_addr().unwrap()));
    let state = forge_server::state::Services::build(config).await.unwrap();
    forge_server::bootstrap::init_admin(&state, None, Some(ADMIN_EMAIL), "Admin", PASSWORD, false).await.unwrap();
    let app = forge_server::App::start_with(state, api, agent).unwrap();
    let base = format!("http://{}", app.api_addr);
    let anon = Client::anonymous(&base);
    let (_, c) = anon.get("/api/v1/auth/challenge").await;
    assert_eq!(c["required"], false);
    let (s, _) = anon.post("/api/v1/auth/login", json!({ "login": ADMIN_EMAIL, "password": PASSWORD })).await;
    assert_eq!(s, StatusCode::OK);
}
