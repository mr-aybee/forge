//! Notification channels, policies, the transition outbox and deliveries
//! (#11, D-110).

use forge_core::alerts::*;
use forge_core::notifications::*;
use forge_core::*;
use forge_store::Store;
use serde_json::json;

async fn store() -> (Store, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open_sqlite(&dir.path().join("forge.db")).await.unwrap();

    (s, dir)
}

fn who() -> Principal {
    Principal::User(UserId::new())
}

fn channel(ws: WorkspaceId) -> Channel {
    Channel {
        id: ChannelId::new(),
        workspace_id: ws,
        name: "Ops webhook".into(),
        kind: ChannelKind::Webhook,
        enabled: true,
        target: "https://hooks.example.com".into(),
        sealed_url: "sealed".into(),
        email: None,
        secret_item: Some(VaultItemId::new()),
        created_by: who(),
        created_at: Timestamp(1),
        updated_at: Timestamp(1),
    }
}

fn policy(ws: WorkspaceId, channels: Vec<ChannelId>) -> NotificationPolicy {
    NotificationPolicy {
        id: NotificationPolicyId::new(),
        workspace_id: ws,
        name: "Critical to ops".into(),
        enabled: true,
        min_severity: Severity::Major,
        rules: vec![AlertRuleId::new()],
        scope: Some(RuleScope::Servers { ids: vec![ServerId::new()] }),
        events: vec![PolicyEvent::Fired, PolicyEvent::Resolved],
        repeat_interval_secs: Some(900),
        channels,
        email_channel: None,
        email_users: vec![],
        email_roles: vec![],
        created_by: who(),
        created_at: Timestamp(1),
        updated_at: Timestamp(1),
    }
}

fn alarm(ws: WorkspaceId, fp: &str, at: i64) -> Alarm {
    Alarm {
        id: AlarmId::new(),
        workspace_id: ws,
        rule_id: AlertRuleId::new(),
        rule_revision: 1,
        title: "CPU high".into(),
        severity: Severity::Major,
        fingerprint: fp.into(),
        resource: EntityRef::new(EntityKind::Server, ServerId::new()),
        server: None,
        subject: "node.cpu.percent".into(),
        state: AlarmState::Firing,
        fired_value: json!({"type": "num", "value": 91.0}),
        fired_at: Timestamp(at),
        resolved_at: None,
        end_reason: None,
        acknowledged_by: None,
        acknowledged_at: None,
        previous: None,
    }
}

fn ev(a: &Alarm, kind: &str, at: i64) -> AlarmEvent {
    AlarmEvent { alarm_id: a.id, at: Timestamp(at), kind: kind.into(), actor: None, detail: json!({}) }
}

fn delivery(a: &Alarm, event: DeliveryEvent, p: &NotificationPolicy, c: &Channel, at: i64) -> Delivery {
    Delivery {
        id: DeliveryId::new(),
        workspace_id: a.workspace_id,
        alarm_id: Some(a.id),
        event,
        occurrence: 0,
        policy_id: Some(p.id),
        channel_id: c.id,
        dedupe_key: dedupe_key(a.id, event, 0, p.id, c.id),
        status: DeliveryStatus::Pending,
        attempts: 0,
        next_attempt_at: Timestamp(at),
        lease_until: None,
        last_error: None,
        last_http_status: None,
        detail: None,
        created_at: Timestamp(at),
        finished_at: None,
        channel_name: c.name.clone(),
        policy_name: p.name.clone(),
        alarm_title: a.title.clone(),
        resource: Some(a.resource.clone()),
        silence_id: None,
        after_silence: false,
        released_at: None,
        recipients: None,
    }
}

async fn ids(s: &Store, f: DeliveryFilter) -> Vec<DeliveryId> {
    s.list_deliveries(&f, 50).await.unwrap().into_iter().map(|d| d.id).collect()
}

