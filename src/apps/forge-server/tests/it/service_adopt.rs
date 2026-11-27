//! Adopting a running container as a service (D-180): the endpoint reads
//! the container without recreating it, needs a new agent and MFA, audits
//! the adoption, and the first deploy replaces the adopted container. The
//! upgrade path adopts the containers of removed object stores.
#![allow(clippy::unwrap_used)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use forge_core::tags::FactsRepository;
use forge_proto::messages::{CommandBody, CommandResult, command_body::Command};
use prost::Message;
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::app_deploy::node;
use crate::common::*;

const SECRET: &str = "hunter2-root-secret";

/// Plays a node's agent: answers `inspect` with a RustFS container and
/// keeps every command it gets (`container:<op>` or the app spec).
struct Node {
    seen: Arc<Mutex<Vec<(String, Value)>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Node {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn inspected(name: &str) -> Value {
    json!({
        "id": "35dfaf1f3aba", "name": name, "image": "rustfs/rustfs:1.0.6",
        "state": { "Status": "running" }, "restart_policy": "unless-stopped",
        "ports": { "9000/tcp": [{ "HostIp": "0.0.0.0", "HostPort": "19000" }],
                   "9001/tcp": [{ "HostIp": "0.0.0.0", "HostPort": "19001" }] },
        "mounts": [{ "source": "/var/lib/docker/volumes/media-data/_data", "destination": "/data", "mode": "z" }],
        "networks": ["bridge"],
        "env": [
            { "key": "PATH", "value": "/usr/bin" },
            { "key": "RUSTFS_ACCESS_KEY", "value": "admin" },
            { "key": "RUSTFS_SECRET_KEY", "value": SECRET },
        ],
    })
}

async fn agent(h: &Harness, server: forge_core::ServerId, facts: Value) -> Node {
    h.app.state.store.put_facts(server, &facts, forge_core::Timestamp(1)).await.unwrap();
    let hub = h.app.state.hub.clone();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();

    let task = tokio::spawn(async move {
        let wake = hub.notifier(server);

        loop {
            for cmd in hub.take(server, 16) {
                let body = CommandBody::decode(cmd.body.as_slice()).unwrap();
                let out = match body.command {
                    Some(Command::ContainerAction(c)) => {
                        log.lock().unwrap().push((format!("container:{}", c.op), json!(c.id)));
                        if c.op == "inspect" { inspected(&c.id) } else { json!({}) }
                    }
                    Some(Command::AppAction(a)) => {
                        log.lock().unwrap().push((a.action.clone(), serde_json::from_str(&a.spec_json).unwrap()));
                        json!({ "state": "active", "logs": null })
                    }
                    _ => json!({}),
                };

                let result = CommandResult {
                    operation_id: body.operation_id,
                    ok: true,
                    output: out.to_string(),
                    ..Default::default()
                };
                hub.resolve(server, result);
            }

            wake.notified().await;
        }
    });

    Node { seen, task }
}

async fn environment(admin: &Client, name: &str) -> Value {
    let (_, cl) = admin.post("/api/v1/clusters", json!({ "name": format!("{name}-cl") })).await;
    let (_, pr) = admin.post("/api/v1/projects", json!({ "name": format!("{name}-pr") })).await;
    let (s, ns) = admin
        .post("/api/v1/environments", json!({ "name": name, "cluster_id": cl["id"], "project_id": pr["id"] }))
        .await;
    assert_eq!(s, StatusCode::CREATED, "{ns}");

    ns
}

async fn action(admin: &Client, id: &str, server: forge_core::ServerId, action: &str) -> Value {
    let (s, r) =
        admin.post(&format!("/api/v1/services/{id}/instances/{server}/actions"), json!({ "action": action })).await;
    assert_eq!(s, StatusCode::OK, "{r}");

    wait_job(admin, r["job_id"].as_str().unwrap(), Duration::from_secs(60)).await
}

#[tokio::test]
async fn adoption_reads_the_container_without_recreating_it_and_the_first_deploy_replaces_it() {
    let h = start().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let server = node(&h, "lab-c").await;
    let ns = environment(&admin, "prod").await;
    let body = json!({ "environment_id": ns["id"], "server_id": server, "container": "rustfs-old" });

    // Reading environment values needs an MFA-verified session.
    let (s, _) = admin.post("/api/v1/services/adopt", body.clone()).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    step_up(&mut admin).await;

    // An agent without the `adopted_containers` fact cannot keep it.
    let old = agent(&h, server, json!({ "container_options": true })).await;
    let (s, e) = admin.post("/api/v1/services/adopt", body.clone()).await;
    assert_eq!((s, e["title"].as_str()), (StatusCode::CONFLICT, Some("agent_update_required")), "{e}");
    drop(old);

    let fake = agent(&h, server, json!({ "adopted_containers": true, "container_options": true })).await;
    let (s, r) = admin
        .post(
            "/api/v1/services/adopt",
            json!({ "environment_id": ns["id"], "server_id": server, "container": "forge-app-prod-web" }),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "already a Forge service: {r}");

    let (s, r) = admin.post("/api/v1/services/adopt", body.clone()).await;
    assert_eq!(s, StatusCode::CREATED, "{r}");
    let svc = &r["service"];
    assert_eq!((svc["name"].as_str(), svc["adopted_from"].as_str()), (Some("rustfs-old"), Some("rustfs-old")));
    assert_eq!(svc["spec"]["image"], "rustfs/rustfs:1.0.6");
    assert_eq!(svc["spec"]["ports"], json!(["19000:9000", "19001:9001"]));
    assert_eq!(svc["spec"]["volumes"], json!(["media-data:/data"]));
    assert_eq!(
        (r["instance"]["status"].as_str(), r["instance"]["state"].as_str()),
        (Some("deployed"), Some("running"))
    );
    assert!(!r.to_string().contains(SECRET), "{r}");

    // Only the inspect reached the node: nothing was recreated.
    let ops: Vec<String> = fake.seen.lock().unwrap().iter().map(|(k, _)| k.clone()).collect();
    assert_eq!(ops, ["container:inspect"]);

    // The same container twice is refused.
    let (s, _) = admin
        .post(
            "/api/v1/services/adopt",
            json!({ "environment_id": ns["id"], "server_id": server, "container": "rustfs-old", "name": "again" }),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT);

    let (_, audit) = admin.get("/api/v1/audit?limit=200").await;
    assert!(audit.to_string().contains("service.adopt"), "{audit}");
    assert!(!audit.to_string().contains(SECRET));

    // Until the first deploy, actions name the adopted container.
    let id = svc["id"].as_str().unwrap().to_owned();
    let done = action(&admin, &id, server, "status").await;
    assert_eq!(done["status"], "succeeded", "{done}");
    assert_eq!(fake.seen.lock().unwrap().last().unwrap().1["adopted_container"], "rustfs-old");

    // The first deploy tells the node to replace it, then forgets it.
    let done = action(&admin, &id, server, "deploy").await;
    assert_eq!(done["status"], "succeeded", "{done}");
    let (kind, spec) = fake.seen.lock().unwrap().last().cloned().unwrap();
    assert_eq!((kind.as_str(), spec["adopted_container"].as_str()), ("deploy", Some("rustfs-old")));
    let secret = spec["env"].as_array().unwrap().iter().find(|e| e[0] == "RUSTFS_SECRET_KEY").cloned();
    assert_eq!(secret, Some(json!(["RUSTFS_SECRET_KEY", SECRET])), "the sealed value reaches the node");
    let (_, now) = admin.get(&format!("/api/v1/services/{id}")).await;
    assert!(now["adopted_from"].is_null(), "{now}");

    action(&admin, &id, server, "status").await;
    assert!(fake.seen.lock().unwrap().last().unwrap().1.get("adopted_container").is_none());
}

#[tokio::test]
async fn the_upgrade_adopts_the_containers_of_removed_object_stores() {
    let h = start().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    step_up(&mut admin).await;
    let server = node(&h, "lab-c").await;
    let ns = environment(&admin, "storage").await;
    let (s, item) = admin
        .post(
            "/api/v1/vault/items",
            json!({ "name": "media (object store root)", "username": "admin", "password": SECRET }),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{item}");
    let fake = agent(&h, server, json!({ "adopted_containers": true })).await;

    // What migration 0009 leaves behind for one store.
    sqlx::query(
        "INSERT INTO object_store_adoptions
            (id, workspace_id, environment_id, server_id, slug, settings, console_port, credential_id, created_by, created_at)
         SELECT $1, $2, $3, $4, 'media', NULL, 19001, $5, 'user:' || id, 1 FROM users WHERE email = $6",
    )
    .bind(format!("store-1:{server}"))
    .bind(&h.default_org)
    .bind(ns["id"].as_str().unwrap())
    .bind(server.to_string())
    .bind(item["id"].as_str().unwrap())
    .bind(ADMIN_EMAIL)
    .execute(h.app.state.store.pool())
    .await
    .unwrap();

    assert_eq!(h.app.state.store_upgrade.run_once().await.unwrap(), 0);
    let (_, list) = admin.get(&format!("/api/v1/services?environment_id={}", ns["id"].as_str().unwrap())).await;
    let svc = list.as_array().unwrap().iter().find(|s| s["name"] == "media").cloned().unwrap();
    assert_eq!(svc["adopted_from"], "forge-objstore-media");
    assert_eq!(svc["spec"]["console"]["port"], 19001);
    let ops: Vec<String> = fake.seen.lock().unwrap().iter().map(|(k, _)| k.clone()).collect();
    assert_eq!(ops, ["container:inspect"], "the container is not restarted");

    // The root credential stays a vault item, linked to the service.
    let (_, items) = admin.get("/api/v1/vault/items").await;
    let it = items.as_array().unwrap().iter().find(|i| i["id"] == item["id"]).cloned().unwrap();
    assert!(it["links"].to_string().contains(svc["id"].as_str().unwrap()), "{it}");

    // Nothing is left to do, and a second pass changes nothing.
    assert_eq!(h.app.state.store_upgrade.run_once().await.unwrap(), 0);
    let (_, audit) = admin.get("/api/v1/audit?limit=200").await;
    assert!(audit.to_string().contains("system:upgrade"), "{audit}");
}
