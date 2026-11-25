//! Observations: timestamped evidence about a resource (D-085).
//!
//! An observation says "this check on this resource was in this state at this
//! time". Health probes (#12) produce them; alerts (#11), automation (#8) and
//! observability (#14) consume the ordered [`Transition`] feed instead of
//! re-running checks.
//!
//! Rules:
//! - Freshness is measured on the server clock (`received_at`), never on the
//!   executor's clock. A stale value reads as [`HealthState::Unknown`]; the
//!   last known state is kept separately.
//! - Unknown is not evidence of failure (or of recovery).
//! - One observation yields at most one transition, however many consumers
//!   read the feed; consumers keep their own cursors.
//!
//! This module is pure: types, rules and ports. No I/O.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::fmt;

use crate::access::EntityRef;
use crate::clock::Timestamp;
use crate::error::{DomainError, DomainResult};
use crate::ids::ServerId;

/// Longest accepted subject, in bytes.
pub const MAX_SUBJECT_LEN: usize = 128;
/// Longest stored detail text, in characters; longer text is cut.
pub const MAX_DETAIL_CHARS: usize = 512;
/// How far an executor's clock may be from the server's before the
/// observation time is clamped and the row flagged as skewed.
pub const MAX_CLOCK_SKEW_MS: i64 = 30_000;

/// What was observed, as a lowercase dotted name such as
/// `health.readiness.accept-requests`. Segments use `a-z`, `0-9`, `-`, `_`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Subject(String);

impl Subject {
    /// Validates and wraps a subject.
    pub fn new(s: impl Into<String>) -> DomainResult<Self> {
        let s = s.into();
        let ok_segment = |seg: &str| {
            !seg.is_empty()
                && seg.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
        };

        if s.is_empty() || s.len() > MAX_SUBJECT_LEN || !s.split('.').all(ok_segment) {
            return Err(DomainError::Invalid(format!(
                "invalid observation subject `{}`: use lowercase dotted segments (a-z, 0-9, -, _), at most {MAX_SUBJECT_LEN} bytes",
                s.chars().take(MAX_SUBJECT_LEN).collect::<String>()
            )));
        }

        Ok(Self(s))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Subject {
    type Error = DomainError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::new(s)
    }
}

impl From<Subject> for String {
    fn from(s: Subject) -> Self {
        s.0
    }
}

impl fmt::Display for Subject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Identifies one observed thing: a resource, optionally the node it lives on
/// (a service instance is a service on one server), and a subject.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ObservationKey {
    pub resource: EntityRef,
    /// The node the observed instance lives on, when the resource spans nodes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<ServerId>,
    pub subject: Subject,
}

/// State of a readiness or liveness check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthState {
    /// Configured; no completed outcome yet (or gated by startup).
    Pending,
    Passing,
    Failing,
    /// No current evidence: stale, executor unreachable, or unsupported.
    Unknown,
    Disabled,
}

/// State of a startup check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartupState {
    Waiting,
    Succeeded,
    /// The failure budget ran out before the instance started.
    Exhausted,
    Unknown,
}

/// The observed value. Only states in v1; measurements come later as new
/// variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", content = "state", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ObservedValue {
    Health(HealthState),
    Startup(StartupState),
    /// A measurement in thousandths of `unit` (`85.5 %` is `85500`); `None`
    /// is the unknown form (D-085 amendment, #11 slice 2).
    Measure {
        milli: Option<i64>,
        unit: Unit,
    },
}

/// The unit of a measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Unit {
    Percent,
    Bytes,
    Seconds,
    Count,
    /// A plain ratio, such as load per CPU (D-118).
    Ratio,
}

/// Subject families: probes (`health.<kind>.<check>` on a service
/// instance), node measurements (`node.<metric>.<unit>`) and control-plane
/// states (`app.`, `package.`, `tls.`; D-118).
pub const KNOWN_SUBJECT_PREFIXES: &[&str] = &["health.", "node.", "app.", "package.", "tls."];

impl ObservedValue {
    /// The same kind of value, meaning "no current evidence".
    #[must_use]
    pub const fn unknown(self) -> Self {
        match self {
            ObservedValue::Health(_) => ObservedValue::Health(HealthState::Unknown),
            ObservedValue::Startup(_) => ObservedValue::Startup(StartupState::Unknown),
            ObservedValue::Measure { unit, .. } => ObservedValue::Measure { milli: None, unit },
        }
    }

