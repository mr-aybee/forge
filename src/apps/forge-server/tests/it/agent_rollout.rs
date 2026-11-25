//! Agent rollouts in batches (canary first, stop at the first failure) and
//! the opt-in automatic update setting (issue #4 follow-up, D-071).
#![allow(clippy::unwrap_used)]

use std::time::Duration;

use crate::common::*;
use forge_agent::enroll::{HostFacts, enroll};
use forge_agent::{Agent, AgentConfig, StatePaths};
use reqwest::StatusCode;
use serde_json::json;

async fn enroll_agent(
    h: &Harness,
    admin: &Client,
    name: &str,
) -> (String, tokio::sync::watch::Sender<bool>, tempfile::TempDir) {
    let (_, body) = admin.post("/api/v1/enrollment-tokens", json!({"ttl_minutes": 10})).await;
    let dir = tempfile::tempdir().unwrap();
    let paths = StatePaths::new(dir.path());
    let facts = HostFacts { hostname: name.into(), os: "linux".into(), arch: "x86_64".into() };
    let id =
        enroll(&paths, AgentConfig::new(h.agent_url.clone()), body["token"].as_str().unwrap(), facts).await.unwrap();
    let (stop, rx) = tokio::sync::watch::channel(false);
    tokio::spawn(Agent::load(paths).unwrap().run(rx));
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

async fn step_up(c: &mut Client) {
    use forge_identity::totp;
    let (_, t) = c.post("/api/v1/auth/totp", json!({})).await;
    let secret = totp::base32_decode(t["secret"].as_str().unwrap()).unwrap();
    let code = || {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        format!("{:06}", totp::code_at(&secret, i64::try_from(now).unwrap()))
    };
    let (s, _) =
        c.post("/api/v1/auth/totp/confirm", json!({"password": PASSWORD, "secret": t["secret"], "code": code()})).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, r) = c.post("/api/v1/auth/step-up", json!({"code": code()})).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    c.csrf = r["csrf_token"].as_str().map(str::to_owned);
}

#[tokio::test]
async fn rollouts_go_batch_by_batch_and_stop_at_the_first_failure() {
    let h = start().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let (a, _sa, _da) = enroll_agent(&h, &admin, "roll-a").await;
    let (b, _sb, _db) = enroll_agent(&h, &admin, "roll-b").await;

    // server.command is MFA-guarded: no rollout from a password-only session.
    let (s, e) = admin.post("/api/v1/agents/rollouts", json!({ "server_ids": [a, b], "batch": 1 })).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{e}");
    step_up(&mut admin).await;

    // Nothing outdated (test agents run this version) → nothing to do.
    let (s, _) = admin.post("/api/v1/agents/rollouts", json!({ "batch": 1 })).await;
    assert_eq!(s, StatusCode::CONFLICT);
    let (s, _) = admin.post("/api/v1/agents/rollouts", json!({ "server_ids": [a], "batch": 0 })).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // Two nodes, one per batch: both batches run.
    let (s, r) = admin.post("/api/v1/agents/rollouts", json!({ "server_ids": [a, b], "batch": 1 })).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{r}");
    assert_eq!(r["batches"], 2);
    let job = wait_job(&admin, r["job"]["id"].as_str().unwrap(), Duration::from_secs(60)).await;
    assert_eq!(job["status"], "succeeded", "{job}");
    assert_eq!(job["state"]["done_batches"], 2, "{job}");

    // A node the caller may not command is refused before anything starts.
    let unknown = "01a1d3c4-0000-7000-8000-000000000000";
    let (s, _) = admin.post("/api/v1/agents/rollouts", json!({ "server_ids": [unknown, b], "batch": 1 })).await;
    assert!(s.is_client_error(), "{s}");

    let (_, log) = admin.get("/api/v1/audit?action=agent.rollout").await;
    assert!(log.as_array().unwrap().len() >= 2, "{log}");
}

