//! The DuckDB store (D-143): `AnalyticsStore` and `SpoolFileWriter` over the
//! day files, the writer thread and a bounded pool of read connections.

use std::path::PathBuf;
use std::sync::mpsc::{SyncSender, TrySendError};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use duckdb::Connection;
use forge_core::analytics::{
    AnalyticsStore, ContainerMinute, ContainerSeries, EngineStats, FileWrite, NodeMinute, NodePoint, NodeProcessMinute,
    ProcessUsage, Resolution, Retention, SpoolFileWriter, SpoolRows, TopBy,
};
use forge_core::logs::{LogBucket, LogEvent, LogQuery};
use forge_core::{Clock, DomainError, DomainResult, ServerId, Timestamp};
use tokio::sync::{Semaphore, oneshot, watch};

use crate::Error;
use crate::catalog::{Catalog, Limits};
use crate::jobs;
use crate::read;
use crate::writer::{self, Cmd, Rolled, Rows, Writer, guard};
use forge_analytics_proto::views::JobReport;

/// Pooled read connections kept.
const MAX_POOL: usize = 16;
/// Timeout of reads other than log searches.
const READ_TIMEOUT: Duration = Duration::from_secs(30);
/// Commands queued for the writer.
const QUEUE: usize = 64;
/// How often the jobs run.
const JOB_EVERY: Duration = Duration::from_secs(3600);
/// The first run after start-up.
const JOB_FIRST: Duration = Duration::from_secs(60);

/// How to open the store.
#[derive(Debug, Clone)]
pub struct DuckDbConfig {
    /// `<data_dir>/analytics/duckdb`.
    pub dir: PathBuf,
    pub limits: Limits,
    /// Most rows in one write batch.
    pub batch_rows: usize,
}

/// The DuckDB analytics store.
pub struct DuckDb {
    catalog: Arc<Catalog>,
    tx: SyncSender<Cmd>,
    pool: Mutex<Vec<Connection>>,
    /// Slots for history and other short reads.
    reads: Arc<Semaphore>,
    /// Slots for log searches and histograms, apart so a slow search never
    /// holds up a history read.
    searches: Arc<Semaphore>,
    rolled: Rolled,
    retention: Mutex<Option<Retention>>,
    reports: Mutex<Vec<JobReport>>,
    limits: Mutex<Limits>,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for DuckDb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DuckDb").field("dir", &self.catalog.root()).finish_non_exhaustive()
    }
}

fn domain(e: Error) -> DomainError {
    DomainError::Internal(format!("analytics: {e}"))
}

impl DuckDb {
    /// Opens the store and starts its writer thread. Blocking: run it on a
    /// blocking thread.
    pub fn open(cfg: &DuckDbConfig, clock: Arc<dyn Clock>) -> Result<Arc<Self>, Error> {
        let catalog = Catalog::open(&cfg.dir, cfg.limits)?;
        let conn = catalog.connect()?;
        let retention = catalog.with_admin(|c| {
            let v: Option<String> =
                c.query_row("SELECT max(value) FROM meta.settings WHERE key = 'version'", [], |r| r.get(0))?;

            if let Some(v) = v.and_then(|v| v.parse::<u32>().ok())
                && v > crate::schema::VERSION
            {
                return Err(Error::Invalid(format!(
                    "the DuckDB analytics files have layout version {v}; this Forge knows up to {}",
                    crate::schema::VERSION
                )));
            }

            let r: Option<String> =
                c.query_row("SELECT max(value) FROM meta.settings WHERE key = 'retention'", [], |r| r.get(0))?;
            Ok(r.and_then(|r| serde_json::from_str::<Retention>(&r).ok()))
        })?;
        let today = clock.now().0.div_euclid(crate::parts::DAY_MS);
        let seen = writer::load_seen(&catalog, &conn, today)?;
        let rolled: Rolled = Arc::new(RwLock::new(writer::load_rolled(&conn)?));
        let (tx, rx) = std::sync::mpsc::sync_channel(QUEUE);
        let w = Writer::new(catalog.clone(), conn, clock.clone(), seen, rolled.clone());
        let rows = cfg.batch_rows.max(1);
        std::thread::Builder::new().name("duckdb-writer".into()).spawn(move || w.run(&rx, rows))?;

        Ok(Arc::new(Self {
            catalog,
            tx,
            pool: Mutex::new(Vec::new()),
            // Per kind, as many queries at once as DuckDB has threads: more
            // would only share the same cores with the writer (D-144).
            reads: Arc::new(Semaphore::new(read_slots(cfg.limits))),
            searches: Arc::new(Semaphore::new(read_slots(cfg.limits))),
            rolled,
            retention: Mutex::new(retention),
            reports: Mutex::new(Vec::new()),
            limits: Mutex::new(cfg.limits),
            clock,
        }))
    }

