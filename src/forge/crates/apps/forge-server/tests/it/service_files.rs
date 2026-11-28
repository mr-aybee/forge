//! D-178, Compose-format services files: a plan writes nothing, apply
//! creates the services, places them one copy per node and submits deploy
//! jobs, applying again changes nothing, a service dropped from the file is
//! removed (and only file-managed ones are), and scale and export work.
#![allow(clippy::unwrap_used)]

use std::time::Duration;

use forge_core::ports::{ServerRepository, ServiceRepository};
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::app_deploy::fake_node;
use crate::common::*;

async fn environment(c: &Client) -> Value {
    let (_, cl) = c.post("/api/v1/clusters", json!({"name": "files"})).await;
    let (_, pr) = c.post("/api/v1/projects", json!({"name": "shop"})).await;
    let (s, ns) =
        c.post("/api/v1/environments", json!({"name": "prod", "cluster_id": cl["id"], "project_id": pr["id"]})).await;
    assert_eq!(s, StatusCode::CREATED, "{ns}");

    ns
}

async fn node(h: &Harness, ns: &Value, hostname: &str) -> forge_core::ServerId {
    let server = forge_core::model::Server {
        id: forge_core::ServerId::new(),
        workspace_id: h.default_org.parse().unwrap(),
        environment_id: None,
        cluster_id: Some(ns["cluster_id"].as_str().unwrap().parse().unwrap()),
        role: forge_core::model::NodeRole::Worker,
        display_name: None,
        hostname: hostname.into(),
        os: "linux".into(),
        arch: "x86_64".into(),
        agent_version: "0".into(),
        status: forge_core::model::AgentStatus::Online,
        last_seen_at: None,
        cert_serial: None,
        cert_not_after: None,
        drained: false,
        created_at: forge_core::Timestamp(0),
    };
    h.app.state.store.create_server(&server).await.unwrap();

    server.id
}

const FILE: &str = r#"
name: shop
services:
  web:
    image: nginx:1.27
    ports: ["8080:80"]
    deploy:
      replicas: 2
  cache:
    image: redis:7
"#;

async fn wait_all(c: &Client, applied: &Value) {
    for j in applied["jobs"].as_array().unwrap() {
        let done = wait_job(c, j["id"].as_str().unwrap(), Duration::from_secs(60)).await;
        assert_eq!(done["status"], "succeeded", "{done}");
    }
}

