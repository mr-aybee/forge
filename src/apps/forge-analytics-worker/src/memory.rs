//! A bounded in-memory analytics store: for tests and trials. Nothing
//! survives a restart of the worker, and each table keeps only its newest
//! rows ([`MAX_ROWS`]).

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use async_trait::async_trait;
use forge_core::analytics::{
    AnalyticsStore, ContainerMinute, ContainerPoint, ContainerSeries, EngineStats, NodeMinute, NodePoint,
    NodeProcessMinute, ProcessUsage, Resolution, Retention, TopBy,
};
use forge_core::logs::{LogBucket, LogEvent, LogPage, LogQuery};
use forge_core::{DomainResult, ServerId, Timestamp};

/// Rows kept per table; the oldest go first.
pub const MAX_ROWS: usize = 200_000;
const HOUR_MS: i64 = 3_600_000;

#[derive(Debug, Default)]
struct Tables {
    minutes: VecDeque<NodeMinute>,
    processes: VecDeque<NodeProcessMinute>,
    containers: VecDeque<ContainerMinute>,
    logs: VecDeque<LogEvent>,
    retention: Option<Retention>,
}

fn push_all<T: Clone>(q: &mut VecDeque<T>, rows: &[T]) {
    for r in rows {
        if q.len() >= MAX_ROWS {
            q.pop_front();
        }
        q.push_back(r.clone());
    }
}

/// The in-memory store.
#[derive(Debug, Default)]
pub struct MemoryStore {
    tables: Mutex<Tables>,
    writes: AtomicU64,
    /// Holds log inserts while set (tests of slow stores and back-pressure).
    gate: test_gate::Gate,
}

