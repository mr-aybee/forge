//! The analytics worker process (D-141): forge-server spawns it, talks to it
//! over its control socket, and restarts it when it dies, hangs or grows too
//! large. While it is down, analytics endpoints answer 503
//! `analytics_worker_down` and the rest of forge-server keeps working.
//!
//! The worker binary is looked up next to the test executable. When it is
//! missing the tests fail if `FORGE_REQUIRE_WORKER_BIN=1` and are skipped
//! otherwise (`cargo build -p forge-analytics-worker` builds it).
#![allow(clippy::unwrap_used)]

use std::time::{Duration, Instant};

use crate::common::*;
use reqwest::StatusCode;
use serde_json::{Value, json};

/// `FORGE_ANALYTICS_WORKER_BIN`, else the nearest `forge-analytics-worker`
/// above the test executable.
fn worker_binary() -> Option<std::path::PathBuf> {
    if let Some(p) = std::env::var_os("FORGE_ANALYTICS_WORKER_BIN").map(std::path::PathBuf::from) {
        return p.is_file().then_some(p);
    }
    let name = format!("forge-analytics-worker{}", std::env::consts::EXE_SUFFIX);
    let exe = std::env::current_exe().ok()?;
    exe.ancestors().skip(1).take(6).map(|d| d.join(&name)).find(|p| p.is_file())
}

fn require_or_skip() -> Option<std::path::PathBuf> {
    let bin = worker_binary();
    if bin.is_none() {
        assert!(
            std::env::var("FORGE_REQUIRE_WORKER_BIN").as_deref() != Ok("1"),
            "forge-analytics-worker not found next to the test target; build it first"
        );
        eprintln!("skipped: forge-analytics-worker binary not built");
    }
    bin
}

async fn start_with_worker(bin: std::path::PathBuf, limit_mb: u32) -> Harness {
    start_configured(move |c| {
        c.analytics.engine = forge_server::config::AnalyticsEngine::Memory;
        c.analytics.worker_binary = Some(bin);
        c.analytics.worker_memory_limit_mb = limit_mb;
    })
    .await
}

