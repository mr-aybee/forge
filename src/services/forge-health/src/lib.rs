//! Observations service (D-085).
//!
//! [`ObservationRecorder`] turns reported observations into stored current
//! values, history and transitions. It owns the rules the store must not:
//! who may report what, input bounds, clock-skew clamping, freshness on the
//! server clock, change detection, and how often rows are written.
//!
//! Write amplification is bounded: a current value is rewritten only when it
//! changes or its freshness runs below half, and history gets a row on every
//! change plus one keep-alive row per [`RecorderConfig::history_keepalive_ms`].

use std::collections::HashMap;
use std::sync::Arc;

use forge_core::observation::*;
use forge_core::{Clock, DomainError, DomainResult, EntityKind, ServerId};

/// Bounds and write policy.
#[derive(Debug, Clone)]
pub struct RecorderConfig {
    /// Most observations accepted in one report.
    pub max_batch: usize,
    /// Shortest and longest accepted freshness.
    pub min_fresh_ms: u64,
    pub max_fresh_ms: u64,
    /// An unchanged value still gets a history row this often.
    pub history_keepalive_ms: i64,
    /// Stale values handled per sweep.
    pub sweep_limit: u32,
}

impl Default for RecorderConfig {
    fn default() -> Self {
        Self {
            max_batch: 500,
            min_fresh_ms: 1_000,
            max_fresh_ms: 3_600_000,
            history_keepalive_ms: 300_000,
            sweep_limit: 500,
        }
    }
}

/// What one report did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RecordReport {
    pub transitions: Vec<Transition>,
    /// Observations dropped as older than the stored value.
    pub outdated: usize,
    /// Observations refused (wrong node).
    pub refused: usize,
}

#[derive(Debug, Clone)]
pub struct ObservationRecorder {
    store: Arc<dyn ObservationStore>,
    placements: Arc<dyn InstancePlacements>,
    clock: Arc<dyn Clock>,
    config: RecorderConfig,
}

impl ObservationRecorder {
    pub fn new(
        store: Arc<dyn ObservationStore>,
        placements: Arc<dyn InstancePlacements>,
        clock: Arc<dyn Clock>,
        config: RecorderConfig,
    ) -> Self {
        Self { store, placements, clock, config }
    }

    /// Whether node `reporter` may report this observation:
    /// - about itself: `server:<reporter>`, with no instance node or itself;
    /// - about a service instance on itself: `service:<id>` with
    ///   `server == reporter`, and an instance of that app placed there.
    ///
    /// Everything else (other servers, clusters, unplaced apps, other kinds)
    /// is refused.
    async fn may_report(
        &self,
        reporter: ServerId,
        key: &ObservationKey,
        placed: &mut HashMap<String, bool>,
    ) -> DomainResult<bool> {
        match key.resource.kind {
            EntityKind::Server => {
                Ok(key.resource.id == reporter.to_string() && key.server.is_none_or(|s| s == reporter))
            }
            EntityKind::Service if key.server == Some(reporter) => {
                // One lookup per service per report.
                if let Some(&ok) = placed.get(&key.resource.id) {
                    return Ok(ok);
                }
                let ok = self.placements.is_placed_on(&key.resource.id, reporter).await?;
                placed.insert(key.resource.id.clone(), ok);
                Ok(ok)
            }
            _ => Ok(false),
        }
    }