    #[must_use]
    pub const fn is_unknown(self) -> bool {
        matches!(
            self,
            ObservedValue::Health(HealthState::Unknown)
                | ObservedValue::Startup(StartupState::Unknown)
                | ObservedValue::Measure { milli: None, .. }
        )
    }
}

/// Why a check did not pass. Lets the UI tell a service failure from a
/// broken check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ReasonCode {
    /// The service answered, but not as expected.
    AppFailure,
    Timeout,
    /// Connection refused or reset.
    ConnectFailed,
    Tls,
    Auth,
    InvalidConfig,
    /// This executor can't run the check (missing interpreter, OS).
    UnsupportedExecutor,
    /// The execution location is unreachable.
    LocationUnavailable,
    PermissionDenied,
    /// No fresh observation arrived in time.
    Stale,
    /// The node running the check went offline.
    NodeOffline,
    /// Several apps on the node declare the same port, so it isn't probed.
    AmbiguousPort,
}

/// Who produced the observation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ObservationSource {
    /// A configured probe: its policy and check name.
    Probe {
        policy: String,
        check: String,
    },
    /// The runtime that owns the workload (`docker`, `systemd`).
    Runtime {
        runtime: String,
    },
    Agent,
    ControlPlane,
}

/// One observation as reported, before the server stores it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Observation {
    pub key: ObservationKey,
    pub value: ObservedValue,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<ReasonCode>,
    /// Sanitized, bounded diagnostic text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub source: ObservationSource,
    /// The node that ran the check (may differ from `key.server`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executor: Option<ServerId>,
    /// Workload instance generation (container id, unit invocation id). A
    /// new generation never inherits the old one's state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<String>,
    /// Executor wall clock (ordering within a generation only).
    pub observed_at: Timestamp,
    /// How long this value counts as current.
    pub fresh_for_ms: u64,
}

/// The stored current value of one key.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Latest {
    pub key: ObservationKey,
    pub value: ObservedValue,
    pub reason: Option<ReasonCode>,
    pub detail: Option<String>,
    pub source: ObservationSource,
    pub executor: Option<ServerId>,
    pub generation: Option<String>,
    /// Observation time after skew clamping.
    pub observed_at: Timestamp,
    /// Server time the value arrived.
    pub received_at: Timestamp,
    /// `received_at + fresh_for_ms`.
    pub fresh_until: Timestamp,
    /// The executor's clock was off by more than [`MAX_CLOCK_SKEW_MS`].
    pub skewed: bool,
    /// When the effective value last changed.
    pub changed_at: Timestamp,
}

impl Latest {
    #[must_use]
    pub fn is_fresh(&self, now: Timestamp) -> bool {
        now < self.fresh_until
    }

    /// The value to act on now: the stored value while fresh, unknown after.
    #[must_use]
    pub fn effective(&self, now: Timestamp) -> ObservedValue {
        if self.is_fresh(now) { self.value } else { self.value.unknown() }
    }
}

/// A change of a key's effective value, in commit order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Transition {
    /// Feed position, increasing. `0` until stored.
    pub seq: i64,
    pub key: ObservationKey,
    /// `None` for a key's first observation.
    pub from: Option<ObservedValue>,
    pub to: ObservedValue,
    pub reason: Option<ReasonCode>,
    pub generation: Option<String>,
    /// Server time the transition was recorded.
    pub at: Timestamp,
    /// The evidence time that caused it: the observation time, or the end of
    /// freshness for a stale value. With the key, generation and values it
    /// identifies the transition, so a replay stores it once.
    pub evidence_at: Timestamp,
}

/// Clamps an executor timestamp to the server clock: returns the time to
/// store and whether the clock was skewed.
#[must_use]
pub fn clamp_observed_at(observed_at: Timestamp, received_at: Timestamp) -> (Timestamp, bool) {
    let lo = received_at.0.saturating_sub(MAX_CLOCK_SKEW_MS);
    let hi = received_at.0.saturating_add(MAX_CLOCK_SKEW_MS);
    if observed_at.0 < lo || observed_at.0 > hi {
        (Timestamp(observed_at.0.clamp(lo, hi)), true)
    } else {
        (observed_at, false)
    }
}

