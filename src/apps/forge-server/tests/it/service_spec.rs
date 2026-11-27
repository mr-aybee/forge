//! The typed container spec (`plans/service-orchestration.md` step 1):
//! restart policy, capabilities, ulimits, tmpfs, DNS, sysctls, devices, init
//! and networks are stored and validated; added capabilities need
//! `stack.privileged`; and an edit based on an old version is refused (409).
#![allow(clippy::unwrap_used)]

use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::common::*;

async fn environment(c: &Client) -> Value {
    let (_, cl) = c.post("/api/v1/clusters", json!({"name": "shop"})).await;
    let (_, pr) = c.post("/api/v1/projects", json!({"name": "grp1"})).await;
    let (s, ns) =
        c.post("/api/v1/environments", json!({"name": "uat", "cluster_id": cl["id"], "project_id": pr["id"]})).await;
    assert_eq!(s, StatusCode::CREATED, "{ns}");

    ns
}

async fn node(h: &Harness) -> forge_core::ServerId {
    let id = forge_core::ServerId::new();
    let server = forge_core::model::Server {
        id,
        workspace_id: h.default_org.parse().unwrap(),
        environment_id: None,
        cluster_id: None,
        role: forge_core::model::NodeRole::Worker,
        display_name: None,
        hostname: "web-1".into(),
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

    id
}

fn body(ns: &Value, extra: Value) -> Value {
    let mut b = json!({ "environment_id": ns["id"], "name": "cache", "runtime": "container", "image": "redis:7" });

    for (k, v) in extra.as_object().unwrap() {
        b[k] = v.clone();
    }

    b
}

#[tokio::test]
async fn container_options_are_stored_and_validated() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let ns = environment(&admin).await;
    let options = json!({
        "restart_policy": "on-failure", "restart_max_attempts": 5,
        "cap_drop": ["ALL"], "ulimits": [{"name": "nofile", "soft": 1024, "hard": 4096}],
        "tmpfs": [{"target": "/tmp", "size_mb": 64}], "dns": ["192.0.2.53"],
        "init": true, "networks": ["backend"],
    });
    let (s, app) = admin.post("/api/v1/services", body(&ns, options)).await;
    assert_eq!(s, StatusCode::CREATED, "{app}");
    assert_eq!(app["version"], 0);
    assert_eq!(app["spec"]["restart_policy"], "on-failure");
    assert_eq!(app["spec"]["restart_max_attempts"], 5);
    assert_eq!(app["spec"]["ulimits"][0]["name"], "nofile");
    assert_eq!(app["spec"]["init"], true);

    // Bad values are refused at save time, naming the problem.
    for (bad, why) in [
        (json!({"name": "a1", "cap_drop": ["net admin"]}), "capability"),
        (json!({"name": "a2", "restart_max_attempts": 3}), "on-failure"),
        (json!({"name": "a3", "devices": [{"host": "/etc/shadow"}]}), "device"),
        (json!({"name": "a4", "sysctls": [["kernel", "1"]]}), "sysctl"),
        (json!({"name": "a5", "ulimits": [{"name": "nofile", "soft": 9, "hard": 1}]}), "ulimit"),
    ] {
        let (s, r) = admin.post("/api/v1/services", body(&ns, bad)).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{why}: {r}");
        assert!(r.to_string().contains(why), "{why}: {r}");
    }

    // Native services take a restart policy but not container options.
    let (s, r) = admin
        .post(
            "/api/v1/services",
            json!({"environment_id": ns["id"], "name": "worker", "runtime": "systemd", "command": "/usr/bin/true",
                   "restart_policy": "always", "init": true}),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{r}");
}

#[tokio::test]
async fn an_edit_from_an_old_version_is_refused() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let ns = environment(&admin).await;
    let (_, app) = admin.post("/api/v1/services", body(&ns, json!({}))).await;
    let id = app["id"].as_str().unwrap();
    let path = format!("/api/v1/services/{id}");

    // Two editors opened version 0; the first save wins.
    let (s, first) = admin.put(&path, body(&ns, json!({"version": 0, "image": "redis:7.2"}))).await;
    assert_eq!(s, StatusCode::OK, "{first}");
    assert_eq!(first["version"], 1);

    let (s, second) = admin.put(&path, body(&ns, json!({"version": 0, "image": "redis:7.4"}))).await;
    assert_eq!(s, StatusCode::CONFLICT, "{second}");
    assert!(second.to_string().contains("changed since you opened it"), "{second}");

    let (_, now) = admin.get(&path).await;
    assert_eq!(now["spec"]["image"], "redis:7.2");
    assert_eq!(now["version"], 1);

    // A client that sends no version (older CLI) still saves.
    let (s, r) = admin.put(&path, body(&ns, json!({"image": "redis:7.4"}))).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    assert_eq!(r["version"], 2);
}

#[tokio::test]
async fn added_capabilities_need_stack_privileged() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let ns = environment(&admin).await;
    let server = node(&h).await;
    let (s, app) = admin.post("/api/v1/services", body(&ns, json!({"cap_add": ["NET_ADMIN"]}))).await;
    assert_eq!(s, StatusCode::CREATED, "{app}");
    let id = app["id"].as_str().unwrap();
    let (s, r) = admin.put(&format!("/api/v1/services/{id}/instances"), json!([{"server_id": server}])).await;
    assert_eq!(s, StatusCode::OK, "{r}");

    // Without a step-up (MFA) the deploy is refused before a job exists.
    let (s, r) = admin.post(&format!("/api/v1/services/{id}/deploy"), json!({})).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{r}");
}
