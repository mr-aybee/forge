//! Docker Swarm in Swarm clusters (D-154): who may join and leave nodes,
//! what the API refuses before a job starts, and what the swarm view shows.
//! The join itself runs on real nodes (the root helper); see the lab checks.
#![allow(clippy::unwrap_used)]

use crate::common::{ADMIN_EMAIL, Client, PASSWORD, start, step_up, user_with_role};
use forge_agent::AgentConfig;
use forge_agent::StatePaths;
use forge_agent::enroll::{HostFacts, enroll};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

async fn node(h: &crate::common::Harness, admin: &Client, name: &str, cluster: &Value) -> String {
    let (_, tok) = admin.post("/api/v1/enrollment-tokens", json!({"ttl_minutes": 10})).await;
    let dir = tempfile::tempdir().unwrap();
    let facts = HostFacts { hostname: name.into(), os: "linux".into(), arch: "x86_64".into() };
    let id = enroll(
        &StatePaths::new(dir.path()),
        AgentConfig::new(h.agent_url.clone()),
        tok["token"].as_str().unwrap(),
        facts,
    )
    .await
    .unwrap()
    .server_id
    .to_string();
    let (s, b) =
        admin.call(Method::PATCH, &format!("/api/v1/servers/{id}"), Some(json!({"cluster_id": cluster["id"]}))).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    id
}

#[tokio::test]
async fn swarm_membership_is_checked_guarded_and_tracked() {
    let h = start().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let (_, plain) = admin.post("/api/v1/clusters", json!({"name": "plain"})).await;
    let (_, swarm) = admin.post("/api/v1/clusters", json!({"name": "swarm-eu", "mode": "swarm"})).await;
    let a = node(&h, &admin, "lab-a", &swarm).await;
    let b = node(&h, &admin, "lab-b", &plain).await;
    let members = format!("/api/v1/clusters/{}/swarm/members", swarm["id"].as_str().unwrap());

    // Without MFA even an administrator is refused (guardrail).
    let (s, _) = admin.post(&members, json!({"server_id": a, "role": "manager"})).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    // Operators may not manage swarms at all.
    let mut op =
        user_with_role(&h, &admin, "op@example.com", "operator", &format!("workspace:{}", h.default_org)).await;
    step_up(&mut op).await;
    let (s, _) = op.post(&members, json!({"server_id": a, "role": "manager"})).await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    step_up(&mut admin).await;
    // Only Swarm clusters have a swarm.
    let plain_members = format!("/api/v1/clusters/{}/swarm/members", plain["id"].as_str().unwrap());
    let (s, e) = admin.post(&plain_members, json!({"server_id": b, "role": "manager"})).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{e}");
    // The node must be in the cluster.
    let (s, e) = admin.post(&members, json!({"server_id": b, "role": "manager"})).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{e}");
    // The first node is a manager.
    let (s, e) = admin.post(&members, json!({"server_id": a, "role": "worker"})).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{e}");
    // A bad advertise address never reaches the node.
    let (s, _) = admin.post(&members, json!({"server_id": a, "role": "manager", "advertise_addr": "0.0.0.0"})).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    let (s, started) = admin.post(&members, json!({"server_id": a, "role": "manager", "autolock": true})).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{started}");
    assert_eq!(started["job"]["kind"], "swarm.member");
    // A second request while the first runs is refused.
    let (s, _) = admin.post(&members, json!({"server_id": a, "role": "manager"})).await;
    assert_eq!(s, StatusCode::CONFLICT);

    let (s, view) = admin.get(&format!("/api/v1/clusters/{}/swarm", swarm["id"].as_str().unwrap())).await;
    assert_eq!(s, StatusCode::OK, "{view}");
    assert_eq!(view["mode"], "swarm");
    assert_eq!(view["members"][0]["server_id"], a);
    assert_eq!(view["members"][0]["hostname"], "lab-a");
    assert!(matches!(view["members"][0]["state"].as_str(), Some("joining" | "failed")), "{view}");
    assert_eq!(view["swarm"]["swarm_id"], "", "the swarm is claimed but not created until the first node joined");
    assert_eq!(view["swarm"]["founder"], a);
    assert!(view.to_string().find("SWMTKN").is_none(), "tokens never leave the server");

    // A swarm member cannot move to another cluster; it leaves the swarm first.
    let (s, e) =
        admin.call(Method::PATCH, &format!("/api/v1/servers/{a}"), Some(json!({"cluster_id": plain["id"]}))).await;
    assert_eq!(s, StatusCode::CONFLICT, "{e}");
    assert!(e.to_string().contains("Swarm"), "{e}");

    // Assigning a node with a swarm role joins it right away.
    let (s, e) = admin
        .call(
            Method::PATCH,
            &format!("/api/v1/servers/{b}"),
            Some(json!({"cluster_id": swarm["id"], "swarm_role": "manager"})),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT, "the first node's join still runs, so a second manager has no swarm yet: {e}");
    // Joins and denials are audited.
    let (_, audit) = admin.get("/api/v1/audit?limit=200").await;
    assert!(audit.to_string().contains("swarm.member.join"), "{audit}");
}

/// Services in a Swarm cluster (D-154): containers only, settings checked,
/// and a deploy needs a swarm manager.
#[tokio::test]
async fn swarm_services_are_containers_with_checked_settings() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let (_, swarm) = admin.post("/api/v1/clusters", json!({"name": "swarm-eu", "mode": "swarm"})).await;
    let (_, shop) = admin.post("/api/v1/projects", json!({"name": "shop"})).await;
    let (s, env) = admin
        .post("/api/v1/environments", json!({"name": "prod", "cluster_id": swarm["id"], "project_id": shop["id"]}))
        .await;
    assert_eq!(s, StatusCode::CREATED, "{env}");

    // System services do not run on Swarm.
    let (s, e) = admin
        .post(
            "/api/v1/services",
            json!({"environment_id": env["id"], "name": "cron", "runtime": "systemd", "command": "true"}),
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{e}");

    // Swarm settings are validated.
    let bad = json!({"environment_id": env["id"], "name": "web", "runtime": "container", "image": "nginx:1.27",
        "swarm": {"replicas": 3, "constraints": ["rm -rf /"]}});
    let (s, _) = admin.post("/api/v1/services", bad).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    let good = json!({"environment_id": env["id"], "name": "web", "runtime": "container", "image": "nginx:1.27",
        "ports": ["8080:80"], "swarm": {"replicas": 3, "constraints": ["node.role==worker"], "spread": ["node.labels.zone"]}});
    let (s, web) = admin.post("/api/v1/services", good).await;
    assert_eq!(s, StatusCode::CREATED, "{web}");
    assert_eq!(web["spec"]["swarm"]["replicas"], 3);

    // Without a manager there is nothing to deploy to.
    let id = web["id"].as_str().unwrap();
    let (s, e) = admin.post(&format!("/api/v1/services/{id}/deploy"), json!({})).await;
    assert_eq!(s, StatusCode::CONFLICT, "{e}");
    let (s, e) = admin.post(&format!("/api/v1/services/{id}/swarm"), json!({"kind": "scale", "replicas": 5000})).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{e}");

    // Release rollouts are not available on Swarm yet.
    let yaml = "apiVersion: forge/v1\nkind: Deployment\nmetadata:\n  name: web\n  environment: prod\nspec:\n  runtime: container\n  artifact:\n    image: nginx:1.27\n";
    let (s, _) = admin.post("/api/v1/deployments/plan", json!({"yaml": yaml})).await;
    assert!(s.is_client_error(), "{s}");
}
