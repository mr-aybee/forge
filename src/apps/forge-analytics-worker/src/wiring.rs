//! The worker's composition root (D-141): the only place that picks the
//! analytics store and wires the services together. Every other module
//! takes `Arc<dyn AnalyticsStore>` or the services built here.

use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use anyhow::Context;
use forge_analytics::{ClickHouse, ClickHouseSpoolWriter, ClickHouseTarget};
use forge_analytics_duckdb::{DuckDb, DuckDbConfig, Limits};
use forge_analytics_proto::config::{StoreSelection, WorkerConfig};
use forge_analytics_proto::{Reply, Request, Response, WorkerError};
use forge_core::Clock;
use forge_core::analytics::{AnalyticsStore, PerTableSpoolWriter, SpoolFileWriter};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::analytics::Analytics;
use crate::batcher::LogBatcher;
use crate::control::{Control, ControlDeps};
use crate::embedded::{self, EngineConfig, ManagedEngine};
use crate::ingest::{FREE_DISK_UNKNOWN, Ingest, IngestDeps};
use crate::maps::Maps;
use crate::memory::MemoryStore;
use crate::search::Search;

/// Every service of the worker.
#[derive(Debug)]
pub struct Worker {
    analytics: Arc<Analytics>,
    ingest: Arc<Ingest>,
    uploads: std::sync::OnceLock<Arc<crate::ingest::upload::Uploads>>,
    /// The agent listener's TLS, when it runs (D-145).
    listener_tls: std::sync::OnceLock<Arc<crate::ingest::upload::ListenerTls>>,
    search: Search,
    control: Arc<Control>,
    maps: Arc<Maps>,
    managed: Option<Arc<ManagedEngine>>,
    duck: Option<Arc<DuckDb>>,
    tuning: watch::Receiver<forge_analytics_proto::tuning::Tuning>,
    clock: Arc<dyn Clock>,
}

/// The store chosen for a worker, with what only some stores have.
#[derive(Clone)]
pub struct Stores {
    pub store: Arc<dyn AnalyticsStore>,
    /// Writes spool files; `None`: one insert per table on `store`.
    pub spool: Option<Arc<dyn SpoolFileWriter>>,
    /// The embedded engine's supervisor handle.
    pub managed: Option<Arc<ManagedEngine>>,
    /// The in-process DuckDB store: its limits and hourly jobs.
    pub duck: Option<Arc<DuckDb>>,
}

impl std::fmt::Debug for Stores {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stores").field("engine", &self.store.engine()).finish_non_exhaustive()
    }
}

impl Stores {
    /// Only `store`.
    #[must_use]
    pub fn of(store: Arc<dyn AnalyticsStore>) -> Self {
        Self { store, spool: None, managed: None, duck: None }
    }
}

/// DuckDB's memory limit and threads for `t`: the memory budget, and the
/// CPU cap rounded up (no cap: [`DUCKDB_DEFAULT_THREADS`]).
#[must_use]
pub fn duck_limits(t: &forge_analytics_proto::tuning::Tuning) -> Limits {
    Limits {
        memory_mb: t.memory_budget_mb,
        threads: crate::embedded::cpu_cap(t.cpu_quota_vcpu).unwrap_or(DUCKDB_DEFAULT_THREADS),
    }
}

/// DuckDB threads when no CPU cap is set.
pub const DUCKDB_DEFAULT_THREADS: u32 = 2;

