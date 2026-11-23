//! Silences (#11, D-112): what a silence does to notifications, one
//! semantic per test, on a manual clock.

use forge_core::silences::{Silence, SilenceRepository, SilenceScope, SilenceState};
use forge_core::{ServerId, SilenceId};

use super::*;

const HOUR: i64 = 60 * MIN;

impl Env {
    /// A silence over the whole workspace from now for `length` ms, changed
    /// by `tune`. Written to the store directly: the service's checks (the
    /// workspace exists, the rules exist) have their own tests.
    async fn silence_with(&self, length: i64, tune: impl FnOnce(&mut Silence)) -> Silence {
        let mut s = Silence {
            id: SilenceId::new(),
            workspace_id: self.ws,
            scope: SilenceScope::Workspace,
            rules: vec![],
            min_severity: None,
            max_severity: None,
            starts_at: self.now(),
            ends_at: Timestamp(self.now().0 + length),
            reason: "Kernel upgrade".into(),
            created_by: self.who.clone(),
            created_at: self.now(),
            updated_at: self.now(),
            ended_by: None,
        };
        tune(&mut s);
        self.store.create_silence(&s).await.unwrap();
        s
    }

    async fn silence(&self, length: i64) -> Silence {
        self.silence_with(length, |_| {}).await
    }

    /// One pass of the background loop.
    async fn pass(&self) -> (PlanReport, DispatchReport) {
        (self.n.plan().await.unwrap(), self.n.dispatch().await.unwrap())
    }

    /// Passes until one does nothing (a channel sends two deliveries per
    /// pass); the sum of what was dispatched.
    async fn drain(&self) -> DispatchReport {
        let mut total = DispatchReport::default();
        for _ in 0..10 {
            let (plan, d) = self.pass().await;
            if plan == PlanReport::default() && d == DispatchReport::default() {
                break;
            }
            total.sent += d.sent;
            total.suppressed += d.suppressed;
            total.cancelled += d.cancelled;
        }
        total
    }

    /// `(event, status)` of every delivery, oldest first.
    async fn history(&self) -> Vec<(DeliveryEvent, DeliveryStatus)> {
        self.deliveries().await.into_iter().map(|d| (d.event, d.status)).collect()
    }

    fn bodies(&self) -> Vec<serde_json::Value> {
        self.receiver.requests().iter().map(|r| serde_json::from_slice(&r.body).unwrap()).collect()
    }

    async fn reminding_policy(&self, c: &Channel) {
        let mut input = self.policy_input(&[c]);
        input.repeat_interval_secs = Some(600);
        self.n.create_policy(self.ws, input, self.who.clone()).await.unwrap();
    }
}

use DeliveryEvent::{Fired, Resolved};
use DeliveryStatus::{Cancelled, Sent, Suppressed};

#[tokio::test]
async fn an_alarm_that_fires_during_a_silence_is_recorded_as_suppressed_and_nothing_is_sent() {
    let e = env().await;
    let c = e.webhook("ops", false).await;
    e.policy(&[&c]).await;
    let s = e.silence(2 * HOUR).await;

    // Evaluation is not touched: the alarm opens and fires as always.
    let a = e.fire(Severity::Major).await;
    let before = e.store.get_alarm(a.id).await.unwrap();
    assert_eq!(before.state, AlarmState::Firing);

    let (plan, dispatch) = e.pass().await;
    assert_eq!((plan.deliveries, dispatch.suppressed, dispatch.sent), (1, 1, 0));
    assert_eq!(e.receiver.count(), 0, "nothing leaves Forge");

    // The delivery history says why nothing arrived.
    let d = e.only().await;
    assert_eq!((d.event, d.status, d.silence_id, d.attempts), (Fired, Suppressed, Some(s.id), 0));
    assert!(!d.after_silence && d.finished_at == Some(e.now()));
    assert_eq!(d.detail.as_deref(), Some("silenced: Kernel upgrade"));

    // Further passes while silenced change nothing.
    e.advance(30 * MIN);
    assert_eq!(e.pass().await, (PlanReport::default(), DispatchReport::default()));
    assert_eq!(e.receiver.count(), 0);
    // The alarm and its timeline are exactly as the evaluator left them.
    assert_eq!(e.store.get_alarm(a.id).await.unwrap(), before);
    let kinds: Vec<String> = e.store.alarm_events(a.id).await.unwrap().into_iter().map(|ev| ev.kind).collect();
    assert_eq!(kinds, ["fired"]);
    // And it shows as silenced, by which silence and until when.
    let mark = e.n.silences.marks(std::slice::from_ref(&before)).await.unwrap().remove(0).unwrap();
    assert_eq!((mark.silence_id, mark.until), (s.id, s.ends_at));
}

