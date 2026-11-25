//! Container shells through the agent link (D-114): a real agent with an
//! in-memory helper answers a signed `ContainerExecOpen`, dials back, and
//! the console's WebSocket frames work as for SSH; resize reaches the
//! helper, the exit code comes back, revoking ends the shell, every step is
//! audited. SSH is used only with `login=`; an agent that never answers
//! gets a clear error in 15 seconds.
#![allow(clippy::unwrap_used)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::common::*;
use forge_agent::container_exec::ExecPipe;
use forge_agent::enroll::{HostFacts, enroll};
use forge_agent::executor::CommandExecutor;
use forge_agent::{Agent, AgentConfig, StatePaths};
use forge_core::ports::ServerRepository;
use forge_core::tags::FactsRepository;
use forge_proto::exec_frame::{Decoder, Frame};
use forge_proto::messages::{CommandBody, SignedCommand};
use futures::{SinkExt, StreamExt};
use prost::Message as _;
use reqwest::StatusCode;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

type Socket = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// What the in-memory helper saw.
#[derive(Debug, Default)]
struct Seen {
    containers: Vec<String>,
    resizes: Vec<(u16, u16)>,
}

/// An agent executor whose helper is in memory: it echoes input, records
/// resizes and exits with 3 on `exit 3`. Container `hang` never answers
/// (an agent that does not know the command).
#[derive(Debug, Clone, Default)]
struct FakeHelper {
    seen: Arc<Mutex<Seen>>,
}

#[async_trait::async_trait]
impl CommandExecutor for FakeHelper {
    async fn execute(&self, cmd: &forge_proto::Command, _: &SignedCommand) -> Result<Value, String> {
        Ok(match cmd {
            forge_proto::Command::CollectFacts(_) => json!({ "os": "linux", "container_exec": true }),
            _ => json!({}),
        })
    }

    async fn open_exec(&self, signed: &SignedCommand) -> Result<ExecPipe, String> {
        let body = CommandBody::decode(signed.body.as_slice()).unwrap();
        let Some(forge_proto::Command::ContainerExecOpen(x)) = body.command else { panic!("not an exec") };
        self.seen.lock().unwrap().containers.push(x.container.clone());

        if x.container == "hang" {
            std::future::pending::<()>().await;
        }

        if x.container == "missing" {
            return Err("no container `missing` on this node".into());
        }

        let (agent, helper) = tokio::io::duplex(64 * 1024);
        let seen = self.seen.clone();
        tokio::spawn(async move {
            let (mut rd, mut wr) = tokio::io::split(helper);
            wr.write_all(&Frame::Data(b"$ ".to_vec()).encode()).await.unwrap();
            let mut dec = Decoder::new();
            let mut buf = vec![0u8; 4096];

            loop {
                let n = rd.read(&mut buf).await.unwrap_or(0);
                if n == 0 {
                    return;
                }

                dec.push(&buf[..n]);

                while let Some(f) = dec.next_frame().unwrap() {
                    match f {
                        Frame::Data(d) if String::from_utf8_lossy(&d).contains("exit 3") => {
                            wr.write_all(&Frame::Exit(3).encode()).await.unwrap();
                            return;
                        }
                        Frame::Data(d) => wr.write_all(&Frame::Data(d).encode()).await.unwrap(),
                        Frame::Resize { cols, rows } => seen.lock().unwrap().resizes.push((cols, rows)),
                        _ => {}
                    }
                }
            }
        });
        let (r, w) = tokio::io::split(agent);

        Ok(ExecPipe { reader: Box::new(r), writer: Box::new(w) })
    }
}