#[tokio::test]
async fn a_failed_single_update_can_be_retried() {
    let h = start().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    step_up(&mut admin).await;

    // An outdated node that is offline: its update fails after its retries.
    let (n, stop, _d) = enroll_agent(&h, &admin, "retry-a").await;
    let _ = stop.send(true);
    tokio::time::sleep(Duration::from_millis(500)).await;
    let id: forge_core::ServerId = n.parse().unwrap();
    use forge_core::ports::ServerRepository;
    h.app.state.store.record_heartbeat(id, forge_core::Timestamp(0), "0.0.1-old").await.unwrap();
    h.app.state.store.set_status(id, forge_core::model::AgentStatus::Offline).await.unwrap();

    let path = format!("/api/v1/servers/{n}/agent/update");
    let (s, first) = admin.post(&path, json!({})).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{first}");
    let first_id = first["id"].as_str().unwrap().to_owned();
    let (_, again) = admin.post(&path, json!({})).await;
    assert_eq!(again["id"], first_id.as_str(), "a double submit finds the queued update");
    let job = wait_job(&admin, &first_id, Duration::from_secs(90)).await;
    assert_eq!(job["status"], "failed", "{job}");

    // Many newer jobs (more than one page of the job list) must not hide the
    // failed attempt: the retry still gets a new key, not the failed job.
    use forge_core::ports::JobRepository;

    for i in 0..520 {
        let job = forge_core::model::NewJob {
            kind: "test.filler".into(),
            workspace_id: None,
            resource: forge_core::EntityRef::platform(),
            principal: forge_core::Principal::User(forge_core::UserId::new()),
            context: forge_core::AuthzContext::default(),
            idempotency_key: Some(format!("filler-{i}")),
            payload: json!({}),
            max_attempts: 1,
        };

        // Dated in the future: newest in the list, and never due to run.
        h.app.state.store.insert_job(job, forge_core::Timestamp(i64::MAX / 2)).await.unwrap();
    }

    let (s, retry) = admin.post(&path, json!({})).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{retry}");
    assert_ne!(retry["id"], first_id.as_str(), "after a failure a new update starts");
    assert_eq!(retry["status"], "queued");
    let (_, again) = admin.post(&path, json!({})).await;
    assert_eq!(again["id"], retry["id"], "a double submit of the retry finds it");
}

#[tokio::test]
async fn automatic_updates_are_off_by_default_and_need_mfa_to_change() {
    let h = start().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let (s, a) = admin.get("/api/v1/agents/auto-update").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(a["enabled"], false);
    assert_eq!(a["timezone"], "UTC");

    let on = json!({ "enabled": true, "days": ["sat", "sun"], "start": "02:00", "end": "05:00", "batch": 2 });
    let (s, e) = admin.call(reqwest::Method::PUT, "/api/v1/agents/auto-update", Some(on.clone())).await;
    assert_eq!((s, e["title"].as_str()), (StatusCode::FORBIDDEN, Some("step_up_required")));

    // The generic settings editor cannot be used around it.
    let (s, _) = admin
        .call(
            reqwest::Method::PUT,
            "/api/v1/settings/agents.auto_update",
            Some(json!({ "value": "{\"enabled\":true}" })),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    step_up(&mut admin).await;
    let bad = json!({ "enabled": true, "days": ["someday"], "start": "02:00", "end": "05:00", "batch": 2 });
    let (s, _) = admin.call(reqwest::Method::PUT, "/api/v1/agents/auto-update", Some(bad)).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, a) = admin.call(reqwest::Method::PUT, "/api/v1/agents/auto-update", Some(on)).await;
    assert_eq!(s, StatusCode::OK, "{a}");
    assert_eq!((a["enabled"].as_bool(), a["batch"].as_u64()), (Some(true), Some(2)));
    assert!(a["enabled_by"].as_str().unwrap().starts_with("user:"));

    // The captured MFA is good for 90 days; the setting says when it lapses.
    let at = a["enabled_at"].as_i64().unwrap();
    assert_eq!(a["expires_at"].as_i64().unwrap() - at, 90 * 86_400_000, "{a}");
    let (_, g) = admin.get("/api/v1/agents/auto-update").await;
    assert_eq!(g["enabled_at"].as_i64(), Some(at), "{g}");
    let (_, log) = admin.get("/api/v1/audit?action=agent.auto_update").await;
    let outcomes: Vec<&str> = log.as_array().unwrap().iter().filter_map(|e| e["outcome"].as_str()).collect();
    assert!(outcomes.contains(&"success") && outcomes.contains(&"denied"), "{outcomes:?}");
}

/// An outdated node that is offline: its update waits for it until the deadline.
async fn stalled_node(h: &Harness, admin: &Client, name: &str) -> (String, tempfile::TempDir) {
    let (n, stop, dir) = enroll_agent(h, admin, name).await;
    let _ = stop.send(true);
    tokio::time::sleep(Duration::from_millis(500)).await;
    let id: forge_core::ServerId = n.parse().unwrap();
    use forge_core::ports::ServerRepository;
    h.app.state.store.record_heartbeat(id, forge_core::Timestamp(0), "0.0.1-old").await.unwrap();
    h.app.state.store.set_status(id, forge_core::model::AgentStatus::Offline).await.unwrap();

    (n, dir)
}