    /// Hands `cmd` to the writer, waiting for room off the async threads.
    async fn send(&self, cmd: Cmd) -> Result<(), Error> {
        match self.tx.try_send(cmd) {
            Ok(()) => Ok(()),
            Err(TrySendError::Disconnected(_)) => Err(Error::Stopped),
            Err(TrySendError::Full(cmd)) => {
                let tx = self.tx.clone();
                tokio::task::spawn_blocking(move || tx.send(cmd).map_err(|_| Error::Stopped))
                    .await
                    .map_err(|_| Error::Stopped)?
            }
        }
    }

    async fn rows(&self, rows: Rows) -> DomainResult<()> {
        let (reply, rx) = oneshot::channel();
        self.send(Cmd::Rows(rows, reply)).await.map_err(domain)?;

        rx.await.map_err(|_| domain(Error::Stopped))?.map_err(domain)
    }

    /// Runs `job` on the writer thread.
    async fn job(&self, job: writer::Job) -> Result<(), Error> {
        let (reply, rx) = oneshot::channel();
        self.send(Cmd::Job(job, reply)).await?;

        rx.await.map_err(|_| Error::Stopped)?
    }

    /// Runs `f` on a pooled connection off the async threads. On timeout
    /// the query is interrupted; its slot is held until it has stopped, so
    /// timed-out queries never pile up.
    async fn read<T, F>(&self, kind: ReadKind, timeout: Duration, f: F) -> Result<T, Error>
    where
        T: Send + 'static,
        F: FnOnce(&Arc<Catalog>, &Rolled, &Connection) -> Result<T, Error> + Send + 'static,
    {
        // Waiting for a slot counts against the query's time limit.
        let started = Instant::now();

        let slots = match kind {
            ReadKind::Search => &self.searches,
            ReadKind::Other => &self.reads,
        };

        let _slot = tokio::time::timeout(timeout, slots.clone().acquire_owned())
            .await
            .map_err(|_| Error::Timeout)?
            .map_err(|_| Error::Stopped)?;
        let timeout = timeout.saturating_sub(started.elapsed());

        let conn = match guard(&self.pool).pop() {
            Some(c) => c,
            None => self.catalog.connect()?,
        };

        let interrupt = conn.interrupt_handle();
        let (catalog, rolled) = (self.catalog.clone(), self.rolled.clone());
        let mut task = tokio::task::spawn_blocking(move || {
            let r = f(&catalog, &rolled, &conn);
            (conn, r)
        });
        let (conn, r) = tokio::select! {
            done = &mut task => done.map_err(|e| Error::Write(e.to_string()))?,
            () = tokio::time::sleep(timeout) => {
                interrupt.interrupt();
                let (conn, _) = task.await.map_err(|e| Error::Write(e.to_string()))?;
                (conn, Err(Error::Timeout))
            }
        };
        let mut pool = guard(&self.pool);
        if pool.len() < MAX_POOL {
            pool.push(conn);
        }

        r
    }

    /// Changes the memory limit and threads of the instance.
    pub fn set_limits(&self, l: Limits) -> Result<(), Error> {
        let mut cur = guard(&self.limits);
        if *cur != l {
            self.catalog.set_limits(l)?;
            let (old, new) = (read_slots(*cur), read_slots(l));

            for slots in [&self.reads, &self.searches] {
                if new > old {
                    slots.add_permits(new - old);
                } else {
                    // Takes effect as running queries finish.
                    slots.forget_permits(old - new);
                }
            }

            *cur = l;
        }

        Ok(())
    }

    /// The last run of each job.
    #[must_use]
    pub fn reports(&self) -> Vec<JobReport> {
        guard(&self.reports).clone()
    }

    /// The files of the store (tests and status).
    #[must_use]
    pub fn catalog(&self) -> &Arc<Catalog> {
        &self.catalog
    }

    fn record(&self, name: &str, started: Instant, at: i64, r: Result<u64, Error>) {
        let report = JobReport {
            name: name.to_owned(),
            last_run: at,
            duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            error: r.as_ref().err().map(ToString::to_string),
            done: *r.as_ref().unwrap_or(&0),
        };

        if let Err(e) = &r {
            tracing::warn!(job = name, error = %e, "DuckDB analytics job failed");
        }

        let mut reports = guard(&self.reports);
        reports.retain(|j| j.name != name);
        reports.push(report);
        reports.sort_by(|a, b| a.name.cmp(&b.name));
    }

