//! Terminal and exec from the CLI (issue #5, D-075): the session registry
//! and the credential rules of the WebSocket routes. The SSH relay itself is
//! covered by the lab tests (it needs a real node).
#![allow(clippy::unwrap_used)]

use reqwest::StatusCode;
use serde_json::json;

use crate::common::{ADMIN_EMAIL, Client, PASSWORD, start, user_with_role};

/// A WebSocket upgrade request (without completing the handshake).
fn upgrade(c: &reqwest::Client, url: &str) -> reqwest::RequestBuilder {
    c.get(url)
        .header("connection", "Upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
}

#[tokio::test]
async fn sessions_list_and_revoke_are_scoped_to_their_owner() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;

    let (s, v) = admin.get("/api/v1/terminal-sessions").await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["sessions"], json!([]));
    let (s, _) = admin.get("/api/v1/terminal-sessions?all=true").await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = admin.post("/api/v1/terminal-sessions/nope/revoke", json!({})).await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // Everyone's sessions need user.manage.
    let ops = user_with_role(&h, &admin, "ops@example.com", "operator", &format!("workspace:{}", h.default_org)).await;
    let (s, _) = ops.get("/api/v1/terminal-sessions").await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = ops.get("/api/v1/terminal-sessions?all=true").await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn exec_and_terminal_refuse_ambiguous_or_missing_credentials() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let (_, nodes) = admin.get("/api/v1/servers").await;

    // No node needed: authentication and credential checks come first.
    let id = nodes
        .as_array()
        .and_then(|a| a.first())
        .and_then(|n| n["id"].as_str())
        .unwrap_or("01a1d3c4-8057-76f1-a2dd-df3f6ebd2625");
    let (_, key) = admin.post("/api/v1/api-keys", json!({ "name": "cli" })).await;
    let key = key["secret"].as_str().unwrap().to_owned();

    for route in ["exec", "terminal"] {
        let url = format!("{}/api/v1/servers/{id}/{route}?login=root", h.base);

        // No credentials at all.
        let r = upgrade(&reqwest::Client::new(), &url).send().await.unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED, "{route}");

        // A bearer token and a session cookie together are refused.
        let r = upgrade(&admin.http, &url).bearer_auth(&key).send().await.unwrap();
        assert_eq!(r.status(), StatusCode::BAD_REQUEST, "{route}");

        // A token in the URL is never read: still unauthenticated.
        let r = upgrade(&reqwest::Client::new(), &format!("{url}&access_token={key}")).send().await.unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED, "{route}");

        // An API key never holds MFA, so `terminal.open` is refused.
        let r = upgrade(&reqwest::Client::new(), &url).bearer_auth(&key).send().await.unwrap();
        assert!(matches!(r.status(), StatusCode::FORBIDDEN | StatusCode::NOT_FOUND), "{route}: {}", r.status());
    }
}
