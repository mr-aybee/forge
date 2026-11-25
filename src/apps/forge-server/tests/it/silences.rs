//! Silences and maintenance windows (#11, D-112): through the API, the
//! background loops and a real HTTP receiver. An alarm that fires while
//! silenced reaches nobody and its delivery is recorded as `suppressed`;
//! ending the silence while it still fires sends exactly one message; an
//! alarm that fired and resolved inside sends nothing. Silences are
//! authorized on their scope and audited.
#![allow(clippy::unwrap_used)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::common::*;
use forge_core::alerts::{Alarm, AlarmEvent, AlarmState, AlertRepository, Severity};
use forge_core::model::{AgentStatus, NodeRole, Server};
use forge_core::ports::ServerRepository;
use forge_core::{AlarmId, AlertRuleId, EntityKind, EntityRef, ServerId};
use reqwest::StatusCode;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// A loopback receiver that answers `204` and keeps every body.
#[derive(Clone)]
struct Receiver {
    base: String,
    bodies: Arc<Mutex<Vec<Value>>>,
}

impl Receiver {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let me = Self { base: format!("http://{}", listener.local_addr().unwrap()), bodies: Arc::default() };
        let bodies = me.bodies.clone();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let bodies = bodies.clone();
                tokio::spawn(async move {
                    let mut raw = Vec::new();
                    let mut buf = [0u8; 8192];
                    let (end, length) = loop {
                        let Ok(n) = stream.read(&mut buf).await else { return };
                        if n == 0 {
                            return;
                        }
                        raw.extend_from_slice(&buf[..n]);
                        let Some(end) = raw.windows(4).position(|w| w == b"\r\n\r\n") else { continue };
                        let head = String::from_utf8_lossy(&raw[..end]).to_string();
                        let length = head
                            .lines()
                            .filter_map(|l| l.split_once(':'))
                            .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                            .and_then(|(_, v)| v.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                        if raw.len() >= end + 4 + length {
                            break (end, length);
                        }
                    };
                    if let Ok(v) = serde_json::from_slice(&raw[end + 4..end + 4 + length]) {
                        bodies.lock().unwrap().push(v);
                    }
                    let _ = stream
                        .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                        .await;
                    let _ = stream.shutdown().await;
                });
            }
        });
        me
    }

    fn bodies(&self) -> Vec<Value> {
        self.bodies.lock().unwrap().clone()
    }
}