    /// Records observations an agent reported. `reporter` is the
    /// authenticated node; it is recorded as the executor. Observations it
    /// may not make ([`Self::may_report`]) are dropped and counted.
    pub async fn record_from_agent(
        &self,
        reporter: ServerId,
        observations: Vec<Observation>,
    ) -> DomainResult<RecordReport> {
        if observations.len() > self.config.max_batch {
            return Err(DomainError::Invalid(format!(
                "too many observations in one report ({}; at most {})",
                observations.len(),
                self.config.max_batch
            )));
        }
        let mut refused = 0;
        let mut own = Vec::with_capacity(observations.len());
        let mut placed = HashMap::new();
        for mut o in observations {
            if self.may_report(reporter, &o.key, &mut placed).await? {
                o.executor = Some(reporter);
                own.push(o);
            } else {
                refused += 1;
            }
        }
        if refused > 0 {
            tracing::warn!(%reporter, refused, "dropped observations the node may not report");
        }
        let mut report = self.record(own).await?;
        report.refused = refused;
        Ok(report)
    }
    /// Records observations from a trusted in-process source.
    pub async fn record(&self, observations: Vec<Observation>) -> DomainResult<RecordReport> {
        if observations.len() > self.config.max_batch {
            return Err(DomainError::Invalid(format!(
                "too many observations in one report ({}; at most {})",
                observations.len(),
                self.config.max_batch
            )));
        }
        let now = self.clock.now();
        // The last observation of a key in a report wins.
        let mut by_key: HashMap<ObservationKey, Observation> = HashMap::new();
        for o in observations {
            by_key.insert(o.key.clone(), o);
        }
        let keys: Vec<ObservationKey> = by_key.keys().cloned().collect();
        let prev: HashMap<ObservationKey, Latest> =
            self.store.latest_for(&keys).await?.into_iter().map(|l| (l.key.clone(), l)).collect();

        let mut batch = ObservationBatch::default();
        let mut outdated = 0;
        for (key, o) in by_key {
            let prev = prev.get(&key);
            let (observed_at, skewed) = clamp_observed_at(o.observed_at, now);
            let fresh_ms = o.fresh_for_ms.clamp(self.config.min_fresh_ms, self.config.max_fresh_ms);
            let transition = match decide(prev, &o, observed_at, now) {
                Outcome::Outdated => {
                    outdated += 1;
                    continue;
                }
                Outcome::Apply { transition } => transition,
            };
            let fresh_until = now.plus(std::time::Duration::from_millis(fresh_ms));
            let changed = transition.is_some();
            // A measurement that moved past its deadband is new evidence, though
            // not a feed transition (D-085 amendment).
            let moved = prev.is_some_and(|p| beyond_deadband(p.value, o.value));
            let needs_refresh = prev.is_none_or(|p| {
                let half = i64::try_from(fresh_ms / 2).unwrap_or(i64::MAX);
                p.fresh_until.0.saturating_sub(now.0) < half || p.reason != o.reason || p.detail != o.detail
            });
            if !changed && !moved && !needs_refresh {
                continue;
            }
            let row = Latest {
                key,
                value: o.value,
                reason: o.reason,
                detail: o.detail.as_deref().map(sanitize_detail),
                source: o.source,
                executor: o.executor,
                generation: o.generation,
                observed_at,
                received_at: now,
                fresh_until,
                skewed,
                changed_at: if changed { now } else { prev.map_or(now, |p| p.changed_at) },
            };
            let keepalive_due = prev.is_none_or(|p| {
                let k = self.config.history_keepalive_ms.max(1);
                now.0 / k != p.received_at.0 / k
            });
            // History: on transitions and keep-alives only. A measurement that
            // moved updates the current value, not history (about 100 rows/s
            // at 1,000 nodes otherwise; review of #34).
            if changed || keepalive_due {
                batch.history.push(row.clone());
            }
            batch.latest.push(row);
            batch.transitions.extend(transition);
        }
        let transitions = self.store.apply(&batch).await?;
        Ok(RecordReport { transitions, outdated, refused: 0 })
    }

    /// Marks every current value about node `server` (and instances on it)
    /// unknown at once, with reason `node_offline`: called when the node goes
    /// offline, so nothing keeps showing its last state until freshness runs
    /// out. Values already unknown are left alone. Returns the transitions.
    pub async fn expire_node(&self, server: ServerId) -> DomainResult<Vec<Transition>> {
        let now = self.clock.now();
        let live = self.store.on_server(server, self.config.sweep_limit).await?;
        let mut batch = ObservationBatch::default();
        for l in live.into_iter().filter(|l| !l.value.is_unknown()) {
            let to = l.value.unknown();
            batch.transitions.push(Transition {
                seq: 0,
                key: l.key.clone(),
                from: Some(l.value),
                to,
                reason: Some(ReasonCode::NodeOffline),
                generation: l.generation.clone(),
                at: now,
                evidence_at: now,
            });
            let row = Latest {
                value: to,
                reason: Some(ReasonCode::NodeOffline),
                detail: Some("the node went offline".into()),
                received_at: now,
                fresh_until: now,
                changed_at: now,
                ..l
            };
            batch.history.push(row.clone());
            batch.latest.push(row);
        }
        if batch.is_empty() {
            return Ok(Vec::new());
        }
        // The rows are unknown already, so the stale sweep has nothing to add.
        batch.stale_noted = batch.latest.iter().map(|l| l.key.clone()).collect();
        self.store.apply(&batch).await
    }

