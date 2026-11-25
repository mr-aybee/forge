//! Applies analytics settings to the engine and reports what took effect
//! (D-136), moved from forge-server (D-141). forge-server validates,
//! confirms, stores and audits a change, then sends the new effective
//! settings here:
//!
//! | Group | Applied by | Verified by |
//! | --- | --- | --- |
//! | Retention | `ALTER TABLE .. MODIFY TTL` | the TTLs read back from the engine |
//! | Process (CPU cap) | CPU affinity of every engine thread; thread pools in its configuration | the affinity read back; pools on restart |
//! | Engine memory budget | the engine's configuration file | `system.server_settings`, else "pending restart" |
//! | Search, ingest, disk | the `watch` channel the worker's own code reads | immediate |
//!
//! It also samples the engine's usage and free disk every few seconds.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use forge_analytics_proto::WorkerError;
use forge_analytics_proto::tuning::{Group, Setting, Tuning};
use forge_analytics_proto::views::{ApplyStatus, SettingResult, Usage, WorkerReport};
use forge_core::analytics::EngineStats;
use tokio::sync::watch;

use forge_analytics_duckdb::DuckDb;

use crate::analytics::Analytics;
use crate::batcher::LogBatcher;
use crate::embedded::{ManagedEngine, cpu_cap};
use crate::ingest::FREE_DISK_UNKNOWN;

/// How often usage and free disk are read.
const SAMPLE_EVERY: Duration = Duration::from_secs(10);
/// How long a memory budget change may take to be picked up by the engine.
const RELOAD_WAIT: Duration = Duration::from_secs(15);
const MIB: f64 = 1024.0 * 1024.0;

#[derive(Debug, Default)]
struct Live {
    stats: Option<EngineStats>,
    process_memory: Option<u64>,
    cpu_vcpu: Option<f64>,
    cpu_sample: Option<(Instant, u64)>,
    allowed_cpus: Option<u32>,
    pending_restart: bool,
}

/// Wiring for [`Control::new`].
#[derive(Debug)]
pub struct ControlDeps {
    pub analytics: Arc<Analytics>,
    pub batcher: LogBatcher,
    pub managed: Option<Arc<ManagedEngine>>,
    pub free_disk: Arc<AtomicU64>,
    pub tuning: watch::Sender<Tuning>,
    /// The in-process DuckDB store (D-143).
    pub duck: Option<Arc<DuckDb>>,
}

/// The worker's settings and usage service.
#[derive(Debug)]
pub struct Control {
    analytics: Arc<Analytics>,
    batcher: LogBatcher,
    managed: Option<Arc<ManagedEngine>>,
    free_disk: Arc<AtomicU64>,
    tuning: watch::Sender<Tuning>,
    duck: Option<Arc<DuckDb>>,
    live: Mutex<Live>,
}

fn mib(bytes: Option<u64>) -> Option<f64> {
    #[allow(clippy::cast_precision_loss)] // reason: MiB for display
    bytes.map(|b| b as f64 / MIB)
}

impl Control {
    #[must_use]
    pub fn new(d: ControlDeps) -> Self {
        Self {
            analytics: d.analytics,
            batcher: d.batcher,
            managed: d.managed,
            free_disk: d.free_disk,
            tuning: d.tuning,
            duck: d.duck,
            live: Mutex::new(Live::default()),
        }
    }

