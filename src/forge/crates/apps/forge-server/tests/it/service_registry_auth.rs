//! Private images on container services: a deploy whose image lives in a
//! registry the workspace knows sends Docker's `X-Registry-Auth` in the
//! signed command's own `registry_auth` field, resolved in the step. Public
//! images and actions that pull nothing send none, and neither the job nor
//! the audit log ever holds the credential.
#![allow(clippy::unwrap_used)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use forge_proto::messages::{CommandBody, CommandResult, command_body::Command};
use prost::Message;
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::app_deploy::node;
use crate::common::*;

const SECRET: &str = "ghp_registry_s3cret";

/// What the fake node saw of one app action.
#[derive(Debug, Clone)]
struct Seen {
    action: String,
    image: String,
    registry_auth: String,
}

/// Plays a node's agent: Docker answers, every app action succeeds, and
/// each app action is kept for the test to read.
struct Node {
    seen: Arc<Mutex<Vec<Seen>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Node {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn agent(h: &Harness, server: forge_core::ServerId) -> Node {
    let hub = h.app.state.hub.clone();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();

    let task = tokio::spawn(async move {
        let wake = hub.notifier(server);

        loop {
            for cmd in hub.take(server, 16) {
                let body = CommandBody::decode(cmd.body.as_slice()).unwrap();
                let out = match body.command {
                    Some(Command::ContainerAction(_)) => json!({ "version": "28.0" }),
                    Some(Command::AppAction(a)) => {
                        let spec: Value = serde_json::from_str(&a.spec_json).unwrap();
                        log.lock().unwrap().push(Seen {
                            action: a.action.clone(),
                            image: spec["image"].as_str().unwrap_or_default().to_owned(),
                            registry_auth: a.registry_auth.clone(),
                        });
                        json!({ "state": "running", "logs": null })
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

/// A container service `name` with `image` in a new environment, placed on `server`.
async fn service(admin: &Client, name: &str, image: &str, server: forge_core::ServerId) -> String {
    let (_, cl) = admin.post("/api/v1/clusters", json!({ "name": format!("{name}-cl") })).await;
    let (_, pr) = admin.post("/api/v1/projects", json!({ "name": format!("{name}-pr") })).await;
    let (s, ns) = admin
        .post(
            "/api/v1/environments",
            json!({ "name": format!("{name}-env"), "cluster_id": cl["id"], "project_id": pr["id"] }),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{ns}");
    let (s, app) = admin
        .post(
            "/api/v1/services",
            json!({ "environment_id": ns["id"], "name": name, "runtime": "container", "image": image }),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{app}");
    let id = app["id"].as_str().unwrap().to_owned();
    let (s, r) = admin.put(&format!("/api/v1/services/{id}/instances"), json!([{ "server_id": server }])).await;
    assert_eq!(s, StatusCode::OK, "{r}");

    id
}

async fn deploy(admin: &Client, app: &str) -> Value {
    let (s, jobs) = admin.post(&format!("/api/v1/services/{app}/deploy"), json!({})).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{jobs}");
    let id =
        jobs.as_array().unwrap().iter().find(|j| j["kind"] == "app.action").unwrap()["id"].as_str().unwrap().to_owned();

    wait_job(admin, &id, Duration::from_secs(60)).await
}

/// The last app action `action` the node saw.
fn last(node: &Node, action: &str) -> Seen {
    node.seen.lock().unwrap().iter().rev().find(|s| s.action == action).cloned().unwrap()
}

#[tokio::test]
async fn private_images_carry_the_registry_credential_only_in_the_command() {
    let h = start().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    step_up(&mut admin).await;
    let (s, r) = admin
        .post(
            "/api/v1/registries",
            json!({ "name": "GitHub", "kind": "ghcr", "host": "ghcr.io", "username": "bot", "secret": SECRET }),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{r}");

    let server = node(&h, "lab-b").await;
    let fake = agent(&h, server);

    // An image on the registry's host: the deploy carries its credential.
    let private = service(&admin, "api", "ghcr.io/acme/app:1", server).await;
    let done = deploy(&admin, &private).await;
    assert_eq!(done["status"], "succeeded", "{done}");
    let sent = last(&fake, "deploy");
    assert_eq!(sent.image, "ghcr.io/acme/app:1");
    assert!(!sent.registry_auth.is_empty(), "a private image is pulled with credentials");
    let decoded = forge_secrets::crypto::b64url_decode(sent.registry_auth.trim_end_matches('=')).unwrap();
    let auth: Value = serde_json::from_slice(&decoded).unwrap();
    assert_eq!(auth, json!({ "username": "bot", "password": SECRET, "serveraddress": "ghcr.io" }));

    // Actions that pull nothing carry none.
    let (s, r) = admin
        .post(&format!("/api/v1/services/{private}/instances/{server}/actions"), json!({ "action": "restart" }))
        .await;
    assert_eq!(s, StatusCode::OK, "{r}");
    wait_job(&admin, r["job_id"].as_str().unwrap(), Duration::from_secs(60)).await;
    assert!(last(&fake, "restart").registry_auth.is_empty());

    // An image on another host gets no credential.
    let public = service(&admin, "web", "nginx:1.27", server).await;
    let done_public = deploy(&admin, &public).await;
    assert_eq!(done_public["status"], "succeeded", "{done_public}");
    let sent = last(&fake, "deploy");
    assert_eq!(sent.image, "nginx:1.27");
    assert!(sent.registry_auth.is_empty(), "a public image is pulled without credentials");

    // No job (payload or state), job list or audit record holds the secret
    // or the header made of it.
    let header = last_private_header(&fake);

    for p in [
        format!("/api/v1/jobs/{}", done["id"].as_str().unwrap()),
        "/api/v1/jobs".to_owned(),
        "/api/v1/audit?limit=500".to_owned(),
        format!("/api/v1/services/{private}"),
    ] {
        let (_, body) = admin.get(&p).await;
        let text = body.to_string();
        assert!(!text.contains(SECRET) && !text.contains(&header), "{p}: {text}");
    }
}

/// The header the node got for the private image.
fn last_private_header(node: &Node) -> String {
    node.seen.lock().unwrap().iter().rev().find(|s| !s.registry_auth.is_empty()).unwrap().registry_auth.clone()
}