/// What applying `next` to the current `prev` means.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// Older than what is stored for the same generation: drop it.
    Outdated,
    /// Store it. `transition` is set when the effective value changed.
    Apply { transition: Option<Transition> },
}

/// Decides what a new observation does, given the stored value (pure).
/// A different generation always applies; within a generation an older
/// `observed_at` is outdated.
#[must_use]
pub fn decide(prev: Option<&Latest>, next: &Observation, observed_at: Timestamp, now: Timestamp) -> Outcome {
    let transition = |from: Option<ObservedValue>| Transition {
        seq: 0,
        key: next.key.clone(),
        from,
        to: next.value,
        reason: next.reason,
        generation: next.generation.clone(),
        at: now,
        evidence_at: observed_at,
    };

    let Some(prev) = prev else {
        return Outcome::Apply { transition: Some(transition(None)) };
    };

    let same_generation = prev.generation == next.generation;
    if same_generation && observed_at < prev.observed_at {
        return Outcome::Outdated;
    }

    let before = prev.effective(now);

    // A measurement changes on almost every sample: only going unknown or
    // coming back is a transition, or the feed would flood (D-085 amendment).
    let changed = match (before, next.value) {
        (ObservedValue::Measure { milli: a, unit: u1 }, ObservedValue::Measure { milli: b, unit: u2 }) => {
            !same_generation || u1 != u2 || a.is_some() != b.is_some()
        }
        _ => before != next.value || !same_generation,
    };

    Outcome::Apply { transition: changed.then(|| transition(Some(before))) }
}

/// The transition for a value that went stale (pure): `None` if it is still
/// fresh or already unknown.
#[must_use]
pub fn stale_transition(prev: &Latest, now: Timestamp) -> Option<Transition> {
    if prev.is_fresh(now) || prev.value.is_unknown() {
        return None;
    }

    Some(Transition {
        seq: 0,
        key: prev.key.clone(),
        from: Some(prev.value),
        to: prev.value.unknown(),
        reason: Some(ReasonCode::Stale),
        generation: prev.generation.clone(),
        at: now,
        evidence_at: prev.fresh_until,
    })
}

/// Which current values to list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ObservationFilter {
    pub resource: Option<EntityRef>,
    pub server: Option<ServerId>,
    /// Subjects starting with this prefix (e.g. `health.`).
    pub subject_prefix: Option<String>,
}

/// One stored write: the new current values, history rows and transitions,
/// applied in a single transaction.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ObservationBatch {
    pub latest: Vec<Latest>,
    /// History rows (written on change, plus periodic keep-alive rows).
    pub history: Vec<Latest>,
    pub transitions: Vec<Transition>,
    /// Keys whose stale transition is recorded (the sweeper won't repeat it).
    pub stale_noted: Vec<ObservationKey>,
}

impl ObservationBatch {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.latest.is_empty() && self.history.is_empty() && self.transitions.is_empty() && self.stale_noted.is_empty()
    }
}

/// Write side, used only by the observation service.
#[async_trait]
pub trait ObservationStore: Send + Sync + std::fmt::Debug {
    /// Current values of these keys (missing keys are absent).
    async fn latest_for(&self, keys: &[ObservationKey]) -> DomainResult<Vec<Latest>>;
    /// Current values that went stale before `now` and are not unknown yet.
    async fn stale_since(&self, now: Timestamp, limit: u32) -> DomainResult<Vec<Latest>>;
    /// Current values about node `server` or instances on it (to mark them
    /// unknown when the node goes offline).
    async fn on_server(&self, server: ServerId, limit: u32) -> DomainResult<Vec<Latest>>;
    /// Applies the batch atomically. Replaying the same batch adds no
    /// duplicate history or transitions. Returns the stored transitions with
    /// their `seq`.
    async fn apply(&self, batch: &ObservationBatch) -> DomainResult<Vec<Transition>>;
}

/// Read side.
#[async_trait]
pub trait ObservationQuery: Send + Sync + std::fmt::Debug {
    async fn latest(&self, filter: &ObservationFilter, limit: u32) -> DomainResult<Vec<Latest>>;
    /// History of one key since `since`, newest first.
    async fn history(&self, key: &ObservationKey, since: Timestamp, limit: u32) -> DomainResult<Vec<Latest>>;
}

