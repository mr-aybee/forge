//! Acceptance: killing the control plane mid-job and restarting it resumes
//! the job from the last completed step.
#![allow(clippy::unwrap_used)]

use std::process::{Child, Command, Stdio};
use std::time::Duration;

use serde_json::{Value, json};

use crate::common::{ADMIN_EMAIL, Client, PASSWORD};

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

struct Instance {
    config: std::path::PathBuf,
}

impl Instance {
    fn new(dir: &std::path::Path) -> (Self, String) {
        let api = free_port();
        let agent = free_port();
        let data = dir.join("data").display().to_string().replace('\\', "/");
        let toml = format!(
            "data_dir = '{data}'\napi_listen = '127.0.0.1:{api}'\nagent_listen = '127.0.0.1:{agent}'\n\
             public_url = 'http://127.0.0.1:{api}'\ncookie_secure = false\n\n[jobs]\nlease_secs = 3\nconcurrency = 2\npoll_ms = 100\n"
        );
        let config = dir.join("forge.toml");
        std::fs::write(&config, toml).unwrap();

        (Self { config }, format!("http://127.0.0.1:{api}"))
    }

    fn command(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_forge-server"));
        c.arg("--config").arg(&self.config).env("RUST_LOG", "warn").stdout(Stdio::null()).stderr(Stdio::null());

        c
    }

    fn init(&self) {
        let status = self
            .command()
            .args(["init", "--email", ADMIN_EMAIL])
            .env("FORGE_ADMIN_PASSWORD", PASSWORD)
            .status()
            .unwrap();
        assert!(status.success());
    }

    async fn serve(&self, base: &str) -> Child {
        let mut child = self.command().arg("serve").spawn().unwrap();

        for _ in 0..100 {
            if reqwest::get(format!("{base}/api/health")).await.is_ok_and(|r| r.status().is_success()) {
                return child;
            }

            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        let _ = child.kill();
        let _ = child.wait();
        panic!("server did not become healthy");
    }
}

async fn steps(c: &Client, job: &str) -> Vec<Value> {
    c.get(&format!("/api/v1/jobs/{job}/steps")).await.1.as_array().cloned().unwrap_or_default()
}

#[tokio::test]
async fn job_survives_control_plane_kill_and_resumes() {
    let dir = tempfile::tempdir().unwrap();
    let (inst, base) = Instance::new(dir.path());
    inst.init();

    let mut first = inst.serve(&base).await;
    let client = Client::login(&base, ADMIN_EMAIL, PASSWORD).await;
    let (_, workspaces) = client.get("/api/v1/workspace").await;
    let org = workspaces["id"].as_str().unwrap().to_owned();

    let (status, job) = client
        .post(
            "/api/v1/jobs",
            json!({ "kind": "diagnostics.sleep", "resource": format!("workspace:{org}"), "payload": { "step_ms": 1500 } }),
        )
        .await;
    assert_eq!(status, reqwest::StatusCode::ACCEPTED, "{job}");
    let id = job["id"].as_str().unwrap().to_owned();

    // Wait until step 0 has completed and step 1 is running, then hard-kill.
    let mut before = Vec::new();

    for _ in 0..100 {
        before = steps(&client, &id).await;

        if before.len() == 2 && before[0]["status"] == "succeeded" && before[1]["status"] == "running" {
            break;
        }

        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    assert_eq!(before.len(), 2, "did not reach step 1: {before:?}");
    first.kill().unwrap();
    first.wait().unwrap();

    // Restart. The session lives in the database, so the client stays logged in.
    let mut second = inst.serve(&base).await;
    let done = crate::common::wait_job(&client, &id, Duration::from_secs(30)).await;
    let after = steps(&client, &id).await;
    second.kill().unwrap();
    let _ = second.wait();

    assert_eq!(done["status"], "succeeded", "{done}");
    assert_eq!(done["state"], json!({"step0": "done", "step1": "done", "step2": "done"}));
    assert_eq!(after.len(), 3);
    assert_eq!(after[0]["started_at"], before[0]["started_at"], "completed step 0 was not re-run");
    assert_ne!(after[1]["started_at"], before[1]["started_at"], "interrupted step 1 was re-run after restart");
}
