//! Alarm notifications (#11, D-110): a fired alarm reaches every channel of
//! its policy once, signed, and recovery follows; a failing receiver is
//! retried and then recorded as failed without touching the alarm; channels
//! are an administrator's, MFA-guarded and never give back their URL.
#![allow(clippy::unwrap_used)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::common::*;
use forge_agent::enroll::{HostFacts, enroll};
use forge_agent::{Agent, AgentConfig, StatePaths};
use forge_alerts::notify::payload;
use forge_core::alerts::{Alarm, AlarmEvent, AlarmState, AlertRepository, Severity};
use forge_core::notifications::NotificationRepository;
use forge_core::{AlarmId, AlertRuleId, EntityKind, EntityRef};
use reqwest::StatusCode;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

mod email;

/// The token in every channel URL of these tests: it must never come back.
const URL_TOKEN: &str = "t0ken-in-the-url";
const SIGNING_SECRET: &str = "whsec-Sup3r-s3cret";

#[derive(Debug, Clone)]
struct Seen {
    path: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

impl Seen {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap()
    }
}

/// A receiver on loopback that records every request and answers with the
/// scripted status lines, then `204`.
#[derive(Clone)]
struct Receiver {
    base: String,
    seen: Arc<Mutex<Vec<Seen>>>,
    script: Arc<Mutex<Vec<String>>>,
    always: Arc<Mutex<Option<String>>>,
}