#[tokio::test]
async fn when_a_silence_expires_a_still_firing_alarm_sends_one_fired_message_that_says_so() {
    let e = env().await;
    let c = e.webhook("ops", false).await;
    e.policy(&[&c]).await;
    let s = e.silence(HOUR).await;
    let a = e.fire(Severity::Major).await;
    e.pass().await;

    // One millisecond before the end: still held.
    e.advance(HOUR - 1);
    assert_eq!(e.pass().await, (PlanReport::default(), DispatchReport::default()));
    // At the end (exclusive): one message.
    e.advance(1);
    let (plan, dispatch) = e.pass().await;
    assert_eq!((plan.after_silence, dispatch.sent), (1, 1));
    assert_eq!(e.receiver.count(), 1);
    let body = e.bodies().remove(0);
    assert_eq!(body["schema_version"], 1, "a new optional field does not change the version");
    assert_eq!(body["event"]["type"], "fired");
    assert_eq!(body["alarm"]["id"], json!(a.id));
    assert_eq!(body["after_silence"], json!({"silence_id": s.id}));
    assert_eq!(header(&e.receiver.requests()[0], payload::HEADER_EVENT), Some("fired"));

    let all = e.deliveries().await;
    assert_eq!(e.history().await, [(Fired, Suppressed), (Fired, Sent)]);
    assert!(all[0].released_at.is_some(), "the suppressed delivery is settled");
    assert!(all[1].after_silence && all[1].silence_id == Some(s.id) && all[1].policy_id == all[0].policy_id);

    // Exactly one: later passes add and send nothing.
    for _ in 0..3 {
        e.advance(MIN);
        assert_eq!(e.pass().await, (PlanReport::default(), DispatchReport::default()));
    }
    assert_eq!(e.receiver.count(), 1);

    // The recovery follows as for any announced alarm, without the marker.
    e.resolve(&a).await;
    assert_eq!(e.pass().await.1.sent, 1);
    let body = e.bodies().remove(1);
    assert_eq!(body["event"]["type"], "resolved");
    assert!(body.get("after_silence").is_none(), "{body}");
}

#[tokio::test]
async fn ending_a_silence_early_notifies_at_once_for_what_still_fires() {
    let e = env().await;
    let (ops, chat) = (e.webhook("ops", false).await, e.webhook("chat", false).await);
    e.policy(&[&ops, &chat]).await;
    let s = e.silence(8 * HOUR).await;
    let still = e.fire(Severity::Major).await;
    let gone = e.fire(Severity::Major).await;
    e.pass().await;
    assert_eq!(e.receiver.count(), 0);
    e.advance(10 * MIN);
    e.resolve(&gone).await;
    e.pass().await;
    assert_eq!(e.receiver.count(), 0);

    let ended = e.n.silences.end(s.id, e.who.clone()).await.unwrap();
    assert_eq!(ended.state(e.now()), SilenceState::Expired);
    let (plan, dispatch) = e.pass().await;
    // One message per channel for the alarm that still fires; none for the
    // one that fired and recovered inside the silence.
    assert_eq!((plan.after_silence, dispatch.sent), (2, 2));
    assert_eq!((e.receiver.count_to("/in/ops"), e.receiver.count_to("/in/chat")), (1, 1));
    for body in e.bodies() {
        assert_eq!((body["alarm"]["id"].clone(), body["event"]["type"].as_str()), (json!(still.id), Some("fired")));
        assert_eq!(body["after_silence"]["silence_id"], json!(s.id));
    }
    assert_eq!(e.pass().await, (PlanReport::default(), DispatchReport::default()));
}

