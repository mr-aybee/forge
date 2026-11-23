#![allow(clippy::unwrap_used)]

use std::sync::Mutex;

use forge_core::alerts::{AlarmEvent, AlertRule};
use forge_core::{AlarmId, EntityKind, ManualClock, ServerId, UserId};
use forge_store::Store;
use serde_json::json;

use super::target::TargetRules;
use super::*;

mod email;
mod people;
mod silence;

const SEC: i64 = 1000;
const MIN: i64 = 60 * SEC;
const SECRET: &[u8] = b"signing-secret-0123456789";

/// A receiver that answers what the test scripts and keeps every request.
#[derive(Debug, Default)]
struct Receiver {
    answers: Mutex<Vec<SendOutcome>>,
    seen: Mutex<Vec<Outbound>>,
    /// Requests whose path contains this get no answer in time.
    hung: Mutex<Option<String>>,
}

impl Receiver {
    /// Answers for the next requests, in order; after them, `204`.
    fn script(&self, answers: &[SendOutcome]) {
        *self.answers.lock().unwrap() = answers.to_vec();
    }

    fn requests(&self) -> Vec<Outbound> {
        self.seen.lock().unwrap().clone()
    }

    fn count(&self) -> usize {
        self.seen.lock().unwrap().len()
    }

    fn count_to(&self, path: &str) -> usize {
        self.seen.lock().unwrap().iter().filter(|r| r.url.path().ends_with(path)).count()
    }
}

#[async_trait]
impl Transport for Receiver {
    async fn send(&self, request: &Outbound) -> SendOutcome {
        self.seen.lock().unwrap().push(request.clone());
        if self.hung.lock().unwrap().as_deref().is_some_and(|h| request.url.path().contains(h)) {
            return SendOutcome::Failed(FailureClass::Timeout);
        }
        let mut a = self.answers.lock().unwrap();
        if a.is_empty() { SendOutcome::Status(204) } else { a.remove(0) }
    }
}

/// A mail server that answers what the test scripts (then `250`) and keeps
/// every email. An address is anything with one `@` and no white space.
#[derive(Debug, Default)]
struct Mailer {
    answers: Mutex<Vec<SendOutcome>>,
    seen: Mutex<Vec<OutboundMail>>,
}

impl Mailer {
    fn script(&self, answers: &[SendOutcome]) {
        *self.answers.lock().unwrap() = answers.to_vec();
    }

    fn mails(&self) -> Vec<OutboundMail> {
        self.seen.lock().unwrap().clone()
    }
}

#[async_trait]
impl MailTransport for Mailer {
    fn mailbox(&self, raw: &str) -> DomainResult<String> {
        if raw.matches('@').count() == 1 && !raw.contains(char::is_whitespace) {
            Ok(raw.to_owned())
        } else {
            Err(DomainError::Invalid("not an address".into()))
        }
    }

    async fn send(&self, mail: &OutboundMail) -> SendOutcome {
        self.seen.lock().unwrap().push(mail.clone());
        let mut a = self.answers.lock().unwrap();
        if a.is_empty() { SendOutcome::Smtp(250) } else { a.remove(0) }
    }
}

/// "Seals" by prefixing the channel id, so a URL sealed for one channel does
/// not open for another, and serves one vault item.
#[derive(Debug)]
struct Secrets {
    item: VaultItemId,
    workspace: WorkspaceId,
    gone: Mutex<bool>,
    short: Mutex<bool>,
}

#[async_trait]
impl ChannelSecrets for Secrets {
    fn seal_url(&self, channel: ChannelId, url: &str) -> DomainResult<String> {
        Ok(format!("{channel}|{url}"))
    }

    fn open_url(&self, channel: ChannelId, sealed: &str) -> DomainResult<String> {
        sealed
            .strip_prefix(&format!("{channel}|"))
            .map(str::to_owned)
            .ok_or_else(|| DomainError::Internal("sealed for another channel".into()))
    }

    async fn signing_secret(&self, workspace: WorkspaceId, item: VaultItemId) -> DomainResult<Vec<u8>> {
        if *self.gone.lock().unwrap() || item != self.item || workspace != self.workspace {
            return Err(DomainError::NotFound("vault item".into()));
        }
        Ok(if *self.short.lock().unwrap() { b"fifteen-bytes-x".to_vec() } else { SECRET.to_vec() })
    }
}

#[derive(Debug)]
struct Names;

#[async_trait]
impl ResourceNames for Names {
    async fn name(&self, _: &EntityRef) -> Option<String> {
        Some("web-01".into())
    }
}

/// Allows `alert.view` to the users it holds, nobody else.
#[derive(Debug, Default)]
struct Access(Mutex<HashSet<UserId>>);

impl Access {
    fn allow(&self, user: UserId) {
        self.0.lock().unwrap().insert(user);
    }
}

#[async_trait]
impl forge_core::ports::Authorizer for Access {
    async fn authorize(&self, request: &forge_core::AccessRequest) -> DomainResult<()> {
        let Principal::User(u) = &request.principal;
        if request.action == forge_core::Action::AlertView && self.0.lock().unwrap().contains(u) {
            Ok(())
        } else {
            Err(DomainError::Denied("not in the test's allow list".into()))
        }
    }
}

struct Env {
    store: Arc<Store>,
    access: Arc<Access>,
    clock: Arc<ManualClock>,
    receiver: Arc<Receiver>,
    mailer: Arc<Mailer>,
    secrets: Arc<Secrets>,
    n: Notifier,
    ws: WorkspaceId,
    who: Principal,
    _dir: tempfile::TempDir,
}

async fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open_sqlite(&dir.path().join("forge.db")).await.unwrap());
    let clock = Arc::new(ManualClock::new(Timestamp(1_000 * MIN)));
    let receiver = Arc::new(Receiver::default());
    let mailer = Arc::new(Mailer::default());
    let ws = WorkspaceId::new();
    let secrets = Arc::new(Secrets {
        item: VaultItemId::new(),
        workspace: ws,
        gone: Mutex::new(false),
        short: Mutex::new(false),
    });
    let access = Arc::new(Access::default());
    let n = Notifier {
        store: store.clone(),
        alerts: store.clone(),
        transport: receiver.clone(),
        mail: mailer.clone(),
        secrets: secrets.clone(),
        names: Arc::new(Names),
        clock: clock.clone(),
        config: NotifyConfig {
            max_attempts: 3,
            retry: Backoff::new(Duration::from_secs(30), Duration::from_secs(300)),
            public_url: "https://forge.example.com".into(),
            ..NotifyConfig::default()
        },
        pauses: ChannelPauses::default(),
        silences: Silences {
            store: store.clone(),
            alerts: store.clone(),
            resolver: store.clone(),
            clock: clock.clone(),
        },
        people: crate::notify::people::People {
            users: store.clone(),
            bindings: store.clone(),
            authorizer: access.clone(),
            resolver: store.clone(),
        },
    };
    Env { store, access, clock, receiver, mailer, secrets, n, ws, who: Principal::User(UserId::new()), _dir: dir }
}

impl Env {
    fn now(&self) -> Timestamp {
        self.clock.now()
    }

    fn advance(&self, ms: i64) {
        self.clock.advance(Duration::from_millis(u64::try_from(ms).unwrap()));
    }

    async fn webhook(&self, name: &str, signed: bool) -> Channel {
        let input = ChannelInput {
            name: name.into(),
            kind: ChannelKind::Webhook,
            enabled: true,
            url: Some(format!("https://hooks.example.com/in/{name}?token=t0p")),
            secret_item: signed.then_some(self.secrets.item),
            email: None,
        };
        self.n.create_channel(self.ws, input, self.who.clone()).await.unwrap()
    }

    fn policy_input(&self, channels: &[&Channel]) -> PolicyInput {
        PolicyInput {
            name: "Everything".into(),
            enabled: true,
            min_severity: Severity::Info,
            rules: vec![],
            scope: None,
            events: vec![PolicyEvent::Fired, PolicyEvent::Resolved],
            repeat_interval_secs: None,
            channels: channels.iter().map(|c| c.id).collect(),
            email_channel: None,
            email_users: vec![],
            email_roles: vec![],
        }
    }

    async fn policy(&self, channels: &[&Channel]) -> NotificationPolicy {
        self.n.create_policy(self.ws, self.policy_input(channels), self.who.clone()).await.unwrap()
    }

    /// Opens an alarm the way the evaluator does.
    async fn fire(&self, severity: Severity) -> Alarm {
        self.fire_at(severity, EntityRef::new(EntityKind::Server, ServerId::new())).await
    }

    /// Opens an alarm on `resource`.
    async fn fire_at(&self, severity: Severity, resource: EntityRef) -> Alarm {
        let a = Alarm {
            id: AlarmId::new(),
            workspace_id: self.ws,
            rule_id: AlertRuleId::new(),
            rule_revision: 1,
            title: "Node offline".into(),
            severity,
            fingerprint: format!("fp-{resource}"),
            resource,
            server: None,
            subject: "node.agent.status".into(),
            state: AlarmState::Firing,
            fired_value: json!({"type": "state", "value": "offline"}),
            fired_at: self.now(),
            resolved_at: None,
            end_reason: None,
            acknowledged_by: None,
            acknowledged_at: None,
            previous: None,
        };
        self.store.open_alarm(&a, &self.event(&a, "fired")).await.unwrap();
        a
    }

    fn event(&self, a: &Alarm, kind: &str) -> AlarmEvent {
        AlarmEvent { alarm_id: a.id, at: self.now(), kind: kind.into(), actor: None, detail: json!({}) }
    }

    async fn resolve(&self, a: &Alarm) {
        let ev = self.event(a, "resolved");
        assert!(self.store.close_alarm(a.id, AlarmState::Resolved, self.now(), None, &ev).await.unwrap());
    }

    async fn deliveries(&self) -> Vec<Delivery> {
        let mut all = self.store.list_deliveries(&DeliveryFilter::default(), 200).await.unwrap();
        all.reverse();
        all
    }

    async fn only(&self) -> Delivery {
        let mut all = self.deliveries().await;
        assert_eq!(all.len(), 1, "{all:?}");
        all.remove(0)
    }
}

fn header<'a>(r: &'a Outbound, name: &str) -> Option<&'a str> {
    r.headers.iter().find(|(k, _)| *k == name).map(|(_, v)| v.as_str())
}

#[tokio::test]
async fn a_fired_alarm_sends_one_signed_message_per_channel() {
    let e = env().await;
    let signed = e.webhook("signed", true).await;
    let plain = e.webhook("plain", false).await;
    e.policy(&[&signed, &plain]).await;
    let a = e.fire(Severity::Critical).await;

    assert_eq!(
        e.n.plan().await.unwrap(),
        PlanReport { transitions: 1, deliveries: 2, reminders: 0, ..PlanReport::default() }
    );
    assert_eq!(e.n.dispatch().await.unwrap().sent, 2);
    let sent = e.receiver.requests();
    assert_eq!(sent.len(), 2);

    let r = sent.iter().find(|r| r.url.path() == "/in/signed").unwrap();
    assert_eq!(r.url.as_str(), "https://hooks.example.com/in/signed?token=t0p", "the whole stored URL");
    assert_eq!(header(r, "content-type"), Some("application/json"));
    assert_eq!(header(r, payload::HEADER_EVENT), Some("fired"));
    let body: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
    assert_eq!(body["schema_version"], 1);
    assert_eq!(body["event"], json!({"type": "fired", "is_resolved": false, "occurrence": 0, "at": e.now().0}));
    assert_eq!(body["alarm"]["id"], json!(a.id));
    assert_eq!(body["alarm"]["severity"], "critical");
    assert_eq!(body["resource"]["name"], "web-01");
    assert_eq!(body["links"]["alarm"], format!("https://forge.example.com/alerts?alarm={}", a.id));
    assert_eq!(header(r, payload::HEADER_DELIVERY), body["delivery_id"].as_str());
    let at: i64 = header(r, payload::HEADER_TIMESTAMP).unwrap().parse().unwrap();
    assert_eq!(at, e.now().0 / 1000);
    assert!(payload::verify(SECRET, at, &r.body, header(r, payload::HEADER_SIGNATURE).unwrap()));
    // Nothing of the secret or the URL's token travels in the body.
    let text = String::from_utf8_lossy(&r.body);
    assert!(!text.contains("signing-secret") && !text.contains("t0p"), "{text}");

    let r = sent.iter().find(|r| r.url.path() == "/in/plain").unwrap();
    assert!(header(r, payload::HEADER_SIGNATURE).is_none() && header(r, payload::HEADER_TIMESTAMP).is_none());

    let all = e.deliveries().await;
    assert!(all.iter().all(|d| d.status == DeliveryStatus::Sent && d.attempts == 1), "{all:?}");
    let (_, attempts) = e.n.delivery(all[0].id).await.unwrap();
    assert_eq!(attempts.len(), 1);
    assert!(attempts[0].ok && attempts[0].http_status == Some(204) && attempts[0].error.is_none());
}