impl Receiver {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let me = Self {
            base: format!("http://{}", listener.local_addr().unwrap()),
            seen: Arc::default(),
            script: Arc::default(),
            always: Arc::default(),
        };
        let this = me.clone();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let this = this.clone();
                tokio::spawn(async move {
                    let mut raw = Vec::new();
                    let mut buf = [0u8; 8192];
                    let (head_end, length) = loop {
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
                    let head = String::from_utf8_lossy(&raw[..head_end]).to_string();
                    let mut lines = head.lines();
                    let path = lines.next().and_then(|l| l.split(' ').nth(1)).unwrap_or_default().to_owned();
                    let headers = lines
                        .filter_map(|l| l.split_once(':'))
                        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned()))
                        .collect();
                    let body = raw[head_end + 4..head_end + 4 + length].to_vec();
                    this.seen.lock().unwrap().push(Seen { path, headers, body });
                    let status = {
                        let mut script = this.script.lock().unwrap();
                        if script.is_empty() {
                            this.always.lock().unwrap().clone().unwrap_or_else(|| "204 No Content".into())
                        } else {
                            script.remove(0)
                        }
                    };
                    let answer = format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                    let _ = stream.write_all(answer.as_bytes()).await;
                    let _ = stream.shutdown().await;
                });
            }
        });
        me
    }

    fn url(&self, path: &str) -> String {
        format!("{}/{path}?token={URL_TOKEN}", self.base)
    }

    fn requests(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    fn to(&self, path: &str) -> Vec<Seen> {
        self.requests().into_iter().filter(|s| s.path.starts_with(&format!("/{path}?"))).collect()
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

/// A server that may call loopback receivers over plain HTTP, with quick
/// retries.
async fn start_lab() -> Harness {
    start_with(|c| {
        c.alerts.webhook_allow_http = true;
        c.alerts.webhook_allow_loopback = true;
        c.alerts.delivery_attempts = 3;
        c.alerts.delivery_retry_base_secs = 1;
        c.alerts.delivery_retry_max_secs = 2;
    })
    .await
}

fn webhook(name: &str, url: &str) -> Value {
    json!({"name": name, "kind": "webhook", "url": url})
}

fn policy(channels: &[&Value]) -> Value {
    json!({
        "name": "Everything",
        "min_severity": "info",
        "events": ["fired", "resolved"],
        "channels": channels.iter().map(|c| c["id"].clone()).collect::<Vec<_>>(),
    })
}

async fn created(c: &Client, path: &str, body: Value) -> Value {
    let (s, v) = c.post(path, body).await;
    assert_eq!(s, StatusCode::CREATED, "{path}: {v}");
    v
}

async fn deliveries(c: &Client, query: &str) -> Vec<Value> {
    let (s, v) = c.get(&format!("/api/v1/notification-deliveries{query}")).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    v.as_array().unwrap().clone()
}

/// What the customer of the authorization test gets for `path`.
async fn customer_get(h: &Harness, path: &str) -> StatusCode {
    let c = Client::login(&h.base, "cust@example.com", PASSWORD).await;
    c.get(path).await.0
}

/// Enrolls a node (its agent is not started) and returns its id.
async fn node(h: &Harness, admin: &Client, name: &str) -> (String, tempfile::TempDir) {
    let (_, tok) = admin.post("/api/v1/enrollment-tokens", json!({"ttl_minutes": 10})).await;
    let dir = tempfile::tempdir().unwrap();
    let facts = HostFacts { hostname: name.into(), os: "linux".into(), arch: "x86_64".into() };
    let config = AgentConfig::new(h.agent_url.clone());
    let id =
        enroll(&StatePaths::new(dir.path()), config, tok["token"].as_str().unwrap(), facts).await.unwrap().server_id;
    (id, dir)
}

/// Opens an alarm on `server` the way the evaluator does: the alarm and its
/// transition in one transaction.
async fn fire(h: &Harness, server: &str) -> Alarm {
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
        resource: EntityRef { kind: EntityKind::Server, id: server.to_owned() },
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

/// The whole path: a rule on a real node's agent status, the evaluator, the
/// outbox, the planner and the HTTP transport.
#[tokio::test]
async fn a_node_alarm_reaches_each_channel_once_signed_and_recovery_follows() {
    let h = start_lab().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    step_up(&mut admin).await;
    let rx = Receiver::start().await;

    let item = json!({"name": "webhook signing", "password": SIGNING_SECRET});
    let item = created(&admin, "/api/v1/vault/items", item).await;
    let mut signed = webhook("Ops webhook", &rx.url("signed"));
    signed["secret_item"] = item["id"].clone();
    let signed = created(&admin, "/api/v1/notification-channels", signed).await;
    let chat = json!({"name": "Team chat", "kind": "slack", "url": rx.url("chat")});
    let chat = created(&admin, "/api/v1/notification-channels", chat).await;
    // What comes back of a channel: its host, never its URL.
    assert_eq!(signed["target"], rx.base);
    assert_eq!(signed["secret_item"], item["id"]);
    for c in [&signed, &chat] {
        assert!(c.get("url").is_none() && c.get("sealed_url").is_none(), "{c}");
        assert!(!c.to_string().contains(URL_TOKEN) && !c.to_string().contains(SIGNING_SECRET), "{c}");
    }
    created(&admin, "/api/v1/notification-policies", policy(&[&signed, &chat])).await;
    // Nothing is sent by configuring.
    assert!(rx.requests().is_empty());

    let rule = json!({
        "name": "Node offline",
        "severity": "critical",
        "scope": {"kind": "servers"},
        "subject": "node.agent.status",
        "spec": {
            "trigger": {"kind": "state_in", "states": ["offline"]},
            "mode": {"mode": "immediate"},
            "recovery": {"condition": {"kind": "state_in", "states": ["online"]}, "hold_ms": 0},
        },
    });
    created(&admin, "/api/v1/alert-rules", rule).await;
    let (id, dir) = node(&h, &admin, "web-01").await;
    let (stop, agent_rx) = tokio::sync::watch::channel(false);
    tokio::spawn(Agent::load(StatePaths::new(dir.path())).unwrap().run(agent_rx));
    wait_for("the node online", 30, || {
        let (c, id) = (&admin, id.clone());
        async move { (c.get(&format!("/api/v1/servers/{id}")).await.1["status"] == "online").then_some(()) }
    })
    .await;
    stop.send(true).unwrap();

    // Fired: one request per channel.
    wait_for("both fired messages", 90, || {
        let rx = rx.clone();
        async move { (rx.to("signed").len() == 1 && rx.to("chat").len() == 1).then_some(()) }
    })
    .await;
    let (_, alarms) = admin.get("/api/v1/alarms?active=true").await;
    let alarm = alarms[0].clone();
    let aid = alarm["id"].as_str().unwrap().to_owned();

    let req = rx.to("signed").remove(0);
    assert!(req.path.ends_with(&format!("token={URL_TOKEN}")), "the stored URL is called whole");
    assert_eq!(req.headers["content-type"], "application/json");
    assert_eq!(req.headers["x-forge-event"], "fired");
    assert!(req.headers["user-agent"].starts_with("Orbyn-Forge-Alerts/"));
    let body = req.json();
    assert_eq!(body["schema_version"], 1);
    assert_eq!(body["event"]["type"], "fired");
    assert_eq!(body["event"]["is_resolved"], false);
    assert_eq!(body["alarm"]["id"], aid);
    assert_eq!(body["alarm"]["title"], "Node offline");
    assert_eq!(body["alarm"]["severity"], "critical");
    assert_eq!(body["alarm"]["state"], "firing");
    assert_eq!(body["alarm"]["acknowledged"], false);
    assert_eq!(body["resource"], json!({"kind": "server", "id": id, "name": "web-01"}));
    assert_eq!(
        body["observation"],
        json!({"subject": "node.agent.status", "value": {"type": "state", "value": "offline"}})
    );
    assert_eq!(body["links"]["alarm"], format!("{}/alerts?alarm={aid}", h.base));
    assert_eq!(body["delivery_id"].as_str(), Some(req.headers["x-forge-delivery"].as_str()));
    // A valid signature over the exact bytes, made with the vault item's password.
    let at: i64 = req.headers["x-forge-timestamp"].parse().unwrap();
    let sig = &req.headers["x-forge-signature"];
    assert!(payload::verify(SIGNING_SECRET.as_bytes(), at, &req.body, sig), "{sig}");
    assert!(!payload::verify(b"another secret", at, &req.body, sig));
    let text = String::from_utf8_lossy(&req.body).to_string();
    assert!(!text.contains(SIGNING_SECRET) && !text.contains(URL_TOKEN), "{text}");

    let req = rx.to("chat").remove(0);
    assert!(!req.headers.contains_key("x-forge-signature"));
    let text = req.json()["text"].as_str().unwrap().to_owned();
    assert!(text.starts_with("ALERT [critical] Node offline\nResource: server web-01\n"), "{text}");

    // History: exactly one delivery per channel, each sent in one attempt.
    let list = deliveries(&admin, &format!("?alarm={aid}")).await;
    assert_eq!(list.len(), 2, "{list:?}");
    for d in &list {
        assert_eq!(
            (d["event"].as_str(), d["status"].as_str(), d["attempt_count"].as_i64()),
            (Some("fired"), Some("sent"), Some(1)),
            "{d}"
        );
        assert_eq!(d["policy_name"], "Everything");
        assert!(!d.to_string().contains(URL_TOKEN), "{d}");
    }
    let one = list.iter().find(|d| d["channel_name"] == "Ops webhook").unwrap();
    let (s, view) = admin.get(&format!("/api/v1/notification-deliveries/{}", one["id"].as_str().unwrap())).await;
    assert_eq!(s, StatusCode::OK, "{view}");
    assert_eq!(view["attempt_count"], 1);
    assert_eq!(view["attempts"].as_array().unwrap().len(), 1);
    assert_eq!(view["attempts"][0]["ok"], true);
    assert_eq!(view["attempts"][0]["http_status"], 204);

    // Replaying the transition, through the planner and behind its back,
    // adds nothing and sends nothing.
    let state = &h.app.state;
    assert_eq!(state.notifier.plan().await.unwrap().deliveries, 0);
    let again = forge_core::notifications::AlarmTransition {
        alarm_id: aid.parse().unwrap(),
        workspace_id: h.default_org.parse().unwrap(),
        kind: forge_core::notifications::TransitionKind::Fired,
        at: state.clock.now(),
    };
    assert_eq!(state.store.plan_transition(&again, &[], state.clock.now()).await.unwrap(), 0);
    assert_eq!(state.notifier.dispatch().await.unwrap().sent, 0);
    tokio::time::sleep(quiet_period(&h)).await;
    assert_eq!(deliveries(&admin, &format!("?alarm={aid}")).await.len(), 2);
    assert_eq!(rx.requests().len(), 2, "one message per channel for one firing");

    // The agent comes back: the recovery goes to both channels.
    let (_stop2, agent_rx2) = tokio::sync::watch::channel(false);
    tokio::spawn(Agent::load(StatePaths::new(dir.path())).unwrap().run(agent_rx2));
    wait_for("both recovery messages", 90, || {
        let rx = rx.clone();
        async move { (rx.to("signed").len() == 2 && rx.to("chat").len() == 2).then_some(()) }
    })
    .await;
    let req = rx.to("signed").remove(1);
    assert_eq!(req.headers["x-forge-event"], "resolved");
    let body = req.json();
    assert_eq!(body["event"]["is_resolved"], true);
    assert_eq!(body["alarm"]["state"], "resolved");
    assert!(body["alarm"]["resolved_at"].is_number());
    assert!(rx.to("chat")[1].json()["text"].as_str().unwrap().starts_with("RECOVERED [critical] Node offline"));
    assert_eq!(deliveries(&admin, &format!("?alarm={aid}&status=sent")).await.len(), 4);
    assert_eq!(rx.requests().len(), 4);

    // The alarm's own timeline knows nothing of notifications.
    let (_, done) = admin.get(&format!("/api/v1/alarms/{aid}")).await;
    let kinds: Vec<&str> = done["events"].as_array().unwrap().iter().map(|e| e["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["fired", "resolved"]);
    h.app.stop().await;
}

#[tokio::test]
async fn a_failing_receiver_is_retried_then_failed_and_the_alarm_is_untouched() {
    let h = start_lab().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    step_up(&mut admin).await;
    let rx = Receiver::start().await;
    *rx.always.lock().unwrap() = Some("503 Service Unavailable".into());
    let c = created(&admin, "/api/v1/notification-channels", webhook("Ops webhook", &rx.url("down"))).await;
    created(&admin, "/api/v1/notification-policies", policy(&[&c])).await;
    let (server, _dir) = node(&h, &admin, "db-01").await;
    let alarm = fire(&h, &server).await;
    let before = h.app.state.store.get_alarm(alarm.id).await.unwrap();

    let failed = wait_for("the delivery to fail", 60, || {
        let (c, aid) = (&admin, alarm.id);
        async move { deliveries(c, &format!("?alarm={aid}&status=failed")).await.pop() }
    })
    .await;
    assert_eq!(failed["attempt_count"], 3, "{failed}");
    assert_eq!((failed["last_error"].as_str(), failed["last_http_status"].as_i64()), (Some("http_status"), Some(503)));
    assert!(failed["finished_at"].is_number());
    let (_, view) = admin.get(&format!("/api/v1/notification-deliveries/{}", failed["id"].as_str().unwrap())).await;
    let attempts = view["attempts"].as_array().unwrap();
    assert_eq!(attempts.iter().map(|a| a["number"].as_i64().unwrap()).collect::<Vec<_>>(), [1, 2, 3]);
    assert!(attempts.iter().all(|a| a["ok"] == false && a["http_status"] == 503 && a["error"] == "http_status"));
    // Each retry waited at least half the backoff ceiling (0.5 s, then 1 s).
    let at = |i: usize| attempts[i]["at"].as_i64().unwrap();
    assert!(at(1) - at(0) >= 500 && at(2) - at(1) >= 1000, "{attempts:?}");

    // No further attempt after the limit, and the same delivery id on every try.
    tokio::time::sleep(quiet_period(&h)).await;
    let tries = rx.to("down");
    assert_eq!(tries.len(), 3);
    assert!(tries.iter().all(|t| t.headers["x-forge-delivery"] == failed["id"].as_str().unwrap()));

    // The alarm is exactly as the evaluator left it, and still firing.
    assert_eq!(h.app.state.store.get_alarm(alarm.id).await.unwrap(), before);
    let (_, shown) = admin.get(&format!("/api/v1/alarms/{}", alarm.id)).await;
    assert_eq!(shown["state"], "firing");
    assert_eq!(shown["events"].as_array().unwrap().len(), 1);

    // It recovers: no "recovered" goes where "firing" never arrived.
    resolve(&h, &alarm).await;
    let cancelled = wait_for("the recovery to be dropped", 30, || {
        let (c, aid) = (&admin, alarm.id);
        async move { deliveries(c, &format!("?alarm={aid}&status=cancelled")).await.pop() }
    })
    .await;
    assert_eq!(cancelled["event"], "resolved");
    assert_eq!(cancelled["detail"], "the firing notification was never delivered");
    assert_eq!(rx.to("down").len(), 3);
    h.app.stop().await;
}

#[tokio::test]
async fn redirects_are_not_followed_and_a_test_message_reports_its_result() {
    let h = start_lab().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    step_up(&mut admin).await;
    let rx = Receiver::start().await;
    let elsewhere = Receiver::start().await;
    let c = created(&admin, "/api/v1/notification-channels", webhook("Ops webhook", &rx.url("hook"))).await;
    let test = format!("/api/v1/notification-channels/{}/test", c["id"].as_str().unwrap());

    let (s, sent) = admin.post(&test, json!({})).await;
    assert_eq!(s, StatusCode::OK, "{sent}");
    assert_eq!((sent["event"].as_str(), sent["status"].as_str()), (Some("test"), Some("sent")), "{sent}");
    assert!(sent.get("alarm_id").is_none());
    let seen = rx.to("hook").remove(0);
    assert_eq!(seen.headers["x-forge-event"], "test");
    assert_eq!(seen.json()["message"], "Test notification from Orbyn Forge. No alarm fired.");
    assert!(seen.json().get("alarm").is_none());

    // A second test right away is refused: nothing leaves, and it is audited.
    let (s, e) = admin.post(&test, json!({})).await;
    assert_eq!((s, e["title"].as_str()), (StatusCode::TOO_MANY_REQUESTS, Some("rate_limited")), "{e}");
    assert_eq!(rx.to("hook").len(), 1);

    // A receiver that redirects (here to another loopback service): the
    // attempt fails, nothing follows the Location, and it is not retried.
    let c2 = created(&admin, "/api/v1/notification-channels", webhook("Second webhook", &rx.url("moved"))).await;
    let test2 = format!("/api/v1/notification-channels/{}/test", c2["id"].as_str().unwrap());
    rx.script.lock().unwrap().push(format!("302 Found\r\nLocation: {}", elsewhere.url("internal")));
    let (s, moved) = admin.post(&test2, json!({})).await;
    assert_eq!(s, StatusCode::OK, "{moved}");
    assert_eq!(moved["status"], "failed", "{moved}");
    assert_eq!((moved["last_error"].as_str(), moved["last_http_status"].as_i64()), (Some("http_status"), Some(302)));
    assert_eq!(moved["attempts"].as_array().unwrap().len(), 1);
    tokio::time::sleep(quiet_period(&h)).await;
    assert!(elsewhere.requests().is_empty(), "the redirect target is never called");
    assert_eq!((rx.to("hook").len(), rx.to("moved").len()), (1, 1));

    // A channel that is turned off is not tested.
    let off = json!({"name": "Ops webhook", "kind": "webhook", "enabled": false});
    let (s, updated) = admin.put(&format!("/api/v1/notification-channels/{}", c["id"].as_str().unwrap()), off).await;
    assert_eq!(s, StatusCode::OK, "{updated}");
    assert_eq!(updated["target"], rx.base, "an update without a URL keeps the stored one");
    assert_eq!(admin.post(&test, json!({})).await.0, StatusCode::CONFLICT);

    // Every test is audited with its outcome and without the URL: the two
    // that were sent, the one refused for coming too soon, the one refused
    // because the channel is off.
    let (_, audit) = admin.get("/api/v1/audit?limit=100").await;
    let text = audit.to_string();
    assert_eq!(text.matches("notification.channel.test").count(), 4, "{text}");
    assert!(!text.contains(URL_TOKEN), "the audit log never holds a channel URL");
    h.app.stop().await;
}

#[tokio::test]
async fn channels_need_an_administrator_with_mfa_and_refuse_internal_targets() {
    // The default rules: public HTTPS only.
    let h = start().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let ws = format!("workspace:{}", h.default_org);
    let good = || webhook("Ops webhook", &format!("https://hooks.example.com/in?token={URL_TOKEN}"));

    // Without a verified second factor: refused, and the refusal is audited.
    let (s, e) = admin.post("/api/v1/notification-channels", good()).await;
    assert_eq!((s, e["title"].as_str()), (StatusCode::FORBIDDEN, Some("step_up_required")), "{e}");
    step_up(&mut admin).await;

    // Targets only the control plane could reach are refused when saving.
    for bad in [
        "http://hooks.example.com/in",
        "https://169.254.169.254/latest/meta-data/iam/",
        "https://[fd00:ec2::254]/",
        "https://[::ffff:169.254.169.254]/",
        "https://2852039166/",
        "https://metadata.google.internal/computeMetadata/v1/",
        "https://127.0.0.1:8443/hook",
        "https://localhost/hook",
        "https://[::1]/hook",
        "https://10.0.0.5/hook",
        "https://192.168.1.20/hook",
        "https://100.64.0.1/hook",
        "https://user:pw@hooks.example.com/hook",
        "ftp://hooks.example.com/hook",
        "file:///etc/passwd",
    ] {
        let (s, e) = admin.post("/api/v1/notification-channels", webhook("bad", bad)).await;
        assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{bad}: {e}");
        assert!(!e.to_string().contains("pw@"), "{e}");
    }
    let (s, e) = admin.post("/api/v1/notification-channels", json!({"name": "no url", "kind": "webhook"})).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{e}");
    let mut unknown = good();
    unknown["headers"] = json!({"authorization": "Bearer x"});
    assert!(
        admin.post("/api/v1/notification-channels", unknown).await.0.is_client_error(),
        "unknown fields are refused"
    );

    let channel = created(&admin, "/api/v1/notification-channels", good()).await;
    let cid = channel["id"].as_str().unwrap().to_owned();
    assert_eq!(channel["target"], "https://hooks.example.com");
    assert!(!channel.to_string().contains(URL_TOKEN) && !channel.to_string().contains("/in"), "{channel}");
    let (_, list) = admin.get("/api/v1/notification-channels").await;
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert!(!list.to_string().contains(URL_TOKEN), "{list}");
    let (_, got) = admin.get(&format!("/api/v1/notification-channels/{cid}")).await;
    assert!(got.get("url").is_none() && !got.to_string().contains(URL_TOKEN), "{got}");

    // Roles. An operator manages policies, not channels; a developer reads;
    // a customer sees nothing.
    let mut operator = user_with_role(&h, &admin, "op@example.com", "operator", &ws).await;
    step_up(&mut operator).await;
    let developer = user_with_role(&h, &admin, "dev@example.com", "developer", &ws).await;
    let customer = user_with_role(&h, &admin, "cust@example.com", "customer", &ws).await;
    let channel_path = format!("/api/v1/notification-channels/{cid}");
    assert_eq!(operator.post("/api/v1/notification-channels", good()).await.0, StatusCode::FORBIDDEN);
    // By id, a channel the caller may not manage answers like an id of
    // nothing (and the refusal is audited, below).
    let nothing = format!("/api/v1/notification-channels/{}", forge_core::ChannelId::new());
    for path in [&channel_path, &nothing] {
        let (s, e) = operator.put(path, good()).await;
        assert_eq!((s, e["detail"].as_str()), (StatusCode::NOT_FOUND, Some("notification channel not found")), "{e}");
        assert_eq!(operator.call(reqwest::Method::DELETE, path, None).await.0, StatusCode::NOT_FOUND);
        assert_eq!(operator.post(&format!("{path}/test"), json!({})).await.0, StatusCode::NOT_FOUND);
        assert_eq!(customer_get(&h, path).await, StatusCode::NOT_FOUND);
    }
    assert_eq!(operator.get("/api/v1/notification-channels").await.0, StatusCode::OK);
    let made = created(&operator, "/api/v1/notification-policies", policy(&[&channel])).await;
    let policy_path = format!("/api/v1/notification-policies/{}", made["id"].as_str().unwrap());
    for path in ["/api/v1/notification-channels", "/api/v1/notification-policies", "/api/v1/notification-deliveries"] {
        assert_eq!(developer.get(path).await.0, StatusCode::OK, "{path}");
        assert_eq!(customer.get(path).await.0, StatusCode::FORBIDDEN, "{path}");
    }
    assert_eq!(developer.post("/api/v1/notification-policies", policy(&[&channel])).await.0, StatusCode::FORBIDDEN);
    assert_eq!(developer.put(&policy_path, policy(&[&channel])).await.0, StatusCode::NOT_FOUND);
    assert_eq!(developer.call(reqwest::Method::DELETE, &policy_path, None).await.0, StatusCode::NOT_FOUND);
    assert_eq!(customer.get(&policy_path).await.0, StatusCode::NOT_FOUND);

    // A workspace administrator creates channels, but signing uses the
    // vault, which is the platform administrators'.
    let mut ws_admin = user_with_role(&h, &admin, "wa@example.com", "workspace_admin", &ws).await;
    // An administrator who has the role but no second factor yet is told to
    // verify, not that the channel is missing.
    let (s, e) = ws_admin.put(&channel_path, good()).await;
    assert_eq!((s, e["title"].as_str()), (StatusCode::FORBIDDEN, Some("step_up_required")), "{e}");
    assert_eq!(
        ws_admin.post("/api/v1/notification-channels", webhook("second", "https://b.example.com/x")).await.0,
        StatusCode::FORBIDDEN
    );
    step_up(&mut ws_admin).await;
    created(&ws_admin, "/api/v1/notification-channels", webhook("second", "https://b.example.com/x")).await;
    let item = created(&admin, "/api/v1/vault/items", json!({"name": "signing", "password": SIGNING_SECRET})).await;
    let mut signed = webhook("signed", "https://c.example.com/x");
    signed["secret_item"] = item["id"].clone();
    assert_eq!(ws_admin.post("/api/v1/notification-channels", signed.clone()).await.0, StatusCode::FORBIDDEN);
    // An API key is not a person at a keyboard: no channel, and no vault item.
    let (_, key) = admin.post("/api/v1/api-keys", json!({"name": "ci", "expires_days": 1})).await;
    let mut with_key = Client::anonymous(&h.base);
    with_key.bearer = key["secret"].as_str().map(str::to_owned);
    assert_eq!(with_key.post("/api/v1/notification-channels", good()).await.0, StatusCode::FORBIDDEN);
    assert_eq!(with_key.post("/api/v1/notification-channels", signed.clone()).await.0, StatusCode::FORBIDDEN);
    // The platform administrator may; a missing item is refused.
    let signed_channel = created(&admin, "/api/v1/notification-channels", signed.clone()).await;
    assert_eq!(signed_channel["secret_item"], item["id"]);
    let mut ghost = webhook("ghost", "https://d.example.com/x");
    ghost["secret_item"] = json!(forge_core::VaultItemId::new());
    assert_eq!(admin.post("/api/v1/notification-channels", ghost).await.0, StatusCode::NOT_FOUND);
    // Pointing a signed channel somewhere else uses the item anew.
    let signed_path = format!("/api/v1/notification-channels/{}", signed_channel["id"].as_str().unwrap());
    let mut moved = signed.clone();
    moved["url"] = json!("https://elsewhere.example.com/x");
    assert_eq!(ws_admin.put(&signed_path, moved).await.0, StatusCode::FORBIDDEN);
    let mut slack = signed;
    slack["kind"] = json!("slack");
    assert_eq!(admin.post("/api/v1/notification-channels", slack).await.0, StatusCode::UNPROCESSABLE_ENTITY);

    // Policies are checked against the workspace's channels and rules.
    let mut bad = policy(&[&channel]);
    bad["channels"] = json!([forge_core::ChannelId::new()]);
    assert_eq!(operator.post("/api/v1/notification-policies", bad).await.0, StatusCode::UNPROCESSABLE_ENTITY);
    let mut bad = policy(&[&channel]);
    bad["repeat_interval_secs"] = json!(60);
    assert_eq!(operator.post("/api/v1/notification-policies", bad).await.0, StatusCode::UNPROCESSABLE_ENTITY);
    let mut bad = policy(&[&channel]);
    bad["rules"] = json!([AlertRuleId::new()]);
    assert_eq!(operator.post("/api/v1/notification-policies", bad).await.0, StatusCode::UNPROCESSABLE_ENTITY);
    let mut renamed = policy(&[&channel]);
    renamed["name"] = json!("Majors");
    renamed["min_severity"] = json!("major");
    renamed["repeat_interval_secs"] = json!(900);
    let (s, p) = operator.put(&policy_path, renamed).await;
    assert_eq!(
        (s, p["name"].as_str(), p["repeat_interval_secs"].as_i64()),
        (StatusCode::OK, Some("Majors"), Some(900)),
        "{p}"
    );

    // A channel a policy uses stays; without the policy it goes.
    let (s, e) = admin.call(reqwest::Method::DELETE, &channel_path, None).await;
    assert_eq!(s, StatusCode::CONFLICT, "{e}");
    assert!(e.to_string().contains("Majors"), "{e}");
    // A signing secret too short to sign with is refused when saving.
    let weak = created(&admin, "/api/v1/vault/items", json!({"name": "weak", "password": "only-15-bytes!!"})).await;
    let mut weakly = webhook("weak", "https://e.example.com/x");
    weakly["secret_item"] = weak["id"].clone();
    let (s, e) = admin.post("/api/v1/notification-channels", weakly).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{e}");
    assert!(e.to_string().contains("16 bytes") && !e.to_string().contains("only-15"), "{e}");
    assert_eq!(operator.call(reqwest::Method::DELETE, &policy_path, None).await.0, StatusCode::NO_CONTENT);
    assert_eq!(admin.call(reqwest::Method::DELETE, &channel_path, None).await.0, StatusCode::NO_CONTENT);
    assert_eq!(admin.get(&channel_path).await.0, StatusCode::NOT_FOUND);

    // Audit: every change, the refusals, and never a URL or a secret.
    let (_, audit) = admin.get("/api/v1/audit?limit=500").await;
    let text = audit.to_string();
    for action in [
        "notification.channel.create",
        "notification.channel.delete",
        "notification.policy.create",
        "notification.policy.update",
        "notification.policy.delete",
        "notification.manage",
    ] {
        assert!(text.contains(action), "{action} audited");
    }
    let denied =
        audit.as_array().or_else(|| audit["events"].as_array()).or_else(|| audit["items"].as_array()).map(|events| {
            events.iter().filter(|e| e["action"] == "notification.manage" && e["outcome"] == "denied").count()
        });
    assert!(denied.is_some_and(|n| n >= 4), "denied channel changes are audited: {denied:?}");
    // A policy edit records what it matched before and after.
    let update = audit
        .as_array()
        .or_else(|| audit["events"].as_array())
        .or_else(|| audit["items"].as_array())
        .and_then(|events| events.iter().find(|e| e["action"] == "notification.policy.update").cloned())
        .unwrap();
    let details = &update["details"];
    assert_eq!(
        (details["before"]["name"].as_str(), details["after"]["name"].as_str()),
        (Some("Everything"), Some("Majors")),
        "{update}"
    );
    assert!(details["after"]["rules"].is_array() && details["after"].get("scope").is_some(), "{update}");
    assert!(!text.contains(URL_TOKEN) && !text.contains(SIGNING_SECRET), "no URL token or secret in the audit log");
    h.app.stop().await;
}