/// Polls the analytics status until the worker runs a pid other than `not`.
async fn running(admin: &Client, not: Option<u64>) -> Value {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let (s, v) = admin.get("/api/v1/analytics/status").await;
        assert_eq!(s, StatusCode::OK, "{v}");
        let pid = v["worker"]["pid"].as_u64();
        if v["worker"]["status"] == "running" && v["ready"] == true && pid.is_some() && pid != not {
            return v;
        }
        assert!(Instant::now() < deadline, "the worker never came up: {v}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// A service with an ingest key: `(service id, token)`.
async fn service_with_key(h: &Harness, admin: &Client) -> (String, String) {
    let (_, cl) = admin.post("/api/v1/clusters", json!({ "name": "logs" })).await;
    let (_, project) = admin.post("/api/v1/projects", json!({ "name": "logs" })).await;
    let (s, env) = admin
        .post("/api/v1/environments", json!({ "name": "prod", "cluster_id": cl["id"], "project_id": project["id"] }))
        .await;
    assert_eq!(s, StatusCode::CREATED, "{env}");
    let (s, svc) = admin
        .post(
            "/api/v1/services",
            json!({ "environment_id": env["id"], "name": "api", "runtime": "systemd", "command": "python3 app.py" }),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{svc}");
    let id = svc["id"].as_str().unwrap().to_owned();
    let (s, key) = admin.post(&format!("/api/v1/services/{id}/log-keys"), json!({ "name": "test" })).await;
    assert_eq!(s, StatusCode::CREATED, "{key}");
    let _ = h;
    (id, key["token"].as_str().unwrap().to_owned())
}

async fn ingest(h: &Harness, token: &str, message: &str) -> (StatusCode, Value) {
    let body = format!("{{\"@t\":\"2024-05-01T10:00:00Z\",\"@mt\":\"{message}\",\"@l\":\"Warning\"}}\n");
    let r = reqwest::Client::new()
        .post(format!("{}/api/events/raw?clef", h.base))
        .header("X-Seq-ApiKey", token)
        .body(body)
        .send()
        .await
        .unwrap();
    let status = r.status();
    (status, r.json().await.unwrap_or(Value::Null))
}

fn kill(pid: u64) {
    let ok = std::process::Command::new("kill").args(["-9", &pid.to_string()]).status().unwrap().success();
    assert!(ok, "kill -9 {pid}");
}

/// Resident memory of this test process (the in-process forge-server and
/// the test harness), MiB.
fn own_rss_mib() -> Option<u64> {
    let s = std::fs::read_to_string("/proc/self/status").ok()?;
    let kb: u64 = s.lines().find_map(|l| l.strip_prefix("VmRSS:"))?.split_whitespace().next()?.parse().ok()?;
    Some(kb / 1024)
}

/// Logs go through the worker; killing it during ingest and during
/// searches gives 503 `analytics_worker_down` (never a hang or a 500),
/// forge-server stays up, the worker is restarted and audited, and
/// everything works again.
#[cfg(unix)]
#[tokio::test]
async fn the_worker_serves_logs_is_restarted_when_killed_and_the_api_recovers() {
    let Some(bin) = require_or_skip() else { return };
    let h = start_with_worker(bin, 2048).await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    step_up(&mut admin).await;
    let (svc, token) = service_with_key(&h, &admin).await;
    let first = running(&admin, None).await;
    let pid = first["worker"]["pid"].as_u64().unwrap();

    // Stored through the worker and found by a search.
    let (s, body) = ingest(&h, &token, "before the crash").await;
    assert_eq!(s, StatusCode::CREATED, "{body}");
    let (s, page) = admin.get(&format!("/api/v1/logs?services={svc}")).await;
    assert_eq!(s, StatusCode::OK, "{page}");
    assert_eq!(page["events"][0]["message"], "before the crash", "{page}");
    let (s, hist) =
        admin.get(&format!("/api/v1/logs/histogram?services={svc}&from=1714550000000&to=1714560000000")).await;
    assert_eq!(s, StatusCode::OK, "{hist}");
    let (s, bad) = ingest(&h, "flk_not-a-key", "x").await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "{bad}");
    eprintln!(
        "RSS with the worker running: test process (in-process forge-server) {:?} MiB, worker {} MiB",
        own_rss_mib(),
        first["worker"]["rss_bytes"].as_u64().unwrap_or(0) / (1024 * 1024)
    );

    // Kill it while ingest and searches are running.
    let load = {
        let (base, token, svc) = (h.base.clone(), token.clone(), svc.clone());
        let cookies =
            Client { http: admin.http.clone(), base: admin.base.clone(), csrf: admin.csrf.clone(), bearer: None };
        tokio::spawn(async move {
            let mut seen = Vec::new();
            for i in 0..40 {
                let body = format!("{{\"@t\":\"2024-05-01T10:00:00Z\",\"@mt\":\"load {i}\"}}\n");
                let r = reqwest::Client::new()
                    .post(format!("{base}/api/events/raw"))
                    .header("X-Seq-ApiKey", &token)
                    .body(body)
                    .send()
                    .await
                    .unwrap();
                seen.push(r.status());
                let (s, _) = cookies.get(&format!("/api/v1/logs?services={svc}")).await;
                seen.push(s);
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            seen
        })
    };
    tokio::time::sleep(Duration::from_millis(200)).await;
    kill(pid);
    // Straight after the kill: down, with the typed error.
    let (s, e) = ingest(&h, &token, "during the crash").await;
    assert_eq!((s, e["title"].as_str()), (StatusCode::SERVICE_UNAVAILABLE, Some("analytics_worker_down")), "{e}");
    let (s, e) = admin.get(&format!("/api/v1/logs?services={svc}")).await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE, "{e}");
    // forge-server itself is fine.
    let (s, _) = admin.get("/api/v1/servers").await;
    assert_eq!(s, StatusCode::OK);
    let seen = load.await.unwrap();
    assert!(
        seen.iter().all(|s| matches!(*s, StatusCode::CREATED | StatusCode::OK | StatusCode::SERVICE_UNAVAILABLE)),
        "only stored, served or 503 while the worker restarts: {seen:?}"
    );

    // Restarted, audited, and serving again.
    let second = running(&admin, Some(pid)).await;
    assert!(second["worker"]["restarts"].as_u64().unwrap() >= 1, "{second}");
    assert_eq!(second["worker"]["last_restart_reason"], "exited", "{second}");
    let (s, body) = ingest(&h, &token, "after the restart").await;
    assert_eq!(s, StatusCode::CREATED, "{body}");
    let (s, page) = admin.get(&format!("/api/v1/logs?services={svc}")).await;
    assert_eq!(s, StatusCode::OK, "{page}");
    assert!(page["events"].as_array().unwrap().iter().any(|e| e["message"] == "after the restart"), "{page}");
    let (_, log) = admin.get("/api/v1/audit?action=analytics.worker.restart").await;
    assert!(!log.as_array().unwrap().is_empty(), "the restart is audited: {log}");
    let (s, v) = admin.get("/api/v1/analytics/settings").await;
    assert_eq!((s, v["worker"]["status"].as_str()), (StatusCode::OK, Some("running")), "{v}");
    h.app.stop().await;
}

/// A worker over its memory limit is restarted, and the restart says why.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_worker_over_its_memory_limit_is_restarted() {
    let Some(bin) = require_or_skip() else { return };
    // 1 MiB: any process is over it (forge.toml refuses values under 128).
    let h = start_with_worker(bin, 1).await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let (_, log) = admin.get("/api/v1/audit?action=analytics.worker.restart").await;
        if log.as_array().is_some_and(|l| l.iter().any(|e| e["details"]["reason"] == "memory_limit")) {
            break;
        }
        assert!(Instant::now() < deadline, "no memory-limit restart was audited: {log}");
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let (_, v) = admin.get("/api/v1/analytics/status").await;
    assert_eq!(v["worker"]["memory_limit_bytes"], 1024 * 1024, "{v}");
    h.app.stop().await;
}