/// The store for `selection`; for the embedded engine also its supervisor
/// handle. Connects lazily: the schema is applied by the flush loop, so a
/// store that is down never stops the worker from starting.
async fn store(config: &WorkerConfig, password: Option<&str>, clock: Arc<dyn Clock>) -> anyhow::Result<Stores> {
    // One batch: what queued while the last one was written, up to the insert size.
    let batch_rows = usize::try_from(config.tuning.ingest_max_batch_rows).unwrap_or(20_000);
    let target = match &config.store {
        StoreSelection::Memory => return Ok(Stores::of(Arc::new(MemoryStore::default()))),
        StoreSelection::Duckdb { dir } => {
            let cfg = DuckDbConfig { dir: dir.clone(), limits: duck_limits(&config.tuning), batch_rows };
            let duck = tokio::task::spawn_blocking(move || DuckDb::open(&cfg, clock)).await??;
            return Ok(Stores { store: duck.clone(), spool: Some(duck.clone()), managed: None, duck: Some(duck) });
        }
        StoreSelection::Clickhouse { url } => ClickHouseTarget::from_url(url, password, "clickhouse"),
        StoreSelection::Embedded { paths, port, database } => {
            let (p, engine_config) = ((**paths).clone(), EngineConfig::of(*port, &config.tuning));
            let pw = tokio::task::spawn_blocking(move || -> anyhow::Result<String> {
                let pw = embedded::password(&p)?;
                embedded::write_config(&p, &engine_config, &pw)?;
                Ok(pw)
            })
            .await??;
            let target = ClickHouseTarget::from_url(&embedded::url(*port, database), Some(&pw), "embedded");
            let managed = Arc::new(ManagedEngine::new((**paths).clone(), *port));
            let store =
                ClickHouse::new(target.map_err(|e| anyhow::anyhow!("{e}"))?).map_err(|e| anyhow::anyhow!("{e}"))?;
            return Ok(Stores { managed: Some(managed), ..clickhouse(store, batch_rows) });
        }
    }
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(clickhouse(ClickHouse::new(target).map_err(|e| anyhow::anyhow!("{e}"))?, batch_rows))
}

/// A ClickHouse store with its group-commit spool writer (D-145).
fn clickhouse(store: ClickHouse, batch_rows: usize) -> Stores {
    let spool = Arc::new(ClickHouseSpoolWriter::start(store.clone(), batch_rows));
    Stores { spool: Some(spool), ..Stores::of(Arc::new(store)) }
}

impl Worker {
    /// Builds the worker for `config`. Must run inside a Tokio runtime (the
    /// batch writer task starts here).
    pub async fn build(config: &WorkerConfig, password: Option<&str>, clock: Arc<dyn Clock>) -> anyhow::Result<Self> {
        let stores = store(config, password, clock.clone()).await.context("analytics store")?;
        Ok(Self::with_stores(config, stores, clock))
    }

    /// Builds the worker around an existing store (tests).
    #[must_use]
    pub fn with_store(
        config: &WorkerConfig,
        store: Arc<dyn AnalyticsStore>,
        managed: Option<Arc<ManagedEngine>>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self::with_stores(config, Stores { managed, ..Stores::of(store) }, clock)
    }

    /// Builds the worker around `stores`.
    #[must_use]
    pub fn with_stores(config: &WorkerConfig, stores: Stores, clock: Arc<dyn Clock>) -> Self {
        let Stores { store, spool, managed, duck } = stores;
        let spool = spool.unwrap_or_else(|| Arc::new(PerTableSpoolWriter(store.clone())));
        let analytics = Arc::new(Analytics::new(store, config.tuning.retention()));
        let (tuning_tx, tuning) = watch::channel(config.tuning);
        let batcher = LogBatcher::start(analytics.clone(), tuning.clone());
        let free_disk = Arc::new(AtomicU64::new(FREE_DISK_UNKNOWN));
        let maps = Arc::new(Maps::default());
        let ingest = Arc::new(Ingest::new(IngestDeps {
            analytics: analytics.clone(),
            batcher: batcher.clone(),
            maps: maps.clone(),
            tuning: tuning.clone(),
            free_disk: free_disk.clone(),
            spool,
        }));
        let search = Search::new(analytics.clone(), tuning.clone());
        let control = Arc::new(Control::new(ControlDeps {
            analytics: analytics.clone(),
            batcher,
            managed: managed.clone(),
            free_disk,
            tuning: tuning_tx,
            duck: duck.clone(),
        }));
        Self {
            analytics,
            ingest,
            uploads: std::sync::OnceLock::new(),
            listener_tls: std::sync::OnceLock::new(),
            search,
            control,
            maps,
            managed,
            duck,
            tuning,
            clock,
        }
    }

    /// The agent upload handler (D-142), built once: `state_dir` holds the
    /// stored file ids.
    pub fn uploads(
        &self,
        state_dir: &std::path::Path,
        max_uploads: usize,
    ) -> std::io::Result<Arc<crate::ingest::upload::Uploads>> {
        if let Some(u) = self.uploads.get() {
            return Ok(u.clone());
        }
        let seen = crate::ingest::seen::SeenLog::open(state_dir)?;
        let u =
            Arc::new(crate::ingest::upload::Uploads::new(self.ingest.clone(), seen, max_uploads, self.clock.clone()));
        Ok(self.uploads.get_or_init(|| u).clone())
    }