/// The ordered transition feed. Each consumer stores its own cursor.
#[async_trait]
pub trait TransitionFeed: Send + Sync + std::fmt::Debug {
    /// Transitions with `seq > after_seq`, oldest first.
    async fn since(&self, after_seq: i64, limit: u32) -> DomainResult<Vec<Transition>>;
    /// The highest stored `seq` (`0` when empty).
    async fn head(&self) -> DomainResult<i64>;
}

/// Which service instances run on which node. The reporting rule uses
/// it: a node may report about a service only where an instance of it
/// is placed on that node.
#[async_trait]
pub trait InstancePlacements: Send + Sync + std::fmt::Debug {
    /// Whether service `app` (its id as text) has an instance on `server`.
    async fn is_placed_on(&self, app: &str, server: ServerId) -> DomainResult<bool>;
}

/// Housekeeping, kept apart so readers can't delete.
#[async_trait]
pub trait ObservationRetention: Send + Sync + std::fmt::Debug {
    /// Deletes history older than `history_before` and transitions with
    /// `seq < transitions_before_seq`. Returns rows removed.
    async fn sweep(&self, history_before: Timestamp, transitions_before_seq: i64) -> DomainResult<u64>;
    /// Deletes current values and history of servers and services that
    /// no longer exist. Transitions stay (they are the record of what
    /// happened). Returns rows removed.
    async fn delete_orphans(&self) -> DomainResult<u64>;
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::access::EntityKind;

    fn key() -> ObservationKey {
        ObservationKey {
            resource: EntityRef::new(EntityKind::Server, "s1"),
            server: None,
            subject: Subject::new("health.readiness.web").unwrap(),
        }
    }

    fn obs(value: HealthState, at: i64, generation: &str) -> Observation {
        Observation {
            key: key(),
            value: ObservedValue::Health(value),
            reason: None,
            detail: None,
            source: ObservationSource::Agent,
            executor: None,
            generation: Some(generation.into()),
            observed_at: Timestamp(at),
            fresh_for_ms: 10_000,
        }
    }

    fn latest(o: &Observation, received: i64) -> Latest {
        Latest {
            key: o.key.clone(),
            value: o.value,
            reason: o.reason,
            detail: None,
            source: o.source.clone(),
            executor: None,
            generation: o.generation.clone(),
            observed_at: o.observed_at,
            received_at: Timestamp(received),
            fresh_until: Timestamp(received + i64::try_from(o.fresh_for_ms).unwrap()),
            skewed: false,
            changed_at: Timestamp(received),
        }
    }

    /// D-085 amendment (#11 slice 2): a measurement changing is not a
    /// feed transition; going unknown or coming back is.
    #[test]
    fn measures_change_without_flooding_the_feed() {
        let m = |milli: Option<i64>, at: i64| Observation {
            value: ObservedValue::Measure { milli, unit: Unit::Percent },
            ..obs(HealthState::Passing, at, "g1")
        };
        let first = m(Some(40_000), 1_000);
        assert!(matches!(
            decide(None, &first, first.observed_at, Timestamp(1_000)),
            Outcome::Apply { transition: Some(_) }
        ));
        let prev = latest(&first, 1_000);
        let next = m(Some(95_000), 2_000);
        assert!(
            matches!(
                decide(Some(&prev), &next, next.observed_at, Timestamp(2_000)),
                Outcome::Apply { transition: None }
            ),
            "40 % to 95 % is a new value, not a transition"
        );
        let gone = m(None, 3_000);
        assert!(matches!(
            decide(Some(&prev), &gone, gone.observed_at, Timestamp(3_000)),
            Outcome::Apply { transition: Some(_) }
        ));
        let back = m(Some(50_000), 4_000);
        assert!(matches!(
            decide(Some(&latest(&gone, 3_000)), &back, back.observed_at, Timestamp(4_000)),
            Outcome::Apply { transition: Some(_) }
        ));

        // A stale measurement reads as unknown, which then comes back as a transition.
        assert!(matches!(
            decide(Some(&prev), &next, next.observed_at, Timestamp(60_000)),
            Outcome::Apply { transition: Some(_) }
        ));
        assert!(ObservedValue::Measure { milli: Some(1), unit: Unit::Bytes }.unknown().is_unknown());
        let json = serde_json::to_value(ObservedValue::Measure { milli: Some(85_500), unit: Unit::Percent }).unwrap();
        assert_eq!(json, serde_json::json!({"type": "measure", "state": {"milli": 85_500, "unit": "percent"}}));
    }