#[tokio::test]
async fn one_rollout_at_a_time_one_update_per_node_and_rollouts_live_on_the_platform() {
    // Long enough that the offline canary keeps the rollout running below.
    let h = start_configured(|c| c.update.agent_batch_timeout_secs = 120).await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    step_up(&mut admin).await;
    let (a, _sa, _da) = enroll_agent(&h, &admin, "excl-a").await;
    let (c, _dc) = stalled_node(&h, &admin, "excl-c").await;

    // Another update of a is queued: a rollout must not send a a second one.
    use forge_core::ports::JobRepository;
    let a_id: forge_core::ServerId = a.parse().unwrap();
    let (other, _) = h
        .app
        .state
        .store
        .insert_job(
            forge_core::model::NewJob {
                kind: "agent.update".into(),
                workspace_id: None,
                resource: forge_core::EntityRef::new(forge_core::EntityKind::Server, a_id),
                principal: forge_core::Principal::User(forge_core::UserId::new()),
                context: forge_core::AuthzContext::default(),
                idempotency_key: Some(format!("agent.update:{a}:other")),
                payload: json!({ "server_id": a }),
                max_attempts: 1,
            },
            // Dated in the future: queued, never due to run.
            forge_core::Timestamp(i64::MAX / 2),
        )
        .await
        .unwrap();
    let (s, r) = admin.post("/api/v1/agents/rollouts", json!({ "server_ids": [a], "batch": 1 })).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{r}");
    let job = wait_job(&admin, r["job"]["id"].as_str().unwrap(), Duration::from_secs(60)).await;
    assert_eq!(job["status"], "failed", "{job}");
    assert!(job["last_error"].as_str().unwrap().contains("another update of this node"), "{job}");
    h.app.state.store.cancel_job(other.id, forge_core::Timestamp(0)).await.unwrap();

    // A rollout whose canary (c) is offline: it waits for c for a while.
    let (s, r) = admin.post("/api/v1/agents/rollouts", json!({ "server_ids": [c, a], "batch": 1 })).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{r}");
    let rollout = r["job"]["id"].as_str().unwrap().to_owned();

    // The parent sits on the platform, outside every workspace.
    assert!(r["job"]["resource"].to_string().contains("platform"), "{r}");
    assert!(r["job"]["workspace_id"].is_null(), "{r}");

    // A second rollout is refused while the first is queued or running, and audited.
    let (s, e) = admin.post("/api/v1/agents/rollouts", json!({ "server_ids": [a], "batch": 1 })).await;
    assert_eq!(s, StatusCode::CONFLICT, "{e}");
    let (_, log) = admin.get("/api/v1/audit?action=agent.rollout").await;
    assert!(log.as_array().unwrap().iter().any(|e| e["outcome"] == "denied"), "{log}");

    // The rollout waits for offline c itself: c has no update job yet (no
    // worker slot is held per offline node), and the batch is still running.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let (_, jobs) = admin.get("/api/v1/jobs?limit=200").await;
    assert!(
        !jobs.as_array().unwrap().iter().any(|j| j["payload"]["rollout"] == rollout.as_str()),
        "no child job while the node is offline: {jobs}"
    );
    let (_, job) = admin.get(&format!("/api/v1/jobs/{rollout}")).await;
    assert_eq!(job["status"], "running", "{job}");

    // An operator of the default workspace neither sees nor cancels the rollout.
    let mut op =
        user_with_role(&h, &admin, "op@example.com", "operator", &format!("workspace:{}", h.default_org)).await;
    step_up(&mut op).await;
    let (s, _) = op.get(&format!("/api/v1/jobs/{rollout}")).await;
    assert!(s == StatusCode::FORBIDDEN || s == StatusCode::NOT_FOUND, "{s}");
    let (s, _) = op.post(&format!("/api/v1/jobs/{rollout}/cancel"), json!({})).await;
    assert!(s == StatusCode::FORBIDDEN || s == StatusCode::NOT_FOUND, "{s}");

    // ...and cannot change the platform-wide automatic update setting.
    let on = json!({ "enabled": true, "days": [], "start": "02:00", "end": "05:00", "batch": 1 });
    let (s, e) = op.call(reqwest::Method::PUT, "/api/v1/agents/auto-update", Some(on)).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{e}");

    // Cancelling the rollout stops the wait: c is never sent an update.
    let (s, e) = admin.post(&format!("/api/v1/jobs/{rollout}/cancel"), json!({})).await;
    assert!(s.is_success(), "{e}");
    let job = wait_job(&admin, &rollout, Duration::from_secs(30)).await;
    assert_eq!(job["status"], "cancelled", "{job}");
    let (_, jobs) = admin.get("/api/v1/jobs?limit=200").await;
    assert!(!jobs.as_array().unwrap().iter().any(|j| j["payload"]["rollout"] == rollout.as_str()), "{jobs}");

    // Once it ended, a new rollout may start.
    let (s, r) = admin.post("/api/v1/agents/rollouts", json!({ "server_ids": [a], "batch": 1 })).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{r}");
}

