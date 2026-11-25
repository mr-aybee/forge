//! Service health checks (D-085): read, validate, save, authorization,
//! MFA for command checks, fleet targets, stale writes and audit redaction.
#![allow(clippy::unwrap_used)]

use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

use crate::common::*;

async fn fleet_node(h: &Harness, hostname: &str) {
    let server = forge_core::model::Server {
        id: forge_core::ServerId::new(),
        workspace_id: h.default_org.parse().unwrap(),
        environment_id: None,
        cluster_id: None,
        role: forge_core::model::NodeRole::Worker,
        display_name: None,
        hostname: hostname.into(),
        os: "linux".into(),
        arch: "x86_64".into(),
        agent_version: "0".into(),
        status: forge_core::model::AgentStatus::Online,
        last_seen_at: None,
        cert_serial: None,
        cert_not_after: None,
        created_at: forge_core::Timestamp(0),
    };
    forge_core::ports::ServerRepository::create_server(&h.app.state.store, &server).await.unwrap();
}

fn http_policy(host: &str) -> Value {
    http_policy_with(host, None)
}

fn http_policy_with(host: &str, secret: Option<&str>) -> Value {
    let headers = match secret {
        Some(s) => json!([{ "name": "Authorization", "secret": s }]),
        None => json!([{ "name": "Accept", "value": "application/json" }]),
    };

    json!({
        "readiness": [{ "name": "api", "success_threshold": 2,
            "probe": { "type": "http", "host": host, "allow_remote_host": !host.is_empty(), "port": 8080, "path": "/ready",
                       "headers": headers } }],
        "liveness": [{ "name": "tcp", "probe": { "type": "tcp", "port": 8080 } }]
    })
}