    /// The agent listener's TLS from the identity handed over at start;
    /// later certificates replace it through [`Request::SetAgentTls`].
    pub fn listener_tls(
        &self,
        ca_pem: &str,
        tls: &forge_analytics_proto::config::AgentTls,
    ) -> anyhow::Result<Arc<crate::ingest::upload::ListenerTls>> {
        if let Some(t) = self.listener_tls.get() {
            return Ok(t.clone());
        }
        let t = Arc::new(crate::ingest::upload::ListenerTls::new(ca_pem, &tls.cert_pem, &tls.key_pem)?);
        Ok(self.listener_tls.get_or_init(|| t).clone())
    }

    /// Applies the schema; true once the store is ready (tests and start-up).
    pub async fn prepare(&self) -> bool {
        self.analytics.prepare().await
    }

    /// Starts the background loops: the minute flush, usage sampling and,
    /// for the embedded engine, its supervisor. They stop on `shutdown`.
    #[must_use]
    pub fn start(&self, shutdown: &watch::Receiver<bool>) -> Vec<JoinHandle<()>> {
        let mut tasks = vec![
            tokio::spawn(crate::analytics::run(self.analytics.clone(), self.clock.clone(), shutdown.clone())),
            tokio::spawn(crate::control::run(self.control.clone(), shutdown.clone())),
        ];
        if let Some(duck) = &self.duck {
            tasks.push(tokio::spawn(duck.clone().run_jobs(shutdown.clone())));
        }
        if let Some(engine) = &self.managed {
            tasks.push(tokio::spawn(embedded::supervise(engine.clone(), self.tuning.clone(), shutdown.clone())));
        }
        tasks
    }

    /// Answers one request.
    pub async fn handle(&self, req: Request, blob: Vec<u8>) -> Response {
        match req {
            Request::Hello { .. } => Err(WorkerError::Protocol { reason: "already authenticated".into() }),
            Request::Ping => Ok(Reply::Pong { pid: std::process::id() }),
            Request::PushMaps(m) => {
                self.maps.replace(m).map_err(|e| WorkerError::Invalid { reason: e.to_string() })?;
                // Ids of nodes that left are forgotten (in the background).
                if let Some(u) = self.uploads.get() {
                    let u = u.clone();
                    tokio::spawn(async move { u.prune().await });
                }
                Ok(Reply::Ok)
            }
            Request::SetTuning(t) => {
                self.control.set_tuning(t);
                Ok(Reply::Ok)
            }
            Request::Report => Ok(Reply::Report(Box::new(self.control.report()))),
            Request::Apply { settings, tuning } => Ok(Reply::Applied(self.control.apply(&settings, tuning).await)),
            Request::RestartEngine => self.control.restart_engine().map(|()| Reply::Ok),
            Request::SetAgentTls(tls) => {
                let current = self
                    .listener_tls
                    .get()
                    .ok_or_else(|| WorkerError::Invalid { reason: "this worker has no agent listener".into() })?;
                current
                    .replace(&tls.cert_pem, &tls.key_pem, self.clock.now())
                    .map_err(|e| WorkerError::Invalid { reason: format!("the certificate does not load: {e}") })?;
                tracing::info!("agent listener certificate replaced");
                Ok(Reply::Ok)
            }
            Request::IngestLogs { service, now } => {
                Ok(Reply::Ingested { count: self.ingest.logs(service, now, &blob).await? })
            }
            Request::Telemetry { server, now, metrics_len } => {
                let split = usize::try_from(metrics_len).unwrap_or(usize::MAX);
                if split > blob.len() {
                    return Err(WorkerError::Protocol { reason: "metrics_len exceeds the payload".into() });
                }
                let (metrics, logs) = blob.split_at(split);
                Ok(Reply::TelemetryAck { ack: self.ingest.telemetry(server, now, metrics, logs).await? })
            }
            Request::NodeHistory { server, from, to } => {
                Ok(Reply::NodeHistory(self.analytics.node_history(server, from, to).await?))
            }
            Request::NodeReport { server, from, to } => {
                Ok(Reply::NodeReport(Box::new(self.analytics.node_report(server, from, to).await?)))
            }
            Request::ContainerHistory { server, container, from, to, limit } => Ok(Reply::ContainerHistory(
                self.analytics.container_history(server, container.as_deref(), (from, to), limit).await?,
            )),
            Request::QueryLogs { query, now } => Ok(Reply::Logs(self.search.query(query, now).await?)),
            Request::LogHistogram { query, now } => Ok(Reply::Histogram(self.search.histogram(query, now).await?)),
        }
    }
}
