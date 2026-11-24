//! Docker images, volumes and networks on a node (D-157): who may change
//! them, what is refused before anything reaches the node, and that slow
//! operations become jobs. The node side runs in its root helper.
#![allow(clippy::unwrap_used)]

use crate::common::{ADMIN_EMAIL, Client, PASSWORD, start, user_with_role};
use forge_agent::AgentConfig;
use forge_agent::StatePaths;
use forge_agent::enroll::{HostFacts, enroll};
use reqwest::StatusCode;
use serde_json::json;

#[tokio::test]
async fn docker_resources_are_checked_guarded_and_audited() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let (_, tok) = admin.post("/api/v1/enrollment-tokens", json!({"ttl_minutes": 10})).await;
    let dir = tempfile::tempdir().unwrap();
    let facts = HostFacts { hostname: "lab-a".into(), os: "linux".into(), arch: "x86_64".into() };
    let id = enroll(
        &StatePaths::new(dir.path()),
        AgentConfig::new(h.agent_url.clone()),
        tok["token"].as_str().unwrap(),
        facts,
    )
    .await
    .unwrap()
    .server_id;
    let ops = format!("/api/v1/servers/{id}/docker/ops");
    let ws = format!("workspace:{}", h.default_org);

    // Credentials never go through the API (they would be stored in jobs).
    let (s, e) = admin.post(&ops, json!({"op": "image_pull", "image": "nginx:1.27", "registry_auth": "e30="})).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{e}");
    // Driver options and protected networks are refused before the node.
    let (s, _) = admin.post(&ops, json!({"op": "volume_create", "name": "data", "driver_opts": {"device": "/"}})).await;
    assert!(s.is_client_error(), "{s}");
    let (s, _) = admin.post(&ops, json!({"op": "network_remove", "id": "forge-net-0190"})).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // Developers cannot change Docker resources; operators can.
    let dev = user_with_role(&h, &admin, "dev@example.com", "developer", &ws).await;
    let (s, _) = dev.post(&ops, json!({"op": "image_pull", "image": "nginx:1.27"})).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let op = user_with_role(&h, &admin, "op@example.com", "operator", &ws).await;
    let (s, job) = op.post(&ops, json!({"op": "image_pull", "image": "nginx:1.27"})).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{job}");
    assert_eq!(job["kind"], "docker.op");
    assert!(job["payload"].to_string().find("registry_auth\":\"").is_none(), "no credentials in the stored job");

    let (_, audit) = admin.get("/api/v1/audit?limit=200").await;
    assert!(audit.to_string().contains("docker.image_pull"), "{audit}");
}

/// Registries (D-158): administrators with MFA manage them; the secret never
/// comes back; pushes go only to registries added in Forge, with their
/// credentials added on the node, never stored in the job.
#[tokio::test]
async fn registries_hold_secrets_and_gate_pushes() {
    let h = start().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let ws = format!("workspace:{}", h.default_org);
    let body = json!({"name": "GitHub", "kind": "ghcr", "host": "https://GHCR.io/", "username": "bot", "secret": "ghp_s3cret"});

    let (s, _) = admin.post("/api/v1/registries", body.clone()).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "MFA first");
    let mut op = user_with_role(&h, &admin, "op2@example.com", "operator", &ws).await;
    crate::common::step_up(&mut op).await;
    let (s, _) = op.post("/api/v1/registries", body.clone()).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "operators cannot hold registry secrets");

    crate::common::step_up(&mut admin).await;
    let (s, r) = admin.post("/api/v1/registries", body.clone()).await;
    assert_eq!(s, StatusCode::CREATED, "{r}");
    assert_eq!(r["host"], "ghcr.io");
    let (s, _) = admin.post("/api/v1/registries", body).await;
    assert_eq!(s, StatusCode::CONFLICT, "one registry per host");
    let (s, _) = admin
        .post("/api/v1/registries", json!({"name": "Hub", "kind": "docker_hub", "host": "example.com", "secret": "x"}))
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let (_, list) = admin.get("/api/v1/registries").await;
    assert_eq!(list[0]["name"], "GitHub");

    let (_, tok) = admin.post("/api/v1/enrollment-tokens", json!({"ttl_minutes": 10})).await;
    let dir = tempfile::tempdir().unwrap();
    let facts = HostFacts { hostname: "lab-b".into(), os: "linux".into(), arch: "x86_64".into() };
    let id = enroll(
        &StatePaths::new(dir.path()),
        AgentConfig::new(h.agent_url.clone()),
        tok["token"].as_str().unwrap(),
        facts,
    )
    .await
    .unwrap()
    .server_id;
    let ops = format!("/api/v1/servers/{id}/docker/ops");
    let (s, e) = admin.post(&ops, json!({"op": "image_push", "image": "attacker.example/x:1"})).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{e}");
    // Pushes need a registry that allows them, and registry.manage.
    let (s, e) = admin.post(&ops, json!({"op": "image_push", "image": "ghcr.io/acme/app:1"})).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "pushes are off by default: {e}");
    let rid = r["id"].as_str().unwrap();
    let (s, e) = admin
        .put(
            &format!("/api/v1/registries/{rid}"),
            json!({"name": "GitHub", "kind": "ghcr", "host": "ghcr.io", "username": "bot", "allow_push": true}),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{e}");
    let (s, e) = admin
        .put(
            &format!("/api/v1/registries/{rid}"),
            json!({"name": "GitHub", "kind": "ghcr", "host": "attacker.example", "username": "bot"}),
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "a new host needs the secret again: {e}");
    let (s, _) = op.post(&ops, json!({"op": "image_push", "image": "ghcr.io/acme/app:1"})).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "operators do not push with the workspace's credentials");
    let (s, job) = admin.post(&ops, json!({"op": "image_push", "image": "ghcr.io/acme/app:1"})).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{job}");

    let (_, audit) = admin.get("/api/v1/audit?limit=200").await;
    for text in [list.to_string(), r.to_string(), job.to_string(), audit.to_string()] {
        assert!(!text.contains("ghp_s3cret"), "the secret leaked: {text}");
    }
}
