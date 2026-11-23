//! Alert subjects and rule templates (D-118): the catalogue endpoints, subject
//! validation on save, a database template firing on a stopped installation
//! and a disk rule firing from the volumes a real agent reports.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::time::Duration;

use crate::common::*;
use forge_agent::enroll::{HostFacts, enroll};
use forge_agent::{Agent, AgentConfig, StatePaths};
use forge_core::model::{AgentStatus, Installation, NodeRole, Server};
use forge_core::ports::{InstallationRepository, ServerRepository};
use forge_core::{InstallationId, ServerId, Timestamp, WorkspaceId};
use reqwest::StatusCode;
use serde_json::{Value, json};

/// Polls `f` until it yields a value, with a deadline.
async fn wait_for<F, Fut>(what: &str, secs: u64, mut f: F) -> Value
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<Value>>,
{
    let end = std::time::Instant::now() + Duration::from_secs(secs);
    while std::time::Instant::now() < end {
        if let Some(v) = f().await {
            return v;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!("timed out waiting for {what}");
}

/// A rule body from a template as the API lists it.
fn from_template(t: &Value, scope_ids: &[String]) -> Value {
    json!({
        "name": t["name"],
        "severity": t["severity"],
        "scope": {"kind": t["scope"], "ids": scope_ids},
        "subject": t["subject"],
        "spec": t["spec"],
    })
}

async fn template(admin: &Client, id: &str) -> Value {
    let (s, all) = admin.get("/api/v1/alert-rule-templates").await;
    assert_eq!(s, StatusCode::OK, "{all}");
    all.as_array().unwrap().iter().find(|t| t["id"] == id).cloned().unwrap_or_else(|| panic!("no template {id}"))
}

#[tokio::test]
async fn subjects_and_templates_are_listed_and_subjects_validated() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;

    let (s, subjects) = admin.get("/api/v1/alert-subjects").await;
    assert_eq!(s, StatusCode::OK, "{subjects}");
    let list = subjects["subjects"].as_array().unwrap();
    let find = |name: &str| list.iter().find(|x| x["subject"] == name).cloned().unwrap();
    assert_eq!(find("package.state")["scopes"], json!(["installations"]));
    assert_eq!(find("node.disk.max_used_percent")["unit"], "percent");
    assert_eq!(find("app.instance.state")["states"], json!(["active", "inactive", "failed"]));
    assert_eq!(find("health.readiness.{check}")["pattern"], true);
    assert!(subjects["seen_health"].is_array());

    let (s, all) = admin.get("/api/v1/alert-rule-templates").await;
    assert_eq!(s, StatusCode::OK);
    assert!(all.as_array().unwrap().len() >= 20);
    let (_, db) = admin.get("/api/v1/alert-rule-templates?place=database").await;
    assert!(db.as_array().unwrap().iter().all(|t| t["place"] == "database"));
    assert!(db.as_array().unwrap().iter().any(|t| t["id"] == "database-stopped"));
    let (s, _) = admin.get("/api/v1/alert-rule-templates?place=moon").await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // Every template saves as a rule as it is (placeholders filled).
    for t in all.as_array().unwrap() {
        let mut body = from_template(t, &[]);
        body["subject"] = json!(t["subject"].as_str().unwrap().replace("{check}", "http"));
        let (s, r) = admin.post("/api/v1/alert-rules", body).await;
        assert_eq!(s, StatusCode::CREATED, "{}: {r}", t["id"]);
    }

    // A typo, a subject for another scope and an unknown installation are refused.
    let mut bad = from_template(&template(&admin, "node-cpu-high").await, &[]);
    bad["subject"] = json!("node.cpu.precent");
    let (s, body) = admin.post("/api/v1/alert-rules", bad).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(body.to_string().contains("did you mean `node.cpu.percent`"), "{body}");
    let mut bad = from_template(&template(&admin, "database-stopped").await, &[]);
    bad["scope"] = json!({"kind": "servers"});
    let (s, body) = admin.post("/api/v1/alert-rules", bad).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(body.to_string().contains("installations"), "{body}");
    let bad = from_template(&template(&admin, "database-stopped").await, &[InstallationId::new().to_string()]);
    assert_eq!(admin.post("/api/v1/alert-rules", bad).await.0, StatusCode::BAD_REQUEST);
    h.app.stop().await;
}

#[tokio::test]
async fn the_database_stopped_template_fires_on_a_stopped_installation() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let ws: WorkspaceId = h.default_org.parse().unwrap();
    let store = &h.app.state.store;
    let node = Server {
        id: ServerId::new(),
        workspace_id: ws,
        environment_id: None,
        cluster_id: None,
        role: NodeRole::Worker,
        display_name: None,
        hostname: "db-1".into(),
        os: "linux".into(),
        arch: "x86_64".into(),
        agent_version: "0".into(),
        status: AgentStatus::Online,
        last_seen_at: None,
        cert_serial: None,
        cert_not_after: None,
        created_at: Timestamp(0),
    };
    store.create_server(&node).await.unwrap();
    let install = |state: &str| Installation {
        id: InstallationId::new(),
        workspace_id: ws,
        server_id: node.id,
        package: "postgresql".into(),
        version: "17".into(),
        params: BTreeMap::new(),
        status: "installed".into(),
        state: Some(state.into()),
        env: BTreeMap::new(),
        last_job_id: None,
        last_error: None,
        versions: None,
        update_policy: "notify".into(),
        created_at: Timestamp(0),
        updated_at: Timestamp(0),
    };
    let stopped = install("stopped");
    let running = Installation { package: "redis".into(), ..install("running") };
    store.put_installation(&stopped).await.unwrap();
    store.put_installation(&running).await.unwrap();

    // The template as listed, scoped to both installations; its two-minute
    // duration shortened to one second so the test stays quick.
    let mut body =
        from_template(&template(&admin, "database-stopped").await, &[stopped.id.to_string(), running.id.to_string()]);
    assert_eq!(body["spec"]["mode"], json!({"mode": "sustained", "for_ms": 120_000}));
    body["spec"]["mode"]["for_ms"] = json!(1_000);
    let (s, rule) = admin.post("/api/v1/alert-rules", body).await;
    assert_eq!(s, StatusCode::CREATED, "{rule}");

    let alarm = wait_for("a database-stopped alarm", 90, || {
        let c = &admin;
        async move { c.get("/api/v1/alarms?active=true").await.1.as_array().and_then(|a| a.first().cloned()) }
    })
    .await;
    assert_eq!(alarm["resource"], json!({"kind": "installation", "id": stopped.id.to_string()}));
    assert_eq!(alarm["server"], node.id.to_string(), "the installation's node");
    assert_eq!(alarm["subject"], "package.state");
    assert_eq!(alarm["fired_value"], json!({"type": "state", "value": "stopped"}));
    let (_, active) = admin.get("/api/v1/alarms?active=true").await;
    assert_eq!(active.as_array().unwrap().len(), 1, "the running one does not fire: {active}");
    let (s, one) = admin.get(&format!("/api/v1/alarms/{}", alarm["id"].as_str().unwrap())).await;
    assert_eq!(s, StatusCode::OK, "visible through its node: {one}");
    h.app.stop().await;
}

