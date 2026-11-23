//! `Idempotency-Key` on mutations (issue #5, D-074): replay, mismatch,
//! per-person scope, released keys after failures, header validation.
#![allow(clippy::unwrap_used)]

use crate::common::{ADMIN_EMAIL, Client, PASSWORD, start, user_with_role};
use reqwest::StatusCode;
use serde_json::{Value, json};

async fn api_key(c: &Client, name: &str) -> String {
    let (s, k) = c.post("/api/v1/api-keys", json!({ "name": name })).await;
    assert_eq!(s, StatusCode::CREATED, "{k}");
    k["secret"].as_str().unwrap().to_owned()
}

async fn post(base: &str, token: &str, path: &str, key: Option<&str>, body: Value) -> (StatusCode, bool, Value) {
    let mut req = reqwest::Client::new().post(format!("{base}{path}")).bearer_auth(token).json(&body);
    if let Some(k) = key {
        req = req.header("Idempotency-Key", k);
    }
    let r = req.send().await.unwrap();
    let status = r.status();
    let replayed = r.headers().get("idempotent-replayed").is_some_and(|v| v == "true");
    (status, replayed, r.json().await.unwrap_or(Value::Null))
}

/// Like [`post`], as a console session (cookie and CSRF).
async fn web_post(c: &Client, path: &str, key: &str, body: Value) -> (StatusCode, bool, Value) {
    let mut req = c.http.post(format!("{}{path}", c.base)).header("Idempotency-Key", key).json(&body);
    if let Some(csrf) = &c.csrf {
        req = req.header("x-csrf-token", csrf);
    }
    let r = req.send().await.unwrap();
    let status = r.status();
    let replayed = r.headers().get("idempotent-replayed").is_some_and(|v| v == "true");
    (status, replayed, r.json().await.unwrap_or(Value::Null))
}

#[tokio::test]
async fn a_mutation_with_the_same_key_runs_once_for_that_person_only() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let token = api_key(&admin, "automation").await;

    // First call creates a tenant; the retry gets the same answer, replayed.
    let (s, replayed, first) = post(&h.base, &token, "/api/v1/tenants", Some("k-1"), json!({ "name": "acme" })).await;
    assert_eq!((s, replayed), (StatusCode::CREATED, false), "{first}");
    let (s, replayed, again) = post(&h.base, &token, "/api/v1/tenants", Some("k-1"), json!({ "name": "acme" })).await;
    assert_eq!((s, replayed), (StatusCode::CREATED, true), "{again}");
    assert_eq!(first, again);
    let (_, tenants) = admin.get("/api/v1/tenants").await;
    assert_eq!(tenants.as_array().unwrap().iter().filter(|t| t["name"] == "acme").count(), 1, "{tenants}");

    // The same key for a different request is refused.
    let (s, _, e) = post(&h.base, &token, "/api/v1/tenants", Some("k-1"), json!({ "name": "other" })).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{e}");
    assert_eq!(e["title"].as_str().or(e["code"].as_str()).unwrap_or_default(), "idempotency_key_reused", "{e}");

    // A one-time secret is replayed to its owner (stored sealed, not in plain text).
    let (s, r1, k1) = web_post(&admin, "/api/v1/api-keys", "key-create", json!({ "name": "ci" })).await;
    let (_, r2, k2) = web_post(&admin, "/api/v1/api-keys", "key-create", json!({ "name": "ci" })).await;
    assert_eq!((s, r1, r2), (StatusCode::CREATED, false, true));
    assert_eq!(k1["secret"], k2["secret"]);
    let secret = k1["secret"].as_str().unwrap();
    for f in ["forge.db", "forge.db-wal"] {
        let db = std::fs::read(h.dir.path().join(f)).unwrap_or_default();
        assert!(!db.windows(secret.len()).any(|w| w == secret.as_bytes()), "{f}: the stored response must be sealed");
    }

    // Another person with the same key and body runs their own request.
    let other =
        user_with_role(&h, &admin, "ops@example.com", "operator", &format!("workspace:{}", h.default_org)).await;
    let (s, replayed, theirs) = web_post(&other, "/api/v1/api-keys", "key-create", json!({ "name": "ci" })).await;
    assert_eq!((s, replayed), (StatusCode::CREATED, false), "{theirs}");
    assert_ne!(theirs["secret"], k1["secret"]);
}

