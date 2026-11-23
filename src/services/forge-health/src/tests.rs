#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use std::time::Duration;

use forge_core::{EntityKind, EntityRef, ManualClock, ServerId, Timestamp};
use forge_store::Store;

use super::*;

/// Placements for unit tests: (app id, node) pairs.
#[derive(Debug, Default)]
struct FakePlacements(std::sync::Mutex<Vec<(String, ServerId)>>);

#[async_trait::async_trait]
impl InstancePlacements for FakePlacements {
    async fn is_placed_on(&self, app: &str, server: ServerId) -> forge_core::DomainResult<bool> {
        Ok(self.0.lock().unwrap().iter().any(|(a, s)| a == app && *s == server))
    }
}

struct Fixture {
    rec: ObservationRecorder,
    store: Store,
    clock: Arc<ManualClock>,
    placements: Arc<FakePlacements>,
    node: ServerId,
    _dir: tempfile::TempDir,
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_sqlite(&dir.path().join("forge.db")).await.unwrap();
    let clock = Arc::new(ManualClock::new(Timestamp(1_000_000)));
    let placements = Arc::new(FakePlacements::default());
    let rec =
        ObservationRecorder::new(Arc::new(store.clone()), placements.clone(), clock.clone(), RecorderConfig::default());
    Fixture { rec, store, clock, placements, node: ServerId::new(), _dir: dir }
}

fn obs(node: ServerId, state: HealthState, at: i64) -> Observation {
    Observation {
        key: ObservationKey {
            resource: EntityRef::new(EntityKind::Server, node),
            server: None,
            subject: Subject::new("health.readiness.web").unwrap(),
        },
        value: ObservedValue::Health(state),
        reason: None,
        detail: None,
        source: ObservationSource::Agent,
        executor: None,
        generation: Some("g1".into()),
        observed_at: Timestamp(at),
        fresh_for_ms: 30_000,
    }
}

#[tokio::test]
async fn only_changes_make_transitions_and_unchanged_values_are_not_rewritten() {
    let f = fixture().await;
    let r = f.rec.record_from_agent(f.node, vec![obs(f.node, HealthState::Passing, 1_000_000)]).await.unwrap();
    assert_eq!(r.transitions.len(), 1);
    let stored = f.store.latest_for(&[obs(f.node, HealthState::Passing, 0).key]).await.unwrap();
    assert_eq!(stored[0].executor, Some(f.node), "the reporter is the executor");

    // Same value 5 s later: nothing written (freshness still above half).
    f.clock.advance(Duration::from_secs(5));
    let r = f.rec.record_from_agent(f.node, vec![obs(f.node, HealthState::Passing, 1_005_000)]).await.unwrap();
    assert!(r.transitions.is_empty());
    let again = f.store.latest_for(&[obs(f.node, HealthState::Passing, 0).key]).await.unwrap();
    assert_eq!(again[0].received_at, Timestamp(1_000_000), "not rewritten");

    // 20 s later freshness is below half: refreshed, still no transition.
    f.clock.advance(Duration::from_secs(15));
    f.rec.record_from_agent(f.node, vec![obs(f.node, HealthState::Passing, 1_020_000)]).await.unwrap();
    let refreshed = f.store.latest_for(&[obs(f.node, HealthState::Passing, 0).key]).await.unwrap();
    assert_eq!(refreshed[0].received_at, Timestamp(1_020_000));
    assert_eq!(refreshed[0].changed_at, Timestamp(1_000_000), "change time kept");

    let r = f.rec.record_from_agent(f.node, vec![obs(f.node, HealthState::Failing, 1_020_500)]).await.unwrap();
    assert_eq!(r.transitions[0].from, Some(ObservedValue::Health(HealthState::Passing)));
}

