//! Elevated API keys and service-account tokens (D-097) and the local
//! `enroll-token` helper (D-099).
#![allow(clippy::unwrap_used)]

use crate::common::*;
use reqwest::StatusCode;
use serde_json::{Value, json};

/// An MFA-guarded call: probing a node needs `server.enroll` with MFA.
async fn guarded(c: &Client) -> (StatusCode, Value) {
    c.post("/api/v1/nodes/probe", json!({"host": "127.0.0.1", "port": 1})).await
}

fn key_client(h: &Harness, secret: &Value) -> Client {
    let mut c = Client::anonymous(&h.base);
    c.bearer = Some(secret.as_str().unwrap().to_owned());

    c
}

#[tokio::test]
async fn an_elevated_key_passes_the_mfa_guardrail_and_a_normal_one_does_not() {
    let h = start().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;

    // Creating an elevated key needs an MFA-verified session.
    let (s, e) = admin.post("/api/v1/api-keys", json!({"name": "ci", "elevated": true})).await;
    assert_eq!((s, e["title"].as_str()), (StatusCode::FORBIDDEN, Some("step_up_required")), "{e}");
    let (s, normal) = admin.post("/api/v1/api-keys", json!({"name": "plain"})).await;
    assert_eq!(s, StatusCode::CREATED, "{normal}");
    assert_eq!(normal["elevated"], false);
    step_up(&mut admin).await;
    let (s, elevated) = admin.post("/api/v1/api-keys", json!({"name": "ci", "elevated": true})).await;
    assert_eq!(s, StatusCode::CREATED, "{elevated}");
    assert_eq!(elevated["elevated"], true);
    let (_, list) = admin.get("/api/v1/api-keys").await;
    assert!(list.as_array().unwrap().iter().any(|k| k["elevated"] == true));

    let (s, e) = guarded(&key_client(&h, &normal["secret"])).await;
    assert_eq!((s, e["title"].as_str()), (StatusCode::FORBIDDEN, Some("step_up_required")), "{e}");
    let (s, e) = guarded(&key_client(&h, &elevated["secret"])).await;
    assert_ne!(e["title"].as_str(), Some("step_up_required"), "{s} {e}");
    assert_ne!(s, StatusCode::FORBIDDEN, "{e}");

    let (_, entries) = admin.get("/api/v1/audit?limit=100").await;
    let text = entries.to_string();
    assert!(text.contains("\"elevated\":true") && text.contains(elevated["id"].as_str().unwrap()), "{text}");
}

#[tokio::test]
async fn with_elevated_tokens_off_creation_is_refused_and_old_keys_are_normal() {
    let h = start_configured(|c| c.security.allow_elevated_tokens = false).await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    step_up(&mut admin).await;
    let (s, e) = admin.post("/api/v1/api-keys", json!({"name": "ci", "elevated": true})).await;
    assert_eq!((s, e["title"].as_str()), (StatusCode::FORBIDDEN, Some("elevated_tokens_disabled")), "{e}");
    let (_, settings) = admin.get("/api/v1/settings").await;
    assert_eq!(settings["auth"]["allow_elevated_tokens"], false);

    // A key created elevated earlier acts as a normal key.
    use forge_core::ports::{ApiKeyRepository, UserRepository};
    let st = &h.app.state;
    let user = st.store.find_user_by_email(ADMIN_EMAIL).await.unwrap().unwrap();
    let secret = "fk_oldkey_elevatedsecretvalue0000000000";
    st.store
        .create_api_key(&forge_core::model::ApiKey {
            id: "old".into(),
            user_id: user.id,
            name: "old".into(),
            prefix: "fk_oldkey".into(),
            secret_hash: forge_secrets::crypto::sha256_hex(secret.as_bytes()),
            expires_at: None,
            last_used_at: None,
            created_at: forge_core::Timestamp(0),
            elevated: true,
            restrictions: None,
        })
        .await
        .unwrap();
    let (s, e) = guarded(&key_client(&h, &json!(secret))).await;
    assert_eq!((s, e["title"].as_str()), (StatusCode::FORBIDDEN, Some("step_up_required")), "{e}");
}

#[tokio::test]
async fn service_account_tokens_can_be_elevated() {
    let h = start().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    step_up(&mut admin).await;
    let (s, sa) = admin.post("/api/v1/service-accounts", json!({"name": "deploy"})).await;
    assert_eq!(s, StatusCode::CREATED, "{sa}");
    let path = format!("/api/v1/service-accounts/{}/tokens", sa["id"].as_str().unwrap());
    let (s, t) = admin.post(&path, json!({"name": "t", "elevated": true})).await;
    assert_eq!(s, StatusCode::CREATED, "{t}");
    let (_, list) = admin.get(&path).await;
    assert_eq!(list[0]["elevated"], true, "{list}");
}

#[tokio::test]
async fn the_local_enroll_token_creates_the_cluster() {
    use forge_core::ports::{ClusterRepository, TenancyRepository};
    let h = start().await;
    let st = &h.app.state;
    let token = forge_server::bootstrap::local_enroll_token(st, "edge", "lab-a", 15).await.unwrap();
    assert!(!token.is_empty() && !token.contains(char::is_whitespace));
    let ws = st.store.list_workspaces().await.unwrap()[0].id;
    let clusters = st.store.list_clusters(ws).await.unwrap();
    assert_eq!(clusters.iter().filter(|c| c.name == "edge").count(), 1);

    // Again: the cluster is reused.
    forge_server::bootstrap::local_enroll_token(st, "edge", "lab-a", 15).await.unwrap();
    assert_eq!(st.store.list_clusters(ws).await.unwrap().iter().filter(|c| c.name == "edge").count(), 1);
    let (_, entries) = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await.get("/api/v1/audit?limit=50").await;
    let text = entries.to_string();
    assert!(text.contains("server.enrollment_token.create") && text.contains("system:forge"), "{text}");
}