    #[test]
    fn subjects_are_validated() {
        assert!(Subject::new("health.liveness.db-ping_1").is_ok());

        for bad in ["", "Health.x", "a..b", ".a", "a.", "a b", "a/b", &"x".repeat(129)] {
            assert!(Subject::new(bad).is_err(), "{bad:?}");
        }

        let json = serde_json::to_string(&Subject::new("a.b").unwrap()).unwrap();
        assert_eq!(json, "\"a.b\"");
        assert!(serde_json::from_str::<Subject>("\"A\"").is_err());
    }

    #[test]
    fn the_first_observation_is_a_transition_from_nothing() {
        let o = obs(HealthState::Passing, 1_000, "g1");

        let Outcome::Apply { transition: Some(t) } = decide(None, &o, o.observed_at, Timestamp(1_000)) else {
            panic!("expected a transition")
        };

        assert_eq!((t.from, t.to), (None, ObservedValue::Health(HealthState::Passing)));
    }

    #[test]
    fn the_same_value_again_is_no_transition() {
        let a = obs(HealthState::Passing, 1_000, "g1");
        let b = obs(HealthState::Passing, 2_000, "g1");
        let prev = latest(&a, 1_000);
        assert_eq!(decide(Some(&prev), &b, b.observed_at, Timestamp(2_000)), Outcome::Apply { transition: None });
    }

    #[test]
    fn an_older_observation_of_the_same_generation_is_dropped() {
        let a = obs(HealthState::Passing, 5_000, "g1");
        let b = obs(HealthState::Failing, 4_000, "g1");
        assert_eq!(decide(Some(&latest(&a, 5_000)), &b, b.observed_at, Timestamp(6_000)), Outcome::Outdated);
    }

    #[test]
    fn a_new_generation_always_transitions_even_with_the_same_value() {
        let a = obs(HealthState::Passing, 5_000, "g1");
        let b = obs(HealthState::Passing, 1_000, "g2");
        let out = decide(Some(&latest(&a, 5_000)), &b, b.observed_at, Timestamp(6_000));
        assert!(matches!(out, Outcome::Apply { transition: Some(_) }), "{out:?}");
    }

    #[test]
    fn a_stale_value_reads_unknown_and_recovery_comes_from_unknown() {
        let a = obs(HealthState::Failing, 1_000, "g1");
        let prev = latest(&a, 1_000);
        assert_eq!(prev.effective(Timestamp(10_999)), ObservedValue::Health(HealthState::Failing));
        assert_eq!(prev.effective(Timestamp(11_000)), ObservedValue::Health(HealthState::Unknown));
        let t = stale_transition(&prev, Timestamp(11_000)).unwrap();
        assert_eq!(t.reason, Some(ReasonCode::Stale));
        assert!(stale_transition(&prev, Timestamp(10_000)).is_none());

        // A later pass after going stale: the transition starts from unknown,
        // not from the old failure.
        let b = obs(HealthState::Passing, 20_000, "g1");

        let Outcome::Apply { transition: Some(t) } = decide(Some(&prev), &b, b.observed_at, Timestamp(20_000)) else {
            panic!()
        };

        assert_eq!(t.from, Some(ObservedValue::Health(HealthState::Unknown)));
    }

    #[test]
    fn executor_clock_skew_is_clamped_and_flagged() {
        let now = Timestamp(100_000);
        assert_eq!(clamp_observed_at(Timestamp(99_000), now), (Timestamp(99_000), false));
        assert_eq!(clamp_observed_at(Timestamp(500_000), now), (Timestamp(130_000), true));
        assert_eq!(clamp_observed_at(Timestamp(0), now), (Timestamp(70_000), true));
    }

    #[test]
    fn values_serialize_with_a_type_tag() {
        let v = ObservedValue::Startup(StartupState::Exhausted);
        assert_eq!(serde_json::to_string(&v).unwrap(), r#"{"type":"startup","state":"exhausted"}"#);
    }
}