#[tokio::test]
async fn an_alarm_that_fires_and_resolves_inside_a_silence_sends_nothing_at_all() {
    let e = env().await;
    let c = e.webhook("ops", false).await;
    e.policy(&[&c]).await;
    e.silence(HOUR).await;
    let a = e.fire(Severity::Major).await;
    e.pass().await;
    e.advance(20 * MIN);
    e.resolve(&a).await;
    let (_, dispatch) = e.pass().await;
    assert_eq!((dispatch.sent, dispatch.cancelled), (0, 1));

    // After the silence: nothing either. Both the alarm's history and the
    // delivery history are complete.
    e.advance(HOUR);
    assert_eq!(e.pass().await, (PlanReport::default(), DispatchReport::default()));
    e.advance(HOUR);
    e.pass().await;
    assert_eq!(e.receiver.count(), 0);
    assert_eq!(e.history().await, [(Fired, Suppressed), (Resolved, Cancelled)]);
    let recovery = e.deliveries().await.remove(1);
    assert!(recovery.detail.unwrap().contains("silenced"));
    assert_eq!(e.store.get_alarm(a.id).await.unwrap().state, AlarmState::Resolved);
}

#[tokio::test]
async fn a_recovery_is_held_back_while_silenced_and_sent_when_the_silence_ends() {
    let e = env().await;
    let c = e.webhook("ops", false).await;
    e.policy(&[&c]).await;
    // Announced before the silence.
    let a = e.fire(Severity::Major).await;
    assert_eq!(e.pass().await.1.sent, 1);

    let s = e.silence(HOUR).await;
    e.advance(10 * MIN);
    e.resolve(&a).await;
    let (_, dispatch) = e.pass().await;
    assert_eq!((dispatch.suppressed, dispatch.sent), (1, 0));
    assert_eq!(e.receiver.count(), 1, "no recovery while silenced");
    assert_eq!(e.history().await, [(Fired, Sent), (Resolved, Suppressed)]);

    // The silence ends: the receiver that was told "fired" is told
    // "recovered", once, late, and marked.
    e.advance(50 * MIN);
    let (plan, dispatch) = e.pass().await;
    assert_eq!((plan.after_silence, dispatch.sent), (1, 1));
    let body = e.bodies().remove(1);
    assert_eq!(
        (body["event"]["type"].as_str(), body["event"]["is_resolved"].as_bool()),
        (Some("resolved"), Some(true))
    );
    assert_eq!(body["after_silence"]["silence_id"], json!(s.id));
    assert_eq!(e.history().await, [(Fired, Sent), (Resolved, Suppressed), (Resolved, Sent)]);
    e.advance(HOUR);
    assert_eq!(e.pass().await, (PlanReport::default(), DispatchReport::default()));
    assert_eq!(e.receiver.count(), 2);
}

#[tokio::test]
async fn reminders_are_suppressed_while_silenced_and_one_follows_the_silence() {
    let e = env().await;
    let c = e.webhook("ops", false).await;
    e.reminding_policy(&c).await;
    let a = e.fire(Severity::Major).await;
    assert_eq!(e.pass().await.1.sent, 1, "fired, before the silence");

    let s = e.silence(35 * MIN).await;
    for n in 1..=3 {
        e.advance(10 * MIN);
        let (plan, dispatch) = e.pass().await;
        assert_eq!((plan.reminders, dispatch.suppressed, dispatch.sent), (1, 1, 0), "reminder {n}");
    }
    assert_eq!(e.receiver.count(), 1);

    // The silence ends at 35 minutes: one reminder stands for the three
    // that were held back.
    e.advance(5 * MIN);
    let (plan, dispatch) = e.pass().await;
    assert_eq!((plan.after_silence, plan.reminders, dispatch.sent), (1, 0, 1));
    let body = e.bodies().remove(1);
    assert_eq!((body["event"]["type"].as_str(), body["event"]["occurrence"].as_i64()), (Some("reminder"), Some(3)));
    assert_eq!(body["after_silence"]["silence_id"], json!(s.id));
    assert_eq!(body["alarm"]["id"], json!(a.id));
    let suppressed = e.deliveries().await.into_iter().filter(|d| d.status == Suppressed).count();
    assert_eq!(suppressed, 3);

    // Reminders go on as before.
    e.advance(5 * MIN);
    let (plan, dispatch) = e.pass().await;
    assert_eq!((plan.reminders, dispatch.sent), (1, 1));
    let body = e.bodies().remove(2);
    assert_eq!(body["event"]["occurrence"], 4);
    assert!(body.get("after_silence").is_none());
}

