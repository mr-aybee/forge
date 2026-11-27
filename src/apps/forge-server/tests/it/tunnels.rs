//! Port tunnels (#35, D-091, D-100): a CLI-style client opens a tunnel to a
//! service from the port catalogue through the relay (WebSocket → control
//! plane → agent dial-back → the port), bytes flow both ways, access comes
//! from an environment role or an explicit grant, raw ports need the
//! administrator exception, lifetimes are capped, and closing ends every leg.
#![allow(clippy::unwrap_used)]

use std::time::Duration;

use forge_agent::enroll::{HostFacts, enroll};
use forge_agent::{Agent, AgentConfig, StatePaths};
use forge_core::ports::{InstallationRepository, ServerRepository, ServiceRepository};
use forge_core::tunnels::TunnelRepository;
use futures::{SinkExt, StreamExt};
use reqwest::StatusCode;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

use crate::common::*;

type Socket = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn ws(base: &str, path: &str, token: &str) -> Socket {
    let url = format!("{}{path}", base.replacen("http", "ws", 1));
    let mut req = url.into_client_request().unwrap();
    req.headers_mut().insert("authorization", format!("Bearer {token}").parse().unwrap());

    tokio_tungstenite::connect_async(req).await.unwrap().0
}

/// An echo server on a loopback port: the "service" on the node.
async fn echo() -> u16 {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = l.accept().await {
            tokio::spawn(async move {
                let mut buf = vec![0u8; 64 * 1024];

                while let Ok(n) = s.read(&mut buf).await {
                    if n == 0 || s.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                }
            });
        }
    });

    port
}