#[tokio::test]
async fn policy_people_and_delivery_recipients_round_trip() {
    let (s, _d) = store().await;
    let ws = WorkspaceId::new();
    let c = channel(ws);
    s.put_channel(&c).await.unwrap();
    let mut p = policy(ws, vec![]);
    p.email_channel = Some(c.id);
    p.email_users = vec![UserId::new(), UserId::new()];
    p.email_roles = vec![Role::Operator, Role::WorkspaceAdmin];
    s.put_policy(&p).await.unwrap();
    assert_eq!(s.get_policy(p.id).await.unwrap(), Some(p.clone()));

    // Without people the stored recipients are cleared.
    p.email_users.clear();
    p.email_roles.clear();
    p.email_channel = None;
    s.put_policy(&p).await.unwrap();
    assert_eq!(s.get_policy(p.id).await.unwrap(), Some(p.clone()));

    let a = alarm(ws, "fp-people", 10);
    s.open_alarm(&a, &ev(&a, "fired", 10)).await.unwrap();
    let skipped = SkippedRecipient { user_id: UserId::new(), reason: RecipientSkip::NoEmail };
    let mut d = delivery(&a, DeliveryEvent::Fired, &p, &c, 10);
    d.dedupe_key = people_dedupe_key(a.id, DeliveryEvent::Fired, 0, p.id, c.id);
    d.recipients = Some(DeliveryRecipients { count: 1, to: vec!["p@example.com".into()], skipped: vec![skipped] });

    // A delivery that resolved to nobody is added already cancelled.
    let mut nobody = delivery(&a, DeliveryEvent::Fired, &p, &c, 10);
    nobody.status = DeliveryStatus::Cancelled;
    nobody.detail = Some("no recipients".into());
    nobody.recipients = Some(DeliveryRecipients { count: 0, to: vec![], skipped: vec![skipped] });
    assert_eq!(s.add_deliveries(&[d.clone(), nobody.clone()]).await.unwrap(), 2);
    assert_eq!(s.get_delivery(d.id).await.unwrap().unwrap().recipients, d.recipients);
    let got = s.get_delivery(nobody.id).await.unwrap().unwrap();
    assert_eq!((got.status, got.detail.as_deref()), (DeliveryStatus::Cancelled, Some("no recipients")));
    assert_eq!(got.finished_at, Some(Timestamp(10)));
    assert_eq!(got.recipients, nobody.recipients);

    // Only the pending one is due.
    let due: Vec<DeliveryId> =
        s.due_deliveries(Timestamp(20), 10, 10).await.unwrap().into_iter().map(|x| x.id).collect();
    assert_eq!(due, vec![d.id]);
}

#[tokio::test]
async fn channels_and_policies_round_trip() {
    let (s, _d) = store().await;
    let ws = WorkspaceId::new();
    let mut c = channel(ws);
    s.put_channel(&c).await.unwrap();
    assert_eq!(s.get_channel(c.id).await.unwrap(), Some(c.clone()));
    c.name = "Renamed".into();
    c.enabled = false;
    c.secret_item = None;
    c.kind = ChannelKind::Slack;
    c.updated_at = Timestamp(5);
    s.put_channel(&c).await.unwrap();
    assert_eq!(s.list_channels(ws).await.unwrap(), vec![c.clone()]);
    assert!(s.list_channels(WorkspaceId::new()).await.unwrap().is_empty());

    let mut p = policy(ws, vec![c.id]);
    s.put_policy(&p).await.unwrap();
    assert_eq!(s.get_policy(p.id).await.unwrap(), Some(p.clone()));
    p.scope = None;
    p.rules.clear();
    p.repeat_interval_secs = None;
    p.events = vec![PolicyEvent::Fired];
    s.put_policy(&p).await.unwrap();
    assert_eq!(s.list_policies(Some(ws)).await.unwrap(), vec![p.clone()]);
    assert_eq!(s.list_policies(None).await.unwrap().len(), 1);
    assert!(s.list_policies(Some(WorkspaceId::new())).await.unwrap().is_empty());

    s.delete_policy(p.id).await.unwrap();
    assert!(matches!(s.delete_policy(p.id).await, Err(DomainError::NotFound(_))));
    s.delete_channel(c.id).await.unwrap();
    assert!(matches!(s.delete_channel(c.id).await, Err(DomainError::NotFound(_))));
    assert_eq!(s.get_channel(c.id).await.unwrap(), None);
}

