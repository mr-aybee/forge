//! The real `forge-agent` binary starts, connects and stays up.
//!
//! In-process agent tests don't catch a panic in the binary's startup path
//! (the lab found one: a Tokio spawn before the runtime existed). This test
//! enrolls a node against the test server, runs the built binary, and checks
//! that it heartbeats and is still running a few seconds later.
//!
//! The binary is looked up next to the test executable's target directory.
//! When it is missing the test fails if `FORGE_REQUIRE_AGENT_BIN=1` (set in
//! CI, which builds it first) and is skipped otherwise.
#![allow(clippy::unwrap_used)]

use std::time::{Duration, Instant};

use forge_agent::enroll::{HostFacts, enroll};
use forge_agent::{AgentConfig, StatePaths};
use forge_core::model::AgentStatus;
use reqwest::StatusCode;
use serde_json::json;

use crate::common::*;

/// `FORGE_AGENT_BIN`, else the nearest `forge-agent` above the test
/// executable: `target/<profile>/deps/it-<hash>` and the newer build-dir
/// layout `target/<profile>/build/<pkg>/<hash>/out/it-<hash>` both have it in
/// `target/<profile>/`.
fn agent_binary() -> Option<std::path::PathBuf> {
    if let Some(p) = std::env::var_os("FORGE_AGENT_BIN").map(std::path::PathBuf::from) {
        return p.is_file().then_some(p);
    }

    let name = format!("forge-agent{}", std::env::consts::EXE_SUFFIX);
    let exe = std::env::current_exe().ok()?;

    exe.ancestors().skip(1).take(6).map(|d| d.join(&name)).find(|p| p.is_file())
}

/// Kills the child when the test ends, however it ends.
struct Child(std::process::Child);

impl Drop for Child {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[tokio::test]
async fn the_agent_binary_starts_heartbeats_and_stays_up() {
    let Some(bin) = agent_binary() else {
        assert!(
            std::env::var("FORGE_REQUIRE_AGENT_BIN").as_deref() != Ok("1"),
            "forge-agent binary not found next to the test target; build it first (cargo build -p forge-agent)"
        );
        eprintln!("skipped: forge-agent binary not built");

        return;
    };

    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let (s, body) = admin.post("/api/v1/enrollment-tokens", json!({ "ttl_minutes": 10 })).await;
    assert_eq!(s, StatusCode::CREATED, "{body}");
    let dir = tempfile::tempdir().unwrap();
    let facts = HostFacts { hostname: "smoke-1".into(), os: "linux".into(), arch: "x86_64".into() };
    let id = enroll(
        &StatePaths::new(dir.path()),
        AgentConfig::new(h.agent_url.clone()),
        body["token"].as_str().unwrap(),
        facts,
    )
    .await
    .unwrap();
    let server_id: forge_core::ServerId = id.server_id.parse().unwrap();

    // stderr goes to a file, so heavy logging can never block the agent.
    let log_path = dir.path().join("agent-stderr.log");
    let log = std::fs::File::create(&log_path).unwrap();
    let log_text = || std::fs::read_to_string(&log_path).unwrap_or_default();
    let mut child = Child(
        std::process::Command::new(&bin)
            .arg("--state-dir")
            .arg(dir.path())
            .arg("run")
            .env("RUST_LOG", "warn")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::from(log))
            .spawn()
            .unwrap(),
    );

    // It heartbeats: the server sees the node online.
    let deadline = Instant::now() + Duration::from_secs(20);

    loop {
        let s = forge_core::ports::ServerRepository::get_server(&h.app.state.store, server_id).await.unwrap();
        if s.status == AgentStatus::Online && s.last_seen_at.is_some() {
            break;
        }

        if let Some(status) = child.0.try_wait().unwrap() {
            panic!("forge-agent exited during startup ({status}):\n{}", log_text());
        }

        assert!(Instant::now() < deadline, "the agent never heartbeated:\n{}", log_text());
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // And it stays up (startup panics, crashes in the first background ticks).
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert!(child.0.try_wait().unwrap().is_none(), "forge-agent exited after connecting:\n{}", log_text());
}