/// The control plane offers the agents in its folder as the version in their
/// `MANIFEST.json`, not its own, and never hands out a file whose hash does
/// not match the manifest.
#[tokio::test]
async fn the_agent_folder_manifest_is_the_target_version() {
    let dist = tempfile::tempdir().unwrap();
    std::fs::write(dist.path().join("forge-agent-linux-x86_64"), b"agent 99").unwrap();
    std::fs::write(dist.path().join("forge-agent-helper-linux-x86_64"), b"replaced by hand").unwrap();
    let sha = |b: &[u8]| forge_secrets::crypto::sha256_hex(b);
    forge_server::install::DistManifest {
        version: "99.0.0".into(),
        files: [
            ("forge-agent-linux-x86_64".to_owned(), sha(b"agent 99")),
            ("forge-agent-helper-linux-x86_64".to_owned(), sha(b"helper 99")),
        ]
        .into(),
    }
    .write(dist.path())
    .unwrap();
    let dir = dist.path().to_path_buf();
    let h = start_configured(move |c| c.agent_dist_dir = Some(dir)).await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let (a, _sa, _da) = enroll_agent(&h, &admin, "manifest-a").await;

    let (s, v) = admin.get("/api/v1/agents/versions").await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["target"], "99.0.0", "{v}");
    assert_eq!(v["server"], forge_server::VERSION);
    let node = v["nodes"].as_array().unwrap().iter().find(|n| n["id"] == a.as_str()).unwrap();
    assert_eq!(node["outdated"], true, "{node}");

    let download = |bin: &str| format!("{}/agent/download/linux/x86_64/{bin}", h.base);
    let ok = reqwest::get(download("forge-agent")).await.unwrap();
    assert_eq!(ok.status(), StatusCode::OK);
    assert_eq!(ok.bytes().await.unwrap().as_ref(), b"agent 99");
    let tampered = reqwest::get(download("forge-agent-helper")).await.unwrap();
    assert_eq!(tampered.status(), StatusCode::NOT_FOUND, "a file that does not match the manifest is not served");
}

/// A rollout skips a node that is offline until the end of its batch or that
/// refuses the update as `unsupported`, lists why, and carries on; a real
/// failure still stops it.
#[tokio::test]
async fn rollouts_skip_offline_and_unsupported_nodes_and_stop_at_a_real_failure() {
    // Agents on offer are newer than the test agents, so every node is outdated.
    let dist = tempfile::tempdir().unwrap();
    let sha = |b: &[u8]| forge_secrets::crypto::sha256_hex(b);
    let mut files = std::collections::BTreeMap::new();

    for (name, body) in [("forge-agent-linux-x86_64", b"agent"), ("forge-agent-helper-linux-x86_64", b"helpr")] {
        std::fs::write(dist.path().join(name), body).unwrap();
        files.insert(name.to_owned(), sha(body));
    }

    forge_server::install::DistManifest { version: "99.0.0".into(), files }.write(dist.path()).unwrap();
    let dir = dist.path().to_path_buf();
    let h = start_configured(move |c| {
        c.agent_dist_dir = Some(dir);
        c.update.agent_batch_timeout_secs = 3;
    })
    .await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    step_up(&mut admin).await;

    // c: offline. b: its local policy does not allow `update` (unsupported).
    // a: allows it, but this host has no root helper: a real failure.
    let (c, _dc) = stalled_node(&h, &admin, "skip-c").await;
    let (b, _sb, db) = enroll_agent(&h, &admin, "skip-b").await;

    // The agent reads its policy at start: restart b without `update`.
    let _ = _sb.send(true);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let paths = StatePaths::new(db.path());
    let mut cfg = paths.load_config().unwrap();
    assert!(cfg.allowed_commands.iter().any(|c| c == "update"));
    cfg.allowed_commands.retain(|c| c != "update");
    paths.save_config(&cfg).unwrap();
    let (stop_b, rx) = tokio::sync::watch::channel(false);
    tokio::spawn(Agent::load(StatePaths::new(db.path())).unwrap().run(rx));
    let (a, _sa, _da) = enroll_agent(&h, &admin, "skip-a").await;

    // Two batches of nodes that cannot take the update: both skipped, the
    // rollout carries on and succeeds.
    let (s, r) = admin.post("/api/v1/agents/rollouts", json!({ "server_ids": [c, b], "batch": 1 })).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{r}");
    assert_eq!(r["target"], "99.0.0");
    let job = wait_job(&admin, r["job"]["id"].as_str().unwrap(), Duration::from_secs(90)).await;
    assert_eq!(job["status"], "succeeded", "{job}");
    assert_eq!(job["state"]["done_batches"], 2, "{job}");
    assert_eq!(job["state"]["skipped"], 2, "{job}");
    let nodes = job["state"]["nodes"].as_array().unwrap();
    let of = |id: &str| nodes.iter().find(|n| n["server_id"] == id).cloned().unwrap_or_default();
    assert_eq!(
        (of(&c)["outcome"].as_str(), of(&c)["skip_reason"].as_str()),
        (Some("skipped"), Some("offline")),
        "{job}"
    );
    assert_eq!(
        (of(&b)["outcome"].as_str(), of(&b)["skip_reason"].as_str()),
        (Some("skipped"), Some("unsupported")),
        "{job}"
    );

    // A node whose update really fails (no helper on this host) still stops
    // the rollout before the next batch.
    let (s, r) = admin.post("/api/v1/agents/rollouts", json!({ "server_ids": [a, c], "batch": 1 })).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{r}");
    let job = wait_job(&admin, r["job"]["id"].as_str().unwrap(), Duration::from_secs(90)).await;
    assert_eq!(job["status"], "failed", "{job}");
    let why = job["last_error"].as_str().unwrap();
    assert!(why.contains("batch 1 of 2") && why.contains(a.as_str()), "{why}");
    drop(stop_b);
}