#[tokio::test]
async fn alarm_changes_write_their_transition_in_the_same_transaction() {
    let (s, _d) = store().await;
    let ws = WorkspaceId::new();
    let a = alarm(ws, "fp-1", 10);
    s.open_alarm(&a, &ev(&a, "fired", 10)).await.unwrap();

    // A refused duplicate leaves no transition behind.
    let dup = alarm(ws, "fp-1", 11);
    assert!(s.open_alarm(&dup, &ev(&dup, "fired", 11)).await.is_err());
    let t = s.unplanned_transitions(10).await.unwrap();
    assert_eq!(
        t,
        vec![AlarmTransition { alarm_id: a.id, workspace_id: ws, kind: TransitionKind::Fired, at: Timestamp(10) }]
    );

    // Acknowledging is not a transition.
    s.acknowledge(a.id, Some(&who()), Timestamp(12), &ev(&a, "acknowledged", 12)).await.unwrap();
    assert_eq!(s.unplanned_transitions(10).await.unwrap().len(), 1);

    assert!(s.close_alarm(a.id, AlarmState::Resolved, Timestamp(20), None, &ev(&a, "resolved", 20)).await.unwrap());

    // A second close changes nothing and records nothing.
    assert!(!s.close_alarm(a.id, AlarmState::Ended, Timestamp(21), Some("x"), &ev(&a, "ended", 21)).await.unwrap());
    let kinds: Vec<TransitionKind> = s.unplanned_transitions(10).await.unwrap().into_iter().map(|t| t.kind).collect();
    assert_eq!(kinds, [TransitionKind::Fired, TransitionKind::Resolved]);

    let b = alarm(ws, "fp-2", 30);
    s.open_alarm(&b, &ev(&b, "fired", 30)).await.unwrap();
    let why = Some("the rule was deleted");
    assert!(s.close_alarm(b.id, AlarmState::Ended, Timestamp(40), why, &ev(&b, "ended", 40)).await.unwrap());
    let last = s.unplanned_transitions(10).await.unwrap().pop().unwrap();
    assert_eq!((last.alarm_id, last.kind), (b.id, TransitionKind::Ended));
}

#[tokio::test]
async fn planning_a_transition_twice_adds_nothing() {
    let (s, _d) = store().await;
    let ws = WorkspaceId::new();
    let (c, c2) = (channel(ws), channel(ws));
    let p = policy(ws, vec![c.id, c2.id]);
    let a = alarm(ws, "fp-1", 10);
    s.open_alarm(&a, &ev(&a, "fired", 10)).await.unwrap();
    let t = s.unplanned_transitions(10).await.unwrap().remove(0);
    let planned = vec![delivery(&a, DeliveryEvent::Fired, &p, &c, 11), delivery(&a, DeliveryEvent::Fired, &p, &c2, 11)];
    assert_eq!(s.plan_transition(&t, &planned, Timestamp(11)).await.unwrap(), 2);
    assert!(s.unplanned_transitions(10).await.unwrap().is_empty());

    // The same transition again, with fresh delivery ids: already planned.
    let again = vec![delivery(&a, DeliveryEvent::Fired, &p, &c, 12), delivery(&a, DeliveryEvent::Fired, &p, &c2, 12)];
    assert_eq!(s.plan_transition(&t, &again, Timestamp(12)).await.unwrap(), 0);

    // And outside a transition the dedupe key refuses a copy.
    assert_eq!(s.add_deliveries(&again).await.unwrap(), 0);
    let all =
        s.list_deliveries(&DeliveryFilter { workspace: Some(ws), ..DeliveryFilter::default() }, 50).await.unwrap();
    assert_eq!(all.len(), 2);
    assert!(all.iter().all(|d| d.status == DeliveryStatus::Pending && d.attempts == 0));
    assert_eq!(all.iter().find(|d| d.id == planned[0].id).unwrap(), &planned[0]);
}

