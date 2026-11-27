//! Secrets and configs as files (D-179): a service stores vault item names
//! only; the deploy step unseals the values and sends them only inside the
//! signed command; referencing an item needs the vault's use permission;
//! an agent without the `service_files` fact is refused; and rotating the
//! item marks the service for a redeploy until the next deploy.
#![allow(clippy::unwrap_used)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use forge_core::tags::FactsRepository;
use forge_proto::messages::{CommandBody, CommandResult, command_body::Command};
use prost::Message;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

use crate::app_deploy::node;
use crate::common::*;

const FIRST: &str = "s3cret-file-value-1";
const SECOND: &str = "s3cret-file-value-2";

/// Plays a node's agent: Docker answers, every app action succeeds, and
/// the spec of every app action is kept for the test to read.
struct Node {
    specs: Arc<Mutex<Vec<Value>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Node {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn agent(h: &Harness, server: forge_core::ServerId, facts: Value) -> Node {
    h.app.state.store.put_facts(server, &facts, forge_core::Timestamp(1)).await.unwrap();
    let hub = h.app.state.hub.clone();
    let specs = Arc::new(Mutex::new(Vec::new()));
    let seen = specs.clone();
    assert!(hub.take(server, 1).is_empty());

    let task = tokio::spawn(async move {
        let wake = hub.notifier(server);

        loop {
            for cmd in hub.take(server, 16) {
                let body = CommandBody::decode(cmd.body.as_slice()).unwrap();
                let out = match body.command {
                    Some(Command::ContainerAction(_)) => json!({ "version": "28.0" }),
                    Some(Command::AppAction(a)) => {
                        seen.lock().unwrap().push(serde_json::from_str::<Value>(&a.spec_json).unwrap());
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

    Node { specs, task }
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

fn service(ns: &Value, name: &str) -> Value {
    json!({
        "environment_id": ns["id"], "name": name, "runtime": "container", "image": "nginx:1.27",
        "secret_files": [
            { "kind": "secret", "item": "db password", "target": "/run/secrets/db" },
            { "kind": "config", "item": "db password", "target": "/etc/app/app.conf", "mode": 0o440, "uid": 101 },
        ],
    })
}

async fn deploy(admin: &Client, app: &str) -> Value {
    let (s, jobs) = admin.post(&format!("/api/v1/services/{app}/deploy"), json!({})).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{jobs}");
    let id =
        jobs.as_array().unwrap().iter().find(|j| j["kind"] == "app.action").unwrap()["id"].as_str().unwrap().to_owned();

    wait_job(admin, &id, Duration::from_secs(60)).await
}

#[tokio::test]
async fn deploys_carry_file_values_only_in_the_command_and_rotation_marks_the_service() {
    let h = start().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    step_up(&mut admin).await;
    let (s, item) =
        admin.post("/api/v1/vault/items", json!({ "name": "db password", "username": "", "password": FIRST })).await;
    assert_eq!(s, StatusCode::CREATED, "{item}");

    let server = node(&h, "lab-b").await;
    let fake = agent(&h, server, json!({ "service_files": true, "container_options": true })).await;
    let ns = environment(&admin, "prod").await;
    let (s, app) = admin.post("/api/v1/services", service(&ns, "api")).await;
    assert_eq!(s, StatusCode::CREATED, "{app}");
    assert_eq!(app["spec"]["secret_files"][0]["item"], "db password");
    let id = app["id"].as_str().unwrap().to_owned();
    let (s, r) = admin.put(&format!("/api/v1/services/{id}/instances"), json!([{ "server_id": server }])).await;
    assert_eq!(s, StatusCode::OK, "{r}");

    let done = deploy(&admin, &id).await;
    assert_eq!(done["status"], "succeeded", "{done}");
    let sent = fake.specs.lock().unwrap().last().cloned().unwrap();
    assert_eq!(
        sent["files"][0],
        json!({ "name": "db", "target": "/run/secrets/db", "mode": 0o400, "uid": 0, "gid": 0, "value": FIRST })
    );
    assert_eq!(sent["files"][1]["target"], "/etc/app/app.conf");
    assert_eq!((sent["files"][1]["mode"].as_u64(), sent["files"][1]["uid"].as_u64()), (Some(0o440), Some(101)));

    // Rotating the item marks the service; the next deploy sends the new
    // value and clears the mark.
    let path = format!("/api/v1/vault/items/{}", item["id"].as_str().unwrap());
    let (s, r) = admin.put(&path, json!({ "name": "db password", "password": SECOND })).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    let (_, now) = admin.get(&format!("/api/v1/services/{id}")).await;
    assert!(now["secrets_changed_at"].is_number(), "{now}");

    let done = deploy(&admin, &id).await;
    assert_eq!(done["status"], "succeeded", "{done}");
    assert_eq!(fake.specs.lock().unwrap().last().unwrap()["files"][0]["value"], SECOND);
    let (_, now) = admin.get(&format!("/api/v1/services/{id}")).await;
    assert!(now["secrets_changed_at"].is_null(), "{now}");

    // Other actions never carry the values.
    let (s, r) =
        admin.post(&format!("/api/v1/services/{id}/instances/{server}/actions"), json!({ "action": "status" })).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    wait_job(&admin, r["job_id"].as_str().unwrap(), Duration::from_secs(60)).await;
    assert!(fake.specs.lock().unwrap().last().unwrap().get("files").is_none());

    // No response, job or audit record carries a value.
    for p in [
        format!("/api/v1/services/{id}"),
        "/api/v1/services".to_owned(),
        "/api/v1/jobs".to_owned(),
        format!("/api/v1/jobs/{}", done["id"].as_str().unwrap()),
        "/api/v1/audit?limit=500".to_owned(),
        format!("/api/v1/services/{id}/export"),
    ] {
        let (_, body) = admin.get(&p).await;
        let text = body.to_string();
        assert!(!text.contains(FIRST) && !text.contains(SECOND), "{p}: {text}");
    }

    let (_, export) = admin.get(&format!("/api/v1/services/{id}/export")).await;
    assert!(export["yaml"].as_str().unwrap().contains("db password"), "{export}");
}

#[tokio::test]
async fn referencing_a_vault_item_needs_its_use_permission_and_a_new_agent() {
    let h = start().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    step_up(&mut admin).await;
    admin.post("/api/v1/vault/items", json!({ "name": "db password", "username": "", "password": FIRST })).await;
    let ns = environment(&admin, "stage").await;

    // A developer may deploy but may not use vault items: denied and audited.
    let dev = user_with_role(&h, &admin, "dev@example.com", "developer", &format!("workspace:{}", h.default_org)).await;
    let (s, r) = dev.post("/api/v1/services", service(&ns, "web")).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{r}");
    let (_, audit) = admin.get("/api/v1/audit?action=service.create").await;
    assert!(audit.to_string().contains("denied"), "{audit}");

    // An unknown item is refused by name.
    let mut bad = service(&ns, "web");
    bad["secret_files"][0]["item"] = json!("missing");
    let (s, r) = admin.post("/api/v1/services", bad).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{r}");
    assert!(r.to_string().contains("missing"), "{r}");

    // An agent without the `service_files` fact is refused before anything is sent.
    let server = node(&h, "lab-c").await;
    let fake = agent(&h, server, json!({ "container_options": true })).await;
    let (_, app) = admin.post("/api/v1/services", service(&ns, "web")).await;
    let id = app["id"].as_str().unwrap().to_owned();
    admin.put(&format!("/api/v1/services/{id}/instances"), json!([{ "server_id": server }])).await;
    let done = deploy(&admin, &id).await;
    assert_eq!(done["status"], "failed", "{done}");
    assert!(done.to_string().contains("update the agent first"), "{done}");
    assert!(fake.specs.lock().unwrap().is_empty());

    // A file entry in a services file is refused with the reason.
    let file = "services:\n  web:\n    image: nginx\n    secrets: [db]\nsecrets:\n  db:\n    file: ./db.txt\n";
    let (s, r) = admin
        .call(Method::POST, "/api/v1/services/plan", Some(json!({ "environment_id": ns["id"], "file": file })))
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{r}");
    assert!(r.to_string().contains("never reads files"), "{r}");
}