async fn wait_for<T, F, Fut>(what: &str, secs: u64, mut f: F) -> T
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<T>>,
{
    let end = std::time::Instant::now() + Duration::from_secs(secs);
    while std::time::Instant::now() < end {
        if let Some(v) = f().await {
            return v;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    panic!("timed out waiting for {what}");
}

/// A server that may call loopback receivers over plain HTTP.
async fn start_lab() -> Harness {
    start_with(|c| {
        c.alerts.webhook_allow_http = true;
        c.alerts.webhook_allow_loopback = true;
    })
    .await
}

/// A node in the store (its agent is never started).
async fn node(h: &Harness, name: &str) -> ServerId {
    let s = Server {
        id: ServerId::new(),
        workspace_id: h.default_org.parse().unwrap(),
        environment_id: None,
        cluster_id: None,
        role: NodeRole::Worker,
        display_name: None,
        hostname: name.into(),
        os: "linux".into(),
        arch: "x86_64".into(),
        agent_version: "0".into(),
        status: AgentStatus::Online,
        last_seen_at: None,
        cert_serial: None,
        cert_not_after: None,
        created_at: h.app.state.clock.now(),
    };
    h.app.state.store.create_server(&s).await.unwrap();
    s.id
}

/// Opens an alarm on `server` the way the evaluator does.
async fn fire(h: &Harness, server: ServerId) -> Alarm {
    let state = &h.app.state;
    let now = state.clock.now();
    let a = Alarm {
        id: AlarmId::new(),
        workspace_id: h.default_org.parse().unwrap(),
        rule_id: AlertRuleId::new(),
        rule_revision: 1,
        title: "Disk almost full".into(),
        severity: Severity::Major,
        fingerprint: format!("test|{server}|{}", AlarmId::new()),
        resource: EntityRef::new(EntityKind::Server, server),
        server: None,
        subject: "node.disk.root.percent".into(),
        state: AlarmState::Firing,
        fired_value: json!({"type": "num", "value": 97.5}),
        fired_at: now,
        resolved_at: None,
        end_reason: None,
        acknowledged_by: None,
        acknowledged_at: None,
        previous: None,
    };
    let ev = AlarmEvent { alarm_id: a.id, at: now, kind: "fired".into(), actor: None, detail: json!({}) };
    state.store.open_alarm(&a, &ev).await.unwrap();
    a
}

async fn resolve(h: &Harness, a: &Alarm) {
    let state = &h.app.state;
    let now = state.clock.now();
    let ev = AlarmEvent { alarm_id: a.id, at: now, kind: "resolved".into(), actor: None, detail: json!({}) };
    assert!(state.store.close_alarm(a.id, AlarmState::Resolved, now, None, &ev).await.unwrap());
}

/// An administrator (with MFA, for the channel) and a policy that sends
/// every alarm to one webhook on `rx`.
async fn admin_with_channel(h: &Harness, rx: &Receiver) -> Client {
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    step_up(&mut admin).await;
    let (s, c) = admin
        .post(
            "/api/v1/notification-channels",
            json!({"name": "Ops", "kind": "webhook", "url": format!("{}/hook", rx.base)}),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{c}");
    let policy =
        json!({"name": "Everything", "min_severity": "info", "events": ["fired", "resolved"], "channels": [c["id"]]});
    let (s, p) = admin.post("/api/v1/notification-policies", policy).await;
    assert_eq!(s, StatusCode::CREATED, "{p}");
    admin
}

async fn deliveries(c: &Client, alarm: AlarmId) -> Vec<Value> {
    let (s, v) = c.get(&format!("/api/v1/notification-deliveries?alarm={alarm}")).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    v.as_array().unwrap().clone()
}

async fn silence(c: &Client, body: Value) -> Value {
    let (s, v) = c.post("/api/v1/silences", body).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    v
}

#[tokio::test]
async fn an_alarm_during_a_silence_is_suppressed_and_ending_it_sends_exactly_one_message() {
    let h = start_lab().await;
    let rx = Receiver::start().await;
    let admin = admin_with_channel(&h, &rx).await;
    let server = node(&h, "db-01").await;
    let s = silence(
        &admin,
        json!({"scope": {"kind": "servers", "ids": [server]}, "duration_secs": 3600, "reason": "Disk replacement"}),
    )
    .await;
    assert_eq!((s["state"].as_str(), s["reason"].as_str()), (Some("active"), Some("Disk replacement")));
    let sid = s["id"].as_str().unwrap().to_owned();

    let alarm = fire(&h, server).await;
    // Recorded as suppressed with the silence; nothing reaches the receiver.
    let held = wait_for("a suppressed delivery", 60, || {
        let (c, a) = (&admin, alarm.id);
        async move { deliveries(c, a).await.into_iter().find(|d| d["status"] == "suppressed") }
    })
    .await;
    assert_eq!(held["silence_id"], sid);
    assert_eq!(held["event"], "fired");
    assert_eq!(held["detail"], "silenced: Disk replacement");
    let (s, filtered) =
        admin.get(&format!("/api/v1/notification-deliveries?alarm={}&status=suppressed", alarm.id)).await;
    assert_eq!((s, filtered.as_array().map(Vec::len)), (StatusCode::OK, Some(1)));
    assert!(rx.bodies().is_empty());

    // The alarm is still firing and says by which silence and until when.
    let (_, a) = admin.get(&format!("/api/v1/alarms/{}", alarm.id)).await;
    assert_eq!(a["state"], "firing");
    assert_eq!(a["silence"]["silence_id"], sid);
    assert_eq!(a["silence"]["until"], s_until(&admin, &sid).await);
    let (_, list) = admin.get("/api/v1/alarms?active=true").await;
    let row = list.as_array().unwrap().iter().find(|r| r["id"] == json!(alarm.id)).unwrap().clone();
    assert_eq!(row["silence"]["silence_id"], sid);

    // Ended early while the alarm still fires: exactly one message, marked.
    let (st, ended) = admin.post(&format!("/api/v1/silences/{sid}/end"), json!({})).await;
    assert_eq!(st, StatusCode::OK, "{ended}");
    assert_eq!(ended["state"], "expired");
    wait_for("the message after the silence", 60, || {
        let rx = rx.clone();
        async move { (!rx.bodies().is_empty()).then_some(()) }
    })
    .await;
    let body = rx.bodies().remove(0);
    assert_eq!(body["event"]["type"], "fired");
    assert_eq!(body["alarm"]["id"], json!(alarm.id));
    assert_eq!(body["after_silence"], json!({"silence_id": sid}));
    assert_eq!(body["schema_version"], 1);
    tokio::time::sleep(quiet_period(&h)).await;
    assert_eq!(rx.bodies().len(), 1, "exactly one message");
    let list = deliveries(&admin, alarm.id).await;
    let statuses: Vec<(&str, bool)> =
        list.iter().map(|d| (d["status"].as_str().unwrap(), d["after_silence"] == true)).collect();
    assert_eq!(statuses, [("sent", true), ("suppressed", false)], "newest first");
    // No longer marked as silenced.
    let (_, a) = admin.get(&format!("/api/v1/alarms/{}", alarm.id)).await;
    assert!(a.get("silence").is_none(), "{a}");
    // Ended twice: refused, nothing changes.
    assert_eq!(admin.post(&format!("/api/v1/silences/{sid}/end"), json!({})).await.0, StatusCode::CONFLICT);
    h.app.stop().await;
}

async fn s_until(c: &Client, id: &str) -> Value {
    c.get(&format!("/api/v1/silences/{id}")).await.1["ends_at"].clone()
}

#[tokio::test]
async fn an_alarm_that_fires_and_resolves_inside_a_silence_sends_nothing() {
    let h = start_lab().await;
    let rx = Receiver::start().await;
    let admin = admin_with_channel(&h, &rx).await;
    let server = node(&h, "web-01").await;
    let s =
        silence(&admin, json!({"scope": {"kind": "workspace"}, "duration_secs": 3600, "reason": "Planned work"})).await;
    let alarm = fire(&h, server).await;
    wait_for("a suppressed delivery", 60, || {
        let (c, a) = (&admin, alarm.id);
        async move { deliveries(c, a).await.into_iter().find(|d| d["status"] == "suppressed").map(|_| ()) }
    })
    .await;
    resolve(&h, &alarm).await;
    wait_for("the recovery to be dropped", 60, || {
        let (c, a) = (&admin, alarm.id);
        async move {
            deliveries(c, a)
                .await
                .into_iter()
                .find(|d| d["event"] == "resolved" && d["status"] == "cancelled")
                .map(|_| ())
        }
    })
    .await;
    let (st, _) = admin.post(&format!("/api/v1/silences/{}/end", s["id"].as_str().unwrap()), json!({})).await;
    assert_eq!(st, StatusCode::OK);
    tokio::time::sleep(quiet_period(&h)).await;
    assert!(rx.bodies().is_empty(), "{:?}", rx.bodies());
    assert_eq!(deliveries(&admin, alarm.id).await.len(), 2, "nothing planned after the silence");
    h.app.stop().await;
}

#[tokio::test]
async fn silences_are_authorized_on_their_scope_validated_and_audited() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let (_, cl) = admin.post("/api/v1/clusters", json!({"name": "edge"})).await;
    let (_, ag) = admin.post("/api/v1/projects", json!({"name": "shop"})).await;
    let (_, web) = admin
        .post("/api/v1/environments", json!({"name": "web", "cluster_id": cl["id"], "project_id": ag["id"]}))
        .await;
    let (_, api) = admin
        .post("/api/v1/environments", json!({"name": "api", "cluster_id": cl["id"], "project_id": ag["id"]}))
        .await;
    let (web_id, api_id) = (web["id"].as_str().unwrap().to_owned(), api["id"].as_str().unwrap().to_owned());
    let ws = h.default_org.clone();
    let body = |scope: Value| json!({"scope": scope, "duration_secs": 7200, "reason": "Release 4.2"});

    // An operator of one environment silences that environment, nothing wider
    // and nothing beside it. No MFA step-up is needed.
    let op = user_with_role(&h, &admin, "op@example.com", "operator", &format!("environment:{web_id}")).await;
    let mine = silence(&op, body(json!({"kind": "environment", "id": web_id}))).await;
    let mine_id = mine["id"].as_str().unwrap().to_owned();
    for scope in [
        json!({"kind": "workspace"}),
        json!({"kind": "environment", "id": api_id}),
        json!({"kind": "cluster", "id": cl["id"]}),
    ] {
        let (s, e) = op.post("/api/v1/silences", body(scope.clone())).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{scope}: {e}");
    }
    // An id of nothing is refused the same way (no probing).
    let (s, _) = op.post("/api/v1/silences", body(json!({"kind": "servers", "ids": [ServerId::new()]}))).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    // The administrator silences the workspace.
    let wide = silence(&admin, body(json!({"kind": "workspace"}))).await;
    let wide_id = wide["id"].as_str().unwrap().to_owned();

    // The operator reads and changes their own silence, not the wide one.
    let (s, list) = op.get("/api/v1/silences").await;
    assert_eq!(s, StatusCode::OK, "{list}");
    let ids: Vec<&str> = list.as_array().unwrap().iter().filter_map(|v| v["id"].as_str()).collect();
    assert_eq!(ids, [mine_id.as_str()]);
    assert_eq!(op.get(&format!("/api/v1/silences/{wide_id}")).await.0, StatusCode::NOT_FOUND);
    assert_eq!(op.post(&format!("/api/v1/silences/{wide_id}/end"), json!({})).await.0, StatusCode::NOT_FOUND);
    let (s, ext) = op.post(&format!("/api/v1/silences/{mine_id}/extend"), json!({"extend_secs": 3600})).await;
    assert_eq!(s, StatusCode::OK, "{ext}");
    assert_eq!(ext["ends_at"].as_i64().unwrap() - mine["ends_at"].as_i64().unwrap(), 3_600_000);
    let (s, end) = op.post(&format!("/api/v1/silences/{mine_id}/end"), json!({})).await;
    assert_eq!((s, end["state"].as_str()), (StatusCode::OK, Some("expired")));
    assert_eq!(end["ended_by"]["type"], "user", "{end}");

    // An auditor reads every silence and changes none; a customer reads none.
    let aud = user_with_role(&h, &admin, "aud@example.com", "auditor", &format!("workspace:{ws}")).await;
    let (s, list) = aud.get("/api/v1/silences").await;
    assert_eq!((s, list.as_array().map(Vec::len)), (StatusCode::OK, Some(2)));
    let (s, list) = aud.get("/api/v1/silences?state=active").await;
    assert_eq!((s, list[0]["id"].as_str()), (StatusCode::OK, Some(wide_id.as_str())));
    assert_eq!(aud.post("/api/v1/silences", body(json!({"kind": "workspace"}))).await.0, StatusCode::FORBIDDEN);
    assert_eq!(aud.post(&format!("/api/v1/silences/{wide_id}/end"), json!({})).await.0, StatusCode::FORBIDDEN);
    let cust = user_with_role(&h, &admin, "cust@example.com", "customer", &format!("workspace:{ws}")).await;
    assert_eq!(cust.get("/api/v1/silences").await.0, StatusCode::FORBIDDEN);
    assert_eq!(cust.get(&format!("/api/v1/silences/{wide_id}")).await.0, StatusCode::NOT_FOUND);

    // Validation.
    let ws_scope = json!({"kind": "workspace"});
    for (bad, status) in [
        (json!({"scope": ws_scope, "duration_secs": 3600, "reason": " "}), StatusCode::UNPROCESSABLE_ENTITY),
        (json!({"scope": ws_scope, "duration_secs": 31 * 86_400, "reason": "x"}), StatusCode::UNPROCESSABLE_ENTITY),
        (json!({"scope": ws_scope, "duration_secs": 10, "reason": "x"}), StatusCode::UNPROCESSABLE_ENTITY),
        (json!({"scope": ws_scope, "reason": "x"}), StatusCode::BAD_REQUEST),
        (json!({"scope": ws_scope, "duration_secs": 60, "ends_at": 1, "reason": "x"}), StatusCode::BAD_REQUEST),
        (
            json!({"scope": {"kind": "servers", "ids": []}, "duration_secs": 3600, "reason": "x"}),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            json!({"scope": ws_scope, "duration_secs": 3600, "reason": "x", "min_severity": "critical", "max_severity": "info"}),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            json!({"scope": ws_scope, "duration_secs": 3600, "reason": "x", "colour": "red"}),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
    ] {
        let (s, e) = admin.post("/api/v1/silences", bad.clone()).await;
        assert!(
            s == status || (status == StatusCode::UNPROCESSABLE_ENTITY && s == StatusCode::BAD_REQUEST),
            "{bad}: {s} {e}"
        );
    }
    // A maintenance window: scheduled until it starts.
    let start = h.app.state.clock.now().0 + 86_400_000;
    let w = silence(
        &admin,
        json!({"scope": ws_scope, "starts_at": start, "ends_at": start + 3_600_000, "reason": "Patch night"}),
    )
    .await;
    assert_eq!((w["state"].as_str(), w["starts_at"].as_i64()), (Some("scheduled"), Some(start)));
    let (_, sched) = admin.get("/api/v1/silences?state=scheduled").await;
    assert_eq!(sched.as_array().unwrap().len(), 1);

    // Audited with the matcher and the times, including the refusals.
    let (_, audit) = admin.get("/api/v1/audit?limit=200").await;
    let rows = audit["items"].as_array().or_else(|| audit.as_array()).cloned().unwrap_or_default();
    let of = |action: &str| rows.iter().filter(|r| r["action"] == action).cloned().collect::<Vec<Value>>();
    let created = of("alert.silence.create");
    assert_eq!(created.len(), 3, "{audit}");
    let d = &created.iter().find(|r| r.to_string().contains("Release 4.2") && r.to_string().contains(&web_id)).unwrap();
    let text = d.to_string();
    for needle in ["\"environment\"", "starts_at", "ends_at", "Release 4.2"] {
        assert!(text.contains(needle), "{needle} in {text}");
    }
    assert!(of("alert.silence.extend").iter().any(|r| r.to_string().contains("previous_ends_at")));
    assert_eq!(of("alert.silence.end").iter().filter(|r| r["outcome"] != "denied").count(), 1);
    assert!(rows.iter().any(|r| r["action"] == "alert.manage" && r["outcome"] == "denied"), "refusals are recorded");
    h.app.stop().await;
}