/// A real agent with the in-memory helper, online, its facts collected.
async fn node(h: &Harness, admin: &Client, helper: FakeHelper) -> (String, tokio::sync::watch::Sender<bool>) {
    let (_, tok) = admin.post("/api/v1/enrollment-tokens", json!({"ttl_minutes": 10})).await;
    let dir = tempfile::tempdir().unwrap();
    let paths = StatePaths::new(dir.path());
    let facts = HostFacts { hostname: "box-1".into(), os: "linux".into(), arch: "x86_64".into() };
    let node = enroll(&paths, AgentConfig::new(h.agent_url.clone()), tok["token"].as_str().unwrap(), facts)
        .await
        .unwrap()
        .server_id;
    let (config, identity) = (paths.load_config().unwrap(), paths.load_identity().unwrap());
    let agent = Agent::from_parts(StatePaths::new(dir.path()), config, identity, Box::new(helper)).unwrap();
    let (stop, rx) = tokio::sync::watch::channel(false);
    tokio::spawn(agent.run(rx));
    std::mem::forget(dir);
    crate::tunnels::wait_online(admin, &node).await;
    let id: forge_core::ServerId = node.parse().unwrap();

    for _ in 0..100 {
        let f = h.app.state.store.all_facts().await.unwrap();
        if f.get(&id).is_some_and(|(v, _)| v["container_exec"] == true) {
            return (node, stop);
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    panic!("facts never arrived");
}

async fn api_key(c: &Client) -> String {
    let (s, key) = c.post("/api/v1/api-keys", json!({"name": "cli", "expires_days": 1})).await;
    assert_eq!(s, StatusCode::CREATED, "{key}");

    key["secret"].as_str().unwrap().to_owned()
}

async fn shell(base: &str, node: &str, container: &str, token: &str) -> Socket {
    let url = format!(
        "{}/api/v1/servers/{node}/containers/{container}/shell?cols=90&rows=20",
        base.replacen("http", "ws", 1)
    );
    let mut req = url.into_client_request().unwrap();
    req.headers_mut().insert("authorization", format!("Bearer {token}").parse().unwrap());

    tokio_tungstenite::connect_async(req).await.unwrap().0
}

/// The next JSON status frame (output frames are collected into `out`).
async fn status(ws: &mut Socket, out: &mut Vec<u8>) -> Value {
    loop {
        match tokio::time::timeout(Duration::from_secs(20), ws.next()).await.unwrap().unwrap().unwrap() {
            Message::Text(t) => return serde_json::from_str(t.as_str()).unwrap(),
            Message::Binary(b) => out.extend_from_slice(&b),
            _ => {}
        }
    }
}

async fn output_until(ws: &mut Socket, out: &mut Vec<u8>, needle: &str) {
    while !String::from_utf8_lossy(out).contains(needle) {
        match tokio::time::timeout(Duration::from_secs(20), ws.next()).await.unwrap().unwrap().unwrap() {
            Message::Binary(b) => out.extend_from_slice(&b),
            Message::Text(t) => panic!("unexpected status {t}"),
            _ => {}
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_container_shell_runs_through_the_agent_without_a_terminal_account() {
    let h = start_with(|c| c.security.require_mfa = false).await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let token = api_key(&admin).await;
    let helper = FakeHelper::default();
    let (node, _agent) = node(&h, &admin, helper.clone()).await;

    let (s, check) = admin.get(&format!("/api/v1/servers/{node}/containers/web-1/shell/check")).await;
    assert_eq!(s, StatusCode::OK, "{check}");
    assert_eq!(check["path"], "agent");

    let mut ws = shell(&h.base, &node, "web-1", &token).await;
    let mut out = Vec::new();
    let ready = status(&mut ws, &mut out).await;
    assert_eq!(ready["type"], "ready", "{ready}");
    assert_eq!(ready["path"], "agent");
    output_until(&mut ws, &mut out, "$ ").await;
    ws.send(Message::Binary(b"hello\n".to_vec().into())).await.unwrap();
    output_until(&mut ws, &mut out, "hello").await;
    ws.send(Message::Text(r#"{"type":"resize","cols":100,"rows":40}"#.into())).await.unwrap();
    ws.send(Message::Binary(b"exit 3\n".to_vec().into())).await.unwrap();
    let closed = status(&mut ws, &mut out).await;
    assert_eq!(closed["type"], "closed", "{closed}");
    assert_eq!(closed["reason"], "shell_exited");
    assert_eq!(closed["exit_code"], 3);

    {
        let seen = helper.seen.lock().unwrap();
        assert_eq!(seen.containers, ["web-1"]);
        assert_eq!(seen.resizes, [(100, 40)]);
    }

    // A refusal from the node (no such container) arrives as an error.
    let mut ws = shell(&h.base, &node, "missing", &token).await;
    let e = status(&mut ws, &mut Vec::new()).await;
    assert_eq!(e["type"], "error", "{e}");
    assert!(e["message"].as_str().unwrap().contains("no container `missing`"), "{e}");

    // Revoking from Remote sessions ends the shell.
    let mut ws = shell(&h.base, &node, "web-1", &token).await;
    let ready = status(&mut ws, &mut Vec::new()).await;
    let session = ready["session"].as_str().unwrap().to_owned();
    let (s, _) = admin.post(&format!("/api/v1/terminal-sessions/{session}/revoke"), json!({})).await;
    assert!(s.is_success(), "{s}");
    let closed = status(&mut ws, &mut Vec::new()).await;
    assert_eq!(closed["reason"], "revoked", "{closed}");

    let (_, audit) = admin.get("/api/v1/audit?limit=100").await;
    let text = audit.to_string();
    assert!(text.contains("container.exec_close") && text.contains("\"path\":\"agent\""), "{text}");
    assert!(text.contains("\"exit_code\":3"), "{text}");
    h.app.stop().await;
}

/// The SSH path is used only with `login=`; without it a node that cannot
/// take the agent path says why, never "no terminal account".
#[tokio::test]
async fn ssh_is_used_only_with_an_explicit_login() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let add = |os: &str| forge_core::model::Server {
        id: forge_core::ServerId::new(),
        workspace_id: h.default_org.parse().unwrap(),
        environment_id: None,
        cluster_id: None,
        role: forge_core::model::NodeRole::Worker,
        display_name: None,
        hostname: format!("{os}-1"),
        os: os.into(),
        arch: "x86_64".into(),
        agent_version: "0".into(),
        status: forge_core::model::AgentStatus::Online,
        last_seen_at: None,
        cert_serial: None,
        cert_not_after: None,
        created_at: forge_core::Timestamp(0),
    };
    let old = add("linux");
    h.app.state.store.create_server(&old).await.unwrap();
    h.app.state.store.put_facts(old.id, &json!({ "os": "linux" }), h.app.state.clock.now()).await.unwrap();
    let (s, e) = admin.get(&format!("/api/v1/servers/{}/containers/web-1/shell/check", old.id)).await;
    assert_eq!(s, StatusCode::CONFLICT, "{e}");
    assert_eq!(e["title"], "agent_update_required", "{e}");
    assert!(!e.to_string().contains("terminal account"), "{e}");

    // With `login=` it is the SSH path (this node has no SSH endpoint).
    let (s, e) = admin.get(&format!("/api/v1/servers/{}/containers/web-1/shell/check?login=forgeops", old.id)).await;
    assert_eq!(s, StatusCode::CONFLICT, "{e}");
    assert!(e.to_string().contains("SSH"), "{e}");

    let win = add("windows");
    h.app.state.store.create_server(&win).await.unwrap();
    let (s, e) = admin.get(&format!("/api/v1/servers/{}/containers/web-1/shell/check", win.id)).await;
    assert_eq!(s, StatusCode::CONFLICT, "{e}");
    assert!(e.to_string().contains("not supported on Windows"), "{e}");

    // Refusals before the shell opens are audited with the reason.
    let (_, audit) = admin.get("/api/v1/audit?limit=50").await;
    let text = audit.to_string();
    assert!(text.contains("container.exec") && text.contains("denied"), "{audit}");
    assert!(text.contains("not supported on Windows nodes"), "{audit}");
    assert!(text.contains("container shells need a current agent"), "{audit}");
}

/// An agent that never answers (too old to know the command): a clear
/// error once the open limit (`container_shell_open_secs`, here 2 s) ends.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_agent_that_does_not_answer_gets_an_update_hint() {
    let h = start_with(|c| {
        c.security.require_mfa = false;
        c.container_shell_open_secs = 2;
    })
    .await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let token = api_key(&admin).await;
    let (node, _agent) = node(&h, &admin, FakeHelper::default()).await;
    let started = std::time::Instant::now();
    let mut ws = shell(&h.base, &node, "hang", &token).await;
    let e = status(&mut ws, &mut Vec::new()).await;
    assert_eq!(e["type"], "error", "{e}");
    assert!(e["message"].as_str().unwrap().contains("update the agent"), "{e}");
    let took = started.elapsed();
    assert!(took >= Duration::from_secs(2) && took < Duration::from_secs(6), "{took:?}");
    h.app.stop().await;
}
