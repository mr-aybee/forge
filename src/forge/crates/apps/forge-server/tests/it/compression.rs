//! Response compression: ordinary responses are compressed with the best
//! encoding the client accepts; responses that may carry a secret are never
//! compressed (BREACH).
#![allow(clippy::unwrap_used)]

use crate::common::*;

async fn encoding(
    h: &Harness,
    method: reqwest::Method,
    path: &str,
    accept: &str,
    body: Option<serde_json::Value>,
) -> Option<String> {
    let c = reqwest::Client::builder().no_proxy().build().unwrap();
    let mut req = c.request(method, format!("{}{path}", h.base)).header("accept-encoding", accept);

    if let Some(b) = body {
        req = req.json(&b);
    }

    let resp = req.send().await.unwrap();

    resp.headers().get("content-encoding").map(|v| v.to_str().unwrap().to_owned())
}

#[tokio::test]
async fn ordinary_responses_are_compressed_with_the_preferred_encoding() {
    let h = start().await;
    let get = reqwest::Method::GET;
    assert_eq!(encoding(&h, get.clone(), "/api/openapi.json", "br", None).await.as_deref(), Some("br"));
    assert_eq!(encoding(&h, get.clone(), "/api/openapi.json", "zstd", None).await.as_deref(), Some("zstd"));
    assert_eq!(encoding(&h, get.clone(), "/api/openapi.json", "gzip", None).await.as_deref(), Some("gzip"));
    assert_eq!(encoding(&h, get, "/api/openapi.json", "identity", None).await, None);
}

#[tokio::test]
async fn secret_bearing_responses_are_never_compressed() {
    let h = start().await;
    let login = login_body(&h.base, ADMIN_EMAIL, PASSWORD).await;
    assert_eq!(encoding(&h, reqwest::Method::POST, "/api/v1/auth/login", "br, zstd, gzip", Some(login)).await, None);
    let token = serde_json::json!({ "grant_type": "authorization_code", "code": "x".repeat(64) });
    assert_eq!(encoding(&h, reqwest::Method::POST, "/oauth/token", "br, zstd, gzip", Some(token)).await, None);
}