    /// Runs `name` on the writer thread and records the outcome.
    async fn run_job(&self, name: &'static str, f: fn(&mut Writer, &Retention) -> Result<u64, Error>) {
        let r = guard(&self.retention).unwrap_or(DEFAULT_RETENTION);
        let (started, at) = (Instant::now(), self.clock.now().0);
        let (done_tx, done_rx) = oneshot::channel();
        let outcome = self
            .job(Box::new(move |w| {
                let _ = done_tx.send(f(w, &r));
                Ok(())
            }))
            .await;

        let r = match outcome {
            Ok(()) => done_rx.await.unwrap_or(Err(Error::Stopped)),
            Err(e) => Err(e),
        };

        self.record(name, started, at, r);
    }

    /// Runs every job now: rollups, retention, compaction.
    pub async fn run_jobs_once(&self) {
        self.run_job("rollups", |w, _| jobs::rollups(w)).await;
        self.run_job("retention", jobs::retention).await;
        self.run_job("compaction", |w, _| jobs::compaction(w)).await;
    }

    /// Runs the jobs a minute after start-up, then hourly, until `shutdown`.
    /// It sleeps in between: no polling.
    pub async fn run_jobs(self: Arc<Self>, mut shutdown: watch::Receiver<bool>) {
        let mut wait = JOB_FIRST;

        loop {
            tokio::select! {
                () = tokio::time::sleep(wait) => {}
                _ = shutdown.changed() => return,
            }
            self.run_jobs_once().await;
            wait = JOB_EVERY;
        }
    }

    fn save_retention(&self, r: Retention) -> Result<(), Error> {
        let json = serde_json::to_string(&r).map_err(|e| Error::Invalid(e.to_string()))?;
        self.catalog.with_admin(|c| {
            c.execute_batch("BEGIN")?;
            let res = (|| -> Result<(), Error> {
                c.execute("DELETE FROM meta.settings WHERE key IN ('retention', 'version')", [])?;
                c.execute("INSERT INTO meta.settings VALUES ('retention', ?)", [&json])?;
                c.execute("INSERT INTO meta.settings VALUES ('version', ?)", [crate::schema::VERSION.to_string()])?;
                Ok(())
            })();

            match res {
                Ok(()) => Ok(c.execute_batch("COMMIT")?),
                Err(e) => {
                    let _ = c.execute_batch("ROLLBACK");
                    Err(e)
                }
            }
        })?;
        *guard(&self.retention) = Some(r);

        Ok(())
    }
}

/// Which slots a read takes.
#[derive(Debug, Clone, Copy)]
enum ReadKind {
    Search,
    Other,
}

/// Queries of each kind DuckDB runs at once for `l`: its thread count.
#[must_use]
pub fn read_slots(l: Limits) -> usize {
    usize::try_from(l.threads).unwrap_or(1).max(1)
}

/// Used by the jobs before the first `migrate`.
const DEFAULT_RETENTION: Retention =
    Retention { node_metrics_days: 7, top_processes_days: 7, node_rollup_days: 365, app_logs_days: 30 };

#[async_trait]
impl SpoolFileWriter for DuckDb {
    async fn write_spool_file(&self, file: SpoolRows) -> DomainResult<FileWrite> {
        let (reply, rx) = oneshot::channel();
        self.send(Cmd::Spool(file, reply)).await.map_err(domain)?;

        rx.await.map_err(|_| domain(Error::Stopped))?.map_err(domain)
    }
}

