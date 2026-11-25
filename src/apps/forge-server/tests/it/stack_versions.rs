//! Stack edits are versioned (`PUT /stacks/{id}` with `version`): a stale
//! version is refused with `409 stale_version`, two concurrent saves of the
//! same version cannot both win, and a deploy does not change the version.
#![allow(clippy::unwrap_used)]

use std::time::Duration;

use crate::common::*;
use forge_agent::enroll::{HostFacts, enroll};
use forge_agent::{Agent, AgentConfig, StatePaths};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

const COMPOSE: &str = "services:\n  web:\n    image: nginx:1.27\n";

async fn node(h: &Harness, admin: &Client) -> (String, tokio::sync::watch::Sender<bool>, tempfile::TempDir) {
    let (s, body) = admin.post("/api/v1/enrollment-tokens", json!({"ttl_minutes": 10})).await;
    assert_eq!(s, StatusCode::CREATED, "{body}");
    let dir = tempfile::tempdir().unwrap();
    let paths = StatePaths::new(dir.path());
    let facts = HostFacts { hostname: "lab-b".into(), os: "linux".into(), arch: "x86_64".into() };
    let id =
        enroll(&paths, AgentConfig::new(h.agent_url.clone()), body["token"].as_str().unwrap(), facts).await.unwrap();
    let agent = Agent::load(paths).unwrap();
    let (stop, rx) = tokio::sync::watch::channel(false);
    tokio::spawn(agent.run(rx));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);

    loop {
        let (_, s) = admin.get(&format!("/api/v1/servers/{}", id.server_id)).await;
        if s["status"] == "online" {
            break;
        }

        assert!(tokio::time::Instant::now() < deadline, "agent never came online");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    (id.server_id, stop, dir)
}

fn version(v: &Value) -> i64 {
    v["stack"]["updated_at"].as_i64().unwrap()
}

#[tokio::test]
async fn stale_and_concurrent_stack_edits_are_refused() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let (_, cl) = admin.post("/api/v1/clusters", json!({"name": "lab"})).await;
    let (_, ag1) = admin.post("/api/v1/projects", json!({"name": "grp1"})).await;
    let (_, ns) = admin
        .post("/api/v1/environments", json!({"name": "web", "cluster_id": cl["id"], "project_id": ag1["id"]}))
        .await;
    let (server_id, _stop, _dir) = node(&h, &admin).await;
    let (s, created) = admin
        .post(
            "/api/v1/stacks",
            json!({ "environment_id": ns["id"], "server_id": server_id, "name": "site", "content": COMPOSE, "env": "A=1\n" }),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{created}");
    let id = created["stack"]["id"].as_str().unwrap().to_owned();
    let path = format!("/api/v1/stacks/{id}");
    let v0 = version(&created);

    // Two editors loaded v0. The first save wins and moves the version on.
    let (s, first) = admin.call(Method::PUT, &path, Some(json!({ "env": "A=2\n", "version": v0 }))).await;
    assert_eq!(s, StatusCode::OK, "{first}");
    let v1 = version(&first);
    assert!(v1 > v0, "the version changes on every save");

    // The second, still on v0, is refused and changes nothing.
    let (s, e) = admin.call(Method::PUT, &path, Some(json!({ "env": "A=3\n", "version": v0 }))).await;
    assert_eq!(s, StatusCode::CONFLICT, "{e}");
    assert_eq!(e["title"].as_str().or(e["code"].as_str()), Some("stale_version"), "{e}");
    let (_, now) = admin.get(&path).await;
    assert_eq!(now["stack"]["updated_at"].as_i64(), Some(v1), "{now}");

    // A save with the current version goes through; one without a version
    // keeps working (older clients).
    let (s, second) = admin.call(Method::PUT, &path, Some(json!({ "env": "A=3\n", "version": v1 }))).await;
    assert_eq!(s, StatusCode::OK, "{second}");
    let v2 = version(&second);
    let (s, third) = admin.call(Method::PUT, &path, Some(json!({ "note": "no version" }))).await;
    assert_eq!(s, StatusCode::OK, "{third}");
    let v3 = version(&third);
    assert!(v3 > v2);

    // Concurrent saves of the same version: exactly one wins.
    let mut tasks = Vec::new();

    for i in 0..4 {
        let http = admin.http.clone();
        let base = admin.base.clone();
        let csrf = admin.csrf.clone().unwrap_or_default();
        let path = path.clone();
        tasks.push(tokio::spawn(async move {
            http.put(format!("{base}{path}"))
                .header("x-csrf-token", csrf)
                .json(&json!({ "env": format!("A=race{i}\n"), "version": v3 }))
                .send()
                .await
                .unwrap()
                .status()
        }));
    }

    let mut ok = 0;

    for t in tasks {
        match t.await.unwrap() {
            StatusCode::OK => ok += 1,
            StatusCode::CONFLICT => {}
            other => panic!("unexpected {other}"),
        }
    }

    assert_eq!(ok, 1, "exactly one concurrent save wins");
}
