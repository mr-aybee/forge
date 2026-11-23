//! Agent enrollment, mTLS heartbeats, signed commands, job-runner and
//! agent-level enforcement, revocation.
#![allow(clippy::unwrap_used)]

use std::time::Duration;

use crate::common::*;
use forge_agent::enroll::{HostFacts, enroll};
use forge_agent::{Agent, AgentConfig, StatePaths};
use reqwest::StatusCode;
use serde_json::{Value, json};

struct Enrolled {
    server_id: String,
    stop: tokio::sync::watch::Sender<bool>,
    _dir: tempfile::TempDir,
}

async fn token(admin: &Client) -> String {
    let (s, body) = admin.post("/api/v1/enrollment-tokens", json!({"ttl_minutes": 10})).await;
    assert_eq!(s, StatusCode::CREATED, "{body}");
    let token = body["token"].as_str().unwrap().to_owned();
    // The enroll commands prompt for the token instead of embedding it.
    for os in ["linux", "windows"] {
        let cmd = body["install"][os].as_str().unwrap();
        assert!(!cmd.contains(&token) && cmd.contains("FORGE_ENROLL_TOKEN"), "{os}: {cmd}");
    }
    token
}

async fn enroll_and_run(h: &Harness, admin: &Client, hostname: &str, allowed: Option<Vec<&str>>) -> Enrolled {
    let dir = tempfile::tempdir().unwrap();
    let paths = StatePaths::new(dir.path());
    let mut config = AgentConfig::new(h.agent_url.clone());
    if let Some(a) = allowed {
        config.allowed_commands = a.into_iter().map(str::to_owned).collect();
    }
    let facts = HostFacts { hostname: hostname.into(), os: "linux".into(), arch: "x86_64".into() };
    let id = enroll(&paths, config, &token(admin).await, facts).await.unwrap();
    let agent = Agent::load(paths).unwrap();
    let (stop, rx) = tokio::sync::watch::channel(false);
    tokio::spawn(agent.run(rx));
    wait_status(admin, &id.server_id, "online").await;
    Enrolled { server_id: id.server_id, stop, _dir: dir }
}

async fn wait_status(c: &Client, server: &str, status: &str) -> Value {
    for _ in 0..100 {
        let (_, s) = c.get(&format!("/api/v1/servers/{server}")).await;
        if s["status"] == status {
            return s;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("server {server} never became {status}");
}

async fn send(c: &Client, server: &str, command: Value) -> (StatusCode, Value) {
    c.post(&format!("/api/v1/servers/{server}/commands"), json!({ "command": command })).await
}

#[tokio::test]
async fn agent_enrolls_heartbeats_and_runs_signed_commands() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let a = enroll_and_run(&h, &admin, "web-01", None).await;

    let (_, servers) = admin.get("/api/v1/servers").await;
    assert_eq!(servers[0]["hostname"], "web-01");
    assert!(servers[0]["cert_serial"].is_string());
    // Enrolled over loopback with no control node yet: it is the Forge host's own agent.
    assert_eq!(servers[0]["role"], "control");

    let (s, job) = send(&admin, &a.server_id, json!({"type": "ping"})).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{job}");
    let done = wait_job(&admin, job["id"].as_str().unwrap(), Duration::from_secs(20)).await;
    assert_eq!(done["status"], "succeeded", "{done}");
    assert_eq!(done["state"]["output"]["pong"], true);

    let (_, job) = send(&admin, &a.server_id, json!({"type": "collect_facts"})).await;
    let done = wait_job(&admin, job["id"].as_str().unwrap(), Duration::from_secs(20)).await;
    assert_eq!(done["state"]["output"]["agent_version"], env!("CARGO_PKG_VERSION"));
    let _ = a.stop.send(true);
}

#[tokio::test]
async fn enrollment_token_is_single_use() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let t = token(&admin).await;
    let facts = || HostFacts { hostname: "h".into(), os: "linux".into(), arch: "x86_64".into() };
    let d1 = tempfile::tempdir().unwrap();
    enroll(&StatePaths::new(d1.path()), AgentConfig::new(h.agent_url.clone()), &t, facts()).await.unwrap();
    let d2 = tempfile::tempdir().unwrap();
    let err =
        enroll(&StatePaths::new(d2.path()), AgentConfig::new(h.agent_url.clone()), &t, facts()).await.unwrap_err();
    assert!(err.to_string().contains("403"), "{err}");
}

#[tokio::test]
async fn token_with_wrong_ca_pin_is_refused_by_agent() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let t = token(&admin).await;
    let (secret, _) = t.rsplit_once('.').unwrap();
    let forged = format!("{secret}.{}", "ab".repeat(32));
    let d = tempfile::tempdir().unwrap();
    let facts = HostFacts { hostname: "h".into(), os: "linux".into(), arch: "x86_64".into() };
    let err =
        enroll(&StatePaths::new(d.path()), AgentConfig::new(h.agent_url.clone()), &forged, facts).await.unwrap_err();
    assert!(err.to_string().contains("fingerprint"), "{err}");
}

#[tokio::test]
async fn agent_local_policy_denies_unlisted_commands() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let a = enroll_and_run(&h, &admin, "locked-down", Some(vec!["ping"])).await;
    let (_, job) = send(&admin, &a.server_id, json!({"type": "service_status", "name": "sshd"})).await;
    let done = wait_job(&admin, job["id"].as_str().unwrap(), Duration::from_secs(20)).await;
    assert_eq!(done["status"], "failed");
    assert!(done["last_error"].as_str().unwrap().contains("not allowed by this agent's local policy"), "{done}");
    let _ = a.stop.send(true);
}

#[tokio::test]
async fn unauthorized_users_cannot_command_servers() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let a = enroll_and_run(&h, &admin, "web-02", None).await;
    let viewer =
        user_with_role(&h, &admin, "customer@example.com", "customer", &format!("workspace:{}", h.default_org)).await;
    // Customers can see the server ...
    assert_eq!(viewer.get(&format!("/api/v1/servers/{}", a.server_id)).await.0, StatusCode::OK);
    // ... but not command it: the job engine refuses the submission.
    let (s, _) = send(&viewer, &a.server_id, json!({"type": "ping"})).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let _ = a.stop.send(true);
}

#[tokio::test]
async fn revoked_agent_is_cut_off() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let a = enroll_and_run(&h, &admin, "compromised", None).await;
    let (s, _) = admin
        .post(&format!("/api/v1/servers/{}/revoke", a.server_id), json!({"confirm_hostname": "compromised"}))
        .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    wait_status(&admin, &a.server_id, "revoked").await;
    // Commands to a revoked server fail.
    let (_, job) = send(&admin, &a.server_id, json!({"type": "ping"})).await;
    let done = wait_job(&admin, job["id"].as_str().unwrap(), Duration::from_secs(20)).await;
    assert_eq!(done["status"], "failed");
    // The audit log records the revocation.
    let (_, entries) = admin.get("/api/v1/audit").await;
    assert!(entries.as_array().unwrap().iter().any(|e| e["action"] == "server.revoke"));
    let _ = a.stop.send(true);
}

#[tokio::test]
async fn unknown_command_types_are_rejected_by_the_api() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let a = enroll_and_run(&h, &admin, "web-03", None).await;
    let (s, _) = send(&admin, &a.server_id, json!({"type": "shell", "cmd": "id"})).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let _ = a.stop.send(true);
}