/// The versions list says what an update would do: a node whose facts say it
/// cannot self-update needs a reinstall and is left out of "every outdated
/// node", but a rollout that names it still sends it the update (its own
/// refusal decides).
#[tokio::test]
async fn a_node_that_cannot_self_update_needs_a_reinstall_but_can_be_named() {
    let dist = tempfile::tempdir().unwrap();
    let sha = |b: &[u8]| forge_secrets::crypto::sha256_hex(b);
    let mut files = std::collections::BTreeMap::new();

    for (name, body) in [("forge-agent-linux-x86_64", b"agent"), ("forge-agent-helper-linux-x86_64", b"helpr")] {
        std::fs::write(dist.path().join(name), body).unwrap();
        files.insert(name.to_owned(), sha(body));
    }

    forge_server::install::DistManifest { version: "99.0.0".into(), files }.write(dist.path()).unwrap();
    let dir = dist.path().to_path_buf();
    let h = start_configured(move |c| c.agent_dist_dir = Some(dir)).await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    step_up(&mut admin).await;
    let (a, _sa, _da) = enroll_agent(&h, &admin, "verdict-a").await;
    let id: forge_core::ServerId = a.parse().unwrap();

    // The agent here has no helper: it reports no capability (an older agent
    // looks the same), so the node is eligible with the verdict `unknown`.
    use forge_core::tags::FactsRepository;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);

    while !h.app.state.store.all_facts().await.unwrap().contains_key(&id) {
        assert!(tokio::time::Instant::now() < deadline, "facts never collected");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let (_, v) = admin.get("/api/v1/agents/versions").await;
    let row = |v: &serde_json::Value| v["nodes"].as_array().unwrap().iter().find(|n| n["id"] == a.as_str()).cloned();
    assert_eq!(row(&v).unwrap()["verdict"], "unknown", "{v}");

    // Facts that say the node cannot self-update.
    let facts = json!({ "self_update": {
        "supervisor": "none", "ready": false, "reasons": ["no_systemd", "no_curl"], "helper_version": "1.0.0"
    }});
    h.app.state.store.put_facts(id, &facts, h.app.state.clock.now()).await.unwrap();
    let (_, v) = admin.get("/api/v1/agents/versions").await;
    let r = row(&v).unwrap();
    assert_eq!(r["verdict"], "needs_reinstall", "{r}");
    assert_eq!(r["reasons"], json!(["no_systemd", "no_curl"]), "{r}");
    assert_eq!(r["outdated"], true);

    // "Every outdated node" leaves it out...
    let (s, e) = admin.post("/api/v1/agents/rollouts", json!({ "batch": 1 })).await;
    assert_eq!(s, StatusCode::CONFLICT, "{e}");

    // ...but naming it still sends the update.
    let (s, r) = admin.post("/api/v1/agents/rollouts", json!({ "server_ids": [a], "batch": 1 })).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{r}");
}