#[tokio::test]
async fn health_policies_are_validated_saved_authorized_and_audited() {
    let h = start().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    fleet_node(&h, "db-1").await;
    let (_, cl) = admin.post("/api/v1/clusters", json!({ "name": "shop" })).await;
    let (_, ag1) = admin.post("/api/v1/projects", json!({"name": "grp1"})).await;
    let (s, ns) = admin
        .post("/api/v1/environments", json!({ "name": "uat", "cluster_id": cl["id"], "project_id": ag1["id"] }))
        .await;
    assert_eq!(s, StatusCode::CREATED, "{ns}");
    let (s, app) = admin
        .post(
            "/api/v1/services",
            json!({ "environment_id": ns["id"], "name": "api", "runtime": "systemd", "command": "python3 app.py" }),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{app}");
    let id = app["id"].as_str().unwrap().to_owned();
    let url = format!("/api/v1/services/{id}/health");

    // Empty to start with.
    let (s, v) = admin.get(&url).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["policy"], json!({}));
    assert_eq!(
        (v["can_configure"].clone(), v["can_command"].clone()),
        (json!(true), json!(false)),
        "command needs MFA: {v}"
    );

    // Unknown fields come back as an error, not a bare 400.
    let (s, r) = admin.post(&format!("{url}/validate"), json!({ "policy": { "readyness": [] } })).await;
    assert_eq!((s, r["valid"].clone()), (StatusCode::OK, json!(false)), "{r}");
    assert!(r["errors"][0]["message"].as_str().unwrap().contains("readyness"), "{r}");
    let updated_at = v["updated_at"].as_i64().unwrap();

    // Validate reports field errors without saving.
    let bad = json!({ "policy": { "readiness": [{ "name": "x", "probe": { "type": "http", "port": 0,
        "host": "169.254.169.254", "allow_remote_host": true } }] } });
    let (s, r) = admin.post(&format!("{url}/validate"), bad).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    assert_eq!(r["valid"], false);
    let fields: Vec<&str> = r["errors"].as_array().unwrap().iter().map(|e| e["field"].as_str().unwrap()).collect();
    assert!(fields.contains(&"readiness[0].probe.host") && fields.contains(&"readiness[0].probe.port"), "{r}");

    // Another node of the fleet, also by its `.local` name.
    for host in ["db-1", "DB-1.local"] {
        let (_, r) = admin.post(&format!("{url}/validate"), json!({ "policy": http_policy(host) })).await;
        assert_eq!(r["errors"][0]["field"], "readiness[0].probe.host", "{host}: {r}");
    }

    // Save an HTTP/TCP policy (no MFA needed).
    let (s, v) = admin
        .call(Method::PUT, &url, Some(json!({ "policy": http_policy(""), "expected_updated_at": updated_at })))
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let configured = v["summaries"].as_array().unwrap().iter().filter(|s| s["default"] != true).count();
    assert_eq!(configured, 2, "{v}");

    // Readiness is configured, so only the default liveness check remains.
    let defaults: Vec<&Value> = v["summaries"].as_array().unwrap().iter().filter(|s| s["default"] == true).collect();
    assert_eq!((defaults.len(), defaults[0]["purpose"].as_str()), (1, Some("liveness")), "{v}");
    assert!(v["summaries"][0]["summary"].as_str().unwrap().contains("ready again after 2 successes"), "{v}");

    // A write based on the old version is refused.
    let (s, _) = admin
        .call(Method::PUT, &url, Some(json!({ "policy": http_policy(""), "expected_updated_at": updated_at })))
        .await;
    assert_eq!(s, StatusCode::CONFLICT);

    // An invalid policy is refused and the saved one stays.
    let (s, _) = admin.call(Method::PUT, &url, Some(json!({ "policy": http_policy("db-1") }))).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(admin.get(&url).await.1["policy"]["liveness"][0]["name"], "tcp");

    // An ordinary service update keeps the health policy.
    let (s, b) = admin
        .call(
            Method::PUT,
            &format!("/api/v1/services/{id}"),
            Some(json!({ "runtime": "systemd", "command": "python3 app.py --port 8080" })),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(admin.get(&url).await.1["policy"]["readiness"][0]["name"], "api");

    // A command check needs health.command, which needs MFA.
    let script = "test -r /run/app/beat || exit 1 # secret-ish-marker";
    let with_shell = json!({ "policy": { "liveness": [{ "name": "beat",
        "probe": { "type": "shell", "script": script } }] } });
    let (s, e) = admin.call(Method::PUT, &url, Some(with_shell.clone())).await;
    assert_eq!((s, e["title"].as_str()), (StatusCode::FORBIDDEN, Some("step_up_required")), "{e}");
    step_up(&mut admin).await;
    let (s, v) = admin.call(Method::PUT, &url, Some(with_shell)).await;
    assert_eq!(s, StatusCode::OK, "{v}");

    // A vault reference needs the right to use that item (platform admin,
    // secret.read with MFA) and the item must exist.
    let (s, item) = admin
        .post("/api/v1/vault/items", json!({ "name": "db root", "username": "root", "password": "s3cret-db-pw" }))
        .await;
    assert_eq!(s, StatusCode::CREATED, "{item}");
    let vref = format!("vault:{}", item["id"].as_str().unwrap());
    let (s, b) = admin.call(Method::PUT, &url, Some(json!({ "policy": http_policy_with("", Some(&vref)) }))).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    let missing = "vault:01900000-0000-7000-8000-000000000000";
    let (s, _) = admin.call(Method::PUT, &url, Some(json!({ "policy": http_policy_with("", Some(missing)) }))).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let dev = user_with_role(
        &h,
        &admin,
        "dev@example.com",
        "developer",
        &format!("environment:{}", ns["id"].as_str().unwrap()),
    )
    .await;
    let (s, e) = dev.call(Method::PUT, &url, Some(json!({ "policy": http_policy_with("", Some(&vref)) }))).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "a developer can't send a vault item to a probe target: {e}");
    let (s, b) = dev.call(Method::PUT, &url, Some(json!({ "policy": http_policy("") }))).await;
    assert_eq!(s, StatusCode::OK, "without vault refs a developer may configure checks: {b}");

    // Read-only roles can read but not change.
    let auditor = user_with_role(&h, &admin, "aud@example.com", "auditor", "platform:root").await;
    let (s, v) = auditor.get(&url).await;
    assert_eq!((s, v["can_configure"].clone()), (StatusCode::OK, json!(false)));
    assert_eq!(auditor.call(Method::PUT, &url, Some(json!({ "policy": {} }))).await.0, StatusCode::FORBIDDEN);

    // The audit shows names and targets, never scripts or header values.
    let (_, audit) = admin.get("/api/v1/audit?limit=50").await;
    let text = audit.to_string();
    assert!(text.contains("service.health.update"), "{text}");
    assert!(!text.contains("secret-ish-marker") && !text.contains("s3cret-db-pw"), "{text}");
    assert!(text.contains(&vref), "the vault ref is audited: {text}");

    // An empty policy removes every check.
    let (s, v) = admin.call(Method::PUT, &url, Some(json!({ "policy": {} }))).await;
    assert_eq!((s, v["policy"].clone()), (StatusCode::OK, json!({})));
}

/// A native service may declare the port it listens on (default TCP check);
/// jobs and containers may not.
#[tokio::test]
async fn native_services_declare_a_listening_port() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let (_, cl) = admin.post("/api/v1/clusters", json!({ "name": "shop" })).await;
    let (_, ag2) = admin.post("/api/v1/projects", json!({"name": "grp2"})).await;
    let (_, ns) = admin
        .post("/api/v1/environments", json!({ "name": "uat", "cluster_id": cl["id"], "project_id": ag2["id"] }))
        .await;
    let (s, app) = admin
        .post(
            "/api/v1/services",
            json!({ "environment_id": ns["id"], "name": "api", "runtime": "systemd", "command": "python3 app.py",
                    "listen_port": 8080 }),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{app}");
    assert_eq!(app["spec"]["listen_port"], 8080);
    let id = app["id"].as_str().unwrap();
    let (s, b) = admin
        .call(
            Method::PUT,
            &format!("/api/v1/services/{id}"),
            Some(json!({ "runtime": "systemd", "command": "python3 app.py", "listen_port": 9090 })),
        )
        .await;
    assert_eq!((s, b["spec"]["listen_port"].clone()), (StatusCode::OK, json!(9090)), "{b}");

    for (runtime, extra) in [("container", json!({ "image": "nginx:1" }))] {
        let mut body = json!({ "environment_id": ns["id"], "name": format!("x-{runtime}"), "runtime": runtime, "listen_port": 80 });
        body.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        let (s, e) = admin.post("/api/v1/services", body).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{runtime}: {e}");
    }
}