#[tokio::test]
async fn a_services_file_is_planned_applied_scaled_and_exported() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let ns = environment(&admin).await;
    let (a, b) = (node(&h, &ns, "lab-a").await, node(&h, &ns, "lab-b").await);
    let (_fa, _fb) = (fake_node(&h, a, true), fake_node(&h, b, true));
    let body = |file: &str| json!({ "environment_id": ns["id"], "file": file });
    let env_id = ns["id"].as_str().unwrap().parse().unwrap();

    // A plan writes nothing.
    let (s, plan) = admin.post("/api/v1/services/plan", body(FILE)).await;
    assert_eq!(s, StatusCode::OK, "{plan}");
    assert_eq!(plan["source"], "shop");
    assert!(plan["changes"].as_array().unwrap().iter().all(|c| c["action"] == "create"), "{plan}");
    assert!(h.app.state.store.list_services(Some(env_id)).await.unwrap().is_empty());

    // Apply: two services, web on both nodes, one job per copy.
    let (s, applied) = admin.post("/api/v1/services/apply", body(FILE)).await;
    assert_eq!(s, StatusCode::OK, "{applied}");
    assert_eq!(applied["jobs"].as_array().unwrap().len(), 3, "{applied}");
    wait_all(&admin, &applied).await;
    let services = h.app.state.store.list_services(Some(env_id)).await.unwrap();
    assert_eq!(services.len(), 2);
    let web = services.iter().find(|s| s.name == "web").unwrap();
    assert_eq!(web.source, "shop");
    assert_eq!(h.app.state.store.list_instances(web.id).await.unwrap().len(), 2);

    // Applying again changes nothing.
    let (_, again) = admin.post("/api/v1/services/apply", body(FILE)).await;
    assert!(again["changes"].as_array().unwrap().iter().all(|c| c["action"] == "unchanged"), "{again}");
    assert!(again["jobs"].as_array().unwrap().is_empty());

    // A service made in the console is never removed by the file.
    let (s, own) = admin
        .post(
            "/api/v1/services",
            json!({ "environment_id": ns["id"], "name": "admin", "runtime": "container", "image": "x:1" }),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{own}");

    // Scale web to one node: one copy is removed.
    let (s, scaled) = admin.post(&format!("/api/v1/services/{}/scale", web.id), json!({ "replicas": 1 })).await;
    assert_eq!(s, StatusCode::OK, "{scaled}");
    assert_eq!(scaled["changes"][0]["removed_nodes"].as_array().unwrap().len(), 1, "{scaled}");
    wait_all(&admin, &scaled).await;
    assert_eq!(h.app.state.store.list_instances(web.id).await.unwrap().len(), 1);

    // Export round-trips: applying the exported service changes nothing.
    let (s, exported) = admin.get(&format!("/api/v1/services/{}/export", web.id)).await;
    assert_eq!(s, StatusCode::OK, "{exported}");
    let yaml = exported["yaml"].as_str().unwrap();
    assert!(yaml.contains("replicas: 1"), "{yaml}");
    let (_, re) =
        admin.post("/api/v1/services/plan", json!({ "environment_id": ns["id"], "file": yaml, "prune": false })).await;
    assert_eq!(re["changes"][0]["action"], "unchanged", "{re}\n{yaml}");
    let (s, all) = admin.get(&format!("/api/v1/environments/{}/services/export", ns["id"].as_str().unwrap())).await;
    assert_eq!(s, StatusCode::OK);
    assert!(all["yaml"].as_str().unwrap().contains("admin:"));

    // Drop cache from the file: only cache goes.
    let only_web = "name: shop\nservices:\n  web:\n    image: nginx:1.27\n    ports: [\"8080:80\"]\n";
    let (s, pruned) = admin.post("/api/v1/services/apply", body(only_web)).await;
    assert_eq!(s, StatusCode::OK, "{pruned}");
    let cache = pruned["changes"].as_array().unwrap().iter().find(|c| c["service"] == "cache").unwrap().clone();
    assert_eq!(cache["action"], "remove");
    wait_all(&admin, &pruned).await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);

    loop {
        h.app.state.reconciler.tick().await.unwrap();
        let names: Vec<String> =
            h.app.state.store.list_services(Some(env_id)).await.unwrap().into_iter().map(|s| s.name).collect();

        if !names.contains(&"cache".to_owned()) {
            assert!(names.contains(&"admin".to_owned()) && names.contains(&"web".to_owned()), "{names:?}");
            break;
        }

        assert!(tokio::time::Instant::now() < deadline, "cache was not removed: {names:?}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn refusals_name_the_key_and_too_few_nodes_are_refused() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let ns = environment(&admin).await;
    node(&h, &ns, "lab-a").await;

    let bad = "services:\n  web:\n    image: x\n    build: .\n";
    let (s, r) = admin.post("/api/v1/services/plan", json!({ "environment_id": ns["id"], "file": bad })).await;
    assert!(s.is_client_error(), "{r}");
    assert!(r.to_string().contains("services.web.build"), "{r}");

    let many = "services:\n  web:\n    image: x\n    deploy: { replicas: 3 }\n";
    let (s, r) = admin.post("/api/v1/services/plan", json!({ "environment_id": ns["id"], "file": many })).await;
    assert!(s.is_client_error(), "{r}");
    assert!(r.to_string().contains("3 nodes wanted, 1 eligible"), "{r}");
}
