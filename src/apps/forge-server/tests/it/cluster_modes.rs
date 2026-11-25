//! Cluster modes (D-154): chosen at creation, `default` when absent, never
//! changed afterwards.
#![allow(clippy::unwrap_used)]

use reqwest::StatusCode;
use serde_json::json;

use crate::common::{ADMIN_EMAIL, Client, PASSWORD, start};

#[tokio::test]
async fn cluster_mode_is_chosen_at_creation_and_then_fixed() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;

    let (s, plain) = admin.post("/api/v1/clusters", json!({"name": "plain"})).await;
    assert_eq!(s, StatusCode::CREATED, "{plain}");
    assert_eq!(plain["mode"], "default");

    let (s, swarm) = admin.post("/api/v1/clusters", json!({"name": "swarm", "mode": "swarm"})).await;
    assert_eq!(s, StatusCode::CREATED, "{swarm}");
    assert_eq!(swarm["mode"], "swarm");
    let path = format!("/api/v1/clusters/{}", swarm["id"].as_str().unwrap());
    let (_, read) = admin.get(&path).await;
    assert_eq!(read["mode"], "swarm", "the mode is stored");

    // Unknown modes are refused.
    let (s, _) = admin.post("/api/v1/clusters", json!({"name": "odd", "mode": "kubernetes"})).await;
    assert!(s.is_client_error(), "{s}");

    // Updates may repeat the mode or leave it out, but never change it.
    let (s, e) = admin.put(&path, json!({"name": "swarm", "mode": "default"})).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{e}");
    let (s, same) = admin.put(&path, json!({"name": "swarm-eu", "mode": "swarm"})).await;
    assert_eq!(s, StatusCode::OK, "{same}");
    let (s, kept) = admin.put(&path, json!({"name": "swarm-eu"})).await;
    assert_eq!(s, StatusCode::OK, "{kept}");
    assert_eq!(kept["mode"], "swarm");
}