pub(crate) async fn wait_online(c: &Client, id: &str) {
    for _ in 0..100 {
        if c.get(&format!("/api/v1/servers/{id}")).await.1["status"] == "online" {
            return;
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    panic!("node never came online");
}

/// A real agent enrolled as `db-1`, online. Returns its server id and the
/// handle that keeps it running (dropping it stops the agent).
pub(crate) async fn real_node(h: &Harness, admin: &Client) -> (String, tokio::sync::watch::Sender<bool>) {
    let (_, tok) = admin.post("/api/v1/enrollment-tokens", json!({"ttl_minutes": 10})).await;
    let dir = tempfile::tempdir().unwrap();
    let facts = HostFacts { hostname: "db-1".into(), os: "linux".into(), arch: "x86_64".into() };
    let node = enroll(
        &StatePaths::new(dir.path()),
        AgentConfig::new(h.agent_url.clone()),
        tok["token"].as_str().unwrap(),
        facts,
    )
    .await
    .unwrap()
    .server_id;
    let (stop, rx) = tokio::sync::watch::channel(false);
    tokio::spawn(Agent::load(StatePaths::new(dir.path())).unwrap().run(rx));

    // The tempdir must outlive the agent task.
    std::mem::forget(dir);
    wait_online(admin, &node).await;

    (node, stop)
}

/// A native app `site` in environment `web`, listening on `port`, placed on
/// `node` (placed directly: deploying needs a real node).
async fn site_app(h: &Harness, admin: &Client, node: &str, port: u16) -> (String, String) {
    let (_, cl) = admin.post("/api/v1/clusters", json!({ "name": "shop" })).await;
    let (_, group) = admin.post("/api/v1/projects", json!({ "name": "shop" })).await;
    let (s, ns) = admin
        .post("/api/v1/environments", json!({ "name": "web", "cluster_id": cl["id"], "project_id": group["id"] }))
        .await;
    assert_eq!(s, StatusCode::CREATED, "{ns}");
    let (s, app) = admin
        .post(
            "/api/v1/services",
            json!({ "environment_id": ns["id"], "name": "site", "runtime": "systemd", "command": "python3 -m http.server", "listen_port": port }),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{app}");
    let service_id: forge_core::ServiceId = app["id"].as_str().unwrap().parse().unwrap();
    h.app
        .state
        .store
        .put_instance(&forge_core::model::ServiceInstance {
            service_id,
            server_id: node.parse().unwrap(),
            role: "active".into(),
            status: "deployed".into(),
            state: Some("active".into()),
            last_job_id: None,
            last_error: None,
            updated_at: h.app.state.clock.now(),
            desired: forge_core::model::InstanceDesired::Running,
            deployed_release: None,
            exit_code: None,
        })
        .await
        .unwrap();

    (ns["id"].as_str().unwrap().to_owned(), service_id.to_string())
}

/// A bearer token (API key) for a client, so it can hold a control socket.
async fn api_key(c: &Client) -> String {
    let (s, key) = c.post("/api/v1/api-keys", json!({"name": "cli", "expires_days": 1})).await;
    assert_eq!(s, StatusCode::CREATED, "{key}");

    key["secret"].as_str().unwrap().to_owned()
}

fn open_app(ports: Option<u16>) -> Value {
    let mut target = json!({ "kind": "app", "ref": "site" });

    if let Some(p) = ports {
        target["ports"] = json!([p]);
    }

    json!({ "target": target, "bind": "127.0.0.1:18080" })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tunnel_reaches_a_catalogue_service_and_relays_bytes() {
    // API keys only count as MFA-verified when the server does not require MFA.
    let h = start_with(|c| c.security.require_mfa = false).await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let token = api_key(&admin).await;
    let mut cli = Client::anonymous(&h.base);
    cli.bearer = Some(token.clone());

    let port = echo().await;
    let (node, _agent) = real_node(&h, &admin).await;
    let (_ns, service_id) = site_app(&h, &admin, &node, port).await;

    // The catalogue lists the app's port on its node, and the node's SSH port.
    let (s, cat) = admin.get("/api/v1/service-ports?service=site").await;
    assert_eq!(s, StatusCode::OK, "{cat}");
    let entries = cat.as_array().unwrap();
    assert_eq!(entries.len(), 1, "{cat}");
    assert_eq!(entries[0]["kind"], "app");
    assert_eq!(entries[0]["id"], service_id.as_str());
    assert_eq!(entries[0]["port"], port);
    assert_eq!(entries[0]["dial_ip"], "127.0.0.1");
    assert_eq!(entries[0]["role"], "http");
    assert_eq!(entries[0]["server_id"], node.as_str());
    let (_, per_node) = admin.get(&format!("/api/v1/servers/{node}/service-ports")).await;
    let kinds: Vec<&str> = per_node["ports"].as_array().unwrap().iter().filter_map(|e| e["kind"].as_str()).collect();
    assert!(kinds.contains(&"app") && kinds.contains(&"ssh"), "{per_node}");
    let ssh = per_node["ports"].as_array().unwrap().iter().find(|e| e["kind"] == "ssh").unwrap();
    assert_eq!(ssh["port"], 22, "no endpoint set: the default");

    let (s, t) = cli.post("/api/v1/tunnels", open_app(None)).await;
    assert_eq!(s, StatusCode::CREATED, "{t}");
    assert_eq!(t["port"], port);
    assert_eq!(t["role"], "http");
    assert_eq!(t["server"], "db-1");
    assert_eq!(t["target"]["kind"], "app");
    assert_eq!(t["target"]["name"], "site");
    assert!(t["expires_at"].is_i64(), "{t}");
    let id = t["id"].as_str().unwrap().to_owned();
    let mut control = ws(&h.base, &format!("/api/v1/tunnels/{id}/control"), &token).await;
    let first = control.next().await.unwrap().unwrap();
    assert!(first.to_text().unwrap().contains("\"open\""), "{first:?}");

    // One connection: a greeting and 1 MiB both ways.
    let mut conn = ws(&h.base, &format!("/api/v1/tunnels/{id}/connect"), &token).await;
    conn.send(Message::Binary(b"hello".to_vec().into())).await.unwrap();
    let mut got = Vec::new();

    while got.len() < 5 {
        match conn.next().await.unwrap().unwrap() {
            Message::Binary(b) => got.extend_from_slice(&b),
            other => panic!("unexpected {other:?}"),
        }
    }

    assert_eq!(got, b"hello");
    let big: Vec<u8> = (0..1024 * 1024).map(|i| (i % 251) as u8).collect();
    let (mut tx, mut rx) = conn.split();

    let send = {
        let big = big.clone();
        tokio::spawn(async move {
            for chunk in big.chunks(64 * 1024) {
                tx.send(Message::Binary(chunk.to_vec().into())).await.unwrap();
            }

            tx
        })
    };

    let mut back = Vec::with_capacity(big.len());

    while back.len() < big.len() {
        match rx.next().await.unwrap().unwrap() {
            Message::Binary(b) => back.extend_from_slice(&b),
            other => panic!("unexpected {other:?}"),
        }
    }

    assert_eq!(back, big, "every byte, in order");
    let mut tx = send.await.unwrap();

    // Half-close: our side is done; the echo server sees EOF and its end
    // comes back as `eof`.
    tx.send(Message::Text(r#"{"type": "eof"}"#.into())).await.unwrap();
    let mut saw_eof = false;

    while let Some(Ok(m)) = rx.next().await {
        match m {
            Message::Text(t) if t.as_str().contains("eof") => {
                saw_eof = true;
                break;
            }
            Message::Binary(b) => panic!("unexpected {} bytes after the echo", b.len()),
            _ => {}
        }
    }

    assert!(saw_eof, "the node side's end arrives as eof");

    // Listed with its traffic, as a row too.
    let (_, list) = cli.get("/api/v1/tunnels").await;
    let mine = &list["tunnels"][0];
    assert_eq!(mine["id"], id.as_str());
    assert!(mine["bytes_up"].as_u64().unwrap() >= big.len() as u64);
    assert_eq!(list["settings"]["allow_never_expiry"], false);
    let row = h.app.state.store.get_tunnel(&id).await.unwrap().unwrap();
    assert!(row.closed_at.is_none() && row.port == port && row.label == "web/site");

    // Closing ends the control socket with a reason and closes the row.
    let (s, _) = cli.call(reqwest::Method::DELETE, &format!("/api/v1/tunnels/{id}"), None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let mut reason = Value::Null;

    while let Some(Ok(m)) = control.next().await {
        if let Message::Text(t) = m {
            reason = serde_json::from_str(t.as_str()).unwrap();
        }
    }

    assert_eq!(reason["reason"], "closed", "{reason}");
    assert!(cli.get("/api/v1/tunnels").await.1["tunnels"].as_array().unwrap().is_empty());
    let (_, history) = cli.get("/api/v1/tunnels?state=closed").await;
    let closed = &history["tunnels"][0];
    assert_eq!(closed["id"], id.as_str());
    assert_eq!(closed["close_reason"], "closed");
    assert!(closed["bytes_up"].as_u64().unwrap() >= big.len() as u64, "{closed}");

    let (_, audit) = admin.get("/api/v1/audit?limit=100").await;
    let text = audit.to_string();

    for a in ["network.tunnel.open", "network.tunnel.close"] {
        assert!(text.contains(a), "{a} audited");
    }

    h.app.stop().await;
}

/// Access: a developer on the environment may open the tunnel; a user without
/// a role is refused until an administrator grants it; the grant caps the
/// lifetime and revoking it closes the live tunnel; raw ports need
/// `network.tunnel.any`; never-expiry is refused without the setting.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn access_comes_from_a_environment_role_or_a_grant() {
    let h = start_with(|c| c.security.require_mfa = false).await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let port = echo().await;
    let (node, _agent) = real_node(&h, &admin).await;
    let (ns, _app_id) = site_app(&h, &admin, &node, port).await;
    let scope = format!("workspace:{}", h.default_org);
    let dev = user_with_role(&h, &admin, "dev@example.com", "developer", &format!("environment:{ns}")).await;
    let auditor = user_with_role(&h, &admin, "aud@example.com", "auditor", &scope).await;
    let operator = user_with_role(&h, &admin, "op@example.com", "operator", &scope).await;

    // A developer on the environment: allowed, default lifetime (8 h).
    let before = h.app.state.clock.now().0;
    let (s, t) = dev.post("/api/v1/tunnels", open_app(None)).await;
    assert_eq!(s, StatusCode::CREATED, "{t}");
    let exp = t["expires_at"].as_i64().unwrap();
    assert!((exp - before - 8 * 3_600_000).abs() < 60_000, "default lifetime is 8 h: {t}");

    // The developer's own view, and not the admin's `all` list without the right.
    assert_eq!(dev.get("/api/v1/tunnels").await.1["tunnels"].as_array().unwrap().len(), 0, "unclaimed tunnels hide");

    // Lifetimes: more than the maximum is refused; never without the setting too.
    let mut long = open_app(None);
    long["lifetime_minutes"] = json!(8 * 24 * 60);
    let (s, e) = dev.post("/api/v1/tunnels", long).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{e}");
    let mut never = open_app(None);
    never["never_expires"] = json!(true);
    let (s, e) = admin.post("/api/v1/tunnels", never).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{e}");

    // No role on the environment: refused and audited with the reason.
    let (s, e) = auditor.post("/api/v1/tunnels", open_app(None)).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{e}");
    let (_, audit) = admin.get("/api/v1/audit?limit=50").await;
    let denied = audit.to_string();
    assert!(denied.contains("network.tunnel.open") && denied.contains("no grant covers it"), "{denied}");

    // An operator on the workspace may not open a raw port; an admin may.
    let raw = json!({ "target": { "kind": "raw", "ref": node, "ports": [port] } });
    let (s, e) = operator.post("/api/v1/tunnels", raw.clone()).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{e}");
    let (s, t) = admin.post("/api/v1/tunnels", raw).await;
    assert_eq!(s, StatusCode::CREATED, "{t}");
    assert_eq!(t["target"]["kind"], "raw");
    let (_, audit) = admin.get("/api/v1/audit?limit=5").await;
    assert!(audit.to_string().contains("\"raw\":true"), "{audit}");

    // A grant for the auditor, 30 minutes: the tunnel's lifetime is capped
    // by it (min of the 8 h default and the grant).
    let (s, e) = auditor
        .post("/api/v1/tunnel-grants", json!({ "user_id": "x", "target": { "kind": "app", "ref": "site" } }))
        .await;
    assert!(s.is_client_error(), "{e}");
    let (_, users) = admin.get("/api/v1/users").await;
    let aud_id = users.as_array().unwrap().iter().find(|u| u["email"] == "aud@example.com").unwrap()["id"].clone();
    let grant_body = json!({
        "user_id": aud_id, "target": { "kind": "app", "ref": "site", "ports": "*" }, "lifetime_minutes": 30, "note": "demo"
    });
    let (s, g) = admin.post("/api/v1/tunnel-grants", grant_body).await;
    assert_eq!(s, StatusCode::CREATED, "{g}");
    assert_eq!(g["target"]["kind"], "app");
    assert_eq!(g["target"]["name"], "site");
    assert_eq!(g["user"], "aud@example.com");
    assert_eq!(g["status"], "active");
    let grant_id = g["id"].as_str().unwrap().to_owned();
    let (_, mine) = auditor.get("/api/v1/tunnel-grants").await;
    assert_eq!(mine["grants"].as_array().unwrap().len(), 1, "{mine}");
    assert_eq!(mine["can_never_expire"], false);

    let before = h.app.state.clock.now().0;
    let (s, t) = auditor.post("/api/v1/tunnels", open_app(Some(port))).await;
    assert_eq!(s, StatusCode::CREATED, "{t}");
    assert_eq!(t["grant_id"], grant_id.as_str());
    let exp = t["expires_at"].as_i64().unwrap();
    assert!(exp <= before + 31 * 60_000 && exp > before + 25 * 60_000, "capped by the grant: {t}");
    let id = t["id"].as_str().unwrap().to_owned();
    let token = api_key(&auditor).await;
    let mut control = ws(&h.base, &format!("/api/v1/tunnels/{id}/control"), &token).await;
    let _ = control.next().await.unwrap().unwrap();

    // A person with NO role at all (not even view on the workspace) is who a
    // grant is for. They must be able to resolve the granted target in the
    // catalogue and open it; anything the grant does not name stays hidden.
    let guest_user = h
        .app
        .state
        .identity
        .create_account(None, Some("guest@example.com"), "guest@example.com", PASSWORD)
        .await
        .unwrap();
    let guest = Client::login(&h.base, "guest@example.com", PASSWORD).await;
    let (s, _) = guest.get("/api/v1/service-ports?service=site").await;
    assert_eq!(s, StatusCode::FORBIDDEN, "no grant yet: the catalogue is closed to a role-less user");
    let (s, g2) = admin
        .post(
            "/api/v1/tunnel-grants",
            json!({ "user_id": guest_user.id, "target": { "kind": "app", "ref": "site", "ports": "*" }, "lifetime_minutes": 30 }),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{g2}");
    let (s, cat) = guest.get("/api/v1/service-ports?service=site").await;
    assert_eq!(s, StatusCode::OK, "{cat}");
    let entries = cat.as_array().unwrap();
    assert!(!entries.is_empty() && entries.iter().all(|e| e["name"] == "site"), "only the granted app: {cat}");
    let (s, hidden) = guest.get("/api/v1/service-ports").await;
    assert!(s == StatusCode::OK && hidden.as_array().unwrap().iter().all(|e| e["name"] == "site"), "{hidden}");
    let (s, t2) = guest.post("/api/v1/tunnels", open_app(Some(port))).await;
    assert_eq!(s, StatusCode::CREATED, "a role-less grant holder can open: {t2}");
    assert_eq!(t2["grant_id"], g2["id"]);

    // Revoking the grant closes the live tunnel at once.
    let (s, _) = admin.call(reqwest::Method::DELETE, &format!("/api/v1/tunnel-grants/{grant_id}"), None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let mut reason = Value::Null;

    while let Some(Ok(m)) = tokio::time::timeout(Duration::from_secs(5), control.next()).await.ok().flatten() {
        if let Message::Text(t) = m {
            reason = serde_json::from_str(t.as_str()).unwrap();
        }
    }

    assert_eq!(reason["reason"], "access_withdrawn", "{reason}");
    let row = h.app.state.store.get_tunnel(&id).await.unwrap().unwrap();
    assert_eq!(row.close_reason.as_deref(), Some("access_withdrawn"));
    let (s, e) = auditor.post("/api/v1/tunnels", open_app(None)).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "revoked: {e}");
    let (_, all) = admin.get("/api/v1/tunnel-grants?all=true").await;
    let revoked = all["grants"].as_array().unwrap().iter().find(|g| g["id"] == grant_id.as_str()).unwrap();
    assert_eq!(revoked["status"], "revoked", "{all}");

    // Active only: the guest's grant is still live, the auditor's is gone.
    let (_, active) = admin.get("/api/v1/tunnel-grants").await;
    let active = active["grants"].as_array().unwrap();
    assert_eq!(active.len(), 1, "{active:?}");
    assert_eq!(active[0]["user"], "guest@example.com");
    let (_, audit) = admin.get("/api/v1/audit?limit=100").await;
    let text = audit.to_string();
    assert!(text.contains("network.tunnel.grant.create") && text.contains("network.tunnel.grant.revoke"), "{text}");
    h.app.stop().await;
}

/// Rows of tunnels open when the server stops are closed as
/// `server_restart` when it starts again; the catalogue merges package ports
/// with the install parameters.
#[tokio::test]
async fn tunnel_rows_survive_a_restart_as_closed_and_packages_are_listed() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let node_id = forge_core::ServerId::new();
    let server = forge_core::model::Server {
        id: node_id,
        workspace_id: h.default_org.parse().unwrap(),
        environment_id: None,
        cluster_id: None,
        role: forge_core::model::NodeRole::Worker,
        display_name: None,
        hostname: "db-2".into(),
        os: "linux".into(),
        arch: "x86_64".into(),
        agent_version: "0".into(),
        status: forge_core::model::AgentStatus::Online,
        last_seen_at: None,
        cert_serial: None,
        cert_not_after: None,
        drained: false,
        created_at: forge_core::Timestamp(0),
    };
    h.app.state.store.create_server(&server).await.unwrap();
    let now = h.app.state.clock.now();
    let inst = forge_core::model::Installation {
        id: forge_core::InstallationId::new(),
        workspace_id: server.workspace_id,
        server_id: node_id,
        package: "postgresql".into(),
        version: "16".into(),
        params: [("port".to_owned(), "5433".to_owned())].into_iter().collect(),
        status: "installed".into(),
        state: Some("running".into()),
        env: Default::default(),
        last_job_id: None,
        last_error: None,
        versions: None,
        update_policy: "notify".into(),
        created_at: now,
        updated_at: now,
    };
    h.app.state.store.put_installation(&inst).await.unwrap();
    let (s, cat) = admin.get("/api/v1/service-ports?kind=package").await;
    assert_eq!(s, StatusCode::OK, "{cat}");
    let pg = &cat.as_array().unwrap()[0];
    assert_eq!(pg["name"], "postgresql");
    assert_eq!(pg["id"], inst.id.to_string());
    assert_eq!(pg["port"], 5433, "the install parameter wins over the default");
    assert_eq!(pg["role"], "db");

    // The package is also reachable by package id or installation id.
    let (_, by_name) = admin.get("/api/v1/service-ports?service=postgresql").await;
    assert_eq!(by_name.as_array().unwrap().len(), 1);

    // An open row (as if the server died holding the tunnel).
    let admin_id: forge_core::UserId = admin.get("/api/v1/me").await.1["user"]["id"].as_str().unwrap().parse().unwrap();
    let row = forge_core::tunnels::TunnelRecord {
        id: "a".repeat(32),
        workspace_id: server.workspace_id,
        user_id: admin_id,
        server_id: node_id,
        target_kind: forge_core::tunnels::TunnelTargetKind::Package,
        target_ref: inst.id.to_string(),
        label: "postgresql on db-2".into(),
        port: 5433,
        dial_ip: "127.0.0.1".into(),
        bind: "127.0.0.1:15432".into(),
        grant_id: None,
        opened_at: now,
        expires_at: forge_core::Timestamp(now.0 + 3_600_000),
        closed_at: None,
        close_reason: None,
        bytes_up: 1,
        bytes_down: 2,
    };
    h.app.state.store.put_tunnel(&row).await.unwrap();
    let config = h.app.state.config.clone();
    h.app.stop().await;

    // A new server process over the same database.
    let state = forge_server::Services::build(config).await.unwrap();
    let after = state.store.get_tunnel(&row.id).await.unwrap().unwrap();
    assert!(after.closed_at.is_some());
    assert_eq!(after.close_reason.as_deref(), Some("server_restart"));
    assert_eq!((after.bytes_up, after.bytes_down), (1, 2));
}