#[tokio::test]
async fn a_severity_above_the_maximum_or_below_the_minimum_is_not_silenced() {
    let e = env().await;
    let c = e.webhook("ops", false).await;
    e.policy(&[&c]).await;
    e.silence_with(HOUR, |s| {
        s.min_severity = Some(Severity::Warning);
        s.max_severity = Some(Severity::Major);
    })
    .await;
    let critical = e.fire(Severity::Critical).await;
    let major = e.fire(Severity::Major).await;
    let info = e.fire(Severity::Info).await;
    let dispatch = e.drain().await;
    assert_eq!((dispatch.sent, dispatch.suppressed), (2, 1));
    let told: Vec<serde_json::Value> = e.bodies().into_iter().map(|b| b["alarm"]["id"].clone()).collect();
    assert!(told.contains(&json!(critical.id)) && told.contains(&json!(info.id)), "{told:?}");
    assert!(!told.contains(&json!(major.id)));
}

#[tokio::test]
async fn a_silence_covers_only_the_rules_and_resources_it_names() {
    let e = env().await;
    let c = e.webhook("ops", false).await;
    e.policy(&[&c]).await;
    let (covered, other_rule, other_node) =
        (e.fire(Severity::Major).await, e.fire(Severity::Major).await, e.fire(Severity::Major).await);
    let node: ServerId = covered.resource.id.parse().unwrap();
    let other: ServerId = other_rule.resource.id.parse().unwrap();
    e.silence_with(HOUR, |s| {
        s.scope = SilenceScope::Servers { ids: vec![node, other] };
        s.rules = vec![covered.rule_id, other_node.rule_id];
    })
    .await;
    let dispatch = e.drain().await;
    assert_eq!((dispatch.suppressed, dispatch.sent), (1, 2));
    let suppressed: Vec<_> = e.deliveries().await.into_iter().filter(|d| d.status == Suppressed).collect();
    assert_eq!(suppressed[0].alarm_id, Some(covered.id));
}

#[tokio::test]
async fn a_scheduled_window_holds_nothing_until_it_starts() {
    let e = env().await;
    let c = e.webhook("ops", false).await;
    e.policy(&[&c]).await;
    e.silence_with(HOUR, |s| {
        s.starts_at = Timestamp(s.starts_at.0 + HOUR);
        s.ends_at = Timestamp(s.ends_at.0 + HOUR);
    })
    .await;
    e.fire(Severity::Major).await;
    assert_eq!(e.pass().await.1.sent, 1, "before the window: sent");
    e.advance(HOUR);
    e.fire(Severity::Major).await;
    let (_, dispatch) = e.pass().await;
    assert_eq!((dispatch.sent, dispatch.suppressed), (0, 1), "inside the window: held");
}

#[tokio::test]
async fn acknowledging_and_silencing_are_independent() {
    let e = env().await;
    let c = e.webhook("ops", false).await;
    e.reminding_policy(&c).await;
    let s = e.silence(HOUR).await;
    let a = e.fire(Severity::Major).await;
    e.pass().await;

    // A silence does not acknowledge.
    assert!(e.store.get_alarm(a.id).await.unwrap().acknowledged_at.is_none());
    // An acknowledgement does not lift the silence: the alarm is still
    // marked, and nothing is sent.
    e.store.acknowledge(a.id, Some(&e.who), e.now(), &e.event(&a, "acknowledged")).await.unwrap();
    let acked = e.store.get_alarm(a.id).await.unwrap();
    assert!(acked.acknowledged_at.is_some());
    assert_eq!(
        e.n.silences.marks(std::slice::from_ref(&acked)).await.unwrap()[0].as_ref().map(|m| m.silence_id),
        Some(s.id)
    );
    e.advance(10 * MIN);
    e.pass().await;
    assert_eq!(e.receiver.count(), 0);

    // When the silence ends the firing message still goes out (the
    // receiver was never told), with the acknowledgement in it; reminders
    // stay off because the alarm is acknowledged.
    e.advance(50 * MIN);
    let (plan, dispatch) = e.pass().await;
    assert_eq!((plan.after_silence, dispatch.sent), (1, 1));
    let body = e.bodies().remove(0);
    assert_eq!((body["event"]["type"].as_str(), body["alarm"]["acknowledged"].as_bool()), (Some("fired"), Some(true)));
    e.advance(30 * MIN);
    e.pass().await;
    assert_eq!(e.receiver.count(), 1);
}

