//! Node and container history (D-127), moved from forge-server (D-141).
//!
//! Heartbeats carry a resource sample (and, once a minute, the top
//! processes). [`Analytics::observe`] folds them into one row per node and
//! minute in memory; [`run`] writes closed minutes in batches once a minute.
//! When the analytics database is unreachable, rows wait in bounded buffers
//! (oldest dropped first, counted) and history reads report it unavailable.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::Duration;

use forge_analytics_proto::views::StoreStatus;
use forge_core::analytics::{
    AnalyticsError, AnalyticsStore, ContainerHistory, ContainerMinute, MAX_HISTORY_CONTAINERS, MAX_WINDOW_MS,
    MinuteBucket, NodeHistory, NodeMinute, NodeProcessMinute, NodeReport, NodeSample, Peak, Resolution, Retention,
    TopBy,
};
use forge_core::{Clock, ServerId, Timestamp};
use forge_proto::messages::NodeMetrics;
use tokio::sync::watch;

const MINUTE_MS: i64 = 60_000;
/// Rows kept while the analytics database is unreachable (about 3 hours
/// for 100 nodes); the oldest are dropped first.
const MAX_PENDING_MINUTES: usize = 20_000;
const MAX_PENDING_PROCESSES: usize = 200_000;
/// Container rows kept while the database is unreachable (about 3 hours of
/// 100 nodes with 20 containers each).
const MAX_PENDING_CONTAINERS: usize = 400_000;
/// Open minutes kept at once: one per node.
const MAX_OPEN_NODES: usize = 100_000;
/// Rows per insert.
const BATCH: usize = 5_000;

/// A node sample as a [`NodeSample`].
fn sample(m: &NodeMetrics) -> NodeSample {
    NodeSample {
        cpu: m.cpu_percent,
        mem_used: m.mem_used_bytes,
        mem_total: m.mem_total_bytes,
        swap_used: m.swap_used_bytes,
        disk_used: m.disk_used_bytes,
        disk_total: m.disk_total_bytes,
        rx: m.net_rx_bytes_per_sec,
        tx: m.net_tx_bytes_per_sec,
        load1: m.load1,
    }
}

#[derive(Debug, Default)]
struct Buffers {
    open: HashMap<ServerId, MinuteBucket>,
    minutes: VecDeque<NodeMinute>,
    processes: VecDeque<NodeProcessMinute>,
    containers: VecDeque<ContainerMinute>,
    dropped: u64,
}

/// The analytics database and the node-history buffers.
pub struct Analytics {
    store: Arc<dyn AnalyticsStore>,
    /// The effective retention: `forge.toml` plus runtime overrides (D-136).
    retention: RwLock<Retention>,
    ready: AtomicBool,
    last_error: Mutex<Option<String>>,
    buffers: Mutex<Buffers>,
}

impl std::fmt::Debug for Analytics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Analytics").field("engine", &self.store.engine()).finish_non_exhaustive()
    }
}

fn push_bounded<T>(q: &mut VecDeque<T>, row: T, max: usize, dropped: &mut u64) {
    if q.len() >= max {
        q.pop_front();
        *dropped += 1;
    }

    q.push_back(row);
}

fn unavailable(e: &forge_core::DomainError) -> AnalyticsError {
    AnalyticsError::Unavailable(e.to_string())
}

impl Analytics {
    #[must_use]
    pub fn new(store: Arc<dyn AnalyticsStore>, retention: Retention) -> Self {
        Self {
            store,
            retention: RwLock::new(retention),
            ready: AtomicBool::new(false),
            last_error: Mutex::new(None),
            buffers: Mutex::new(Buffers::default()),
        }
    }