/// Default checks reported by a node become per-node status; reports for
/// apps not placed on the node are dropped; a node going offline makes them
/// unknown; two apps declaring one port on a node get a warning (D-085).
#[tokio::test]
async fn node_reports_become_instance_status_and_go_unknown_when_offline() {
    use forge_core::ports::ServiceRepository;
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let (_, cl) = admin.post("/api/v1/clusters", json!({ "name": "shop" })).await;
    let (_, ag3) = admin.post("/api/v1/projects", json!({"name": "grp3"})).await;
    let (_, ns) = admin
        .post("/api/v1/environments", json!({ "name": "uat", "cluster_id": cl["id"], "project_id": ag3["id"] }))
        .await;
    let node_id = forge_core::ServerId::new();
    let server = forge_core::model::Server {
        id: node_id,
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
        created_at: forge_core::Timestamp(0),
    };
    forge_core::ports::ServerRepository::create_server(&h.app.state.store, &server).await.unwrap();
    let mut ids = Vec::new();

    for name in ["api", "admin", "loose"] {
        let (s, a) = admin
            .post(
                "/api/v1/services",
                json!({ "environment_id": ns["id"], "name": name, "runtime": "systemd", "command": "x", "listen_port": 8080 }),
            )
            .await;
        assert_eq!(s, StatusCode::CREATED, "{a}");
        ids.push(a["id"].as_str().unwrap().to_owned());
    }

    // `api` and `admin` are placed on the node; `loose` isn't.
    for id in &ids[..2] {
        h.app
            .state
            .store
            .put_instance(&forge_core::model::ServiceInstance {
                service_id: id.parse().unwrap(),
                server_id: node_id,
                role: "active".into(),
                status: "deployed".into(),
                state: Some("running".into()),
                last_job_id: None,
                last_error: None,
                updated_at: forge_core::Timestamp(1),
            })
            .await
            .unwrap();
    }

    let report = |app: &str, purpose: &str, state: &str| forge_proto::messages::HealthReport {
        environment: "uat".into(),
        app: app.into(),
        purpose: purpose.into(),
        check: "default".into(),
        state: state.into(),
        reason: if state == "passing" { "ambiguous_port".into() } else { String::new() },
        detail: String::new(),
        generation: "inv-1".into(),
        observed_at_ms: h.app.state.clock.now().0,
        fresh_for_ms: 90_000,
        policy_revision: 0,
    };
    forge_server::health_ingest::ingest(
        &h.app.state,
        node_id,
        vec![
            report("api", "liveness", "passing"),
            report("api", "readiness", "passing"),
            report("loose", "liveness", "failing"),
        ],
    );
    let url = format!("/api/v1/services/{}/health", ids[0]);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);

    let v = loop {
        let (_, v) = admin.get(&url).await;
        if v["instances"][0]["status"] == "healthy" || std::time::Instant::now() > deadline {
            break v;
        }

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    };

    assert_eq!(v["instances"][0]["status"], "healthy", "{v}");
    assert_eq!(v["instances"][0]["checks"].as_array().unwrap().len(), 2, "{v}");

    // The effective default checks are listed with their resolved form.
    let defaults: Vec<&Value> = v["summaries"].as_array().unwrap().iter().filter(|s| s["default"] == true).collect();
    assert_eq!(defaults.len(), 2, "{v}");
    assert!(defaults[0]["summary"].as_str().unwrap().contains("TCP connect to port 8080"), "{v}");

    // Two apps declare port 8080 on this node.
    assert_eq!(v["warnings"][0]["code"], "ambiguous_listen_port", "{v}");
    assert!(v["warnings"][0]["message"].as_str().unwrap().contains("admin"), "{v}");

    // The unplaced app's report was dropped.
    let (_, loose) = admin.get(&format!("/api/v1/services/{}/health", ids[2])).await;
    assert!(loose["instances"].as_array().unwrap().is_empty(), "{loose}");
    let rows = h.app.state.store.list_all_instances(None).await.unwrap();
    assert_eq!(rows.len(), 2);

    // The node goes offline: its checks read unknown at once.
    h.app.state.health.expire_node(node_id).await.unwrap();
    let (_, v) = admin.get(&url).await;
    assert_eq!(v["instances"][0]["status"], "unknown", "{v}");
    assert!(v["instances"][0]["checks"].as_array().unwrap().iter().all(|c| c["reason"] == "node_offline"), "{v}");
}

