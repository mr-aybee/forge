//! Device sign-in (RFC 8628) and step-up for bearer tokens (RFC 9470), as
//! `forge auth login --device` and the CLI's MFA prompt use them.
#![allow(clippy::unwrap_used)]

use crate::common::*;
use reqwest::StatusCode;
use serde_json::{Value, json};

const GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";

async fn form(h: &Harness, path: &str, body: &[(&str, &str)]) -> (StatusCode, Value) {
    let r = reqwest::Client::new().post(format!("{}{path}", h.base)).form(body).send().await.unwrap();
    let s = r.status();
    (s, r.json().await.unwrap_or(Value::Null))
}

async fn start_device(h: &Harness) -> (String, String) {
    let (s, d) = form(h, "/oauth/device_authorization", &[("client_id", "forge-app")]).await;
    assert_eq!(s, StatusCode::OK, "{d}");
    assert_eq!(d["verification_uri"], format!("{}/device", h.base));
    assert!(d["verification_uri_complete"].as_str().unwrap().ends_with(d["user_code"].as_str().unwrap()));
    assert_eq!(d["interval"], 5);
    (d["device_code"].as_str().unwrap().to_owned(), d["user_code"].as_str().unwrap().to_owned())
}

async fn poll(h: &Harness, device_code: &str) -> (StatusCode, Value) {
    form(h, "/oauth/token", &[("grant_type", GRANT), ("device_code", device_code), ("client_id", "forge-app")]).await
}