    /// Records a stale→unknown transition for every value whose freshness
    /// ran out. Run periodically; returns the transitions stored.
    pub async fn sweep_stale(&self) -> DomainResult<Vec<Transition>> {
        let now = self.clock.now();
        let stale = self.store.stale_since(now, self.config.sweep_limit).await?;
        if stale.is_empty() {
            return Ok(Vec::new());
        }
        let mut batch = ObservationBatch::default();
        for l in &stale {
            batch.transitions.extend(stale_transition(l, now));
            batch.stale_noted.push(l.key.clone());
        }
        self.store.apply(&batch).await
    }
}

/// The overall health of one instance (service on one node).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InstanceStatus {
    /// Started, every required readiness check passes and liveness passes.
    Healthy,
    /// An optional readiness check fails; everything required passes.
    Degraded,
    /// A required check fails, or startup ran out of budget.
    Unhealthy,
    /// No current evidence (stale, node offline, not reported yet).
    Unknown,
}

/// One check's current value as the status rule sees it.
#[derive(Debug, Clone, Copy)]
pub struct CheckValue {
    pub value: ObservedValue,
    /// Readiness only: a failing optional check degrades, not fails.
    pub required: bool,
}

/// The status rule (D-085): pure, so the API and later the ingress can share
/// it. `checks` are effective values (stale already reads unknown).
#[must_use]
pub fn instance_status(startup: &[CheckValue], readiness: &[CheckValue], liveness: &[CheckValue]) -> InstanceStatus {
    use forge_core::observation::{HealthState as H, StartupState as S};
    let state = |c: &CheckValue| match c.value {
        ObservedValue::Health(h) => Some(h),
        _ => None,
    };
    if startup.iter().any(|c| c.value == ObservedValue::Startup(S::Exhausted))
        || liveness.iter().chain(readiness.iter().filter(|c| c.required)).any(|c| state(c) == Some(H::Failing))
    {
        return InstanceStatus::Unhealthy;
    }
    let started = startup.iter().all(|c| c.value == ObservedValue::Startup(S::Succeeded));
    let all_pass = |list: &[CheckValue]| {
        list.iter().filter(|c| c.required).all(|c| matches!(state(c), Some(H::Passing | H::Disabled)))
    };
    if liveness.is_empty() || !started || !all_pass(liveness) || !all_pass(readiness) {
        return InstanceStatus::Unknown;
    }
    if readiness.iter().any(|c| !c.required && state(c) == Some(H::Failing)) {
        return InstanceStatus::Degraded;
    }
    InstanceStatus::Healthy
}

/// Drops control characters and bounds the length of diagnostic text.
#[must_use]
pub fn sanitize_detail(s: &str) -> String {
    let mut out: String =
        s.chars().map(|c| if c.is_control() && c != '\n' { ' ' } else { c }).take(MAX_DETAIL_CHARS).collect();
    if s.chars().count() > MAX_DETAIL_CHARS {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests;

/// Whether a measurement moved enough to be written: ±1.0 for percentages,
/// ±1 % of the previous value (at least one unit step) otherwise. Values of
/// other kinds never use a deadband.
#[must_use]
pub fn beyond_deadband(
    prev: forge_core::observation::ObservedValue,
    next: forge_core::observation::ObservedValue,
) -> bool {
    use forge_core::observation::{ObservedValue, Unit};
    match (prev, next) {
        (ObservedValue::Measure { milli: Some(a), unit: u1 }, ObservedValue::Measure { milli: Some(b), unit: u2 })
            if u1 == u2 =>
        {
            let band = match u1 {
                Unit::Percent => 1_000,
                _ => (a.saturating_abs() / 100).max(1),
            };
            a.saturating_sub(b).saturating_abs() >= band
        }
        _ => false,
    }
}