#[tokio::test]
async fn a_silence_that_starts_while_a_delivery_waits_for_its_retry_holds_it_back() {
    let e = env().await;
    let c = e.webhook("ops", false).await;
    e.policy(&[&c]).await;
    e.fire(Severity::Major).await;
    e.receiver.script(&[SendOutcome::Status(503)]);
    assert_eq!(e.pass().await.1.retried, 1);
    assert_eq!(e.receiver.count(), 1);

    // The silence is checked just before every send, not when planning.
    let s = e.silence(HOUR).await;
    e.advance(MIN);
    assert_eq!(e.pass().await.1.suppressed, 1);
    assert_eq!(e.receiver.count(), 1, "no second attempt");
    let d = e.only().await;
    assert_eq!((d.status, d.silence_id, d.attempts), (Suppressed, Some(s.id), 1));

    e.advance(HOUR);
    assert_eq!(e.pass().await.1.sent, 1);
    assert_eq!(e.receiver.count(), 2);
}

#[tokio::test]
async fn overlapping_silences_hold_until_the_last_one_ends_and_extending_moves_the_end() {
    let e = env().await;
    let c = e.webhook("ops", false).await;
    e.policy(&[&c]).await;
    let first = e.silence(HOUR).await;
    e.fire(Severity::Major).await;
    e.pass().await;
    assert_eq!(e.only().await.silence_id, Some(first.id));

    // A second silence begins before the first ends.
    e.advance(30 * MIN);
    let second = e.silence(HOUR).await;
    // The first ends: the alarm is still covered, so nothing is sent and
    // the held delivery now names the silence that holds it.
    e.advance(30 * MIN);
    assert_eq!(e.pass().await, (PlanReport::default(), DispatchReport::default()));
    let held = e.only().await;
    assert_eq!((held.status, held.silence_id, held.released_at), (Suppressed, Some(second.id), None));

    // Extended by an hour: its old end passes without a message.
    let extended = e.n.silences.extend(second.id, Timestamp(second.ends_at.0 + HOUR)).await.unwrap();
    e.advance(30 * MIN);
    assert_eq!(e.pass().await, (PlanReport::default(), DispatchReport::default()));
    assert_eq!(e.receiver.count(), 0);
    // The new end: exactly one message.
    e.advance(HOUR);
    assert_eq!(e.now(), extended.ends_at);
    assert_eq!(e.pass().await.1.sent, 1);
    assert_eq!(e.bodies()[0]["after_silence"]["silence_id"], json!(second.id));
    e.advance(HOUR);
    e.pass().await;
    assert_eq!(e.receiver.count(), 1);
}

#[tokio::test]
async fn a_message_that_follows_a_silence_is_held_by_the_next_silence() {
    let e = env().await;
    let c = e.webhook("ops", false).await;
    e.policy(&[&c]).await;
    let first = e.silence(HOUR).await;
    e.fire(Severity::Major).await;
    e.pass().await;
    // The first ends and the receiver is down; before the retry a second
    // silence starts.
    e.advance(HOUR);
    e.receiver.script(&[SendOutcome::Status(503)]);
    assert_eq!(e.pass().await.1.retried, 1);
    let second = e.silence(HOUR).await;
    e.advance(MIN);
    assert_eq!(e.pass().await.1.suppressed, 1);
    e.advance(HOUR);
    assert_eq!(e.pass().await.1.sent, 1);
    assert_eq!(e.receiver.count(), 2, "one failed attempt, one message");
    assert_eq!(e.bodies()[1]["after_silence"]["silence_id"], json!(second.id));
    let all = e.deliveries().await;
    assert_eq!(all.iter().map(|d| d.status).collect::<Vec<_>>(), [Suppressed, Suppressed, Sent]);
    assert_eq!(all[0].silence_id, Some(first.id));
}

