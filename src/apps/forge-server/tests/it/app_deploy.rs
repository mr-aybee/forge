//! Issue #64, the deploy path end to end against a node played by the test:
//!
//! * deploying a container on a node that says it has no Docker Engine
//!   installs the engine first (the App Store's install job), and the deploy
//!   job waits for it;
//! * a node whose engine answers, or that Forge installed it on, gets no
//!   install; a node that is not connected is not asked at all;
//! * the state a deploy recorded survives following the container's logs;
//! * `POST /sites` takes typed `targets` without an `upstreams` list.
#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use forge_core::ports::{InstallationRepository, ServiceRepository};
use forge_proto::messages::{CommandBody, CommandResult, command_body::Command};
use prost::Message;
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::common::*;

pub(crate) async fn node(h: &Harness, hostname: &str) -> forge_core::ServerId {
    let server = forge_core::model::Server {
        id: forge_core::ServerId::new(),
        workspace_id: h.default_org.parse().unwrap(),
        environment_id: None,
        cluster_id: None,
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
    forge_core::ports::ServerRepository::create_server(&h.app.state.store, &server).await.unwrap();

    server.id
}

/// A container service `name` in a new environment, placed on `server`.
pub(crate) async fn container(admin: &Client, name: &str, server: forge_core::ServerId) -> String {
    let (_, cl) = admin.post("/api/v1/clusters", json!({ "name": format!("{name}-lab") })).await;
    let (_, group) = admin.post("/api/v1/projects", json!({ "name": format!("{name}-app") })).await;
    let (s, ns) = admin
        .post(
            "/api/v1/environments",
            json!({ "name": format!("{name}-prod"), "cluster_id": cl["id"], "project_id": group["id"] }),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{ns}");
    let (s, app) = admin
        .post(
            "/api/v1/services",
            json!({ "environment_id": ns["id"], "name": name, "runtime": "container", "image": "nginx:1.27" }),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{app}");
    let id = app["id"].as_str().unwrap().to_owned();
    let (s, r) = admin.put(&format!("/api/v1/services/{id}/instances"), json!([{ "server_id": server }])).await;
    assert_eq!(s, StatusCode::OK, "{r}");

    id
}

/// Plays the node's agent until dropped: polls for commands like a heartbeat
/// and answers them. `docker` is whether the node has a Docker Engine now;
/// installing the `docker` package turns it on.
pub(crate) struct FakeNode {
    docker: Arc<AtomicBool>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for FakeNode {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub(crate) fn fake_node(h: &Harness, server: forge_core::ServerId, docker: bool) -> FakeNode {
    let hub = h.app.state.hub.clone();
    let docker = Arc::new(AtomicBool::new(docker));
    let has_docker = docker.clone();

    // The first poll happens here, so the server sees a connected agent at once.
    assert!(hub.take(server, 1).is_empty());
    let task = tokio::spawn(async move {
        let wake = hub.notifier(server);

        loop {
            for cmd in hub.take(server, 16) {
                let body = CommandBody::decode(cmd.body.as_slice()).unwrap();
                let gone = format!("{} (/var/run/docker.sock: No such file)", forge_catalog::apps::DOCKER_UNAVAILABLE);

                let answer: Result<Value, String> = match body.command {
                    Some(Command::ContainerAction(c)) if c.op == "info" => {
                        if has_docker.load(Ordering::SeqCst) {
                            Ok(json!({ "version": "27.0" }))
                        } else {
                            Err(gone)
                        }
                    }
                    Some(Command::PackageAction(p)) => {
                        if p.package == "docker" && p.action == "install" {
                            has_docker.store(true, Ordering::SeqCst);
                        }

                        Ok(json!({ "output": "forge: done", "state": "running" }))
                    }
                    Some(Command::AppAction(_)) if !has_docker.load(Ordering::SeqCst) => Err(gone),
                    // A container's follow segment has log lines and no state.
                    Some(Command::AppAction(a)) if a.action == "follow" => {
                        Ok(json!({ "cursor": "1700000000", "lines": "started\n", "state": null }))
                    }
                    Some(Command::AppAction(_)) => Ok(json!({ "state": "active", "logs": null })),
                    _ => Err("unexpected command".into()),
                };

                let result = match answer {
                    Ok(out) => CommandResult {
                        operation_id: body.operation_id,
                        ok: true,
                        output: out.to_string(),
                        ..Default::default()
                    },
                    Err(error) => {
                        CommandResult { operation_id: body.operation_id, ok: false, error, ..Default::default() }
                    }
                };

                hub.resolve(server, result);
            }

            wake.notified().await;
        }
    });

    FakeNode { docker, task }
}

async fn instance(h: &Harness, app: &str) -> forge_core::model::ServiceInstance {
    h.app.state.store.list_instances(app.parse().unwrap()).await.unwrap().remove(0)
}

fn kinds(jobs: &Value) -> Vec<&str> {
    jobs.as_array().unwrap().iter().map(|j| j["kind"].as_str().unwrap()).collect()
}

#[tokio::test]
async fn deploying_a_container_installs_docker_on_a_node_that_has_none() {
    let h = start().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let server = node(&h, "lab-b").await;
    let app = container(&admin, "web", server).await;
    let fake = fake_node(&h, server, false);

    // Installing a package needs package.manage with MFA: without it nothing starts.
    let (s, r) = admin.post(&format!("/api/v1/services/{app}/deploy"), json!({})).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{r}");
    assert!(h.app.state.store.list_installations(Some(server)).await.unwrap().is_empty());
    assert_eq!(instance(&h, &app).await.status, "pending", "no deploy job either");

    step_up(&mut admin).await;
    let (s, jobs) = admin.post(&format!("/api/v1/services/{app}/deploy"), json!({})).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{jobs}");
    assert_eq!(kinds(&jobs), ["package.action", "app.action"], "the install comes first: {jobs}");
    assert_eq!(jobs[0]["payload"]["package"], "docker");
    assert_eq!(jobs[0]["payload"]["action"], "install");
    assert_eq!(jobs[0]["payload"]["server_id"], json!(server));

    // The deploy job waits for the install, then the node deploys.
    for j in jobs.as_array().unwrap() {
        let done = wait_job(&admin, j["id"].as_str().unwrap(), Duration::from_secs(60)).await;
        assert_eq!(done["status"], "succeeded", "{done}");
    }

    assert!(fake.docker.load(Ordering::SeqCst));
    let installed = h.app.state.store.list_installations(Some(server)).await.unwrap();
    assert_eq!(
        (installed.len(), installed[0].package.as_str(), installed[0].status.as_str()),
        (1, "docker", "installed")
    );
    let i = instance(&h, &app).await;
    assert_eq!((i.status.as_str(), i.state.as_deref()), ("deployed", Some("active")));
    let (_, audit) = admin.get("/api/v1/audit?action=service.deploy").await;
    assert!(audit.to_string().contains("\"runtime_installs\":1"), "{audit}");
    let (_, audit) = admin.get("/api/v1/audit?action=package.install").await;
    assert!(audit.to_string().contains("docker"), "{audit}");

    // Forge installed the engine: the next deploy starts no install.
    let (s, jobs) = admin.post(&format!("/api/v1/services/{app}/deploy"), json!({})).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{jobs}");
    assert_eq!(kinds(&jobs), ["app.action"]);
    let done = wait_job(&admin, jobs[0]["id"].as_str().unwrap(), Duration::from_secs(60)).await;
    assert_eq!(done["status"], "succeeded", "{done}");

    // Issue #64, RUNNING 0/N: following the container's logs answers
    // without a state; the state the deploy recorded stays.
    let (s, seg) =
        admin.post(&format!("/api/v1/services/{app}/instances/{server}/actions"), json!({ "action": "follow" })).await;
    assert_eq!(s, StatusCode::OK, "{seg}");
    let done = wait_job(&admin, seg["job_id"].as_str().unwrap(), Duration::from_secs(60)).await;
    assert_eq!(done["status"], "succeeded", "{done}");
    let (_, list) = admin.get("/api/v1/services").await;
    let listed = list.as_array().unwrap().iter().find(|a| a["id"] == json!(app)).unwrap();
    assert_eq!(listed["instances"][0]["state"], "active", "{listed}");
    assert_eq!(listed["instances"][0]["status"], "deployed", "{listed}");
}

#[tokio::test]
async fn docker_is_left_alone_when_it_answers_or_the_node_is_silent() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;

    // An engine installed outside Forge answers: no install, no MFA needed.
    let with_docker = node(&h, "lab-c").await;
    let app = container(&admin, "api", with_docker).await;
    let _fake = fake_node(&h, with_docker, true);
    let (s, jobs) = admin.post(&format!("/api/v1/services/{app}/deploy"), json!({})).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{jobs}");
    assert_eq!(kinds(&jobs), ["app.action"]);
    assert!(h.app.state.store.list_installations(Some(with_docker)).await.unwrap().is_empty());
    let done = wait_job(&admin, jobs[0]["id"].as_str().unwrap(), Duration::from_secs(60)).await;
    assert_eq!(done["status"], "succeeded", "{done}");

    // A node no agent is polling for is not asked (the request does not
    // wait for it) and gets no install; its deploy job is simply queued.
    let silent = node(&h, "deb-d").await;
    let app = container(&admin, "worker", silent).await;
    let started = std::time::Instant::now();
    let (s, jobs) = admin.post(&format!("/api/v1/services/{app}/deploy"), json!({})).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{jobs}");
    assert_eq!(kinds(&jobs), ["app.action"]);
    assert!(started.elapsed() < Duration::from_secs(10), "a silent node must not hold the request");
    assert!(h.app.state.store.list_installations(Some(silent)).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_site_takes_targets_without_an_upstreams_list() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let edge = node(&h, "edge-a").await;
    let backend = node(&h, "web-1").await;

    // `upstreams` left out: they are resolved from the typed targets.
    let site = json!({
        "name": "shop", "domains": ["shop.example.com"], "edges": [edge],
        "targets": [{ "kind": "node", "server_id": backend, "port": 8080 }],
    });
    let (s, preview) = admin.post("/api/v1/sites/preview", site.clone()).await;
    assert_eq!(s, StatusCode::OK, "{preview}");
    let (s, created) = admin.post("/api/v1/sites", site).await;
    assert_eq!(s, StatusCode::CREATED, "{created}");
    assert_eq!(created["config"]["spec"]["upstreams"], json!(["web-1:8080"]), "{created}");
    assert_eq!(created["config"]["spec"]["upstreams"], preview["upstreams"], "{preview}");

    // Neither upstreams nor targets: still refused, with the reason.
    let (s, e) =
        admin.post("/api/v1/sites", json!({ "name": "empty", "domains": ["e.example.com"], "edges": [edge] })).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{e}");
    assert!(e.to_string().contains("at least one upstream"), "{e}");

    // An explicit list works as before.
    let (s, created) = admin
        .post(
            "/api/v1/sites",
            json!({ "name": "plain", "domains": ["p.example.com"], "edges": [edge], "upstreams": ["10.0.0.5:8000"] }),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{created}");
}
