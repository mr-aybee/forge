//! Changes analytics settings at runtime and reports what took effect
//! (D-136). The API handler authorizes and audits; this type validates,
//! asks for confirmation where a change can lose data, stores the
//! overrides, publishes the new values and has the analytics worker apply
//! them (D-141), which reports per setting what took effect.
//!
//! CPU and memory settings apply to the embedded engine only; for an
//! external ClickHouse (or analytics off) a change to them is refused with
//! [`UpdateError::NotManaged`]. Removing an override is allowed.
//!
//! The stored overrides are written before anything is applied: they are
//! the administrator's intent, and a setting that could not be applied now
//! (worker or database down) is applied when the worker next starts, since
//! it starts with the effective settings.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use forge_analytics_proto::views::{ApplyStatus, QueueStats, SettingResult, Usage, WorkerReport};
use forge_core::analytics::Retention;
use forge_core::ports::SettingsRepository;
use forge_core::{Clock, DomainError};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use crate::analytics_tuning::{self as tuning, Group, Invalid, Overrides, Patch, Setting, Tuning};
use crate::analytics_worker::{WorkerClient, WorkerState, WorkerSupervisor};
use crate::config::AnalyticsEngine;

/// How long the settings page waits for the worker's report.
const REPORT_WAIT: Duration = Duration::from_secs(5);

/// The result of one change.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct ApplyReport {
    /// Epoch ms.
    pub at: i64,
    pub results: Vec<SettingResult>,
}

/// One setting's old and new effective value, for the audit log.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct Change {
    pub setting: Setting,
    pub old: f64,
    pub new: f64,
    /// The new override (`null`: back to `forge.toml`).
    pub override_value: Option<f64>,
}

/// What a change needs the administrator to confirm.
#[derive(Debug, Clone, Copy, Default, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Confirmations {
    /// Data older than a shortened retention becomes eligible for deletion.
    #[serde(default)]
    pub shorten_retention: bool,
}

/// A confirmation the request lacked.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct NeedsConfirmation {
    /// The `confirm` field that must be `true`.
    pub confirm: &'static str,
    pub message: String,
}

#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error(transparent)]
    Invalid(#[from] Invalid),
    #[error("the change needs confirmation")]
    Confirm(Vec<NeedsConfirmation>),
    #[error("nothing to change")]
    Empty,
    /// CPU and memory settings of an engine Forge does not run.
    #[error(
        "{0:?} can only be changed for the embedded analytics engine; an external ClickHouse server manages its own CPU and memory"
    )]
    NotManaged(Vec<Setting>),
    #[error(transparent)]
    Store(#[from] DomainError),
}

#[derive(Debug, thiserror::Error)]
pub enum RestartError {
    #[error("only the analytics engine run by Forge can be restarted from Forge")]
    NotManaged,
    #[error("the restart time is more than 7 days ahead")]
    TooFar,
}

/// One setting as the console shows it.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct SettingView {
    pub setting: Setting,
    pub group: Group,
    /// From `forge.toml`.
    pub configured: f64,
    /// The administrator's override, if any.
    pub override_value: Option<f64>,
    pub effective: f64,
    pub min: f64,
    pub max: f64,
    /// Whether Forge applies it for this engine (`false`: managed by the
    /// external ClickHouse host).
    pub applies: bool,
}

/// `GET /api/v1/analytics/settings`.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct SettingsView {
    pub engine: AnalyticsEngine,
    /// The engine is run by Forge (`embedded`).
    pub managed: bool,
    pub ready: bool,
    pub last_error: Option<String>,
    pub settings: Vec<SettingView>,
    pub usage: Usage,
    pub queue: QueueStats,
    /// Why the CPU cap is not in effect (`null`: it is, none is set, or
    /// the engine is external).
    pub process_limits_unavailable: Option<String>,
    pub pending_restart: bool,
    /// A scheduled engine restart, epoch ms.
    pub restart_at: Option<i64>,
    pub last_apply: Option<ApplyReport>,
    /// The analytics worker process.
    pub worker: WorkerState,
    /// The store's scheduled jobs (`duckdb`: rollups, retention,
    /// compaction); empty for other engines.
    pub jobs: Vec<forge_analytics_proto::views::JobReport>,
}

/// `GET /api/v1/analytics/status`.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct AnalyticsStatus {
    pub engine: AnalyticsEngine,
    /// Schema applied and the last write or check succeeded.
    pub ready: bool,
    pub last_error: Option<String>,
    /// Node and container rows the worker holds for the next write.
    pub pending_rows: usize,
    pub dropped_rows: u64,
    #[schemars(with = "serde_json::Value")]
    pub retention: Retention,
    pub worker: WorkerState,
}

#[derive(Debug, Default)]
struct Live {
    last_apply: Option<ApplyReport>,
    restart_at: Option<i64>,
    restart_task: Option<tokio::task::JoinHandle<()>>,
}