#[tokio::test]
async fn a_device_signs_in_only_after_a_person_approves_it() {
    let h = start().await;
    let web = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;

    let (s, e) = form(&h, "/oauth/device_authorization", &[("client_id", "mcp_someone")]).await;
    assert_eq!((s, e["error"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid_client")));

    let (device_code, user_code) = start_device(&h).await;
    let (s, e) = poll(&h, &device_code).await;
    assert_eq!((s, e["error"].as_str()), (StatusCode::BAD_REQUEST, Some("authorization_pending")));
    let (_, e) = poll(&h, &device_code).await;
    assert_eq!(e["error"], "slow_down", "polling faster than the interval");

    // The console: a wrong code is not found; lower case and spaces are fine.
    assert_eq!(web.get("/api/v1/oauth/device/BBBB-BBBB").await.0, StatusCode::NOT_FOUND);
    let typed = user_code.to_lowercase().replace('-', " ");
    let (s, info) = web.get(&format!("/api/v1/oauth/device/{typed}")).await;
    assert_eq!(s, StatusCode::OK, "{info}");
    assert_eq!(info["user_code"], user_code);
    assert_eq!(info["client_id"], "forge-app");
    // Security review: the person sees where the request came from.
    assert_eq!(info["requested_from"], "127.0.0.1", "{info}");
    assert!(info["user_agent"].as_str().is_some());

    // Only a console session approves: not a bearer token, not without CSRF.
    let mut no_csrf = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    no_csrf.csrf = None;
    let (s, _) = no_csrf.post(&format!("/api/v1/oauth/device/{user_code}"), json!({ "approve": true })).await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    let (s, _) = web.post(&format!("/api/v1/oauth/device/{user_code}"), json!({ "approve": true })).await;
    assert_eq!(s, StatusCode::OK);
    tokio::time::sleep(std::time::Duration::from_secs(5)).await; // the client keeps the interval
    let (s, tokens) = poll(&h, &device_code).await;
    assert_eq!(s, StatusCode::OK, "{tokens}");
    let access = tokens["access_token"].as_str().unwrap().to_owned();
    let (_, e) = poll(&h, &device_code).await;
    assert_eq!(e["error"], "expired_token", "a device code is used once");

    let mut cli = Client::anonymous(&h.base);
    cli.bearer = Some(access);
    let (s, me) = cli.get("/api/v1/me").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(me["session_kind"], "bearer");
    // Security review: device tokens never carry MFA, whatever the approving
    // session had; the CLI steps up when it needs to.
    assert_eq!(me["mfa"], false, "{me}");
    let (_, audit) = web.get("/api/v1/audit?limit=50").await;
    assert!(audit.to_string().contains("oauth.device.token"), "issuing tokens is audited");
    let (s, _) = cli.post(&format!("/api/v1/oauth/device/{user_code}"), json!({ "approve": true })).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "a bearer token cannot approve devices");

    // Denied.
    let (device_code, user_code) = start_device(&h).await;
    web.post(&format!("/api/v1/oauth/device/{user_code}"), json!({ "approve": false })).await;
    let (_, e) = poll(&h, &device_code).await;
    assert_eq!(e["error"], "access_denied");

    let (_, log) = web.get("/api/v1/audit?action=oauth.device").await;
    assert!(log.as_array().unwrap().len() >= 2, "{log}");
}

#[tokio::test]
async fn bearer_tokens_step_up_with_a_code_and_never_without() {
    let h = start().await;
    let mut web = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    // Enroll an authenticator app.
    let (_, t) = web.post("/api/v1/auth/totp", json!({})).await;
    let secret = forge_identity::totp::base32_decode(t["secret"].as_str().unwrap()).unwrap();
    let code_at = |offset: i64| {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        format!("{:06}", forge_identity::totp::code_at(&secret, i64::try_from(now).unwrap() + offset))
    };
    let (s, _) = web
        .post("/api/v1/auth/totp/confirm", json!({ "password": PASSWORD, "secret": t["secret"], "code": code_at(0) }))
        .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    web.csrf = web.csrf.clone();

    // A bearer session without MFA (approved from a password-only console session).
    let (device_code, user_code) = start_device(&h).await;
    web.post(&format!("/api/v1/oauth/device/{user_code}"), json!({ "approve": true })).await;
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    let (_, tokens) = poll(&h, &device_code).await;
    let access = tokens["access_token"].as_str().unwrap().to_owned();
    let refresh = tokens["refresh_token"].as_str().unwrap().to_owned();

    // RFC 9470: the guarded action names the problem.
    let r = reqwest::Client::new()
        .post(format!("{}/api/v1/nodes/probe", h.base))
        .bearer_auth(&access)
        .json(&json!({ "host": "127.0.0.1", "port": 1 }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    let wa = r.headers()["www-authenticate"].to_str().unwrap().to_owned();
    assert!(wa.contains("insufficient_user_authentication") && wa.contains("acr_values"), "{wa}");
    let body: Value = r.json().await.unwrap();
    assert_eq!(body["title"], "step_up_required");

    // A wrong code changes nothing.
    let mut cli = Client::anonymous(&h.base);
    cli.bearer = Some(access.clone());
    let (s, _) = cli.post("/api/v1/auth/step-up", json!({ "code": "000000" })).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    // The right code: a new token pair with MFA replaces the old one.
    let (s, up) = cli.post("/api/v1/auth/step-up", json!({ "code": code_at(30) })).await;
    assert_eq!(s, StatusCode::OK, "{up}");
    let stepped = up["access_token"].as_str().unwrap().to_owned();
    assert_eq!(cli.get("/api/v1/me").await.0, StatusCode::UNAUTHORIZED, "the old token is gone");
    let (_, e) = form(&h, "/oauth/token", &[("grant_type", "refresh_token"), ("refresh_token", &refresh)]).await;
    assert!(e.get("access_token").is_none(), "and so is its refresh token");
    cli.bearer = Some(stepped);
    let (s, me) = cli.get("/api/v1/me").await;
    assert_eq!((s, me["mfa"].as_bool()), (StatusCode::OK, Some(true)), "{me}");
    let (s, body) = cli.post("/api/v1/nodes/probe", json!({ "host": "127.0.0.1", "port": 1 })).await;
    assert_ne!(body["title"], "step_up_required", "{s} {body}");
}