#[tokio::test]
async fn a_test_message_is_not_silenced_and_a_resolved_only_policy_is() {
    let e = env().await;
    let c = e.webhook("ops", false).await;
    let mut input = e.policy_input(&[&c]);
    input.events = vec![PolicyEvent::Resolved];
    e.n.create_policy(e.ws, input, e.who.clone()).await.unwrap();
    e.silence(HOUR).await;

    // A test names no alarm: no silence covers it.
    let (d, _) = e.n.test_channel(c.id).await.unwrap();
    assert_eq!(d.status, Sent);
    assert_eq!(e.receiver.count(), 1);

    // A policy that only reports recoveries: held while silenced, sent after.
    let a = e.fire(Severity::Major).await;
    e.pass().await;
    e.resolve(&a).await;
    assert_eq!(e.pass().await.1.suppressed, 1);
    assert_eq!(e.receiver.count(), 1);
    e.advance(HOUR);
    assert_eq!(e.pass().await.1.sent, 1);
    assert_eq!(e.bodies()[1]["event"]["type"], "resolved");
}

#[tokio::test]
async fn settling_is_safe_to_repeat_and_survives_a_deleted_alarm_or_silence() {
    let e = env().await;
    let c = e.webhook("ops", false).await;
    e.policy(&[&c]).await;
    let s = e.silence(HOUR).await;
    e.fire(Severity::Major).await;
    e.pass().await;
    let held = e.only().await;
    e.advance(HOUR);

    // Released behind the planner's back, twice: one follow-up, then none.
    let follow_up = Notifier::after_silence(&held, e.now());
    assert_eq!(e.store.release_suppressed(&[held.id], std::slice::from_ref(&follow_up), e.now()).await.unwrap(), 1);
    let again = Notifier::after_silence(&held, e.now());
    assert_eq!(e.store.release_suppressed(&[held.id], std::slice::from_ref(&again), e.now()).await.unwrap(), 0);
    // Even a planner that missed the release cannot add a second message:
    // the key (alarm, event, silence, channel) is taken.
    assert_eq!(e.store.add_deliveries(std::slice::from_ref(&again)).await.unwrap(), 0);
    assert_eq!(e.n.plan().await.unwrap(), PlanReport::default());
    assert_eq!(e.n.dispatch().await.unwrap().sent, 1);
    assert_eq!(e.receiver.count(), 1);
    assert_eq!(e.bodies()[0]["after_silence"]["silence_id"], json!(s.id));

    // Old history goes; the silence follows once no delivery names it.
    e.advance(40 * 24 * HOUR);
    assert_eq!(e.n.prune().await.unwrap(), 2);
    assert!(e.store.get_silence(s.id).await.unwrap().is_none());
}

#[tokio::test]
async fn a_suppressed_delivery_is_kept_until_it_was_settled() {
    let e = env().await;
    let c = e.webhook("ops", false).await;
    e.policy(&[&c]).await;
    // Chained 30-day silences hold an alarm back past the retention.
    let first = e.silence(30 * 24 * HOUR).await;
    e.fire(Severity::Major).await;
    e.pass().await;
    e.advance(29 * 24 * HOUR);
    let second = e.silence(30 * 24 * HOUR).await;
    e.advance(5 * 24 * HOUR);
    e.pass().await;
    assert_eq!(e.n.prune().await.unwrap(), 0, "not settled yet: kept");
    let held = e.only().await;
    assert_eq!((held.status, held.silence_id), (Suppressed, Some(second.id)));
    assert_ne!(first.id, second.id);
    e.advance(30 * 24 * HOUR);
    assert_eq!(e.pass().await.1.sent, 1, "the problem that outlived both windows is announced");
}