/// The `health` app commands queued for `node`, as their specs.
fn pushed_policies(h: &Harness, node: forge_core::ServerId) -> Vec<Value> {
    use forge_proto::messages::{CommandBody, command_body::Command};
    use prost::Message;

    h.app
        .state
        .hub
        .take(node, 16)
        .iter()
        .filter_map(|c| match CommandBody::decode(c.body.as_slice()).ok()?.command? {
            Command::AppAction(a) if a.action == "health" => serde_json::from_str(&a.spec_json).ok(),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn a_saved_policy_is_sent_to_the_nodes_and_configured_checks_decide_the_status() {
    use forge_core::ports::ServiceRepository;
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let (_, cl) = admin.post("/api/v1/clusters", json!({ "name": "shop" })).await;
    let (_, group) = admin.post("/api/v1/projects", json!({ "name": "grp4" })).await;
    let (_, ns) = admin
        .post("/api/v1/environments", json!({ "name": "uat", "cluster_id": cl["id"], "project_id": group["id"] }))
        .await;
    fleet_node(&h, "web-1").await;
    let node = forge_core::ports::ServerRepository::list_servers(&h.app.state.store, None).await.unwrap()[0].id;
    let (s, app) = admin
        .post(
            "/api/v1/services",
            json!({ "environment_id": ns["id"], "name": "api", "runtime": "systemd", "command": "x", "listen_port": 8080 }),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{app}");
    let id = app["id"].as_str().unwrap().to_owned();
    h.app
        .state
        .store
        .put_instance(&forge_core::model::ServiceInstance {
            service_id: id.parse().unwrap(),
            server_id: node,
            role: "active".into(),
            status: "deployed".into(),
            state: Some("running".into()),
            last_job_id: None,
            last_error: None,
            updated_at: forge_core::Timestamp(1),
        })
        .await
        .unwrap();
    let url = format!("/api/v1/services/{id}/health");

    // Saving sends the policy to the node at once, with its revision and
    // nothing of the service's environment.
    let (s, saved) = admin.call(Method::PUT, &url, Some(json!({ "policy": http_policy("") }))).await;
    assert_eq!(s, StatusCode::OK, "{saved}");
    let pushed = pushed_policies(&h, node);
    assert_eq!(pushed.len(), 1, "{pushed:?}");
    let spec = &pushed[0];
    assert_eq!(
        (&spec["environment"], &spec["name"], &spec["listen_port"]),
        (&json!("uat"), &json!("api"), &json!(8080))
    );
    assert_eq!(spec["policy"]["liveness"][0]["name"], "tcp", "{spec}");
    let revision = spec["revision"].as_i64().unwrap();
    assert!(revision > 0 && spec.get("env").is_none() && spec.get("command").is_none(), "{spec}");

    // Before the node reports: the configured checks are listed as unknown.
    let checks = saved["instances"][0]["checks"].as_array().unwrap();
    assert_eq!(checks.len(), 2, "{saved}");
    assert!(checks.iter().all(|c| c["state"] == "unknown" && c["received_at"].is_null()), "{saved}");
    assert_eq!(saved["instances"][0]["status"], "unknown", "{saved}");

    let report = |purpose: &str, check: &str, state: &str, policy_revision: i64| forge_proto::messages::HealthReport {
        environment: "uat".into(),
        app: "api".into(),
        purpose: purpose.into(),
        check: check.into(),
        state: state.into(),
        reason: String::new(),
        detail: String::new(),
        generation: "inv-1".into(),
        observed_at_ms: h.app.state.clock.now().0,
        fresh_for_ms: 90_000,
        policy_revision,
    };
    let status = |want: &'static str| {
        let (admin, url) = (&admin, &url);
        async move {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);

            loop {
                let (_, v) = admin.get(url).await;
                if v["instances"][0]["status"] == want || std::time::Instant::now() > deadline {
                    break v;
                }

                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
    };

    // The node reports under that revision: nothing is sent again, the
    // status follows the configured checks, and a result of a check the
    // policy no longer has is left out.
    forge_server::health_ingest::ingest(
        &h.app.state,
        node,
        vec![
            report("liveness", "tcp", "passing", revision),
            report("liveness", "default", "passing", revision),
            report("readiness", "api", "failing", revision),
            report("readiness", "default", "passing", revision),
            report("liveness", "removed-check", "failing", revision),
        ],
    );
    let v = status("unhealthy").await;
    assert_eq!(v["instances"][0]["status"], "unhealthy", "{v}");
    let names: Vec<(&str, &str)> = v["instances"][0]["checks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| (c["purpose"].as_str().unwrap(), c["name"].as_str().unwrap()))
        .collect();
    assert_eq!(names.len(), 3, "{v}");
    assert!(!names.contains(&("liveness", "removed-check")), "{v}");
    assert!(!names.contains(&("readiness", "default")), "readiness is configured: {v}");
    forge_server::health_ingest::ingest(&h.app.state, node, vec![report("readiness", "api", "passing", revision)]);
    let v = status("healthy").await;
    assert_eq!(v["instances"][0]["status"], "healthy", "{v}");
    assert!(pushed_policies(&h, node).is_empty(), "the node is up to date");

    // A node that checks under another revision (it was redeployed, or was
    // offline when the policy was saved) gets the stored policy again.
    let mut stored = h.app.state.store.get_service(id.parse().unwrap()).await.unwrap().unwrap();
    stored.spec["health"]["liveness"][0]["period_secs"] = json!(30);
    h.app.state.store.put_service(&stored).await.unwrap();
    forge_server::health_ingest::ingest(&h.app.state, node, vec![report("liveness", "tcp", "passing", revision)]);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);

    let again = loop {
        let p = pushed_policies(&h, node);
        if !p.is_empty() || std::time::Instant::now() > deadline {
            break p;
        }

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    };

    assert_eq!(again.len(), 1, "{again:?}");
    assert_ne!(again[0]["revision"].as_i64().unwrap(), revision);
    assert_eq!(again[0]["policy"]["liveness"][0]["period_secs"], 30);
}

/// End to end with a real agent: a saved HTTP and TCP policy reaches the
/// node as a signed command, the agent probes a local listener, its
/// heartbeats carry the results, and the status follows the listener.
#[tokio::test]
async fn a_real_agent_runs_configured_checks_and_reports_them() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let h = start().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;

    // A vault item for a credential header needs an MFA-verified session.
    step_up(&mut admin).await;
    let (_, cl) = admin.post("/api/v1/clusters", json!({ "name": "shop" })).await;
    let (_, group) = admin.post("/api/v1/projects", json!({ "name": "grp5" })).await;
    let (s, ns) = admin
        .post("/api/v1/environments", json!({ "name": "hc-e2e", "cluster_id": cl["id"], "project_id": group["id"] }))
        .await;
    assert_eq!(s, StatusCode::CREATED, "{ns}");
    let (node, _stop) = crate::tunnels::real_node(&h, &admin).await;

    // The "service": answers 200 on a loopback port until it is stopped.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (seen_tx, mut seen) = tokio::sync::mpsc::unbounded_channel::<String>();
    let service = tokio::spawn(async move {
        while let Ok((mut s, _)) = listener.accept().await {
            let mut buf = [0u8; 1024];
            let n = s.read(&mut buf).await.unwrap_or(0);
            let _ = seen_tx.send(String::from_utf8_lossy(&buf[..n]).into_owned());
            let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n").await;
        }
    });

    let (s, app) = admin
        .post(
            "/api/v1/services",
            json!({ "environment_id": ns["id"], "name": "probe", "runtime": "systemd", "command": "x", "listen_port": port }),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{app}");
    let id = app["id"].as_str().unwrap().to_owned();
    forge_core::ports::ServiceRepository::put_instance(
        &h.app.state.store,
        &forge_core::model::ServiceInstance {
            service_id: id.parse().unwrap(),
            server_id: node.parse().unwrap(),
            role: "active".into(),
            status: "deployed".into(),
            state: Some("running".into()),
            last_job_id: None,
            last_error: None,
            updated_at: forge_core::Timestamp(1),
        },
    )
    .await
    .unwrap();

    // What a deploy leaves on the node (deploying needs the root helper).
    let dir = forge_agent::health::descriptor_dir().expect("the agent set up health checks");
    std::fs::create_dir_all(dir).unwrap();
    let descriptor = json!({ "environment": "hc-e2e", "name": "probe", "runtime": "systemd", "listen_port": port,
        "default_checks": true, "has_readiness": false });
    std::fs::write(dir.join("hc-e2e--probe.json"), descriptor.to_string()).unwrap();

    // Defaults off: there is no systemd unit behind this app.
    let fast = json!({ "period_secs": 2, "timeout_secs": 1, "failure_threshold": 1 });
    let check = |name: &str, probe: Value| {
        let mut c = fast.clone();
        c["name"] = json!(name);
        c["probe"] = probe;
        c
    };

    // One check sends a credential from the vault (D-111): the agent asks
    // Forge for the value over its own link just before it probes.
    let (s, item) = admin
        .post("/api/v1/vault/items", json!({ "name": "probe token", "username": "", "password": "Bearer e2e-token" }))
        .await;
    assert_eq!(s, StatusCode::CREATED, "{item}");
    let vref = format!("vault:{}", item["id"].as_str().unwrap());
    let policy = json!({
        "default_checks": false,
        "readiness": [check("port", json!({ "type": "tcp", "port": port }))],
        "liveness": [
            check("http", json!({ "type": "http", "port": port, "path": "/healthz" })),
            check("auth", json!({ "type": "http", "port": port, "path": "/private",
                "headers": [{ "name": "Authorization", "secret": vref }] }))
        ]
    });
    let url = format!("/api/v1/services/{id}/health");
    let (s, saved) = admin.call(Method::PUT, &url, Some(json!({ "policy": policy }))).await;
    assert_eq!(s, StatusCode::OK, "{saved}");

    let wait_for = |want: &'static str| {
        let (admin, url) = (&admin, &url);
        async move {
            // The agent's first round starts within 10 s; then one period and a heartbeat.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(90);

            loop {
                let (_, v) = admin.get(url).await;
                if v["instances"][0]["status"] == want || std::time::Instant::now() > deadline {
                    break v;
                }

                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            }
        }
    };
    let v = wait_for("healthy").await;
    assert_eq!(v["instances"][0]["status"], "healthy", "{v}");
    let checks = v["instances"][0]["checks"].as_array().unwrap();
    assert_eq!(checks.len(), 3, "{v}");
    assert!(checks.iter().all(|c| c["state"] == "passing" && c["received_at"].is_i64()), "{v}");

    // The value reached the target in the header, and only there.
    let mut requests = Vec::new();

    while let Ok(r) = seen.try_recv() {
        requests.push(r);
    }

    let private: Vec<&String> = requests.iter().filter(|r| r.starts_with("GET /private ")).collect();
    assert!(!private.is_empty(), "{requests:?}");
    assert!(private.iter().all(|r| r.contains("Authorization: Bearer e2e-token\r\n")), "{private:?}");
    assert!(requests.iter().filter(|r| !r.starts_with("GET /private ")).all(|r| !r.contains("e2e-token")));
    assert!(!v.to_string().contains("e2e-token"), "no value in the reported status: {v}");
    let descriptor = std::fs::read_to_string(dir.join("hc-e2e--probe.json")).unwrap();
    assert!(descriptor.contains(&vref) && !descriptor.contains("e2e-token"), "nothing at rest on the node");

    // The service stops answering: both checks fail on the node.
    service.abort();
    let _ = service.await;
    let v = wait_for("unhealthy").await;
    assert_eq!(v["instances"][0]["status"], "unhealthy", "{v}");
    let http = v["instances"][0]["checks"].as_array().unwrap().iter().find(|c| c["name"] == "http").unwrap().clone();
    assert_eq!(http["state"], "failing", "{v}");
    assert!(matches!(http["reason"].as_str(), Some("connect_failed" | "timeout")), "{v}");
}

/// D-111: a node gets the vault-backed header values of one of its checks
/// only while the person who last saved the policy may still use the item;
/// the answer is audited once, not per probe, and never with the value.
#[tokio::test]
async fn vault_header_values_reach_a_node_on_the_savers_authority_only() {
    use forge_core::ports::{HealthAuthorityRepository, RoleBindingRepository, ServiceRepository};
    use forge_proto::messages::HealthSecretsRequest;
    let h = start().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    step_up(&mut admin).await;
    let (_, cl) = admin.post("/api/v1/clusters", json!({ "name": "shop" })).await;
    let (_, group) = admin.post("/api/v1/projects", json!({ "name": "grp6" })).await;
    let (_, ns) = admin
        .post("/api/v1/environments", json!({ "name": "uat", "cluster_id": cl["id"], "project_id": group["id"] }))
        .await;
    fleet_node(&h, "web-1").await;
    let node = forge_core::ports::ServerRepository::list_servers(&h.app.state.store, None).await.unwrap()[0].id;
    let (s, app) = admin
        .post(
            "/api/v1/services",
            json!({ "environment_id": ns["id"], "name": "api", "runtime": "systemd", "command": "x", "listen_port": 8080 }),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{app}");
    let id = app["id"].as_str().unwrap().to_owned();
    let service_id: forge_core::ServiceId = id.parse().unwrap();
    h.app
        .state
        .store
        .put_instance(&forge_core::model::ServiceInstance {
            service_id,
            server_id: node,
            role: "active".into(),
            status: "deployed".into(),
            state: Some("running".into()),
            last_job_id: None,
            last_error: None,
            updated_at: forge_core::Timestamp(1),
        })
        .await
        .unwrap();
    let url = format!("/api/v1/services/{id}/health");
    let (s, item) = admin
        .post("/api/v1/vault/items", json!({ "name": "probe token", "username": "", "password": "Bearer e2e-token" }))
        .await;
    assert_eq!(s, StatusCode::CREATED, "{item}");
    let vref = format!("vault:{}", item["id"].as_str().unwrap());

    // Another platform administrator saves the policy: the values are
    // handed out on their authority.
    let mut saver = user_with_role(&h, &admin, "saver@example.com", "platform_admin", "platform:root").await;
    step_up(&mut saver).await;
    let (s, saved) = saver.call(Method::PUT, &url, Some(json!({ "policy": http_policy_with("", Some(&vref)) }))).await;
    assert_eq!(s, StatusCode::OK, "{saved}");
    let pushed = pushed_policies(&h, node);
    let revision = pushed[0]["revision"].as_i64().unwrap();
    assert!(!pushed[0].to_string().contains("e2e-token"), "the policy sent to the node holds the reference only");
    let authority = h.app.state.store.get_health_authority(service_id).await.unwrap().unwrap();
    let saver_id = authority.saved_by.to_string();

    let ask = |purpose: &str, check: &str, policy_revision: i64| HealthSecretsRequest {
        environment: "uat".into(),
        app: "api".into(),
        purpose: purpose.into(),
        check: check.into(),
        policy_revision,
    };
    let secrets = &h.app.state.health_secrets;

    for _ in 0..3 {
        let r = secrets.resolve(node, &ask("readiness", "api", revision)).await;
        assert!(r.granted && r.denial.is_empty(), "{r:?}");
        assert_eq!(r.ttl_secs, 300);
        assert_eq!(r.headers.len(), 1);
        assert_eq!((r.headers[0].name.as_str(), r.headers[0].value.as_str()), ("Authorization", "Bearer e2e-token"));
    }

    // What a node may not ask for: another revision, a check without a
    // reference, an unknown check, and a service placed elsewhere.
    for (req, code) in [
        (ask("readiness", "api", revision + 1), "stale_policy"),
        (ask("liveness", "tcp", revision), "no_such_check"),
        (ask("readiness", "nope", revision), "no_such_check"),
        (ask("liveness", "api", revision), "no_such_check"),
    ] {
        let r = secrets.resolve(node, &req).await;
        assert!(!r.granted && r.headers.is_empty(), "{r:?}");
        assert_eq!(r.denial, code);
    }

    fleet_node(&h, "web-2").await;
    let other = forge_core::ports::ServerRepository::list_servers(&h.app.state.store, None)
        .await
        .unwrap()
        .iter()
        .find(|s| s.id != node)
        .unwrap()
        .id;
    let r = secrets.resolve(other, &ask("readiness", "api", revision)).await;
    assert_eq!((r.granted, r.denial.as_str()), (false, "not_placed"), "a node asks only for its own services");

    let uses = |audit: &Value| -> Vec<Value> {
        audit
            .as_array()
            .or_else(|| audit["entries"].as_array())
            .or_else(|| audit["items"].as_array())
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|e| e.to_string().contains("service.health.secret_use"))
            .collect()
    };
    let (_, audit) = admin.get("/api/v1/audit?limit=200").await;
    let used = uses(&audit);
    assert_eq!(used.len(), 1, "one entry for the grant, none per probe or for the refused requests: {used:?}");
    let text = used[0].to_string();
    assert!(text.contains(&saver_id) && text.contains(&vref["vault:".len()..]), "{text}");
    assert!(!audit.to_string().contains("e2e-token"), "a value never reaches the audit log");

    // The authority holds for the revision that was saved only: a policy
    // that changed some other way has none until someone saves it again.
    let original = h.app.state.store.get_service(service_id).await.unwrap().unwrap();
    let mut changed = original.clone();
    changed.spec["listen_port"] = json!(8081);
    h.app.state.store.put_service(&changed).await.unwrap();
    let other_revision = forge_server::health_ingest::apply_spec(&changed).unwrap().revision;
    assert_ne!(other_revision, revision);
    let r = secrets.resolve(node, &ask("readiness", "api", other_revision)).await;
    assert_eq!((r.granted, r.denial.as_str(), r.headers.len()), (false, "no_authority", 0), "{r:?}");
    h.app.state.store.put_service(&original).await.unwrap();
    assert!(secrets.resolve(node, &ask("readiness", "api", revision)).await.granted);

    // The saver loses the role: refused at the next request, and audited.
    for b in h.app.state.store.list_bindings_for_user(saver_id["user:".len()..].parse().unwrap()).await.unwrap() {
        h.app.state.revoke(b.id).await.unwrap();
    }

    for _ in 0..2 {
        let r = secrets.resolve(node, &ask("readiness", "api", revision)).await;
        assert_eq!((r.granted, r.denial.as_str(), r.headers.len()), (false, "not_allowed", 0), "{r:?}");
        assert_eq!(r.ttl_secs, 60);
    }

    let (_, audit) = admin.get("/api/v1/audit?limit=200").await;
    let used = uses(&audit);
    assert_eq!(used.len(), 4, "granted, no authority, granted again, not allowed: {used:?}");
    assert!(used.iter().any(|e| e.to_string().contains("not_allowed")), "{used:?}");

    // Someone who may use the item saves the policy again: granted again.
    let (s, b) = admin.call(Method::PUT, &url, Some(json!({ "policy": http_policy_with("", Some(&vref)) }))).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert!(secrets.resolve(node, &ask("readiness", "api", revision)).await.granted);

    // The item is deleted: the check gets nothing.
    let (s, b) =
        admin.call(Method::DELETE, &format!("/api/v1/vault/items/{}", item["id"].as_str().unwrap()), None).await;
    assert!(s.is_success(), "{s}: {b}");
    let r = secrets.resolve(node, &ask("readiness", "api", revision)).await;
    assert_eq!((r.granted, r.denial.as_str()), (false, "item_missing"));

    // A policy without references keeps no record of who saved it.
    let (s, b) = admin.call(Method::PUT, &url, Some(json!({ "policy": http_policy("") }))).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert!(h.app.state.store.get_health_authority(service_id).await.unwrap().is_none());
}

/// D-111: a remote probe target is checked against the fleet on the node
/// too, so the policy carries the other nodes' addresses; a policy that
/// probes only the workload carries none.
#[tokio::test]
async fn a_policy_with_a_remote_target_carries_the_fleets_addresses() {
    use forge_core::ports::ServiceRepository;
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let (_, cl) = admin.post("/api/v1/clusters", json!({ "name": "shop" })).await;
    let (_, group) = admin.post("/api/v1/projects", json!({ "name": "grp7" })).await;
    let (_, ns) = admin
        .post("/api/v1/environments", json!({ "name": "uat", "cluster_id": cl["id"], "project_id": group["id"] }))
        .await;
    fleet_node(&h, "web-1").await;
    fleet_node(&h, "db-1").await;
    let servers = forge_core::ports::ServerRepository::list_servers(&h.app.state.store, None).await.unwrap();
    let node = servers.iter().find(|s| s.hostname == "web-1").unwrap().id;
    let db = servers.iter().find(|s| s.hostname == "db-1").unwrap().id;
    forge_server::facts::note_address(node, "10.9.0.1".parse().unwrap());
    forge_server::facts::note_address(db, "10.9.0.2".parse().unwrap());
    let (s, app) = admin
        .post(
            "/api/v1/services",
            json!({ "environment_id": ns["id"], "name": "api", "runtime": "systemd", "command": "x" }),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{app}");
    let id = app["id"].as_str().unwrap().to_owned();
    h.app
        .state
        .store
        .put_instance(&forge_core::model::ServiceInstance {
            service_id: id.parse().unwrap(),
            server_id: node,
            role: "active".into(),
            status: "deployed".into(),
            state: Some("running".into()),
            last_job_id: None,
            last_error: None,
            updated_at: forge_core::Timestamp(1),
        })
        .await
        .unwrap();
    let url = format!("/api/v1/services/{id}/health");

    // The address of another node is refused when the policy is saved.
    let (s, e) = admin.call(Method::PUT, &url, Some(json!({ "policy": http_policy("db-1") }))).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{e}");

    // IPv6 transition addresses are refused whatever they stand for.
    for host in ["2002:a9fe:a9fe::1", "64:ff9b:1::a9fe:a9fe"] {
        let (s, e) = admin.call(Method::PUT, &url, Some(json!({ "policy": http_policy(host) }))).await;
        assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{host}: {e}");
        assert!(e.to_string().contains("transition"), "{host}: {e}");
    }

    // A name Forge can't judge is saved, and the node gets the list.
    let (s, b) = admin.call(Method::PUT, &url, Some(json!({ "policy": http_policy("api.partner.example") }))).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);

    let pushed = loop {
        let p = pushed_policies(&h, node);
        if !p.is_empty() || std::time::Instant::now() > deadline {
            break p;
        }

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    };

    assert_eq!(pushed.len(), 1, "{pushed:?}");
    assert_eq!(pushed[0]["fleet"], json!(["10.9.0.2"]), "the other node's address, not its own: {}", pushed[0]);

    // Probing only the workload: no list.
    let (s, b) = admin.call(Method::PUT, &url, Some(json!({ "policy": http_policy("") }))).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    let pushed = pushed_policies(&h, node);
    assert_eq!(pushed.len(), 1, "{pushed:?}");
    assert!(pushed[0].get("fleet").is_none(), "{}", pushed[0]);
}

#[tokio::test]
async fn health_summary_lists_visible_apps_and_can_deploy_is_a_per_caller_hint() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let (_, cl) = admin.post("/api/v1/clusters", json!({ "name": "explorer" })).await;
    let (_, ag) = admin.post("/api/v1/projects", json!({"name": "grp"})).await;
    let (_, ns) = admin
        .post("/api/v1/environments", json!({ "name": "prod", "cluster_id": cl["id"], "project_id": ag["id"] }))
        .await;
    let (s, app) = admin
        .post(
            "/api/v1/services",
            json!({ "environment_id": ns["id"], "name": "api", "runtime": "systemd", "command": "python3 app.py" }),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{app}");

    // One request covers every visible service; unplaced apps have no instances.
    let (s, v) = admin.get("/api/v1/services/health-summary").await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["truncated"], false, "{v}");
    let item = v["items"].as_array().unwrap().iter().find(|i| i["service_id"] == app["id"]).cloned();
    assert_eq!(item.map(|i| i["instances"].clone()), Some(json!([])), "{v}");

    // The environment filter keeps only that environment.
    let (_, other) =
        admin.get("/api/v1/services/health-summary?environment_id=00000000-0000-0000-0000-000000000000").await;
    assert_eq!(other["items"], json!([]), "{other}");

    // The admin may deploy; a read-only auditor sees the app but not the right.
    let (_, list) = admin.get("/api/v1/services").await;
    assert_eq!(list[0]["can_deploy"], true, "{list}");
    let auditor =
        user_with_role(&h, &admin, "aud@example.com", "auditor", &format!("workspace:{}", h.default_org)).await;
    let (s, list) = auditor.get("/api/v1/services").await;
    assert_eq!(s, StatusCode::OK, "{list}");

    if let Some(a) = list.as_array().unwrap().first() {
        assert_eq!(a["can_deploy"], false, "{list}");
    }

    let (s, one) = auditor.get(&format!("/api/v1/services/{}", app["id"].as_str().unwrap())).await;
    if s == StatusCode::OK {
        assert_eq!(one["can_deploy"], false, "{one}");
    }
}