#[tokio::test]
async fn planning_and_dispatching_again_never_sends_twice() {
    let e = env().await;
    let c = e.webhook("ops", false).await;
    let p = e.policy(&[&c]).await;
    let a = e.fire(Severity::Major).await;
    let transition = e.store.unplanned_transitions(10).await.unwrap().remove(0);
    e.n.plan().await.unwrap();
    e.n.dispatch().await.unwrap();
    assert_eq!(e.receiver.count(), 1);

    // The same transition replayed, by the planner and behind its back.
    for _ in 0..3 {
        assert_eq!(e.n.plan().await.unwrap(), PlanReport::default());
        assert_eq!(e.n.dispatch().await.unwrap(), DispatchReport::default());
    }
    let copy = Notifier::new_delivery(&a, DeliveryEvent::Fired, 0, &p, &c, e.now());
    assert_eq!(e.store.plan_transition(&transition, std::slice::from_ref(&copy), e.now()).await.unwrap(), 0);
    assert_eq!(e.store.add_deliveries(std::slice::from_ref(&copy)).await.unwrap(), 0, "the key is taken");
    e.advance(10 * MIN);
    e.n.plan().await.unwrap();
    e.n.dispatch().await.unwrap();
    assert_eq!(e.receiver.count(), 1, "one message for one firing");
    assert_eq!(e.only().await.status, DeliveryStatus::Sent);
}

#[tokio::test]
async fn a_failing_receiver_is_retried_with_backoff_then_the_delivery_fails_and_the_alarm_is_untouched() {
    let e = env().await;
    let c = e.webhook("ops", false).await;
    e.policy(&[&c]).await;
    let a = e.fire(Severity::Major).await;
    let before = e.store.get_alarm(a.id).await.unwrap();
    e.receiver.script(&[
        SendOutcome::Status(503),
        SendOutcome::Failed(FailureClass::Timeout),
        SendOutcome::Status(500),
        SendOutcome::Status(204),
    ]);
    e.n.plan().await.unwrap();

    assert_eq!(e.n.dispatch().await.unwrap().retried, 1);
    let d = e.only().await;
    assert_eq!((d.status, d.attempts, d.last_http_status), (DeliveryStatus::Pending, 1, Some(503)));
    let wait = d.next_attempt_at.0 - e.now().0;
    assert!((15 * SEC..=30 * SEC).contains(&wait), "first retry after 15 to 30 s, got {wait} ms");
    // Not due yet: nothing is sent.
    e.advance(wait - 1);
    assert_eq!(e.n.dispatch().await.unwrap(), DispatchReport::default());
    assert_eq!(e.receiver.count(), 1);

    e.advance(1);
    assert_eq!(e.n.dispatch().await.unwrap().retried, 1);
    let d = e.only().await;
    assert_eq!((d.attempts, d.last_error, d.last_http_status), (2, Some(FailureClass::Timeout), None));
    let wait = d.next_attempt_at.0 - e.now().0;
    assert!((30 * SEC..=60 * SEC).contains(&wait), "second retry after 30 to 60 s, got {wait} ms");

    // The third attempt is the last one allowed.
    e.advance(wait);
    assert_eq!(e.n.dispatch().await.unwrap().failed, 1);
    let d = e.only().await;
    assert_eq!((d.status, d.attempts, d.last_http_status), (DeliveryStatus::Failed, 3, Some(500)));
    assert!(d.finished_at.is_some());
    e.advance(60 * MIN);
    assert_eq!(e.n.dispatch().await.unwrap(), DispatchReport::default());
    assert_eq!(e.receiver.count(), 3, "the fourth answer is never asked for");
    let (_, attempts) = e.n.delivery(d.id).await.unwrap();
    assert_eq!(attempts.iter().map(|a| a.number).collect::<Vec<_>>(), [1, 2, 3]);
    assert!(attempts.iter().all(|a| !a.ok));

    // The alarm and its timeline are exactly what the evaluator wrote.
    assert_eq!(e.store.get_alarm(a.id).await.unwrap(), before);
    let kinds: Vec<String> = e.store.alarm_events(a.id).await.unwrap().into_iter().map(|x| x.kind).collect();
    assert_eq!(kinds, ["fired"]);
}

#[tokio::test]
async fn refusals_that_a_retry_cannot_fix_fail_at_once() {
    for (answer, class, status) in [
        (SendOutcome::Status(404), FailureClass::HttpStatus, Some(404)),
        (SendOutcome::Status(401), FailureClass::HttpStatus, Some(401)),
        (SendOutcome::Status(302), FailureClass::HttpStatus, Some(302)),
        (SendOutcome::Failed(FailureClass::BlockedTarget), FailureClass::BlockedTarget, None),
    ] {
        let e = env().await;
        let c = e.webhook("ops", false).await;
        e.policy(&[&c]).await;
        e.fire(Severity::Major).await;
        e.receiver.script(&[answer]);
        e.n.plan().await.unwrap();
        assert_eq!(e.n.dispatch().await.unwrap().failed, 1, "{answer:?}");
        let d = e.only().await;
        assert_eq!(
            (d.status, d.attempts, d.last_error, d.last_http_status),
            (DeliveryStatus::Failed, 1, Some(class), status)
        );
    }
    for s in [408, 425, 429, 500, 502, 503, 599] {
        assert!(retryable_status(s), "{s}");
    }
    for s in [300, 301, 400, 401, 403, 404, 410, 422] {
        assert!(!retryable_status(s), "{s}");
    }
}