#[tokio::test]
async fn a_delivery_is_claimed_once_retried_and_finished() {
    let (s, _d) = store().await;
    let ws = WorkspaceId::new();
    let c = channel(ws);
    let p = policy(ws, vec![c.id]);
    let a = alarm(ws, "fp-1", 10);
    let d = delivery(&a, DeliveryEvent::Fired, &p, &c, 100);
    assert_eq!(s.add_deliveries(std::slice::from_ref(&d)).await.unwrap(), 1);

    assert!(s.due_deliveries(Timestamp(99), 10, 10).await.unwrap().is_empty(), "not due yet");
    assert_eq!(s.due_deliveries(Timestamp(100), 10, 10).await.unwrap().len(), 1);
    assert!(!s.claim_delivery(d.id, Timestamp(99), Timestamp(160)).await.unwrap());
    assert!(s.claim_delivery(d.id, Timestamp(100), Timestamp(160)).await.unwrap());
    assert!(!s.claim_delivery(d.id, Timestamp(101), Timestamp(161)).await.unwrap(), "one worker per attempt");
    assert!(s.due_deliveries(Timestamp(150), 10, 10).await.unwrap().is_empty(), "leased");

    // A failed attempt schedules the next one.
    let fail = DeliveryAttempt {
        delivery_id: d.id,
        number: 1,
        at: Timestamp(105),
        ok: false,
        http_status: Some(503),
        error: Some(FailureClass::HttpStatus),
        duration_ms: 5,
    };
    assert!(s.finish_attempt(&fail, AttemptResult::Retry(Timestamp(200))).await.unwrap());
    let got = s.get_delivery(d.id).await.unwrap().unwrap();
    assert_eq!((got.status, got.attempts, got.next_attempt_at), (DeliveryStatus::Pending, 1, Timestamp(200)));
    assert_eq!((got.last_error, got.last_http_status), (Some(FailureClass::HttpStatus), Some(503)));
    assert!(s.due_deliveries(Timestamp(199), 10, 10).await.unwrap().is_empty());

    // The worker dies mid-attempt: the lease expires and the delivery is due again.
    assert!(s.claim_delivery(d.id, Timestamp(200), Timestamp(260)).await.unwrap());
    assert_eq!(s.due_deliveries(Timestamp(260), 10, 10).await.unwrap().len(), 1);
    assert!(s.claim_delivery(d.id, Timestamp(260), Timestamp(320)).await.unwrap());

    let ok = DeliveryAttempt {
        delivery_id: d.id,
        number: 3,
        at: Timestamp(261),
        ok: true,
        http_status: Some(204),
        error: None,
        duration_ms: 3,
    };

    // The second attempt (lease lost) reports late: it no longer holds the
    // delivery, so nothing changes and no attempt row is written.
    let late = DeliveryAttempt { number: 2, at: Timestamp(261), ok: true, http_status: Some(200), ..fail.clone() };
    assert!(!s.finish_attempt(&late, AttemptResult::Sent).await.unwrap());
    let got = s.get_delivery(d.id).await.unwrap().unwrap();
    assert_eq!((got.status, got.attempts, got.last_http_status), (DeliveryStatus::Sending, 3, Some(503)));
    assert_eq!(s.delivery_attempts(d.id).await.unwrap(), vec![fail.clone()]);

    assert!(s.finish_attempt(&ok, AttemptResult::Sent).await.unwrap());

    // A repeat of the finished attempt is refused too: it is no longer sending.
    assert!(!s.finish_attempt(&ok, AttemptResult::Failed).await.unwrap());
    let got = s.get_delivery(d.id).await.unwrap().unwrap();
    assert_eq!((got.status, got.attempts, got.finished_at), (DeliveryStatus::Sent, 3, Some(Timestamp(261))));
    assert_eq!((got.last_error, got.last_http_status), (None, Some(204)));
    assert_eq!(s.delivery_attempts(d.id).await.unwrap(), vec![fail, ok]);

    // Final: nothing claims or cancels it any more.
    assert!(!s.claim_delivery(d.id, Timestamp(9999), Timestamp(99_999)).await.unwrap());
    s.cancel_delivery(d.id, "late", Timestamp(300)).await.unwrap();
    assert_eq!(s.get_delivery(d.id).await.unwrap().unwrap().status, DeliveryStatus::Sent);
}