#[tokio::test]
async fn a_node_reports_only_about_itself_and_apps_placed_on_it() {
    let f = fixture().await;
    let other = ServerId::new();
    let about = |kind, id: String, server: Option<ServerId>| {
        let mut o = obs(f.node, HealthState::Failing, 1_000_000);
        o.key.resource = EntityRef::new(kind, id);
        o.key.server = server;
        o
    };
    f.placements.0.lock().unwrap().push(("app-here".into(), f.node));
    f.placements.0.lock().unwrap().push(("app-there".into(), other));
    let refused = vec![
        // Another server, also when it names itself as the instance node.
        about(EntityKind::Server, other.to_string(), None),
        about(EntityKind::Server, other.to_string(), Some(f.node)),
        // Itself, but claiming an instance node elsewhere.
        about(EntityKind::Server, f.node.to_string(), Some(other)),
        // An app not placed here (unknown, or placed on another node,
        // e.g. in another workspace), even when it claims this node.
        about(EntityKind::Service, "app-unplaced".into(), Some(f.node)),
        about(EntityKind::Service, "app-there".into(), Some(f.node)),
        about(EntityKind::Service, "app-there".into(), Some(other)),
        // A placed app without naming the node.
        about(EntityKind::Service, "app-here".into(), None),
        // Clusters and every other kind.
        about(EntityKind::Cluster, "c1".into(), Some(f.node)),
        about(EntityKind::Environment, "n1".into(), Some(f.node)),
    ];
    let n = refused.len();
    let r = f.rec.record_from_agent(f.node, refused).await.unwrap();
    assert_eq!((r.refused, r.transitions.len()), (n, 0));
    assert_eq!(f.store.head().await.unwrap(), 0);

    let accepted = vec![
        about(EntityKind::Server, f.node.to_string(), None),
        about(EntityKind::Server, f.node.to_string(), Some(f.node)),
        about(EntityKind::Service, "app-here".into(), Some(f.node)),
    ];
    let r = f.rec.record_from_agent(f.node, accepted).await.unwrap();
    // The two server keys differ only by `server`, so both are stored.
    assert_eq!((r.refused, r.transitions.len()), (0, 3));
}
#[tokio::test]
async fn freshness_uses_the_server_clock_and_the_sweeper_marks_stale_once() {
    let f = fixture().await;
    // The executor's clock is an hour behind; freshness still counts from now.
    let r = f.rec.record_from_agent(f.node, vec![obs(f.node, HealthState::Passing, 1_000_000 - 3_600_000)]).await;
    let stored = f.store.latest_for(&[obs(f.node, HealthState::Passing, 0).key]).await.unwrap();
    assert!(r.is_ok());
    assert!(stored[0].skewed);
    assert_eq!(stored[0].fresh_until, Timestamp(1_030_000));

    assert!(f.rec.sweep_stale().await.unwrap().is_empty(), "still fresh");
    f.clock.advance(Duration::from_secs(31));
    let t = f.rec.sweep_stale().await.unwrap();
    assert_eq!(t.len(), 1);
    assert_eq!(t[0].to, ObservedValue::Health(HealthState::Unknown));
    assert!(f.rec.sweep_stale().await.unwrap().is_empty(), "recorded once");
}

#[tokio::test]
async fn replaying_a_report_after_a_crash_yields_one_transition() {
    let f = fixture().await;
    let report = vec![obs(f.node, HealthState::Failing, 1_000_000)];
    f.rec.record_from_agent(f.node, report.clone()).await.unwrap();
    let r = f.rec.record_from_agent(f.node, report).await.unwrap();
    assert!(r.transitions.is_empty());
    assert_eq!(f.store.head().await.unwrap(), 1);
}

#[tokio::test]
async fn oversized_reports_are_refused_and_details_are_bounded() {
    let f = fixture().await;
    let many = vec![obs(f.node, HealthState::Passing, 1_000_000); 501];
    assert!(f.rec.record_from_agent(f.node, many).await.is_err());

    let d = sanitize_detail(&format!("bad\u{1b}[31m{}", "x".repeat(1_000)));
    assert!(!d.contains('\u{1b}'));
    assert_eq!(d.chars().count(), MAX_DETAIL_CHARS + 1);
}

/// D-085 amendment (#11 slice 2): measurements are written when they move
/// past the deadband, never as feed transitions while they stay known.
#[tokio::test]
async fn measures_use_a_deadband_and_no_feed_transitions() {
    let f = fixture().await;
    let cpu = |milli: Option<i64>, at: i64| Observation {
        key: ObservationKey {
            resource: EntityRef::new(EntityKind::Server, f.node),
            server: None,
            subject: Subject::new("node.cpu.percent").unwrap(),
        },
        value: ObservedValue::Measure { milli, unit: forge_core::observation::Unit::Percent },
        ..obs(f.node, HealthState::Passing, at)
    };
    let key = cpu(None, 0).key;
    let read = || async { f.store.latest_for(std::slice::from_ref(&key)).await.unwrap()[0].clone() };
    assert_eq!(f.rec.record_from_agent(f.node, vec![cpu(Some(40_000), 1_000_000)]).await.unwrap().transitions.len(), 1);
    // Within ±1 %: not rewritten.
    f.clock.advance(Duration::from_secs(2));
    f.rec.record_from_agent(f.node, vec![cpu(Some(40_600), 1_002_000)]).await.unwrap();
    assert_eq!(
        read().await.value,
        ObservedValue::Measure { milli: Some(40_000), unit: forge_core::observation::Unit::Percent }
    );
    // Past the deadband: rewritten, but no transition.
    f.clock.advance(Duration::from_secs(2));
    let r = f.rec.record_from_agent(f.node, vec![cpu(Some(92_000), 1_004_000)]).await.unwrap();
    assert!(r.transitions.is_empty(), "a new value is not a transition");
    assert_eq!(
        read().await.value,
        ObservedValue::Measure { milli: Some(92_000), unit: forge_core::observation::Unit::Percent }
    );
    // Going unknown is a transition.
    f.clock.advance(Duration::from_secs(2));
    let r = f.rec.record_from_agent(f.node, vec![cpu(None, 1_006_000)]).await.unwrap();
    assert_eq!(r.transitions.len(), 1);
}