#[tokio::test]
async fn a_missing_secret_or_a_target_now_forbidden_fails_without_sending() {
    let e = env().await;
    let c = e.webhook("ops", true).await;
    e.policy(&[&c]).await;
    e.fire(Severity::Major).await;
    *e.secrets.gone.lock().unwrap() = true;
    e.n.plan().await.unwrap();
    assert_eq!(e.n.dispatch().await.unwrap().failed, 1);
    assert_eq!(e.only().await.last_error, Some(FailureClass::Secret));
    assert_eq!(e.receiver.count(), 0, "an unsigned copy is never sent instead");

    // A channel saved while private targets were allowed; they no longer are.
    let mut e = env().await;
    e.n.config.rules = TargetRules { allow_http: true, allow_private: true, allow_loopback: false };
    let input = ChannelInput {
        name: "on-prem".into(),
        kind: ChannelKind::Webhook,
        enabled: true,
        url: Some("http://10.0.0.5:9000/hook".into()),
        secret_item: None,
        email: None,
    };
    let c = e.n.create_channel(e.ws, input, e.who.clone()).await.unwrap();
    assert_eq!(c.target, "http://10.0.0.5:9000");
    e.policy(&[&c]).await;
    e.fire(Severity::Major).await;
    e.n.plan().await.unwrap();
    e.n.config.rules = TargetRules::default();
    assert_eq!(e.n.dispatch().await.unwrap().failed, 1);
    assert_eq!(e.only().await.last_error, Some(FailureClass::BlockedTarget));
    assert_eq!(e.receiver.count(), 0);
}

#[tokio::test]
async fn recovery_is_sent_where_the_firing_message_arrived() {
    let e = env().await;
    let c = e.webhook("ops", false).await;
    e.policy(&[&c]).await;
    let a = e.fire(Severity::Major).await;
    e.n.plan().await.unwrap();
    e.n.dispatch().await.unwrap();
    e.advance(5 * MIN);
    e.resolve(&a).await;
    assert_eq!(e.n.plan().await.unwrap().deliveries, 1);
    assert_eq!(e.n.dispatch().await.unwrap().sent, 1);
    let sent = e.receiver.requests();
    assert_eq!(sent.len(), 2);
    let body: serde_json::Value = serde_json::from_slice(&sent[1].body).unwrap();
    assert_eq!(body["event"]["type"], "resolved");
    assert_eq!(body["event"]["is_resolved"], true);
    assert_eq!(body["alarm"]["state"], "resolved");
    assert_eq!(body["alarm"]["resolved_at"], e.now().0);
    assert_ne!(body["delivery_id"], serde_json::from_slice::<serde_json::Value>(&sent[0].body).unwrap()["delivery_id"]);
}

#[tokio::test]
async fn an_alarm_that_recovers_before_its_message_left_sends_neither() {
    let e = env().await;
    let c = e.webhook("ops", false).await;
    e.policy(&[&c]).await;
    let a = e.fire(Severity::Major).await;
    e.receiver.script(&[SendOutcome::Status(503)]);
    e.n.plan().await.unwrap();
    assert_eq!(e.n.dispatch().await.unwrap().retried, 1);
    // Recovered while the firing message waits for its retry.
    e.resolve(&a).await;
    e.n.plan().await.unwrap();
    // The recovery waits for the firing message's fate…
    assert_eq!(e.n.dispatch().await.unwrap().postponed, 1);
    e.advance(MIN);
    // …the stale firing message is dropped, and with it the recovery: in
    // the same pass when the recovery is looked at second, in the next one
    // when it is looked at first (deliveries are worked on concurrently).
    let first = e.n.dispatch().await.unwrap();
    e.advance(MIN);
    let second = e.n.dispatch().await.unwrap();
    assert_eq!(first.cancelled + second.cancelled, 2, "{first:?} {second:?}");
    assert_eq!(first.sent + second.sent, 0);
    assert_eq!(e.receiver.count(), 1, "only the first, refused attempt");
    let all = e.deliveries().await;
    assert!(all.iter().all(|d| d.status == DeliveryStatus::Cancelled), "{all:?}");
    assert_eq!(all[0].detail.as_deref(), Some("the alarm is no longer firing"));
    assert_eq!(all[1].detail.as_deref(), Some("the firing notification was never delivered"));
}

#[tokio::test]
async fn an_ended_alarm_says_ended_not_recovered() {
    let e = env().await;
    let c = e.webhook("ops", false).await;
    e.policy(&[&c]).await;
    let a = e.fire(Severity::Major).await;
    e.n.plan().await.unwrap();
    e.n.dispatch().await.unwrap();
    let ev = e.event(&a, "ended");
    e.store.close_alarm(a.id, AlarmState::Ended, e.now(), Some("the rule was deleted"), &ev).await.unwrap();
    e.n.plan().await.unwrap();
    e.n.dispatch().await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&e.receiver.requests()[1].body).unwrap();
    assert_eq!(body["event"], json!({"type": "ended", "is_resolved": false, "occurrence": 0, "at": e.now().0}));
    assert_eq!(body["alarm"]["end_reason"], "the rule was deleted");
}

#[tokio::test]
async fn reminders_repeat_while_firing_and_stop_on_acknowledgement() {
    let e = env().await;
    let c = e.webhook("ops", false).await;
    let mut input = e.policy_input(&[&c]);
    input.repeat_interval_secs = Some(600);
    e.n.create_policy(e.ws, input, e.who.clone()).await.unwrap();
    let a = e.fire(Severity::Major).await;
    e.n.plan().await.unwrap();
    e.n.dispatch().await.unwrap();

    e.advance(9 * MIN);
    assert_eq!(e.n.plan().await.unwrap().reminders, 0, "not ten minutes yet");
    e.advance(MIN);
    assert_eq!(e.n.plan().await.unwrap().reminders, 1);
    assert_eq!(e.n.plan().await.unwrap().reminders, 0, "the same reminder is not planned twice");
    assert_eq!(e.n.dispatch().await.unwrap().sent, 1);
    let body: serde_json::Value = serde_json::from_slice(&e.receiver.requests()[1].body).unwrap();
    assert_eq!((body["event"]["type"].as_str(), body["event"]["occurrence"].as_i64()), (Some("reminder"), Some(1)));

    // The planner did not run for a long time: one reminder, not five.
    e.advance(50 * MIN);
    assert_eq!(e.n.plan().await.unwrap().reminders, 1);
    assert_eq!(e.n.dispatch().await.unwrap().sent, 1);
    assert_eq!(e.receiver.count(), 3);

    // Acknowledged: no new reminders, and one already planned is dropped.
    e.advance(10 * MIN);
    assert_eq!(e.n.plan().await.unwrap().reminders, 1);
    e.store.acknowledge(a.id, Some(&e.who), e.now(), &e.event(&a, "acknowledged")).await.unwrap();
    assert_eq!(e.n.dispatch().await.unwrap().cancelled, 1);
    e.advance(30 * MIN);
    assert_eq!(e.n.plan().await.unwrap().reminders, 0);
    assert_eq!(e.receiver.count(), 3);

    // Resolved: the recovery goes out, reminders are over.
    e.resolve(&a).await;
    e.n.plan().await.unwrap();
    assert_eq!(e.n.dispatch().await.unwrap().sent, 1);
    e.advance(60 * MIN);
    assert_eq!(e.n.plan().await.unwrap(), PlanReport::default());
}