/// Wiring for [`AnalyticsControl::new`].
#[derive(Debug)]
pub struct ControlDeps {
    pub engine: AnalyticsEngine,
    pub defaults: Tuning,
    pub overrides: Overrides,
    pub tx: watch::Sender<Tuning>,
    pub settings: Arc<dyn SettingsRepository>,
    pub clock: Arc<dyn Clock>,
    pub worker: Arc<WorkerClient>,
    pub supervisor: Arc<WorkerSupervisor>,
}

/// The analytics settings service.
pub struct AnalyticsControl {
    engine: AnalyticsEngine,
    defaults: Tuning,
    /// Held across a whole update, so two administrators' changes apply in turn.
    overrides: tokio::sync::Mutex<Overrides>,
    tx: watch::Sender<Tuning>,
    settings: Arc<dyn SettingsRepository>,
    clock: Arc<dyn Clock>,
    worker: Arc<WorkerClient>,
    supervisor: Arc<WorkerSupervisor>,
    live: Mutex<Live>,
}

impl std::fmt::Debug for AnalyticsControl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnalyticsControl").finish_non_exhaustive()
    }
}

/// The worker's report, or why there is none.
fn unavailable_reason(engine: AnalyticsEngine, e: &crate::analytics_worker::CallError) -> String {
    if engine == AnalyticsEngine::Off { "analytics is off".into() } else { e.to_string() }
}

impl AnalyticsControl {
    #[must_use]
    pub fn new(d: ControlDeps) -> Self {
        Self {
            engine: d.engine,
            defaults: d.defaults,
            overrides: tokio::sync::Mutex::new(d.overrides),
            tx: d.tx,
            settings: d.settings,
            clock: d.clock,
            worker: d.worker,
            supervisor: d.supervisor,
            live: Mutex::new(Live::default()),
        }
    }