#[async_trait]
impl AnalyticsStore for DuckDb {
    fn engine(&self) -> &'static str {
        "duckdb"
    }

    async fn migrate(&self, retention: Retention) -> DomainResult<()> {
        self.save_retention(retention).map_err(domain)
    }

    async fn health(&self) -> DomainResult<()> {
        self.read(ReadKind::Other, READ_TIMEOUT, |_, _, c| Ok(c.execute_batch("SELECT 1")?)).await.map_err(domain)
    }

    /// Stores the retention; files past it are dropped now.
    async fn apply_retention(&self, retention: Retention) -> DomainResult<()> {
        self.save_retention(retention).map_err(domain)?;
        self.run_job("retention", jobs::retention).await;

        Ok(())
    }

    async fn effective_retention(&self) -> DomainResult<Retention> {
        guard(&self.retention).ok_or_else(|| DomainError::Internal("the DuckDB store is not prepared".into()))
    }

    async fn engine_stats(&self) -> DomainResult<EngineStats> {
        let limit = u64::from(guard(&self.limits).memory_mb) * 1024 * 1024;
        let used = self
            .read(ReadKind::Other, READ_TIMEOUT, |_, _, c| {
                Ok(c.query_row("SELECT sum(memory_usage_bytes)::UBIGINT FROM duckdb_memory()", [], |r| {
                    r.get::<_, Option<u64>>(0)
                })?)
            })
            .await
            .map_err(domain)?;
        let dir = self.catalog.root().to_path_buf();
        let free = tokio::task::spawn_blocking(move || free_disk_bytes(&dir)).await.ok().flatten();

        Ok(EngineStats { memory_used_bytes: used, server_memory_limit_bytes: Some(limit), free_disk_bytes: free })
    }

    async fn write_node_minutes(&self, rows: &[NodeMinute]) -> DomainResult<()> {
        self.rows(Rows { minutes: rows.to_vec(), ..Rows::default() }).await
    }

    async fn write_top_processes(&self, rows: &[NodeProcessMinute]) -> DomainResult<()> {
        self.rows(Rows { processes: rows.to_vec(), ..Rows::default() }).await
    }

    async fn node_series(
        &self,
        server: ServerId,
        from: Timestamp,
        to: Timestamp,
        resolution: Resolution,
    ) -> DomainResult<Vec<NodePoint>> {
        self.read(ReadKind::Other, READ_TIMEOUT, move |cat, rolled, c| {
            read::node_series(cat, rolled, c, server, (from, to), resolution)
        })
        .await
        .map_err(domain)
    }

    async fn top_processes(
        &self,
        server: ServerId,
        from: Timestamp,
        to: Timestamp,
        by: TopBy,
        limit: u32,
    ) -> DomainResult<Vec<ProcessUsage>> {
        self.read(ReadKind::Other, READ_TIMEOUT, move |cat, _, c| {
            read::top_processes(cat, c, server, (from, to), by, limit)
        })
        .await
        .map_err(domain)
    }

    async fn write_container_minutes(&self, rows: &[ContainerMinute]) -> DomainResult<()> {
        self.rows(Rows { containers: rows.to_vec(), ..Rows::default() }).await
    }

    async fn container_series(
        &self,
        server: ServerId,
        container: Option<&str>,
        window: (Timestamp, Timestamp),
        resolution: Resolution,
        limit: u32,
    ) -> DomainResult<Vec<ContainerSeries>> {
        let container = container.map(str::to_owned);
        self.read(ReadKind::Other, READ_TIMEOUT, move |cat, rolled, c| {
            let q = read::ContainerQuery { server, container: container.as_deref(), window, resolution, limit };
            read::container_series(cat, rolled, c, &q)
        })
        .await
        .map_err(domain)
    }

    async fn write_logs(&self, events: &[LogEvent]) -> DomainResult<()> {
        self.rows(Rows { logs: events.to_vec(), ..Rows::default() }).await
    }

    async fn query_logs(&self, q: &LogQuery) -> DomainResult<Vec<LogEvent>> {
        let q2 = q.clone();
        self.read(ReadKind::Search, search_timeout(q), move |cat, _, c| read::query_logs(cat, c, &q2))
            .await
            .map_err(search_err)
    }

    async fn log_histogram(&self, q: &LogQuery, step_ms: i64) -> DomainResult<Vec<LogBucket>> {
        let q2 = q.clone();
        self.read(ReadKind::Search, search_timeout(q), move |cat, _, c| read::log_histogram(cat, c, &q2, step_ms))
            .await
            .map_err(search_err)
    }
}

fn search_timeout(q: &LogQuery) -> Duration {
    match q.limits.timeout_secs {
        0 => READ_TIMEOUT,
        s => Duration::from_secs(u64::from(s)),
    }
}

fn search_err(e: Error) -> DomainError {
    match e {
        Error::Timeout => DomainError::Internal("analytics: the search took longer than its time limit".into()),
        e => domain(e),
    }
}

/// Free space on the volume holding `dir`. `None` when it can't be read:
/// the stat is informational, so an unknown value is shown as unknown.
#[cfg(unix)]
fn free_disk_bytes(dir: &std::path::Path) -> Option<u64> {
    rustix::fs::statvfs(dir).ok().map(|s| s.f_bavail.saturating_mul(s.f_frsize))
}

/// Not reported on Windows yet (no statvfs); the console shows it as unknown.
#[cfg(not(unix))]
fn free_disk_bytes(_dir: &std::path::Path) -> Option<u64> {
    None
}