#[tokio::test]
async fn policies_choose_by_severity_rule_scope_and_event() {
    let e = env().await;
    let a = e.fire(Severity::Major).await;
    let c = e.webhook("ops", false).await;
    let base = e.policy(&[&c]).await;
    let with = |f: &dyn Fn(&mut NotificationPolicy)| {
        let mut p = base.clone();
        f(&mut p);
        policy_matches(&p, &a)
    };
    assert!(policy_matches(&base, &a));
    assert!(with(&|p| p.min_severity = Severity::Major));
    assert!(!with(&|p| p.min_severity = Severity::Critical));
    assert!(!with(&|p| p.enabled = false));
    assert!(!with(&|p| p.workspace_id = WorkspaceId::new()), "never another workspace's alarms");
    assert!(with(&|p| p.rules = vec![AlertRuleId::new(), a.rule_id]));
    assert!(!with(&|p| p.rules = vec![AlertRuleId::new()]));
    assert!(with(&|p| p.scope = Some(RuleScope::Servers { ids: vec![] })));
    assert!(with(&|p| p.scope = Some(RuleScope::Servers { ids: vec![a.resource.id.parse().unwrap()] })));
    assert!(!with(&|p| p.scope = Some(RuleScope::Servers { ids: vec![ServerId::new()] })));
    assert!(!with(&|p| p.scope = Some(RuleScope::Services { ids: vec![] })));

    // A policy for resolved only says nothing when the alarm fires.
    let mut only_resolved = e.policy_input(&[&c]);
    only_resolved.events = vec![PolicyEvent::Resolved];
    e.n.update_policy(base.id, only_resolved).await.unwrap();
    assert_eq!(
        e.n.plan().await.unwrap(),
        PlanReport { transitions: 1, deliveries: 0, reminders: 0, ..PlanReport::default() }
    );
    // …and speaks when it resolves, with no firing message to wait for.
    e.resolve(&a).await;
    assert_eq!(e.n.plan().await.unwrap().deliveries, 1);
    assert_eq!(e.n.dispatch().await.unwrap().sent, 1);
}

#[tokio::test]
async fn a_channel_in_two_policies_gets_one_message_and_a_disabled_one_none() {
    let e = env().await;
    let c = e.webhook("ops", false).await;
    let off = e.webhook("off", false).await;
    e.policy(&[&c, &off]).await;
    let mut second = e.policy_input(&[&c]);
    second.name = "Also everything".into();
    e.n.create_policy(e.ws, second, e.who.clone()).await.unwrap();
    let input = ChannelInput {
        name: "off".into(),
        kind: ChannelKind::Webhook,
        enabled: false,
        url: None,
        secret_item: None,
        email: None,
    };
    let updated = e.n.update_channel(off.id, input).await.unwrap();
    assert_eq!(updated.sealed_url, off.sealed_url, "no URL in the update keeps the stored one");
    e.fire(Severity::Major).await;
    assert_eq!(e.n.plan().await.unwrap().deliveries, 1);
    assert_eq!(e.only().await.policy_name, "Also everything", "the first policy by name");

    // Turned off after planning: the delivery is cancelled, not sent.
    let input = ChannelInput {
        name: "ops".into(),
        kind: ChannelKind::Webhook,
        enabled: false,
        url: None,
        secret_item: None,
        email: None,
    };
    e.n.update_channel(c.id, input).await.unwrap();
    assert_eq!(e.n.dispatch().await.unwrap().cancelled, 1);
    assert_eq!(e.only().await.detail.as_deref(), Some("the channel is turned off"));
    assert_eq!(e.receiver.count(), 0);
}

#[tokio::test]
async fn an_attempt_that_never_reports_back_still_counts() {
    let e = env().await;
    let c = e.webhook("ops", false).await;
    e.policy(&[&c]).await;
    e.fire(Severity::Major).await;
    e.n.plan().await.unwrap();
    let d = e.only().await;
    // Three workers take the delivery and die before recording anything.
    for _ in 0..3 {
        assert!(e.store.claim_delivery(d.id, e.now(), Timestamp(e.now().0 + 60 * SEC)).await.unwrap());
        e.advance(61 * SEC);
    }
    // All attempts are used up: the delivery fails without another send.
    assert_eq!(e.n.dispatch().await.unwrap().failed, 1);
    let d = e.only().await;
    assert_eq!((d.status, d.attempts, d.last_error), (DeliveryStatus::Failed, 4, Some(FailureClass::Timeout)));
    assert_eq!(e.receiver.count(), 0);
}