    fn live<R>(&self, f: impl FnOnce(&mut Live) -> R) -> R {
        f(&mut self.live.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// The engine is run by Forge.
    #[must_use]
    pub fn managed(&self) -> bool {
        self.engine == AnalyticsEngine::Embedded
    }

    #[must_use]
    pub fn subscribe(&self) -> watch::Receiver<Tuning> {
        self.tx.subscribe()
    }

    /// Whether Forge applies settings of `group` for this engine.
    fn applies(&self, group: Group) -> bool {
        match group {
            // DuckDB runs in the worker: its threads and memory limit apply at once.
            Group::Process | Group::EngineMemory => self.managed() || self.engine == AnalyticsEngine::Duckdb,
            Group::Retention | Group::Forge => true,
        }
    }

    async fn report(&self) -> Result<WorkerReport, String> {
        match tokio::time::timeout(REPORT_WAIT, self.worker.report()).await {
            Ok(Ok(r)) => Ok(r),
            Ok(Err(e)) => Err(unavailable_reason(self.engine, &e)),
            Err(_) => Err("the analytics worker did not answer in time".into()),
        }
    }

    /// Engine, readiness, buffered rows and the worker process.
    pub async fn status(&self) -> AnalyticsStatus {
        let worker = self.supervisor.state();

        match self.report().await {
            Ok(r) => AnalyticsStatus {
                engine: self.engine,
                ready: r.store.ready,
                last_error: r.store.last_error,
                pending_rows: r.store.pending_rows,
                dropped_rows: r.store.dropped_rows,
                retention: r.store.retention,
                worker,
            },
            Err(why) => AnalyticsStatus {
                engine: self.engine,
                ready: false,
                last_error: (self.engine != AnalyticsEngine::Off).then_some(why),
                pending_rows: 0,
                dropped_rows: 0,
                retention: self.tx.borrow().retention(),
                worker,
            },
        }
    }

    pub async fn view(&self) -> SettingsView {
        let overrides = *self.overrides.lock().await;
        let effective = *self.tx.borrow();
        let settings = Setting::ALL
            .iter()
            .map(|s| {
                let (min, max) = s.bounds();
                SettingView {
                    setting: *s,
                    group: s.group(),
                    configured: self.defaults.get(*s),
                    override_value: overrides.get(*s),
                    effective: effective.get(*s),
                    min,
                    max,
                    applies: self.applies(s.group()),
                }
            })
            .collect();
        let report = self.report().await;
        let (restart_at, last_apply) = self.live(|l| (l.restart_at, l.last_apply.clone()));
        let worker = self.supervisor.state();

        match report {
            Ok(r) => SettingsView {
                engine: self.engine,
                managed: self.managed(),
                ready: r.store.ready,
                last_error: r.store.last_error,
                settings,
                usage: r.usage,
                queue: r.queue,
                process_limits_unavailable: r.process_limits_unavailable,
                pending_restart: r.pending_restart,
                restart_at,
                last_apply,
                worker,
                jobs: r.jobs,
            },
            Err(why) => SettingsView {
                engine: self.engine,
                managed: self.managed(),
                ready: false,
                last_error: (self.engine != AnalyticsEngine::Off).then_some(why),
                settings,
                usage: Usage::default(),
                queue: QueueStats::default(),
                process_limits_unavailable: None,
                pending_restart: false,
                restart_at,
                last_apply,
                worker,
                jobs: Vec::new(),
            },
        }
    }

    /// Validates, confirms, stores and applies `patch`. Returns what changed
    /// (for the audit log) and the per-setting report.
    pub async fn update(
        &self,
        patch: &Patch,
        confirm: Confirmations,
    ) -> Result<(Vec<Change>, ApplyReport), UpdateError> {
        let touched = Overrides::touched(patch);
        if touched.is_empty() {
            return Err(UpdateError::Empty);
        }

        let mut guard = self.overrides.lock().await;
        let mut next = *guard;
        next.apply(patch);
        tuning::validate(&next)?;

        // Setting a value Forge would not apply is refused; removing a
        // stale override is not.
        let foreign: Vec<Setting> =
            touched.iter().filter(|(s, v)| v.is_some() && !self.applies(s.group())).map(|(s, _)| *s).collect();

        if !foreign.is_empty() {
            return Err(UpdateError::NotManaged(foreign));
        }

        let old = self.defaults.with(&guard);
        let new = self.defaults.with(&next);
        Self::confirmations(&old, &new, confirm)?;

        if next == Overrides::default() {
            self.settings.delete_setting(tuning::OVERRIDES_KEY).await?;
        } else {
            let json = serde_json::to_string(&next).map_err(|e| DomainError::Internal(e.to_string()))?;
            self.settings.put_setting(tuning::OVERRIDES_KEY, &json, self.clock.now()).await?;
        }

        *guard = next;
        self.tx.send_replace(new);

        let changes = touched
            .iter()
            .map(|(s, v)| Change { setting: *s, old: old.get(*s), new: new.get(*s), override_value: *v })
            .collect();
        let settings: Vec<Setting> = touched.iter().map(|(s, _)| *s).collect();
        let results = self.apply(settings, new).await;
        drop(guard);
        let report = ApplyReport { at: self.clock.now().0, results };
        self.live(|l| l.last_apply = Some(report.clone()));

        Ok((changes, report))
    }

    fn confirmations(old: &Tuning, new: &Tuning, confirm: Confirmations) -> Result<(), UpdateError> {
        let mut need = Vec::new();
        let shorter = tuning::shortened_retention(old, new);
        if !shorter.is_empty() && !confirm.shorten_retention {
            let list: Vec<String> = shorter.iter().map(|(s, o, n)| format!("{s:?} {o} → {n} days")).collect();
            need.push(NeedsConfirmation {
                confirm: "shorten_retention",
                message: format!(
                    "Shortening retention ({}) makes existing data older than the new limit eligible for deletion by background merges.",
                    list.join(", ")
                ),
            });
        }

        if need.is_empty() { Ok(()) } else { Err(UpdateError::Confirm(need)) }
    }

    /// Has the worker apply `settings`; without a worker, says when they apply.
    async fn apply(&self, settings: Vec<Setting>, t: Tuning) -> Vec<SettingResult> {
        if self.engine == AnalyticsEngine::Off {
            return settings
                .iter()
                .filter(|s| self.applies(s.group()))
                .map(|s| match s.group() {
                    Group::Forge => SettingResult {
                        setting: *s,
                        status: ApplyStatus::Applied,
                        effective: Some(t.get(*s)),
                        message: None,
                    },
                    _ => SettingResult {
                        setting: *s,
                        status: ApplyStatus::Pending,
                        effective: None,
                        message: Some("analytics is off; applied when it is turned on".into()),
                    },
                })
                .collect();
        }

        match self.worker.apply(settings.clone(), t).await {
            Ok(results) => results,
            Err(e) => settings
                .iter()
                .map(|s| SettingResult {
                    setting: *s,
                    status: ApplyStatus::Pending,
                    effective: None,
                    message: Some(format!("applied when the analytics worker runs again ({e})")),
                })
                .collect(),
        }
    }

    /// Restarts the managed engine now (`at` is `None` or past) or at `at`
    /// (epoch ms). A new schedule replaces the previous one. Returns when it
    /// will happen.
    pub fn schedule_restart(self: &Arc<Self>, at: Option<i64>) -> Result<i64, RestartError> {
        if !self.managed() {
            return Err(RestartError::NotManaged);
        }

        let now = self.clock.now().0;
        let at = at.unwrap_or(now).max(now);
        if at - now > 7 * 24 * 3_600_000 {
            return Err(RestartError::TooFar);
        }

        let me = self.clone();
        let wait = Duration::from_millis(u64::try_from(at - now).unwrap_or(0));
        let task = tokio::spawn(async move {
            tokio::time::sleep(wait).await;
            me.live(|l| {
                l.restart_at = None;
                l.restart_task = None;
            });

            if let Err(e) = me.worker.restart_engine().await {
                tracing::warn!(error = %e, "restarting the analytics engine failed");
            }
        });
        self.live(|l| {
            if let Some(old) = l.restart_task.replace(task) {
                old.abort();
            }

            l.restart_at = Some(at);
        });

        Ok(at)
    }

    /// Cancels a scheduled restart; returns whether one was scheduled.
    pub fn cancel_restart(&self) -> bool {
        self.live(|l| {
            l.restart_at = None;
            l.restart_task.take().map(|t| t.abort()).is_some()
        })
    }
}