impl MemoryStore {
    fn with<R>(&self, f: impl FnOnce(&mut Tables) -> R) -> R {
        f(&mut self.tables.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// Log rows stored.
    #[must_use]
    pub fn log_count(&self) -> usize {
        self.with(|t| t.logs.len())
    }

    /// Rows stored per table: logs, node minutes, top processes, container minutes.
    #[must_use]
    pub fn counts(&self) -> [usize; 4] {
        self.with(|t| [t.logs.len(), t.minutes.len(), t.processes.len(), t.containers.len()])
    }

    /// Every stored log row, oldest first.
    #[must_use]
    pub fn logs(&self) -> Vec<LogEvent> {
        self.with(|t| t.logs.iter().cloned().collect())
    }

    /// Log inserts begun.
    #[must_use]
    pub fn writes_started(&self) -> u64 {
        self.writes.load(Ordering::Acquire)
    }
}

/// A switch that holds log inserts, so tests can make the store slow.
pub mod test_gate {
    use tokio::sync::watch;

    /// Holds log inserts until released.
    #[derive(Debug)]
    pub struct Gate(pub watch::Sender<bool>);

    impl Default for Gate {
        fn default() -> Self {
            Self(watch::channel(false).0)
        }
    }

    #[derive(Debug)]
    pub struct Release(watch::Sender<bool>);

    impl Release {
        pub fn release(self) {
            self.0.send_replace(false);
        }
    }

    impl super::MemoryStore {
        /// Holds log inserts until the returned handle is released.
        #[must_use]
        pub fn hold_writes(&self) -> Release {
            self.gate.0.send_replace(true);
            Release(self.gate.0.clone())
        }

        pub(super) async fn wait_gate(&self) {
            let mut rx = self.gate.0.subscribe();
            let _ = rx.wait_for(|held| !*held).await;
        }
    }
}

/// A property equals a filter value: strings as they are, other values as JSON text.
fn prop_eq(p: &serde_json::Value, v: &str) -> bool {
    match p {
        serde_json::Value::String(s) => s == v,
        other => serde_json::to_string(other).is_ok_and(|t| t == v),
    }
}

fn matches(e: &LogEvent, q: &LogQuery) -> bool {
    let scoped = e.service_id.is_some_and(|s| q.services.contains(&s))
        || q.instances.iter().any(|(s, n)| e.service_id == Some(*s) && e.server_id == Some(*n))
        || (e.service_id.is_none() && e.server_id.is_some_and(|n| q.open_nodes.contains(&n)));
    let text = |needle: &str| {
        let n = needle.to_lowercase();
        e.message.to_lowercase().contains(&n) || e.exception.as_deref().is_some_and(|x| x.to_lowercase().contains(&n))
    };
    scoped
        && (q.nodes.is_empty() || e.server_id.is_some_and(|n| q.nodes.contains(&n)))
        && q.container.as_deref().is_none_or(|c| {
            e.container_name.as_deref() == Some(c) || e.container_id.as_deref().is_some_and(|id| id.starts_with(c))
        })
        && q.stack.as_deref().is_none_or(|s| e.stack.as_deref() == Some(s))
        && q.source.is_none_or(|s| e.source == s)
        && (q.levels.is_empty() || q.levels.contains(&e.level))
        && q.filter.text.as_deref().is_none_or(text)
        && q.filter.properties.iter().all(|(k, v)| e.properties.get(k).is_some_and(|p| prop_eq(p, v)))
        && q.from.is_none_or(|f| e.ts >= f)
        && q.to.is_none_or(|t| e.ts < t)
}

#[async_trait]
impl AnalyticsStore for MemoryStore {
    fn engine(&self) -> &'static str {
        "memory"
    }

    async fn migrate(&self, retention: Retention) -> DomainResult<()> {
        self.with(|t| t.retention = Some(retention));
        Ok(())
    }

    async fn health(&self) -> DomainResult<()> {
        Ok(())
    }

    async fn apply_retention(&self, retention: Retention) -> DomainResult<()> {
        self.with(|t| t.retention = Some(retention));
        Ok(())
    }

    async fn effective_retention(&self) -> DomainResult<Retention> {
        self.with(|t| t.retention)
            .ok_or_else(|| forge_core::DomainError::Internal("the memory store is not prepared".into()))
    }

    async fn engine_stats(&self) -> DomainResult<EngineStats> {
        Ok(EngineStats::default())
    }

    async fn write_node_minutes(&self, rows: &[NodeMinute]) -> DomainResult<()> {
        self.with(|t| push_all(&mut t.minutes, rows));
        Ok(())
    }

    async fn write_top_processes(&self, rows: &[NodeProcessMinute]) -> DomainResult<()> {
        self.with(|t| push_all(&mut t.processes, rows));
        Ok(())
    }

    async fn node_series(
        &self,
        server: ServerId,
        from: Timestamp,
        to: Timestamp,
        resolution: Resolution,
    ) -> DomainResult<Vec<NodePoint>> {
        let step = if resolution == Resolution::Hour { HOUR_MS } else { 60_000 };
        let mut points: BTreeMap<i64, NodePoint> = BTreeMap::new();
        self.with(|t| {
            for m in t.minutes.iter().filter(|m| m.server_id == server && m.minute >= from && m.minute < to) {
                let at = m.minute.0 - m.minute.0.rem_euclid(step);
                let p = points.entry(at).or_insert(NodePoint {
                    t: at,
                    cpu_avg: m.cpu_avg,
                    cpu_max: 0.0,
                    mem_used: 0,
                    mem_total: 0,
                    disk_used: 0,
                    disk_total: 0,
                    rx: 0,
                    tx: 0,
                    load1: 0.0,
                });
                p.cpu_max = p.cpu_max.max(m.cpu_max);
                (p.mem_used, p.mem_total, p.disk_used, p.disk_total) =
                    (m.mem_used, m.mem_total, m.disk_used, m.disk_total);
                (p.rx, p.tx, p.load1) = (m.rx, m.tx, m.load1);
            }
        });
        Ok(points.into_values().collect())
    }

    async fn top_processes(
        &self,
        server: ServerId,
        from: Timestamp,
        to: Timestamp,
        by: TopBy,
        limit: u32,
    ) -> DomainResult<Vec<ProcessUsage>> {
        let mut groups: BTreeMap<(String, String), ProcessUsage> = BTreeMap::new();
        self.with(|t| {
            for p in t.processes.iter().filter(|p| p.server_id == server && p.minute >= from && p.minute < to) {
                let g = groups.entry((p.name.clone(), p.user.clone())).or_insert(ProcessUsage {
                    name: p.name.clone(),
                    user: p.user.clone(),
                    cpu_avg: 0.0,
                    cpu_max: 0.0,
                    mem_avg: 0,
                    mem_max: 0,
                    minutes: 0,
                    cmd: p.cmd.clone(),
                });
                g.minutes += 1;
                g.cpu_max = g.cpu_max.max(p.cpu);
                g.mem_max = g.mem_max.max(p.mem);
                #[allow(clippy::cast_precision_loss)] // reason: running averages for display
                {
                    g.cpu_avg += (p.cpu - g.cpu_avg) / g.minutes as f32;
                }
                g.mem_avg = g.mem_avg.saturating_add(p.mem.saturating_sub(g.mem_avg) / u64::from(g.minutes));
            }
        });
        let mut out: Vec<ProcessUsage> = groups.into_values().collect();
        match by {
            TopBy::Cpu => out.sort_by(|a, b| b.cpu_avg.total_cmp(&a.cpu_avg)),
            TopBy::Mem => out.sort_by_key(|a| std::cmp::Reverse(a.mem_avg)),
        }
        out.truncate(limit as usize);
        Ok(out)
    }

    async fn write_container_minutes(&self, rows: &[ContainerMinute]) -> DomainResult<()> {
        self.with(|t| push_all(&mut t.containers, rows));
        Ok(())
    }

    async fn container_series(
        &self,
        server: ServerId,
        container: Option<&str>,
        window: (Timestamp, Timestamp),
        _resolution: Resolution,
        limit: u32,
    ) -> DomainResult<Vec<ContainerSeries>> {
        let mut series: BTreeMap<String, ContainerSeries> = BTreeMap::new();
        self.with(|t| {
            for c in t.containers.iter().filter(|c| {
                c.server_id == server
                    && c.minute >= window.0
                    && c.minute < window.1
                    && container.is_none_or(|n| n == c.container_name)
            }) {
                let s = series.entry(c.container_name.clone()).or_insert_with(|| ContainerSeries {
                    container_name: c.container_name.clone(),
                    container_id: String::new(),
                    image: String::new(),
                    service_id: None,
                    stack: None,
                    points: Vec::new(),
                });
                (s.container_id, s.image, s.service_id, s.stack) =
                    (c.container_id.clone(), c.image.clone(), c.service_id, c.stack.clone());
                s.points.push(ContainerPoint {
                    t: c.minute.0,
                    cpu_avg: c.cpu,
                    cpu_max: c.cpu,
                    mem_used: c.mem_used,
                    mem_limit: c.mem_limit,
                    rx: c.rx,
                    tx: c.tx,
                    block_read: c.block_read,
                    block_write: c.block_write,
                    restarts: c.restarts,
                });
            }
        });
        Ok(series.into_values().take(limit as usize).collect())
    }

    async fn write_logs(&self, events: &[LogEvent]) -> DomainResult<()> {
        self.writes.fetch_add(1, Ordering::AcqRel);
        self.wait_gate().await;
        self.with(|t| push_all(&mut t.logs, events));
        Ok(())
    }

    async fn query_logs(&self, q: &LogQuery) -> DomainResult<Vec<LogEvent>> {
        let mut hits: Vec<LogEvent> = self.with(|t| t.logs.iter().filter(|e| matches(e, q)).cloned().collect());
        hits.sort_by_key(|a| std::cmp::Reverse((a.ts, a.id)));
        let limit = q.limit as usize;
        Ok(match q.page {
            LogPage::Older(None) => hits.into_iter().take(limit).collect(),
            LogPage::Older(Some(c)) => hits.into_iter().filter(|e| (e.ts.0, e.id) < (c.ts, c.id)).take(limit).collect(),
            LogPage::Newer(c) => {
                let mut newer: Vec<LogEvent> = hits.into_iter().filter(|e| (e.ts.0, e.id) > (c.ts, c.id)).collect();
                newer.reverse();
                newer.truncate(limit);
                newer.reverse();
                newer
            }
        })
    }

    async fn log_histogram(&self, q: &LogQuery, step_ms: i64) -> DomainResult<Vec<LogBucket>> {
        let step = step_ms.max(1);
        let mut counts: BTreeMap<(i64, forge_core::logs::LogLevel), u64> = BTreeMap::new();
        self.with(|t| {
            for e in t.logs.iter().filter(|e| matches(e, q)) {
                *counts.entry((e.ts.0 - e.ts.0.rem_euclid(step), e.level)).or_default() += 1;
            }
        });
        Ok(counts.into_iter().map(|((t, level), count)| LogBucket { t, level, count }).collect())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // reason: tests
mod tests {
    use super::*;
    use forge_core::ServiceId;
    use forge_core::logs::{IngestContext, LogFilter, SearchLimits, parse_batch};

    #[tokio::test]
    async fn logs_are_scoped_filtered_and_paged() {
        let s = MemoryStore::default();
        let (mine, other) = (ServiceId::new(), ServiceId::new());
        let body = "{\"@t\":\"2024-01-01T00:00:01Z\",\"@mt\":\"one\"}\n{\"@t\":\"2024-01-01T00:00:02Z\",\"@mt\":\"two boom\",\"@l\":\"Error\"}\n";
        let cx = |svc| IngestContext { service_id: svc, server_id: None, node_ts: None, now: Timestamp(1) };
        s.write_logs(&parse_batch(body, cx(mine)).unwrap()).await.unwrap();
        s.write_logs(&parse_batch(body, cx(other)).unwrap()).await.unwrap();
        let mut q = LogQuery {
            services: vec![mine],
            instances: vec![],
            open_nodes: vec![],
            nodes: vec![],
            container: None,
            stack: None,
            source: None,
            levels: vec![],
            filter: LogFilter::default(),
            from: None,
            to: None,
            page: LogPage::Older(None),
            limit: 1,
            limits: SearchLimits::default(),
        };
        let page = s.query_logs(&q).await.unwrap();
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].message, "two boom", "newest first");
        q.page = LogPage::Older(Some(page[0].cursor()));
        assert_eq!(s.query_logs(&q).await.unwrap()[0].message, "one");
        q.page = LogPage::Older(None);
        q.limit = 10;
        q.filter = LogFilter::parse("BOOM");
        assert_eq!(s.query_logs(&q).await.unwrap().len(), 1, "text, case-insensitive");
        let h = s.log_histogram(&q, 1_000).await.unwrap();
        assert_eq!(h.iter().map(|b| b.count).sum::<u64>(), 1);
    }
}