#[tokio::test]
async fn channels_are_validated_and_keep_their_url_sealed() {
    let e = env().await;
    let input = |url: &str| ChannelInput {
        name: "ops".into(),
        kind: ChannelKind::Webhook,
        enabled: true,
        url: Some(url.into()),
        secret_item: None,
        email: None,
    };
    for bad in [
        "http://hooks.example.com/x",
        "https://169.254.169.254/latest/meta-data/",
        "https://127.0.0.1/x",
        "https://localhost/x",
        "https://10.0.0.5/x",
        "https://user:pw@hooks.example.com/x",
        "ftp://hooks.example.com/x",
        "not a url",
    ] {
        let err = e.n.create_channel(e.ws, input(bad), e.who.clone()).await.unwrap_err();
        assert!(matches!(err, DomainError::Invalid(_)), "{bad}: {err}");
        assert!(!err.to_string().contains("pw@"), "the refusal does not echo the URL");
    }
    let mut no_url = input("x");
    no_url.url = None;
    assert!(matches!(e.n.create_channel(e.ws, no_url, e.who.clone()).await, Err(DomainError::Invalid(_))));
    let mut no_name = input("https://hooks.example.com/x");
    no_name.name = "  ".into();
    assert!(matches!(e.n.create_channel(e.ws, no_name, e.who.clone()).await, Err(DomainError::Invalid(_))));
    let slack = ChannelInput {
        name: "chat".into(),
        kind: ChannelKind::Slack,
        enabled: true,
        url: Some("https://hooks.slack.example/services/T/B/x".into()),
        secret_item: Some(e.secrets.item),
        email: None,
    };
    assert!(matches!(e.n.create_channel(e.ws, slack, e.who.clone()).await, Err(DomainError::Invalid(_))));

    let c = e.webhook("ops", false).await;
    assert_eq!(c.target, "https://hooks.example.com");
    let json = serde_json::to_string(&c).unwrap();
    assert!(!json.contains("t0p") && !json.contains("/in/ops"), "{json}");
    // A channel a policy uses cannot be deleted from under it.
    let p = e.policy(&[&c]).await;
    assert!(matches!(e.n.delete_channel(c.id).await, Err(DomainError::Conflict(_))));
    e.n.delete_policy(p.id).await.unwrap();
    e.n.delete_channel(c.id).await.unwrap();
    assert!(matches!(e.n.channel(c.id).await, Err(DomainError::NotFound(_))));
}

#[tokio::test]
async fn policies_are_validated_against_their_workspace() {
    let e = env().await;
    let c = e.webhook("ops", false).await;
    let ok = e.policy_input(&[&c]);
    let check = |f: &dyn Fn(&mut PolicyInput)| {
        let mut p = ok.clone();
        f(&mut p);
        p
    };
    for bad in [
        check(&|p| p.name = String::new()),
        check(&|p| p.events.clear()),
        check(&|p| p.channels.clear()),
        check(&|p| p.channels = vec![ChannelId::new()]),
        check(&|p| p.channels = (0..11).map(|_| ChannelId::new()).collect()),
        check(&|p| p.rules = vec![AlertRuleId::new()]),
        check(&|p| p.repeat_interval_secs = Some(60)),
        check(&|p| p.repeat_interval_secs = Some(8 * 24 * 3600)),
        check(&|p| {
            p.events = vec![PolicyEvent::Resolved];
            p.repeat_interval_secs = Some(600);
        }),
        check(&|p| p.scope = Some(RuleScope::Servers { ids: (0..501).map(|_| ServerId::new()).collect() })),
    ] {
        let err = e.n.create_policy(e.ws, bad.clone(), e.who.clone()).await.unwrap_err();
        assert!(matches!(err, DomainError::Invalid(_)), "{bad:?}: {err}");
    }
    // A channel of another workspace is not a channel of this one.
    assert!(matches!(
        e.n.create_policy(WorkspaceId::new(), ok.clone(), e.who.clone()).await,
        Err(DomainError::Invalid(_))
    ));
    // A rule of this workspace is accepted; duplicates collapse.
    let rule = AlertRule {
        id: AlertRuleId::new(),
        workspace_id: e.ws,
        name: "r".into(),
        description: String::new(),
        enabled: true,
        severity: Severity::Major,
        scope: RuleScope::Servers { ids: vec![] },
        subject: "node.agent.status".into(),
        spec: json!({}),
        revision: 1,
        created_by: e.who.clone(),
        created_at: e.now(),
        updated_at: e.now(),
    };
    e.store.create_rule(&rule).await.unwrap();
    let p = check(&|p| {
        p.rules = vec![rule.id, rule.id];
        p.channels = vec![c.id, c.id];
        p.events = vec![PolicyEvent::Fired, PolicyEvent::Fired];
    });
    let made = e.n.create_policy(e.ws, p, e.who.clone()).await.unwrap();
    assert_eq!((made.rules.len(), made.channels.len(), made.events.len()), (1, 1, 1));
}

#[tokio::test]
async fn a_test_message_is_one_attempt_with_its_result() {
    let e = env().await;
    let c = e.webhook("ops", true).await;
    let (d, attempts) = e.n.test_channel(c.id).await.unwrap();
    assert_eq!((d.event, d.status, d.alarm_id, attempts.len()), (DeliveryEvent::Test, DeliveryStatus::Sent, None, 1));
    let r = &e.receiver.requests()[0];
    assert_eq!(header(r, payload::HEADER_EVENT), Some("test"));
    let at: i64 = header(r, payload::HEADER_TIMESTAMP).unwrap().parse().unwrap();
    assert!(payload::verify(SECRET, at, &r.body, header(r, payload::HEADER_SIGNATURE).unwrap()));
    let body: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
    assert!(body.get("alarm").is_none());

    // A second test right away is refused, and nothing is sent.
    assert!(matches!(e.n.test_channel(c.id).await, Err(TestError::Cooldown(10))));
    e.advance(9 * SEC);
    assert!(matches!(e.n.test_channel(c.id).await, Err(TestError::Cooldown(1))));
    assert_eq!(e.receiver.count(), 1);
    e.advance(SEC);

    // A refused test is failed at once and never retried in the background.
    e.receiver.script(&[SendOutcome::Status(503)]);
    let (d, attempts) = e.n.test_channel(c.id).await.unwrap();
    assert_eq!((d.status, d.last_http_status, attempts.len()), (DeliveryStatus::Failed, Some(503), 1));
    e.advance(60 * MIN);
    assert_eq!(e.n.dispatch().await.unwrap(), DispatchReport::default());
    assert_eq!(e.receiver.count(), 2);

    // The background loop never takes a test from the request that sends
    // it, and drops one whose request died before sending.
    let mut orphan = d.clone();
    orphan.id = DeliveryId::new();
    orphan.dedupe_key = format!("test|{}", orphan.id);
    orphan.created_at = e.now();
    orphan.next_attempt_at = Timestamp(e.now().0 + 60 * SEC);
    e.store.add_deliveries(std::slice::from_ref(&orphan)).await.unwrap();
    assert_eq!(e.n.dispatch().await.unwrap(), DispatchReport::default(), "not due for the loop");
    e.advance(61 * SEC);
    assert_eq!(e.n.dispatch().await.unwrap().cancelled, 1);
    let (gone, _) = e.n.delivery(orphan.id).await.unwrap();
    assert_eq!(gone.detail.as_deref(), Some("the test was interrupted"));
    assert_eq!(e.receiver.count(), 2);
}

