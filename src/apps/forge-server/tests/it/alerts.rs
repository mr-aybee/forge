//! Alerts (#11, D-087): a rule on a node's agent status opens an alarm when
//! the agent stops and resolves it when the agent is back; rules are
//! validated, alarms acknowledged and noted, everything audited.
#![allow(clippy::unwrap_used)]

use std::time::Duration;

use forge_agent::enroll::{HostFacts, enroll};
use forge_agent::{Agent, AgentConfig, StatePaths};
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::common::*;

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

fn offline_rule() -> Value {
    json!({
        "name": "Node offline",
        "severity": "critical",
        "scope": {"kind": "servers"},
        "subject": "node.agent.status",
        "spec": {
            "trigger": {"kind": "state_in", "states": ["offline"]},
            "mode": {"mode": "immediate"},
            "recovery": {"condition": {"kind": "state_in", "states": ["online"]}, "hold_ms": 0},
        },
    })
}

#[tokio::test]
async fn an_offline_node_opens_an_alarm_that_recovery_resolves() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;

    // Validation: bad subject, bad condition, foreign node in the scope.
    let mut bad = offline_rule();
    bad["subject"] = json!("Node Status");
    assert_eq!(admin.post("/api/v1/alert-rules", bad).await.0, StatusCode::BAD_REQUEST);
    let mut bad = offline_rule();
    bad["spec"]["mode"] = json!({"mode": "sustained", "for_ms": 0});
    assert_eq!(admin.post("/api/v1/alert-rules", bad).await.0, StatusCode::BAD_REQUEST);
    let mut bad = offline_rule();
    bad["scope"] = json!({"kind": "servers", "ids": [forge_core::ServerId::new()]});
    assert_eq!(admin.post("/api/v1/alert-rules", bad).await.0, StatusCode::BAD_REQUEST);

    let (s, rule) = admin.post("/api/v1/alert-rules", offline_rule()).await;
    assert_eq!(s, StatusCode::CREATED, "{rule}");
    let (_, got) = admin.get(&format!("/api/v1/alert-rules/{}", rule["id"].as_str().unwrap())).await;
    assert_eq!(got["condition"], "is offline");

    // A node that comes online, then stops.
    let (_, tok) = admin.post("/api/v1/enrollment-tokens", json!({"ttl_minutes": 10})).await;
    let dir = tempfile::tempdir().unwrap();
    let paths = StatePaths::new(dir.path());
    let config = AgentConfig::new(h.agent_url.clone());
    let facts = HostFacts { hostname: "web-01".into(), os: "linux".into(), arch: "x86_64".into() };
    let id = enroll(&paths, config, tok["token"].as_str().unwrap(), facts).await.unwrap().server_id;
    let (stop, rx) = tokio::sync::watch::channel(false);
    tokio::spawn(Agent::load(StatePaths::new(dir.path())).unwrap().run(rx));
    let server = id.clone();
    wait_for("online", 30, || {
        let (c, server) = (&admin, server.clone());
        async move {
            let (_, s) = c.get(&format!("/api/v1/servers/{server}")).await;
            (s["status"] == "online").then_some(s)
        }
    })
    .await;
    assert!(admin.get("/api/v1/alarms?active=true").await.1.as_array().unwrap().is_empty());
    stop.send(true).unwrap();

    // Offline after 5 s (test config), then the next pass (15 s) fires.
    let alarm = wait_for("an alarm", 60, || {
        let c = &admin;
        async move {
            let (_, v) = c.get("/api/v1/alarms?active=true").await;
            v.as_array().and_then(|a| a.first().cloned())
        }
    })
    .await;
    assert_eq!(alarm["resource"]["id"], id);
    assert_eq!(alarm["severity"], "critical");
    assert_eq!(alarm["fired_value"], json!({"type": "state", "value": "offline"}));
    let aid = alarm["id"].as_str().unwrap().to_owned();

    // Acknowledge and note: still firing.
    let (s, acked) = admin.post(&format!("/api/v1/alarms/{aid}/acknowledge"), json!({"note": "on it"})).await;
    assert_eq!(s, StatusCode::OK, "{acked}");
    assert_eq!(acked["state"], "firing");
    assert!(acked["acknowledged_at"].is_number());
    let (s, _) = admin.post(&format!("/api/v1/alarms/{aid}/notes"), json!({"note": "agent stopped for a test"})).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, _) = admin.post(&format!("/api/v1/alarms/{aid}/notes"), json!({"note": ""})).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // The agent comes back: the alarm resolves.
    let (_stop2, rx2) = tokio::sync::watch::channel(false);
    tokio::spawn(Agent::load(StatePaths::new(dir.path())).unwrap().run(rx2));
    let done = wait_for("recovery", 60, || {
        let (c, aid) = (&admin, aid.clone());
        async move {
            let (_, v) = c.get(&format!("/api/v1/alarms/{aid}")).await;
            (v["state"] == "resolved").then_some(v)
        }
    })
    .await;
    let kinds: Vec<&str> = done["events"].as_array().unwrap().iter().map(|e| e["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["fired", "acknowledged", "note", "resolved"], "{done}");

    // Audit: the rule and the operator actions, never the evaluator's passes.
    let (_, audit) = admin.get("/api/v1/audit?limit=100").await;
    let text = serde_json::to_string(&audit).unwrap();

    for a in ["alert.rule.create", "alert.alarm.acknowledge", "alert.alarm.note"] {
        assert!(text.contains(a), "{a} audited");
    }

    // Deleting the rule keeps the alarm as history.
    let path = format!("/api/v1/alert-rules/{}", rule["id"].as_str().unwrap());
    let (s, _) = admin.call(reqwest::Method::DELETE, &path, None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert_eq!(admin.get(&format!("/api/v1/alarms/{aid}")).await.1["state"], "resolved");
    h.app.stop().await;
}

/// Prints real `forge alerts` output for the docs. Run on demand:
/// `cargo test -p forge-server --test it capture_alerts_cli -- --ignored --nocapture`
/// after `cargo build -p forge-cli`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "documentation capture; needs the forge CLI binary"]
async fn capture_alerts_cli() {
    let cli = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .join(std::env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| "target".into()))
        .join("debug/forge");
    let cli = if cli.with_extension("exe").exists() { cli.with_extension("exe") } else { cli };
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let (_, key) = admin.post("/api/v1/api-keys", json!({"name": "docs", "expires_days": 1})).await;
    let run = |args: &[&str]| {
        let out = std::process::Command::new(&cli)
            .args(args)
            .env("FORGE_SERVER", &h.base)
            .env("FORGE_TOKEN", key["secret"].as_str().unwrap())
            .env("FORGE_ALLOW_HTTP", "1")
            .env("NO_COLOR", "1")
            .output()
            .unwrap();
        println!(
            "$ forge {}   # exit {}\n{}{}",
            args.join(" "),
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    };
    let dir = tempfile::tempdir().unwrap();
    let rule = dir.path().join("node-offline.json");
    let mut r = offline_rule();
    r["spec"]["mode"] = json!({"mode": "sustained", "for_ms": 60_000});
    std::fs::write(&rule, r.to_string()).unwrap();
    run(&["alerts", "create-rule", &format!("@{}", rule.display())]);
    run(&["alerts", "rules"]);
    let (_, tok) = admin.post("/api/v1/enrollment-tokens", json!({"ttl_minutes": 10})).await;
    let adir = tempfile::tempdir().unwrap();
    let facts = HostFacts { hostname: "web-01".into(), os: "linux".into(), arch: "x86_64".into() };
    enroll(&StatePaths::new(adir.path()), AgentConfig::new(h.agent_url.clone()), tok["token"].as_str().unwrap(), facts)
        .await
        .unwrap();
    let (stop, rx) = tokio::sync::watch::channel(false);
    tokio::spawn(Agent::load(StatePaths::new(adir.path())).unwrap().run(rx));
    tokio::time::sleep(Duration::from_secs(5)).await;
    stop.send(true).unwrap();
    let alarm = wait_for("an alarm", 180, || {
        let c = &admin;
        async move { c.get("/api/v1/alarms?active=true").await.1.as_array().and_then(|a| a.first().cloned()) }
    })
    .await;
    let id = alarm["id"].as_str().unwrap().to_owned();
    run(&["alerts", "alarms", "--active"]);
    run(&["alerts", "ack", &id, "--note", "Rebooting web-01"]);
    let (_s2, rx2) = tokio::sync::watch::channel(false);
    tokio::spawn(Agent::load(StatePaths::new(adir.path())).unwrap().run(rx2));
    wait_for("recovery", 60, || {
        let (c, id) = (&admin, id.clone());
        async move { (c.get(&format!("/api/v1/alarms/{id}")).await.1["state"] == "resolved").then_some(Value::Null) }
    })
    .await;
    run(&["alerts", "alarm", &id]);
    run(&["alerts", "alarms"]);
    h.app.stop().await;
}

/// Review of #23: the rules per workspace are capped by `[alerts]`.
#[tokio::test]
async fn rules_per_workspace_are_capped() {
    let h = start_with(|c| c.alerts.max_rules_per_workspace = 1).await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    assert_eq!(admin.post("/api/v1/alert-rules", offline_rule()).await.0, StatusCode::CREATED);
    let (s, body) = admin.post("/api/v1/alert-rules", offline_rule()).await;
    assert_eq!(s, StatusCode::CONFLICT, "{body}");
    assert!(body.to_string().contains("max_rules_per_workspace"));
    h.app.stop().await;
}

/// Slice 2: heartbeat telemetry becomes `node.cpu.percent` (and memory and
/// root disk) observations, and a threshold rule on them opens an alarm.
#[tokio::test]
async fn heartbeat_measures_feed_threshold_rules() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;

    // Any CPU reading at all (>= 0 %) fires at once: the point is the path.
    let rule = json!({
        "name": "CPU seen",
        "severity": "info",
        "scope": {"kind": "servers"},
        "subject": "node.cpu.percent",
        "spec": {
            "trigger": {"kind": "threshold", "cmp": "ge", "value": 0.0},
            "mode": {"mode": "immediate"},
            "recovery": {"hold_ms": 0},
        },
    });
    assert_eq!(admin.post("/api/v1/alert-rules", rule).await.0, StatusCode::CREATED);
    let (_, tok) = admin.post("/api/v1/enrollment-tokens", json!({"ttl_minutes": 10})).await;
    let dir = tempfile::tempdir().unwrap();
    let facts = HostFacts { hostname: "web-02".into(), os: "linux".into(), arch: "x86_64".into() };
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

    // Generous: GitHub-hosted runners run this suite several times slower.
    let alarm = wait_for("a CPU alarm", 240, || {
        let c = &admin;
        async move { c.get("/api/v1/alarms?active=true").await.1.as_array().and_then(|a| a.first().cloned()) }
    })
    .await;
    assert_eq!(alarm["resource"]["id"], id);
    assert_eq!(alarm["subject"], "node.cpu.percent");
    assert_eq!(alarm["fired_value"]["type"], "num");
    let v = alarm["fired_value"]["value"].as_f64().unwrap();
    assert!((0.0..=100.0).contains(&v), "{v}");
    h.app.stop().await;
}