    fn live<R>(&self, f: impl FnOnce(&mut Live) -> R) -> R {
        f(&mut self.live.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// Publishes new effective settings to the batcher, ingest and search.
    pub fn set_tuning(&self, t: Tuning) {
        self.analytics.set_retention(t.retention());
        self.tuning.send_replace(t);

        if let Some(duck) = self.duck.clone() {
            let limits = crate::wiring::duck_limits(&t);

            // `SET` on the instance: quick, but off the async threads.
            tokio::task::spawn_blocking(move || {
                if let Err(e) = duck.set_limits(limits) {
                    tracing::warn!(error = %e, "applying the DuckDB limits failed");
                }
            });
        }
    }

    /// DuckDB applies the CPU cap (threads) and memory budget at once.
    fn duck_applied(&self, settings: &[Setting], t: &Tuning, group: Group) -> Option<Vec<SettingResult>> {
        self.duck.as_ref()?;
        let l = crate::wiring::duck_limits(t);

        Some(
            settings
                .iter()
                .filter(|s| s.group() == group)
                .map(|s| SettingResult {
                    setting: *s,
                    status: ApplyStatus::Applied,
                    effective: Some(if *s == Setting::memory_budget_mb { f64::from(l.memory_mb) } else { t.get(*s) }),
                    message: (*s == Setting::cpu_quota_vcpu).then(|| format!("DuckDB uses {} threads", l.threads)),
                })
                .collect(),
        )
    }

    /// Status, usage and the ingest queue.
    #[must_use]
    pub fn report(&self) -> WorkerReport {
        let t = *self.tuning.borrow();
        let per_query = (t.search_memory_mb > 0).then(|| u64::from(t.search_memory_mb) * 1024 * 1024);

        self.live(|l| WorkerReport {
            store: self.analytics.status(),
            usage: Usage {
                cpu_vcpu: l.cpu_vcpu,
                process_memory_bytes: l.process_memory,
                engine_memory_bytes: l.stats.and_then(|s| s.memory_used_bytes),
                server_memory_limit_bytes: l.stats.and_then(|s| s.server_memory_limit_bytes),
                per_query_memory_bytes: per_query,
                allowed_cpus: l.allowed_cpus,
                free_disk_bytes: l.stats.and_then(|s| s.free_disk_bytes),
            },
            queue: self.batcher.stats(),
            process_limits_unavailable: self.managed.as_ref().and_then(|e| e.pin_error()).map(|e| e.to_string()),
            pending_restart: l.pending_restart,
            jobs: self.duck.as_ref().map(|d| d.reports()).unwrap_or_default(),
        })
    }

    /// Restarts the managed engine now.
    pub fn restart_engine(&self) -> Result<(), WorkerError> {
        let engine = self.managed.as_ref().ok_or(WorkerError::NotManaged)?;
        self.live(|l| l.pending_restart = false);
        engine.request_restart();

        Ok(())
    }

    /// Applies `settings` of `t` and reports each one.
    pub async fn apply(&self, settings: &[Setting], t: Tuning) -> Vec<SettingResult> {
        self.set_tuning(t);
        let has = |g: Group| settings.iter().any(|s| s.group() == g);
        let mut out = Vec::new();

        if has(Group::Retention) {
            out.extend(self.apply_retention(settings, &t).await);
        }

        if let Some(r) = self.duck_applied(settings, &t, Group::Process) {
            out.extend(r);
        } else if has(Group::Process) {
            out.extend(self.apply_process(settings, &t).await);
        }

        if let Some(r) = self.duck_applied(settings, &t, Group::EngineMemory) {
            out.extend(r);
        } else if has(Group::EngineMemory) {
            out.push(self.apply_memory_budget(&t).await);
        }

        for s in settings.iter().filter(|s| s.group() == Group::Forge) {
            out.push(SettingResult {
                setting: *s,
                status: ApplyStatus::Applied,
                effective: Some(t.get(*s)),
                message: None,
            });
        }

        out
    }

    async fn apply_retention(&self, settings: &[Setting], t: &Tuning) -> Vec<SettingResult> {
        let mine: Vec<Setting> = settings.iter().copied().filter(|s| s.group() == Group::Retention).collect();
        let all = |status, message: Option<String>| -> Vec<SettingResult> {
            mine.iter()
                .map(|s| SettingResult { setting: *s, status, effective: None, message: message.clone() })
                .collect()
        };

        let Ok(store) = self.analytics.usable() else {
            return all(ApplyStatus::Pending, Some("applied when the analytics database is reachable".into()));
        };

        if let Err(e) = store.apply_retention(t.retention()).await {
            return all(ApplyStatus::Failed, Some(format!("the analytics database refused the change: {e}")));
        }

        match store.effective_retention().await {
            Ok(r) => {
                let got = Tuning {
                    node_metrics_days: r.node_metrics_days,
                    top_processes_days: r.top_processes_days,
                    node_rollup_days: r.node_rollup_days,
                    app_logs_days: r.app_logs_days,
                    ..*t
                };
                mine.iter()
                    .map(|s| {
                        let (want, have) = (t.get(*s), got.get(*s));
                        let ok = (want - have).abs() < f64::EPSILON;
                        SettingResult {
                            setting: *s,
                            status: if ok { ApplyStatus::Applied } else { ApplyStatus::Failed },
                            effective: Some(have),
                            message: (!ok).then(|| format!("the engine reports {have} days")),
                        }
                    })
                    .collect()
            }
            Err(e) => all(ApplyStatus::Failed, Some(format!("changed, but the TTLs could not be read back: {e}"))),
        }
    }

    /// Pins the running engine to the new CPU cap at once and rewrites its
    /// configuration; the thread pools are resized when it restarts.
    async fn apply_process(&self, settings: &[Setting], t: &Tuning) -> Vec<SettingResult> {
        let mine: Vec<Setting> = settings.iter().copied().filter(|s| s.group() == Group::Process).collect();

        let Some(engine) = self.managed.clone() else {
            // forge-server refuses these for an engine Forge does not run; a
            // reset of a stale override has nothing to apply.
            return Vec::new();
        };

        let tt = *t;
        let res = tokio::task::spawn_blocking(move || {
            let written = engine.write_config(&tt).map_err(|e| format!("{e:#}"));
            let pinned = engine.pin(&tt).map_err(|e| e.to_string());
            (written, pinned)
        })
        .await;

        let (written, pinned) = match res {
            Ok(r) => r,
            Err(e) => (Err(e.to_string()), Err(e.to_string())),
        };

        let want = cpu_cap(t.cpu_quota_vcpu);
        let result = |status, effective, message: Option<String>| -> Vec<SettingResult> {
            mine.iter().map(|s| SettingResult { setting: *s, status, effective, message: message.clone() }).collect()
        };

        if let Err(e) = written {
            return result(ApplyStatus::Failed, None, Some(e));
        }

        match pinned {
            Err(e) => result(ApplyStatus::Failed, None, Some(e)),
            Ok(None) => result(ApplyStatus::PendingRestart, None, Some("applied when the engine starts".into())),
            Ok(Some(have)) => {
                self.live(|l| {
                    l.allowed_cpus = Some(have);
                    l.pending_restart = true;
                });

                match want {
                    Some(w) if w != have => result(
                        ApplyStatus::Failed,
                        Some(f64::from(have)),
                        Some(format!("the engine may use {have} CPUs, not {w}")),
                    ),
                    _ => result(
                        ApplyStatus::Applied,
                        Some(want.map_or(0.0, f64::from)),
                        Some(format!("the engine runs on {have} CPUs; restart it to resize its thread pools to match")),
                    ),
                }
            }
        }
    }

    async fn apply_memory_budget(&self, t: &Tuning) -> SettingResult {
        let setting = Setting::memory_budget_mb;
        let failed = |message: String| SettingResult {
            setting,
            status: ApplyStatus::Failed,
            effective: None,
            message: Some(message),
        };

        let Some(engine) = self.managed.clone() else {
            // Only a reset reaches here for an engine Forge does not run.
            return SettingResult { setting, status: ApplyStatus::Applied, effective: None, message: None };
        };

        let tt = *t;
        match tokio::task::spawn_blocking(move || engine.write_config(&tt)).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return failed(format!("{e:#}")),
            Err(e) => return failed(e.to_string()),
        }

        let want = Some(u64::from(t.memory_budget_mb) * 1024 * 1024);
        let deadline = Instant::now() + RELOAD_WAIT;
        let mut seen = None;

        while Instant::now() < deadline {
            if let Ok(store) = self.analytics.usable()
                && let Ok(s) = store.engine_stats().await
            {
                seen = s.server_memory_limit_bytes;

                if seen == want {
                    self.live(|l| l.stats = Some(s));
                    return SettingResult {
                        setting,
                        status: ApplyStatus::Applied,
                        effective: mib(seen),
                        message: None,
                    };
                }
            }

            tokio::time::sleep(Duration::from_secs(1)).await;
        }

        self.live(|l| l.pending_restart = true);

        SettingResult {
            setting,
            status: ApplyStatus::PendingRestart,
            effective: mib(seen),
            message: Some("the engine has not picked up the new budget; restart the analytics engine".into()),
        }
    }

    /// Reads usage and free disk once.
    async fn sample(&self) {
        match self.analytics.configured_store().engine_stats().await {
            Ok(s) => {
                self.free_disk.store(s.free_disk_bytes.unwrap_or(FREE_DISK_UNKNOWN), Ordering::Relaxed);
                self.live(|l| l.stats = Some(s));
            }
            // Unknown free disk never blocks ingest; reads already report
            // the engine as unavailable.
            Err(_) => self.free_disk.store(FREE_DISK_UNKNOWN, Ordering::Relaxed),
        }

        let Some(engine) = self.managed.clone() else { return };
        let res = tokio::task::spawn_blocking(move || (engine.usage(), engine.allowed_cpus())).await;
        let Ok((usage, cpus)) = res else { return };
        let now = Instant::now();
        self.live(|l| {
            l.allowed_cpus = cpus;
            l.process_memory = usage.map(|u| u.memory_bytes);

            match (usage, l.cpu_sample) {
                (Some(u), Some((then, cpu))) if u.cpu_usec >= cpu => {
                    let secs = now.duration_since(then).as_secs_f64();
                    #[allow(clippy::cast_precision_loss)] // reason: CPU seconds for display
                    let used = (u.cpu_usec - cpu) as f64 / 1e6;
                    l.cpu_vcpu = (secs > 0.0).then(|| used / secs);
                }
                _ => l.cpu_vcpu = None,
            }

            l.cpu_sample = usage.map(|u| (now, u.cpu_usec));
        });
    }
}

/// Background loop: samples usage and free disk until shutdown.
pub async fn run(control: Arc<Control>, mut shutdown: watch::Receiver<bool>) {
    loop {
        control.sample().await;
        tokio::select! {
            () = tokio::time::sleep(SAMPLE_EVERY) => {}
            _ = shutdown.changed() => return,
        }
    }
}
