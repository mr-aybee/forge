//! Agents pin the control-plane key (D-167): enrollment and the link work
//! by an address the server certificate does not name, agents enrolled
//! before pin the key Forge announces over their existing link, and a key
//! mismatch is reported clearly and fixed with a pin, without re-enrolling.
#![allow(clippy::unwrap_used)]

use std::time::Duration;

use crate::common::*;
use forge_agent::enroll::{HostFacts, enroll};
use forge_agent::{Agent, AgentConfig, StatePaths};
use reqwest::StatusCode;
use serde_json::json;

async fn token(admin: &Client) -> String {
    let (s, body) = admin.post("/api/v1/enrollment-tokens", json!({"ttl_minutes": 10})).await;
    assert_eq!(s, StatusCode::CREATED, "{body}");
    body["token"].as_str().unwrap().to_owned()
}

fn facts(name: &str) -> HostFacts {
    HostFacts { hostname: name.into(), os: "linux".into(), arch: "x86_64".into() }
}

fn run(paths: StatePaths) -> tokio::sync::watch::Sender<bool> {
    let agent = Agent::load(paths).unwrap();
    let (stop, rx) = tokio::sync::watch::channel(false);
    tokio::spawn(agent.run(rx));
    stop
}

/// Polls `check` until it holds or `secs` pass.
async fn until(secs: u64, what: &str, mut check: impl AsyncFnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    while tokio::time::Instant::now() < deadline {
        if check().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("timed out waiting for {what}");
}

async fn ping(admin: &Client, server: &str) {
    let (s, job) =
        admin.post(&format!("/api/v1/servers/{server}/commands"), json!({ "command": {"type": "ping"} })).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{job}");
    let done = wait_job(admin, job["id"].as_str().unwrap(), Duration::from_secs(30)).await;
    assert_eq!(done["status"], "succeeded", "{done}");
}

async fn online(admin: &Client, server: &str) -> bool {
    admin.get(&format!("/api/v1/servers/{server}")).await.1["status"] == "online"
}

#[tokio::test]
async fn an_agent_enrolls_and_links_by_an_address_the_certificate_does_not_name() {
    // The certificate names only forge.invalid; the agent dials 127.0.0.1.
    let h = start_configured(|c| c.agent_hostnames = vec!["forge.invalid".into()]).await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let t = token(&admin).await;
    let parsed = forge_pki::enrollment::parse(&t).unwrap();
    let keys = &h.app.state.agent_keys;
    assert_eq!(parsed.server_key_sha256.as_deref(), Some(keys.current_fingerprint().as_str()));

    let dir = tempfile::tempdir().unwrap();
    let paths = StatePaths::new(dir.path());
    let id = enroll(&paths, AgentConfig::new(h.agent_url.clone()), &t, facts("anywhere")).await.unwrap();
    // Pinned: the current and the next key.
    let pinned = forge_agent::pins::load(&paths.pins()).unwrap().fingerprints();
    let listed: Vec<String> = keys.keys().into_iter().take(2).map(|k| k.sha256).collect();
    assert_eq!(pinned, listed);

    let _stop = run(paths);
    until(20, "the agent online", async || online(&admin, &id.server_id).await).await;
    ping(&admin, &id.server_id).await;
    // Its heartbeats acknowledge both keys.
    let next = listed[1].clone();
    until(10, "the acknowledgement", async || {
        keys.acknowledgements().get(&id.server_id).is_some_and(|a| a.keys.contains(&next))
    })
    .await;

    // An agent from before D-167 cannot use this address: the CA check
    // needs the name, which is why agents pin the key now.
    let old = tempfile::tempdir().unwrap();
    let t2 = token(&admin).await;
    let p2 = forge_pki::enrollment::parse(&t2).unwrap();
    let legacy = format!("{}.{}", p2.secret, p2.ca_fingerprint);
    let err = enroll(&StatePaths::new(old.path()), AgentConfig::new(h.agent_url.clone()), &legacy, facts("legacy"))
        .await
        .unwrap_err();
    assert!(format!("{err:#}").to_lowercase().contains("certificate"), "{err:#}");
}

#[tokio::test]
async fn an_agent_enrolled_before_pins_the_key_announced_over_its_link() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    // A token without the key, as before D-167; the server name matches.
    let t = token(&admin).await;
    let p = forge_pki::enrollment::parse(&t).unwrap();
    let legacy = format!("{}.{}", p.secret, p.ca_fingerprint);
    let dir = tempfile::tempdir().unwrap();
    let paths = StatePaths::new(dir.path());
    let id = enroll(&paths, AgentConfig::new(h.agent_url.clone()), &legacy, facts("veteran")).await.unwrap();
    // Enrolled before pins existed: no pin file.
    std::fs::remove_file(paths.pins()).unwrap();

    let _stop = run(paths.clone());
    until(20, "the agent online", async || online(&admin, &id.server_id).await).await;
    // The first heartbeat reply announces the keys; the agent pins them
    // and keeps working over the pinned link.
    let current = h.app.state.agent_keys.current_fingerprint();
    until(20, "the pins", async || {
        forge_agent::pins::load(&paths.pins()).is_ok_and(|s| s.fingerprints().contains(&current))
    })
    .await;
    ping(&admin, &id.server_id).await;
}

#[tokio::test]
async fn a_key_mismatch_is_reported_and_fixed_by_pinning_without_re_enrolling() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let dir = tempfile::tempdir().unwrap();
    let paths = StatePaths::new(dir.path());
    let id = enroll(&paths, AgentConfig::new(h.agent_url.clone()), &token(&admin).await, facts("moved")).await.unwrap();
    // The node pins only some other key.
    let mut wrong = forge_agent::pins::PinSet::default();
    wrong.add(&"ab".repeat(32), "manual", 0).unwrap();
    forge_agent::pins::save(&paths.pins(), &wrong).unwrap();

    let _stop = run(paths.clone());
    until(20, "the mismatch report", async || {
        std::fs::read_to_string(paths.link_error())
            .is_ok_and(|e| e.contains("is not pinned (pinned: sha256:abababab") && e.contains("forge-agent pins add"))
    })
    .await;

    // `forge-agent pins add <current>`: picked up at the next attempt.
    let mut fixed = wrong.clone();
    fixed.add(&h.app.state.agent_keys.current_fingerprint(), "manual", 0).unwrap();
    forge_agent::pins::save(&paths.pins(), &fixed).unwrap();
    until(90, "the link back", async || !paths.link_error().exists()).await;
    ping(&admin, &id.server_id).await;
}