#[tokio::test]
async fn deliveries_postpone_cancel_filter_and_prune() {
    let (s, _d) = store().await;
    let ws = WorkspaceId::new();
    let c = channel(ws);
    let p = policy(ws, vec![c.id]);
    let a = alarm(ws, "fp-1", 10);
    s.open_alarm(&a, &ev(&a, "fired", 10)).await.unwrap();
    let t = s.unplanned_transitions(10).await.unwrap().remove(0);
    let fired = delivery(&a, DeliveryEvent::Fired, &p, &c, 100);
    let resolved = delivery(&a, DeliveryEvent::Resolved, &p, &c, 200);
    s.plan_transition(&t, std::slice::from_ref(&fired), Timestamp(100)).await.unwrap();
    s.add_deliveries(std::slice::from_ref(&resolved)).await.unwrap();

    s.postpone_delivery(resolved.id, Timestamp(500)).await.unwrap();
    let due: Vec<DeliveryId> =
        s.due_deliveries(Timestamp(300), 10, 10).await.unwrap().into_iter().map(|d| d.id).collect();
    assert_eq!(due, [fired.id]);
    s.cancel_delivery(fired.id, "alarm no longer firing", Timestamp(300)).await.unwrap();
    let got = s.get_delivery(fired.id).await.unwrap().unwrap();
    assert_eq!((got.status, got.detail.as_deref()), (DeliveryStatus::Cancelled, Some("alarm no longer firing")));

    assert_eq!(ids(&s, DeliveryFilter { alarm: Some(a.id), ..Default::default() }).await, [resolved.id, fired.id]);
    assert_eq!(
        ids(&s, DeliveryFilter { status: Some(DeliveryStatus::Cancelled), ..Default::default() }).await,
        [fired.id]
    );
    assert!(ids(&s, DeliveryFilter { channel: Some(ChannelId::new()), ..Default::default() }).await.is_empty());
    assert!(ids(&s, DeliveryFilter { workspace: Some(WorkspaceId::new()), ..Default::default() }).await.is_empty());
    let page = DeliveryFilter { before: Some((resolved.created_at, resolved.id)), ..Default::default() };
    assert_eq!(ids(&s, page).await, [fired.id]);

    // Pruning removes final deliveries only, and planned transitions.
    assert_eq!(s.prune_deliveries(Timestamp(1000)).await.unwrap(), 1);
    assert_eq!(ids(&s, DeliveryFilter::default()).await, [resolved.id], "a pending delivery is kept");
    assert!(s.unplanned_transitions(10).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_transition_that_cannot_be_planned_is_given_up_after_the_limit() {
    let (s, _d) = store().await;
    let ws = WorkspaceId::new();
    let a = alarm(ws, "fp-1", 10);
    s.open_alarm(&a, &ev(&a, "fired", 10)).await.unwrap();
    let t = s.unplanned_transitions(10).await.unwrap().remove(0);
    assert!(!s.fail_transition(&t, 3, "alarm unreadable", Timestamp(11)).await.unwrap());
    assert!(!s.fail_transition(&t, 3, "alarm unreadable", Timestamp(12)).await.unwrap());
    assert_eq!(s.unplanned_transitions(10).await.unwrap().len(), 1, "still waiting");
    assert!(s.fail_transition(&t, 3, "alarm unreadable", Timestamp(13)).await.unwrap());
    assert!(s.unplanned_transitions(10).await.unwrap().is_empty(), "given up: out of the queue");

    // Given up is final: no later failure or plan changes it.
    assert!(!s.fail_transition(&t, 3, "again", Timestamp(14)).await.unwrap());
    let c = channel(ws);
    let p = policy(ws, vec![c.id]);
    let late = delivery(&a, DeliveryEvent::Fired, &p, &c, 15);
    assert_eq!(s.plan_transition(&t, std::slice::from_ref(&late), Timestamp(15)).await.unwrap(), 0);
}

#[tokio::test]
async fn due_deliveries_take_a_few_per_channel_longest_due_first() {
    let (s, _d) = store().await;
    let ws = WorkspaceId::new();
    let (busy, quiet) = (channel(ws), channel(ws));
    let p = policy(ws, vec![busy.id, quiet.id]);

    // Twenty deliveries wait on one channel; one, planned last, on another.
    let mut backlog = Vec::new();

    for i in 0..20 {
        let a = alarm(ws, &format!("fp-{i}"), 10);
        backlog.push(delivery(&a, DeliveryEvent::Fired, &p, &busy, 100 + i));
    }

    let a = alarm(ws, "fp-quiet", 10);
    let other = delivery(&a, DeliveryEvent::Fired, &p, &quiet, 500);
    s.add_deliveries(&backlog).await.unwrap();
    s.add_deliveries(std::slice::from_ref(&other)).await.unwrap();

    let due: Vec<DeliveryId> =
        s.due_deliveries(Timestamp(1000), 2, 10).await.unwrap().into_iter().map(|d| d.id).collect();
    assert_eq!(due, [backlog[0].id, backlog[1].id, other.id], "two of the backlog, and the other channel is seen");

    // Postponed deliveries move back in their lane.
    s.postpone_delivery(backlog[0].id, Timestamp(900)).await.unwrap();
    let due: Vec<DeliveryId> =
        s.due_deliveries(Timestamp(1000), 2, 10).await.unwrap().into_iter().map(|d| d.id).collect();
    assert_eq!(due, [backlog[1].id, backlog[2].id, other.id]);
}

#[tokio::test]
async fn an_email_channel_keeps_its_smtp_settings() {
    let (s, _d) = store().await;
    let ws = WorkspaceId::new();
    let mut c = channel(ws);
    c.kind = ChannelKind::Email;
    c.target = "smtp://smtp.example.com:587".into();
    c.secret_item = None;
    c.email = Some(EmailSettings {
        smtp_host: "smtp.example.com".into(),
        smtp_port: 587,
        tls: SmtpTls::Starttls,
        username: Some("forge".into()),
        password_set: true,
        from: "Forge <forge@example.com>".into(),
        to: vec!["ops@example.com".into(), "oncall@example.com".into()],
        subject_prefix: Some("[prod]".into()),
    });
    s.put_channel(&c).await.unwrap();
    assert_eq!(s.get_channel(c.id).await.unwrap(), Some(c.clone()));

    // Back to a webhook: the settings go.
    c.kind = ChannelKind::Webhook;
    c.email = None;
    s.put_channel(&c).await.unwrap();
    assert_eq!(s.get_channel(c.id).await.unwrap().and_then(|c| c.email), None);
}