#[test]
fn deadbands_per_unit() {
    use forge_core::observation::{ObservedValue, Unit};
    let m = |milli: i64, unit: Unit| ObservedValue::Measure { milli: Some(milli), unit };
    assert!(!beyond_deadband(m(50_000, Unit::Percent), m(50_999, Unit::Percent)));
    assert!(beyond_deadband(m(50_000, Unit::Percent), m(51_000, Unit::Percent)));
    assert!(!beyond_deadband(m(1_000_000, Unit::Bytes), m(1_009_000, Unit::Bytes)));
    assert!(beyond_deadband(m(1_000_000, Unit::Bytes), m(1_010_000, Unit::Bytes)));
    assert!(beyond_deadband(m(0, Unit::Count), m(1, Unit::Count)), "at least one step");
    assert!(!beyond_deadband(m(1, Unit::Percent), m(1, Unit::Bytes)), "different units never compare");
}

#[tokio::test]
async fn a_node_going_offline_makes_its_values_unknown_at_once() {
    let f = fixture().await;
    f.placements.0.lock().unwrap().push(("app-here".into(), f.node));
    let mut inst = obs(f.node, HealthState::Passing, 1_000_000);
    inst.key.resource = EntityRef::new(EntityKind::Service, "app-here");
    inst.key.server = Some(f.node);
    f.rec.record_from_agent(f.node, vec![obs(f.node, HealthState::Passing, 1_000_000), inst]).await.unwrap();

    let t = f.rec.expire_node(f.node).await.unwrap();
    assert_eq!(t.len(), 2, "the node and its instance");
    assert!(t.iter().all(|t| t.to.is_unknown() && t.reason == Some(ReasonCode::NodeOffline)), "{t:?}");
    let now = f.store.latest_for(&[obs(f.node, HealthState::Passing, 0).key]).await.unwrap();
    assert_eq!(now[0].value, ObservedValue::Health(HealthState::Unknown));
    assert!(f.rec.expire_node(f.node).await.unwrap().is_empty(), "already unknown");
    assert!(f.rec.sweep_stale().await.unwrap().is_empty(), "the stale sweep adds nothing");
    // Coming back: the next report transitions from unknown.
    f.clock.advance(Duration::from_secs(5));
    let r = f.rec.record_from_agent(f.node, vec![obs(f.node, HealthState::Passing, 1_005_000)]).await.unwrap();
    assert_eq!(r.transitions[0].from, Some(ObservedValue::Health(HealthState::Unknown)));
}
#[test]
fn instance_status_follows_the_rule() {
    use forge_core::observation::StartupState;
    let h = |s: HealthState| CheckValue { value: ObservedValue::Health(s), required: true };
    let opt = |s: HealthState| CheckValue { value: ObservedValue::Health(s), required: false };
    let st = |s: StartupState| CheckValue { value: ObservedValue::Startup(s), required: true };
    use HealthState::*;
    assert_eq!(instance_status(&[], &[h(Passing)], &[h(Passing)]), InstanceStatus::Healthy);
    assert_eq!(instance_status(&[], &[], &[h(Failing)]), InstanceStatus::Unhealthy);
    assert_eq!(instance_status(&[], &[h(Failing)], &[h(Passing)]), InstanceStatus::Unhealthy);
    assert_eq!(instance_status(&[], &[h(Passing), opt(Failing)], &[h(Passing)]), InstanceStatus::Degraded);
    assert_eq!(instance_status(&[], &[], &[h(Unknown)]), InstanceStatus::Unknown);
    assert_eq!(instance_status(&[], &[], &[]), InstanceStatus::Unknown, "nothing reported yet");
    assert_eq!(instance_status(&[], &[h(Pending)], &[h(Passing)]), InstanceStatus::Unknown);
    assert_eq!(instance_status(&[st(StartupState::Waiting)], &[], &[h(Passing)]), InstanceStatus::Unknown);
    assert_eq!(instance_status(&[st(StartupState::Exhausted)], &[], &[h(Passing)]), InstanceStatus::Unhealthy);
    assert_eq!(instance_status(&[st(StartupState::Succeeded)], &[], &[h(Passing)]), InstanceStatus::Healthy);
}