#[tokio::test]
async fn failures_release_the_key_and_bad_keys_are_refused() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let token = api_key(&admin, "automation").await;

    // A failed request is not stored: the retry runs again.
    let bad = json!({ "name": "" });
    let (s1, r1, _) = post(&h.base, &token, "/api/v1/tenants", Some("fail-1"), bad.clone()).await;
    let (s2, r2, _) = post(&h.base, &token, "/api/v1/tenants", Some("fail-1"), bad).await;
    assert!(s1.is_client_error() && s2 == s1 && !r1 && !r2, "{s1} {s2}");
    // ... and the key is free for the corrected request.
    let (s, r, _) = post(&h.base, &token, "/api/v1/tenants", Some("fail-1"), json!({ "name": "fixed" })).await;
    assert_eq!((s, r), (StatusCode::CREATED, false));

    // Keys must be 1 to 255 visible ASCII characters.
    let (s, _, _) = post(&h.base, &token, "/api/v1/tenants", Some("has space"), json!({ "name": "x" })).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let long = "k".repeat(256);
    let (s, _, _) = post(&h.base, &token, "/api/v1/tenants", Some(&long), json!({ "name": "x" })).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // Without a key nothing changes: two calls, two attempts.
    let (s, _, _) = post(&h.base, &token, "/api/v1/tenants", None, json!({ "name": "twice" })).await;
    assert_eq!(s, StatusCode::CREATED);
    let (s, _, _) = post(&h.base, &token, "/api/v1/tenants", None, json!({ "name": "twice" })).await;
    assert!(s == StatusCode::CREATED || s == StatusCode::CONFLICT, "{s}");

    // An unauthenticated request with a key fails as usual.
    let (s, _, _) = post(&h.base, "fk_nope", "/api/v1/tenants", Some("x"), json!({ "name": "y" })).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
}

/// Security review: stored outcomes were shared by every credential of a
/// person, so a leaked API key replaying a console request's key got the
/// console's stored answer (a one-time secret, a revealed password) without
/// the checks that guarded it. They are now scoped to the credential kind,
/// and secret-bearing paths are never stored.
#[tokio::test]
async fn a_weaker_credential_never_gets_a_stored_answer() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let token = api_key(&admin, "automation").await;

    let (s, r1, console) = web_post(&admin, "/api/v1/api-keys", "shared-key", json!({ "name": "ci" })).await;
    assert_eq!((s, r1), (StatusCode::CREATED, false), "{console}");
    // The same person's API key, same key and body: runs on its own.
    let (s, replayed, by_key) =
        post(&h.base, &token, "/api/v1/api-keys", Some("shared-key"), json!({ "name": "ci" })).await;
    assert!(!replayed, "an API key must not receive the console session's stored answer");
    assert_ne!(by_key.get("secret"), console.get("secret"), "{s} {by_key}");

    // Secret-bearing paths are never stored: a retry runs again.
    let (_, r1, _) = web_post(&admin, "/api/v1/auth/totp", "totp-1", json!({})).await;
    let (_, r2, _) = web_post(&admin, "/api/v1/auth/totp", "totp-1", json!({})).await;
    assert!(!r1 && !r2, "the authenticator secret is never kept for replay");

    // A replay is audited.
    let (_, _, _) = post(&h.base, &token, "/api/v1/tenants", Some("audited"), json!({ "name": "audit-me" })).await;
    let (_, replayed, _) =
        post(&h.base, &token, "/api/v1/tenants", Some("audited"), json!({ "name": "audit-me" })).await;
    assert!(replayed);
    let (_, audit) = admin.get("/api/v1/audit?limit=50").await;
    assert!(audit.to_string().contains("idempotency.replay"), "{audit}");
}