#[tokio::test]
async fn slack_channels_send_text_only() {
    let e = env().await;
    let input = ChannelInput {
        name: "chat".into(),
        kind: ChannelKind::Slack,
        enabled: true,
        url: Some("https://hooks.slack.example/services/T/B/x".into()),
        secret_item: None,
        email: None,
    };
    let c = e.n.create_channel(e.ws, input, e.who.clone()).await.unwrap();
    e.policy(&[&c]).await;
    let a = e.fire(Severity::Critical).await;
    e.n.plan().await.unwrap();
    e.n.dispatch().await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&e.receiver.requests()[0].body).unwrap();
    assert_eq!(body.as_object().unwrap().len(), 1, "only `text`: {body}");
    let text = body["text"].as_str().unwrap();
    assert_eq!(
        text,
        format!(
            "ALERT [critical] Node offline\nResource: server web-01\nObserved: node.agent.status = offline\nhttps://forge.example.com/alerts?alarm={}",
            a.id
        )
    );
}

#[tokio::test]
async fn old_final_deliveries_are_pruned() {
    let e = env().await;
    let c = e.webhook("ops", false).await;
    e.policy(&[&c]).await;
    e.fire(Severity::Major).await;
    e.n.plan().await.unwrap();
    e.n.dispatch().await.unwrap();
    e.advance(29 * 24 * 60 * MIN);
    assert_eq!(e.n.prune().await.unwrap(), 0);
    e.advance(2 * 24 * 60 * MIN);
    assert_eq!(e.n.prune().await.unwrap(), 1);
    assert!(e.deliveries().await.is_empty());
}

#[test]
fn retry_waits_grow_and_stay_within_the_backoff() {
    let retry = Backoff::new(Duration::from_secs(30), Duration::from_secs(900));
    for _ in 0..50 {
        let id = DeliveryId::new();
        let mut last = Duration::ZERO;
        for attempts in 1..=8 {
            let d = retry_delay(retry, attempts, id);
            let ceiling = retry.ceiling(attempts - 1);
            assert!(d >= ceiling / 2 && d <= ceiling, "{attempts}: {d:?} of {ceiling:?}");
            assert!(d >= last, "never shorter than the wait before");
            last = d;
        }
        assert!(last <= Duration::from_secs(900));
        assert_eq!(retry_delay(retry, 3, id), retry_delay(retry, 3, id), "predictable for one delivery");
    }
}

/// The store's alarms, except the ones a test marks unreadable.
#[derive(Debug)]
struct Unreadable {
    inner: Arc<Store>,
    bad: Mutex<HashSet<AlarmId>>,
}

#[async_trait]
impl AlertRepository for Unreadable {
    async fn create_rule(&self, rule: &AlertRule) -> DomainResult<()> {
        self.inner.create_rule(rule).await
    }
    async fn update_rule(&self, rule: &AlertRule) -> DomainResult<()> {
        self.inner.update_rule(rule).await
    }
    async fn delete_rule(&self, id: AlertRuleId) -> DomainResult<()> {
        self.inner.delete_rule(id).await
    }
    async fn get_rule(&self, id: AlertRuleId) -> DomainResult<AlertRule> {
        self.inner.get_rule(id).await
    }
    async fn list_rules(&self, workspace: Option<WorkspaceId>) -> DomainResult<Vec<AlertRule>> {
        self.inner.list_rules(workspace).await
    }
    async fn open_alarm(&self, alarm: &Alarm, event: &AlarmEvent) -> DomainResult<()> {
        self.inner.open_alarm(alarm, event).await
    }
    async fn close_alarm(
        &self,
        id: AlarmId,
        state: AlarmState,
        at: Timestamp,
        reason: Option<&str>,
        event: &AlarmEvent,
    ) -> DomainResult<bool> {
        self.inner.close_alarm(id, state, at, reason, event).await
    }
    async fn acknowledge(
        &self,
        id: AlarmId,
        by: Option<&Principal>,
        at: Timestamp,
        event: &AlarmEvent,
    ) -> DomainResult<()> {
        self.inner.acknowledge(id, by, at, event).await
    }
    async fn add_alarm_event(&self, event: &AlarmEvent) -> DomainResult<()> {
        self.inner.add_alarm_event(event).await
    }
    async fn get_alarm(&self, id: AlarmId) -> DomainResult<Alarm> {
        if self.bad.lock().unwrap().contains(&id) {
            return Err(DomainError::Internal("column severity: unknown variant".into()));
        }
        self.inner.get_alarm(id).await
    }
    async fn list_alarms(&self, filter: &AlarmFilter, limit: u32) -> DomainResult<Vec<Alarm>> {
        self.inner.list_alarms(filter, limit).await
    }
    async fn active_alarm(&self, fingerprint: &str) -> DomainResult<Option<Alarm>> {
        self.inner.active_alarm(fingerprint).await
    }
    async fn last_alarm(&self, fingerprint: &str) -> DomainResult<Option<Alarm>> {
        self.inner.last_alarm(fingerprint).await
    }
    async fn alarm_events(&self, id: AlarmId) -> DomainResult<Vec<AlarmEvent>> {
        self.inner.alarm_events(id).await
    }
}

