//! Real `forge alerts channels | policies | deliveries` output for the CLI
//! reference (`docs/src/content/docs/reference/cli/alerts.md`).

use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::common::*;

/// A receiver on loopback that answers every request with `status_line`.
/// Returns its `http://127.0.0.1:<port>` base.
async fn receiver(status_line: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                // Read the head and the body it announces, then answer.
                let mut seen = Vec::new();
                let mut buf = [0u8; 4096];
                loop {
                    let Ok(n) = stream.read(&mut buf).await else { return };
                    if n == 0 {
                        break;
                    }
                    seen.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&seen);
                    if let Some(head_end) = text.find("\r\n\r\n") {
                        let length = text[..head_end]
                            .lines()
                            .filter_map(|l| l.split_once(':'))
                            .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                            .and_then(|(_, v)| v.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                        if seen.len() >= head_end + 4 + length {
                            break;
                        }
                    }
                }
                let answer = format!("HTTP/1.1 {status_line}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                let _ = stream.write_all(answer.as_bytes()).await;
                let _ = stream.shutdown().await;
            });
        }
    });
    base
}

/// Prints real `forge alerts channels|policies|deliveries` output for the
/// docs. Run on demand, after `cargo build -p forge-cli`:
/// `cargo test -p forge-server --test it capture_notifications_cli -- --ignored --nocapture`
///
/// MFA is not required in this server, so the CLI's API key may use
/// `notification.manage`; loopback HTTP receivers are allowed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "documentation capture; needs the forge CLI binary"]
async fn capture_notifications_cli() {
    let cli = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .join(std::env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| "target".into()))
        .join("debug/forge");
    let cli = if cli.with_extension("exe").exists() { cli.with_extension("exe") } else { cli };
    let h = start_with(|c| {
        c.security.require_mfa = false;
        c.alerts.webhook_allow_loopback = true;
        c.alerts.webhook_allow_http = true;
    })
    .await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let (_, key) = admin.post("/api/v1/api-keys", json!({"name": "docs", "expires_days": 1})).await;
    let run = |args: &[&str]| {
        let out = std::process::Command::new(&cli)
            .args(args)
            .env("FORGE_SERVER", &h.base)
            .env("FORGE_TOKEN", key["secret"].as_str().unwrap())
            .env("FORGE_ALLOW_HTTP", "1")
            .env("NO_COLOR", "1")
            .stdin(std::process::Stdio::null())
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
    let ops_url = dir.path().join("ops-url.txt");
    std::fs::write(&ops_url, format!("{}/hooks/ops?token=example\n", receiver("204 No Content").await)).unwrap();
    let chat_url = dir.path().join("chat-url.txt");
    std::fs::write(&chat_url, format!("{}/services/example\n", receiver("500 Internal Server Error").await)).unwrap();
    let (ops_url, chat_url) = (format!("@{}", ops_url.display()), format!("@{}", chat_url.display()));

    run(&["alerts", "channels", "create", "--name", "Ops webhook", "--kind", "webhook", "--url", &ops_url]);
    run(&["alerts", "channels", "create", "--name", "Team chat", "--kind", "slack", "--url", &chat_url]);
    run(&[
        "alerts",
        "channels",
        "create",
        "--name",
        "Refused",
        "--kind",
        "slack",
        "--url",
        "https://hooks.example.com/x",
    ]);
    run(&["alerts", "channels", "list"]);
    run(&["alerts", "channels", "show", "Ops webhook"]);
    run(&["alerts", "channels", "test", "Ops webhook"]);
    run(&["alerts", "channels", "test", "Team chat"]);
    run(&["alerts", "channels", "test", "Team chat", "-o", "json"]);
    run(&["alerts", "channels", "update", "Team chat", "--name", "Chat", "--disabled", "--yes"]);
    run(&["alerts", "channels", "test", "Chat"]);

    run(&[
        "alerts",
        "policies",
        "create",
        "--name",
        "Critical to ops",
        "--min-severity",
        "critical",
        "--event",
        "fired",
        "--event",
        "resolved",
        "--channel",
        "Ops webhook",
        "--scope",
        "servers",
        "--repeat",
        "1h",
    ]);
    run(&["alerts", "policies", "list"]);
    run(&["alerts", "policies", "show", "Critical to ops"]);
    run(&["alerts", "policies", "update", "Critical to ops", "--min-severity", "major", "--repeat", "never", "--yes"]);
    run(&["alerts", "policies", "update", "Critical to ops", "--event", "resolved", "--repeat", "30m", "--yes"]);
    run(&["alerts", "channels", "delete", "Ops webhook", "--yes"]);

    run(&["alerts", "deliveries", "list"]);
    run(&["alerts", "deliveries", "list", "--status", "failed", "--channel", "Chat"]);
    let (_, failed) = admin.get("/api/v1/notification-deliveries?status=failed").await;
    run(&["alerts", "deliveries", "show", failed[0]["id"].as_str().unwrap()]);
    let (_, raw) = admin.get(&format!("/api/v1/notification-deliveries/{}", failed[0]["id"].as_str().unwrap())).await;
    println!("raw delivery view: {raw}");

    run(&["alerts", "policies", "delete", "Critical to ops", "--yes"]);
    run(&["alerts", "channels", "delete", "Chat", "--yes"]);
    run(&["alerts", "channels", "delete", "Ops webhook", "--yes"]);
    h.app.stop().await;
}