#[tokio::test]
async fn a_disk_rule_fires_from_the_volumes_an_agent_reports() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    // The disk-filling template with its threshold below 0 %: any local
    // volume fires, so the test proves the path from volumes to alarm.
    let mut body = from_template(&template(&admin, "node-disk-filling").await, &[]);
    assert_eq!(body["subject"], "node.disk.max_used_percent");
    body["spec"]["trigger"]["value"] = json!(-1.0);
    body["spec"]["mode"] = json!({"mode": "immediate"});
    assert_eq!(admin.post("/api/v1/alert-rules", body).await.0, StatusCode::CREATED);
    let (_, tok) = admin.post("/api/v1/enrollment-tokens", json!({"ttl_minutes": 10})).await;
    let dir = tempfile::tempdir().unwrap();
    let facts = HostFacts { hostname: "disk-01".into(), os: "linux".into(), arch: "x86_64".into() };
    let id = enroll(
        &StatePaths::new(dir.path()),
        AgentConfig::new(h.agent_url.clone()),
        tok["token"].as_str().unwrap(),
        facts,
    )
    .await
    .unwrap()
    .server_id;
    let (_stop, rx) = tokio::sync::watch::channel(false);
    tokio::spawn(Agent::load(StatePaths::new(dir.path())).unwrap().run(rx));

    let alarm = wait_for("a disk alarm", 180, || {
        let c = &admin;
        async move { c.get("/api/v1/alarms?active=true").await.1.as_array().and_then(|a| a.first().cloned()) }
    })
    .await;
    assert_eq!(alarm["resource"]["id"], id);
    assert_eq!(alarm["subject"], "node.disk.max_used_percent");
    let v = alarm["fired_value"]["value"].as_f64().unwrap();
    assert!((0.0..=100.0).contains(&v), "{v}");
    h.app.stop().await;
}