#[tokio::test]
async fn an_unreadable_alarm_is_skipped_then_given_up_and_never_blocks_the_others() {
    let mut e = env().await;
    let alerts = Arc::new(Unreadable { inner: e.store.clone(), bad: Mutex::default() });
    e.n.alerts = alerts.clone();
    let c = e.webhook("ops", false).await;
    let mut input = e.policy_input(&[&c]);
    input.repeat_interval_secs = Some(600);
    e.n.create_policy(e.ws, input, e.who.clone()).await.unwrap();

    // An older alarm whose reminder falls due in the pass that hits the bad one.
    let old = e.fire(Severity::Major).await;
    e.n.plan().await.unwrap();
    e.n.dispatch().await.unwrap();
    e.advance(10 * MIN);
    let bad = e.fire(Severity::Major).await;
    alerts.bad.lock().unwrap().insert(bad.id);
    let good = e.fire(Severity::Major).await;

    // The bad transition is first in the queue; the one behind it is
    // planned all the same, and so is the reminder.
    let r = e.n.plan().await.unwrap();
    assert_eq!(
        r,
        PlanReport { transitions: 1, deliveries: 1, reminders: 1, after_silence: 0, failed: 1, abandoned: 0 }
    );
    assert_eq!(e.n.dispatch().await.unwrap().sent, 2);
    let sent_for = |alarm: AlarmId, all: &[Delivery]| all.iter().filter(|d| d.alarm_id == Some(alarm)).count();
    let all = e.deliveries().await;
    assert_eq!((sent_for(good.id, &all), sent_for(bad.id, &all), sent_for(old.id, &all)), (1, 0, 2));

    // It is tried again each pass, and given up at the limit.
    for pass in 2..PLAN_FAILURE_LIMIT {
        let r = e.n.plan().await.unwrap();
        assert_eq!((r.failed, r.abandoned), (1, 0), "pass {pass}");
    }
    let r = e.n.plan().await.unwrap();
    assert_eq!((r.failed, r.abandoned), (1, 1));
    assert!(e.store.unplanned_transitions(10).await.unwrap().is_empty(), "out of the queue");
    let r = e.n.plan().await.unwrap();
    assert_eq!((r.transitions, r.failed, r.abandoned), (0, 0, 0));

    // Readable again later: the given-up transition stays given up (no late
    // "fired" for an alarm that may long be over).
    alerts.bad.lock().unwrap().clear();
    e.n.plan().await.unwrap();
    assert_eq!(sent_for(bad.id, &e.deliveries().await), 0);
}

#[tokio::test]
async fn a_hung_receiver_costs_one_request_a_pass_and_never_delays_another_channel() {
    let e = env().await;
    let hung = e.webhook("hung", false).await;
    let healthy = e.webhook("healthy", false).await;
    *e.receiver.hung.lock().unwrap() = Some("/in/hung".into());

    // Sixty alarms wait on the hung channel, more than a batch holds.
    let p = e.policy(&[&hung]).await;
    for _ in 0..60 {
        e.fire(Severity::Major).await;
    }
    assert_eq!(e.n.plan().await.unwrap().deliveries, 60);
    // One alarm, fired last, goes to the healthy channel.
    let mut only_healthy = e.policy_input(&[&healthy]);
    only_healthy.name = p.name.clone();
    e.n.update_policy(p.id, only_healthy).await.unwrap();
    e.advance(SEC);
    e.fire(Severity::Major).await;
    assert_eq!(e.n.plan().await.unwrap().deliveries, 1);

    // First pass: the healthy channel's message goes out, behind sixty
    // older ones; the hung channel is asked once and its lane stops.
    let r = e.n.dispatch().await.unwrap();
    assert_eq!((r.sent, r.retried), (1, 1), "{r:?}");
    assert_eq!((e.receiver.count_to("/in/healthy"), e.receiver.count_to("/in/hung")), (1, 1));

    // Second timeout in a row: the channel is left alone for a minute.
    assert_eq!(e.n.dispatch().await.unwrap().retried, 1);
    assert_eq!(e.receiver.count_to("/in/hung"), 2);
    for _ in 0..3 {
        assert_eq!(e.n.dispatch().await.unwrap(), DispatchReport::default());
        e.advance(15 * SEC);
    }
    assert_eq!(e.receiver.count_to("/in/hung"), 2, "paused");
    // Meanwhile the healthy channel is served at once.
    e.fire(Severity::Major).await;
    e.n.plan().await.unwrap();
    assert_eq!(e.n.dispatch().await.unwrap().sent, 1);
    assert_eq!(e.receiver.count_to("/in/healthy"), 2);

    // After the pause it is tried again; an answer clears the count.
    e.advance(20 * SEC);
    *e.receiver.hung.lock().unwrap() = None;
    let r = e.n.dispatch().await.unwrap();
    assert_eq!(r.sent, 2, "two per channel a pass: {r:?}");
    assert_eq!(e.n.dispatch().await.unwrap().sent, 2);
}

#[tokio::test]
async fn a_signing_secret_shorter_than_sixteen_bytes_is_refused_at_save_and_at_send() {
    let e = env().await;
    let c = e.webhook("ops", true).await;
    e.policy(&[&c]).await;

    // The vault item's password was changed to a short one afterwards.
    *e.secrets.short.lock().unwrap() = true;
    e.fire(Severity::Major).await;
    e.n.plan().await.unwrap();
    assert_eq!(e.n.dispatch().await.unwrap().failed, 1);
    let d = e.only().await;
    assert_eq!((d.status, d.last_error, d.attempts), (DeliveryStatus::Failed, Some(FailureClass::Secret), 1));
    assert_eq!(e.receiver.count(), 0, "neither weakly signed nor unsigned");

    // And a channel cannot be saved with it.
    let input = |url: Option<&str>| ChannelInput {
        name: "second".into(),
        kind: ChannelKind::Webhook,
        enabled: true,
        url: url.map(str::to_owned),
        secret_item: Some(e.secrets.item),
        email: None,
    };
    let err = e.n.create_channel(e.ws, input(Some("https://hooks.example.com/x")), e.who.clone()).await.unwrap_err();
    assert!(matches!(&err, DomainError::Invalid(m) if m.contains("16 bytes")), "{err}");
    assert!(matches!(e.n.update_channel(c.id, input(None)).await, Err(DomainError::Invalid(_))));
    // An item that does not exist is refused the same way.
    let mut ghost = input(Some("https://hooks.example.com/x"));
    ghost.secret_item = Some(VaultItemId::new());
    assert!(matches!(e.n.create_channel(e.ws, ghost, e.who.clone()).await, Err(DomainError::Invalid(_))));
}