    fn with<R>(&self, f: impl FnOnce(&mut Buffers) -> R) -> R {
        f(&mut self.buffers.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// The store, whether or not it is ready (usage sampling, health checks).
    #[must_use]
    pub fn configured_store(&self) -> &Arc<dyn AnalyticsStore> {
        &self.store
    }

    /// The store when its schema is in place, or why it cannot be used.
    pub fn usable(&self) -> Result<&Arc<dyn AnalyticsStore>, AnalyticsError> {
        if self.ready.load(Ordering::Relaxed) {
            return Ok(&self.store);
        }

        Err(AnalyticsError::Unavailable(
            self.status().last_error.unwrap_or_else(|| "the analytics database is starting".into()),
        ))
    }

    fn fail(&self, e: &impl std::fmt::Display) {
        self.ready.store(false, Ordering::Relaxed);
        *self.last_error.lock().unwrap_or_else(PoisonError::into_inner) = Some(e.to_string());
    }

    fn ok(&self) {
        self.ready.store(true, Ordering::Relaxed);
        *self.last_error.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }

    #[must_use]
    pub fn status(&self) -> StoreStatus {
        let (pending_rows, dropped_rows) =
            self.with(|b| (b.minutes.len() + b.processes.len() + b.containers.len(), b.dropped));
        StoreStatus {
            ready: self.ready.load(Ordering::Relaxed),
            last_error: self.last_error.lock().unwrap_or_else(PoisonError::into_inner).clone(),
            pending_rows,
            dropped_rows,
            retention: self.retention(),
        }
    }

    #[must_use]
    pub fn retention(&self) -> Retention {
        *self.retention.read().unwrap_or_else(PoisonError::into_inner)
    }

    /// Sets the retention used when the schema is next prepared. The tables
    /// themselves are changed by [`AnalyticsStore::apply_retention`].
    pub fn set_retention(&self, r: Retention) {
        *self.retention.write().unwrap_or_else(PoisonError::into_inner) = r;
    }

    /// Folds one heartbeat sample into the node's current minute.
    pub fn observe(&self, server: ServerId, now: Timestamp, m: &NodeMetrics) {
        let minute = now.0 - now.0.rem_euclid(MINUTE_MS);
        self.with(|b| {
            if !b.open.contains_key(&server) && b.open.len() >= MAX_OPEN_NODES {
                b.dropped += 1;
                return;
            }

            let bucket = b.open.entry(server).or_insert_with(|| MinuteBucket::new(minute));
            if bucket.minute() != minute {
                let closed = NodeMinute::from_row(server, &bucket.close());
                *bucket = MinuteBucket::new(minute);
                push_bounded(&mut b.minutes, closed, MAX_PENDING_MINUTES, &mut b.dropped);
            }

            if let Some(bucket) = b.open.get_mut(&server) {
                bucket.add(&sample(m));
            }

            for p in &m.top_processes {
                let row = NodeProcessMinute {
                    server_id: server,
                    minute: Timestamp(minute),
                    pid: p.pid,
                    name: p.name.clone(),
                    user: p.user.clone(),
                    cpu: p.cpu_percent,
                    mem: p.mem_bytes,
                    cmd: p.cmd.clone(),
                };
                push_bounded(&mut b.processes, row, MAX_PENDING_PROCESSES, &mut b.dropped);
            }
        });
    }

    /// Queues per-minute container rows for the next write (bounded,
    /// oldest dropped first).
    pub fn push_container_minutes(&self, rows: Vec<ContainerMinute>) {
        self.with(|b| {
            for r in rows {
                push_bounded(&mut b.containers, r, MAX_PENDING_CONTAINERS, &mut b.dropped);
            }
        });
    }

    /// Closes every minute that ended before `now` (nodes that stopped
    /// sending keep no open minute).
    pub fn close_ended(&self, now: Timestamp) {
        let current = now.0 - now.0.rem_euclid(MINUTE_MS);
        self.with(|b| {
            let ended: Vec<ServerId> = b.open.iter().filter(|(_, k)| k.minute() < current).map(|(s, _)| *s).collect();

            for s in ended {
                if let Some(k) = b.open.remove(&s) {
                    let row = NodeMinute::from_row(s, &k.close());
                    push_bounded(&mut b.minutes, row, MAX_PENDING_MINUTES, &mut b.dropped);
                }
            }
        });
    }

    /// Writes buffered rows. On failure the rows stay buffered.
    pub async fn flush(&self) {
        if !self.ready.load(Ordering::Relaxed) {
            return;
        }

        let store = self.store.clone();

        loop {
            let minutes: Vec<NodeMinute> = self.with(|b| b.minutes.iter().take(BATCH).cloned().collect());
            if minutes.is_empty() {
                break;
            }

            match store.write_node_minutes(&minutes).await {
                Ok(()) => self.with(|b| drop(b.minutes.drain(..minutes.len().min(b.minutes.len())))),
                Err(e) => return self.write_failed(&e),
            }
        }

        loop {
            let procs: Vec<NodeProcessMinute> = self.with(|b| b.processes.iter().take(BATCH).cloned().collect());
            if procs.is_empty() {
                break;
            }

            match store.write_top_processes(&procs).await {
                Ok(()) => self.with(|b| drop(b.processes.drain(..procs.len().min(b.processes.len())))),
                Err(e) => return self.write_failed(&e),
            }
        }

        loop {
            let rows: Vec<ContainerMinute> = self.with(|b| b.containers.iter().take(BATCH).cloned().collect());
            if rows.is_empty() {
                break;
            }

            match store.write_container_minutes(&rows).await {
                Ok(()) => self.with(|b| drop(b.containers.drain(..rows.len().min(b.containers.len())))),
                Err(e) => return self.write_failed(&e),
            }
        }
    }

    fn write_failed(&self, e: &forge_core::DomainError) {
        tracing::warn!(error = %e, "analytics write failed; rows stay buffered");
        self.fail(e);
    }

    /// Applies the schema; retried by [`run`] until it succeeds.
    pub async fn prepare(&self) -> bool {
        match self.store.migrate(self.retention()).await {
            Ok(()) => {
                tracing::info!(engine = self.store.engine(), "analytics database ready");
                self.ok();
                true
            }
            Err(e) => {
                tracing::warn!(error = %e, engine = self.store.engine(), "analytics database not ready");
                self.fail(&e);
                false
            }
        }
    }

    /// History of `server` in `[from, to)`; minutes up to two days, hours beyond.
    pub async fn node_history(&self, server: ServerId, from: i64, to: i64) -> Result<NodeHistory, AnalyticsError> {
        check_window(from, to)?;
        let store = self.usable()?;
        let (f, t) = (Timestamp(from), Timestamp(to));
        let resolution = Resolution::for_window(f, t);
        let points = store.node_series(server, f, t, resolution).await.map_err(|e| unavailable(&e))?;

        Ok(NodeHistory { server_id: server, from, to, resolution, points })
    }

    /// Container history of `server` in `[from, to)`: every container (up to
    /// `limit`, by name), or the one named.
    pub async fn container_history(
        &self,
        server: ServerId,
        container: Option<&str>,
        window: (i64, i64),
        limit: u32,
    ) -> Result<ContainerHistory, AnalyticsError> {
        let (from, to) = window;
        check_window(from, to)?;
        let store = self.usable()?;
        let (f, t) = (Timestamp(from), Timestamp(to));
        let resolution = Resolution::for_window(f, t);
        let containers = store
            .container_series(server, container, (f, t), resolution, limit.clamp(1, MAX_HISTORY_CONTAINERS))
            .await
            .map_err(|e| unavailable(&e))?;

        Ok(ContainerHistory { server_id: server, from, to, resolution, containers })
    }

    /// History plus peaks and the top 10 processes by CPU and by memory.
    pub async fn node_report(&self, server: ServerId, from: i64, to: i64) -> Result<NodeReport, AnalyticsError> {
        let history = self.node_history(server, from, to).await?;
        let store = self.usable()?;
        let (f, t) = (Timestamp(from), Timestamp(to));
        let (cpu, mem) = tokio::join!(
            store.top_processes(server, f, t, TopBy::Cpu, 10),
            store.top_processes(server, f, t, TopBy::Mem, 10)
        );
        let peak = |v: &dyn Fn(&forge_core::analytics::NodePoint) -> f64| {
            history.points.iter().map(|p| Peak { t: p.t, value: v(p) }).max_by(|a, b| a.value.total_cmp(&b.value))
        };

        #[allow(clippy::cast_precision_loss)] // reason: percentages for display
        let pct = |used: u64, total: u64| if total == 0 { 0.0 } else { used as f64 * 100.0 / total as f64 };
        Ok(NodeReport {
            cpu_peak: peak(&|p| f64::from(p.cpu_max)),
            mem_peak: peak(&|p| pct(p.mem_used, p.mem_total)),
            disk_peak: peak(&|p| pct(p.disk_used, p.disk_total)),
            top_cpu: cpu.map_err(|e| unavailable(&e))?,
            top_mem: mem.map_err(|e| unavailable(&e))?,
            history,
        })
    }

    /// Checks a store that failed a write; marks it ready again once it answers.
    async fn recheck(&self) {
        if !self.ready.load(Ordering::Relaxed) && self.store.health().await.is_ok() {
            self.ok();
        }
    }
}

fn check_window(from: i64, to: i64) -> Result<(), AnalyticsError> {
    if to <= from || to - from > MAX_WINDOW_MS {
        return Err(AnalyticsError::Invalid("`from` must be before `to`, at most 400 days apart".into()));
    }

    Ok(())
}

/// Background loop: prepares the schema, then writes closed minutes every
/// minute. On shutdown it closes every open minute and writes once more.
pub async fn run(analytics: Arc<Analytics>, clock: Arc<dyn Clock>, mut shutdown: watch::Receiver<bool>) {
    let mut prepared = false;

    loop {
        if prepared {
            analytics.close_ended(clock.now());
            analytics.flush().await;

            // A failed write marks the store not ready; check it again.
            analytics.recheck().await;
        } else {
            prepared = analytics.prepare().await;
        }

        let wait = if prepared { Duration::from_secs(60) } else { Duration::from_secs(15) };
        tokio::select! {
            () = tokio::time::sleep(wait) => {}
            _ = shutdown.changed() => {
                analytics.close_ended(Timestamp(i64::MAX));
                analytics.flush().await;
                break;
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // reason: tests
mod tests {
    use super::*;

    fn retention() -> Retention {
        Retention { node_metrics_days: 7, top_processes_days: 7, node_rollup_days: 365, app_logs_days: 30 }
    }

    fn metrics(cpu: f32, rx: u64) -> NodeMetrics {
        NodeMetrics { cpu_percent: cpu, net_rx_bytes_per_sec: rx, mem_used_bytes: 5, ..NodeMetrics::default() }
    }

    #[test]
    fn folds_samples_into_minutes() {
        let a = Analytics::new(Arc::new(crate::memory::MemoryStore::default()), retention());
        let s = ServerId::new();
        a.observe(s, Timestamp(60_000), &metrics(10.0, 100));
        a.observe(s, Timestamp(90_000), &metrics(30.0, 300));

        // The next minute closes the first one.
        a.observe(s, Timestamp(120_500), &metrics(50.0, 0));
        let rows: Vec<NodeMinute> = a.with(|b| b.minutes.iter().cloned().collect());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].minute, Timestamp(60_000));
        assert!((rows[0].cpu_avg - 20.0).abs() < 0.01);
        assert!((rows[0].cpu_max - 30.0).abs() < 0.01);
        assert_eq!(rows[0].rx, 200);
        assert_eq!(rows[0].samples, 2);
        a.close_ended(Timestamp(400_000));
        assert_eq!(a.with(|b| b.minutes.len()), 2);
    }

    #[test]
    fn buffer_is_bounded() {
        let mut q = VecDeque::new();
        let mut dropped = 0;

        for i in 0..5 {
            push_bounded(&mut q, i, 3, &mut dropped);
        }

        assert_eq!(q, VecDeque::from([2, 3, 4]));
        assert_eq!(dropped, 2);
    }

    #[tokio::test]
    async fn rows_reach_the_store_and_history_reads_them() {
        let store = Arc::new(crate::memory::MemoryStore::default());
        let a = Analytics::new(store, retention());
        assert!(matches!(a.node_history(ServerId::new(), 0, 1).await, Err(AnalyticsError::Unavailable(_))));
        assert!(a.prepare().await);
        let s = ServerId::new();
        a.observe(s, Timestamp(60_000), &metrics(10.0, 100));
        a.close_ended(Timestamp(200_000));
        a.flush().await;
        assert_eq!(a.status().pending_rows, 0);
        let h = a.node_history(s, 0, 300_000).await.unwrap();
        assert_eq!(h.points.len(), 1);
        assert!(matches!(a.node_history(s, 5, 5).await, Err(AnalyticsError::Invalid(_))));
    }
}
