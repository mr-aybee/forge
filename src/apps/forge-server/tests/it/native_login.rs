//! Native clients (the `forge` CLI, apps): OAuth 2.1 + PKCE sign-in with a
//! loopback redirect, bearer use, refresh, and sign-out that revokes the
//! whole bearer session (access and refresh token).
#![allow(clippy::unwrap_used)]

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::common::{ADMIN_EMAIL, Client, PASSWORD, start};

#[tokio::test]
async fn pkce_login_bearer_use_and_logout_revokes_the_session() {
    let h = start().await;
    let web = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;

    // Authorize: the signed-in browser session approves a loopback redirect.
    let verifier = "a-verifier-with-enough-entropy-0123456789abcdef";
    let challenge = URL_SAFE_NO_PAD.encode(ring::digest::digest(&ring::digest::SHA256, verifier.as_bytes()).as_ref());
    let redirect = "http://127.0.0.1:53682/callback";
    let resp = web
        .http
        .get(format!("{}/oauth/authorize", h.base))
        .query(&[
            ("response_type", "code"),
            ("client_id", "forge-app"),
            ("redirect_uri", redirect),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("state", "s1"),
        ])
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_redirection(), "{}", resp.status());
    let location = resp.headers()["location"].to_str().unwrap().to_owned();
    let url = url::Url::parse(&location).unwrap();
    assert!(location.starts_with(redirect));
    let q: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(q["state"], "s1");

    // Exchange the code (a wrong verifier is refused).
    let http = reqwest::Client::new();
    let token = |pairs: Vec<(&'static str, String)>| {
        let http = http.clone();
        let base = h.base.clone();
        async move { http.post(format!("{base}/oauth/token")).form(&pairs).send().await.unwrap() }
    };
    let bad = token(vec![
        ("grant_type", "authorization_code".into()),
        ("code", q["code"].clone()),
        ("code_verifier", "wrong-verifier-wrong-verifier-wrong-verifier-x".into()),
        ("client_id", "forge-app".into()),
        ("redirect_uri", redirect.into()),
    ])
    .await;
    assert!(!bad.status().is_success());

    // The code is single use: start again for the real exchange.
    let resp = web
        .http
        .get(format!("{}/oauth/authorize", h.base))
        .query(&[
            ("response_type", "code"),
            ("client_id", "forge-app"),
            ("redirect_uri", redirect),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
        ])
        .send()
        .await
        .unwrap();
    let url = url::Url::parse(resp.headers()["location"].to_str().unwrap()).unwrap();
    let code = url.query_pairs().find(|(k, _)| k == "code").unwrap().1.into_owned();
    let ok = token(vec![
        ("grant_type", "authorization_code".into()),
        ("code", code),
        ("code_verifier", verifier.into()),
        ("client_id", "forge-app".into()),
        ("redirect_uri", redirect.into()),
    ])
    .await;
    assert_eq!(ok.status(), StatusCode::OK);
    let tokens: Value = ok.json().await.unwrap();
    let access = tokens["access_token"].as_str().unwrap().to_owned();
    let refresh = tokens["refresh_token"].as_str().unwrap().to_owned();

    // Bearer use.
    let mut cli = Client::anonymous(&h.base);
    cli.bearer = Some(access.clone());
    let (s, me) = cli.get("/api/v1/me").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(me["session_kind"], "bearer");

    // Sign-out revokes the bearer session...
    let (s, _) = cli.post("/api/v1/auth/logout", json!({})).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, _) = cli.get("/api/v1/me").await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "access token must stop working after logout");

    // ...including its refresh token.
    let r = token(vec![("grant_type", "refresh_token".into()), ("refresh_token", refresh)]).await;
    assert!(!r.status().is_success(), "refresh token must be revoked with the session");

    // The browser session is untouched.
    let (s, _) = web.get("/api/v1/me").await;
    assert_eq!(s, StatusCode::OK);
    h.app.stop().await;
}
