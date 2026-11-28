//! Analytics: node history kept in a separate analytics database (D-127).
//!
//! The operational database never holds these rows. Analytics refers to
//! operational records by id only, and operations never wait for it.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{DomainResult, ServerId, Timestamp};

/// One node's resources aggregated over one minute.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeMinute {
    pub server_id: ServerId,
    /// Start of the minute, epoch ms.
    pub minute: Timestamp,
    pub cpu_avg: f32,
    pub cpu_max: f32,
    pub mem_used: u64,
    pub mem_total: u64,
    pub swap_used: u64,
    pub disk_used: u64,
    pub disk_total: u64,
    /// Average bytes per second received / sent.
    pub rx: u64,
    pub tx: u64,
    pub load1: f32,
    /// Heartbeat samples folded into this minute.
    pub samples: u32,
}

/// One process in a node's top list for one minute.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeProcessMinute {
    pub server_id: ServerId,
    pub minute: Timestamp,
    pub pid: u32,
    pub name: String,
    pub user: String,
    pub cpu: f32,
    pub mem: u64,
    pub cmd: String,
}

/// One container's resources over one minute.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContainerMinute {
    pub server_id: ServerId,
    /// Start of the minute, epoch ms.
    pub minute: Timestamp,
    pub container_name: String,
    /// Short id; changes when the container is re-created.
    pub container_id: String,
    pub image: String,
    /// The service the container belongs to, when Forge could tell.
    pub service_id: Option<crate::ServiceId>,
    /// Stack (`forge.stack` label, else the compose project), when any.
    pub stack: Option<String>,
    /// CPU percent of one core (may exceed 100 on several cores).
    pub cpu: f32,
    pub mem_used: u64,
    /// The container's memory limit (the host's memory when unlimited).
    pub mem_limit: u64,
    /// Average bytes per second received / sent.
    pub rx: u64,
    pub tx: u64,
    /// Average bytes per second read from / written to block devices.
    pub block_read: u64,
    pub block_write: u64,
    /// Docker's restart count.
    pub restarts: u32,
}

/// One point of a container's history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContainerPoint {
    /// Epoch ms (start of the minute or hour).
    pub t: i64,
    pub cpu_avg: f32,
    pub cpu_max: f32,
    pub mem_used: u64,
    pub mem_limit: u64,
    pub rx: u64,
    pub tx: u64,
    pub block_read: u64,
    pub block_write: u64,
    pub restarts: u32,
}

/// One container's points in a window, with what it was last seen as.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContainerSeries {
    pub container_name: String,
    pub container_id: String,
    pub image: String,
    pub service_id: Option<crate::ServiceId>,
    pub stack: Option<String>,
    pub points: Vec<ContainerPoint>,
}

/// Point resolution of a history query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Resolution {
    /// Per-minute rows (kept `node_metrics_days`).
    Minute,
    /// Hourly rollups (kept `node_rollup_days`).
    Hour,
}

impl Resolution {
    /// Minutes for windows up to two days, hours beyond.
    #[must_use]
    pub fn for_window(from: Timestamp, to: Timestamp) -> Self {
        if to.0 - from.0 <= 2 * 24 * 3_600_000 { Self::Minute } else { Self::Hour }
    }
}

/// One point of a node's history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodePoint {
    /// Epoch ms (start of the minute or hour).
    pub t: i64,
    pub cpu_avg: f32,
    pub cpu_max: f32,
    pub mem_used: u64,
    pub mem_total: u64,
    pub disk_used: u64,
    pub disk_total: u64,
    pub rx: u64,
    pub tx: u64,
    pub load1: f32,
}

/// How to rank processes in a report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TopBy {
    Cpu,
    Mem,
}

/// One process over a report window (grouped by name and user).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProcessUsage {
    pub name: String,
    pub user: String,
    pub cpu_avg: f32,
    pub cpu_max: f32,
    pub mem_avg: u64,
    pub mem_max: u64,
    /// Minutes the process was in the top list.
    pub minutes: u32,
    /// A command line seen for it.
    pub cmd: String,
}

/// Retention per stream, in days.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Retention {
    /// Per-minute node metrics.
    pub node_metrics_days: u32,
    /// Per-minute top processes.
    pub top_processes_days: u32,
    /// Hourly node rollups.
    pub node_rollup_days: u32,
    /// Service logs, counted from ingestion time.
    pub app_logs_days: u32,
}

/// What the analytics engine reports about itself. Each value is `None`
/// when the engine does not expose it (or the user may not read it).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineStats {
    /// Memory the engine tracks as in use, bytes.
    pub memory_used_bytes: Option<u64>,
    /// The engine's own memory budget (`max_server_memory_usage`), bytes;
    /// `Some(0)` means the engine derives it from the host's memory.
    pub server_memory_limit_bytes: Option<u64>,
    /// Free space on the engine's emptiest disk, bytes.
    pub free_disk_bytes: Option<u64>,
}

/// The analytics database. Implemented by ClickHouse (a server, or the
/// Forge-managed embedded engine) and, for small installs, PostgreSQL.
#[async_trait]
pub trait AnalyticsStore: Send + Sync {
    /// Engine name for messages (`clickhouse`, `embedded`, `postgres`).
    fn engine(&self) -> &'static str;
    /// Creates or upgrades the tables and applies the retention.
    async fn migrate(&self, retention: Retention) -> DomainResult<()>;
    /// Cheap reachability check.
    async fn health(&self) -> DomainResult<()>;
    /// Changes the retention of the existing tables (`ALTER TABLE .. MODIFY
    /// TTL`). Rows older than the new limit are deleted by background merges.
    async fn apply_retention(&self, retention: Retention) -> DomainResult<()>;
    /// The retention the tables actually have, read back from the engine.
    async fn effective_retention(&self) -> DomainResult<Retention>;
    /// Memory, memory budget and free disk as the engine reports them.
    async fn engine_stats(&self) -> DomainResult<EngineStats>;
    async fn write_node_minutes(&self, rows: &[NodeMinute]) -> DomainResult<()>;
    async fn write_top_processes(&self, rows: &[NodeProcessMinute]) -> DomainResult<()>;
    /// History of one node in `[from, to)`, oldest first.
    async fn node_series(
        &self,
        server: ServerId,
        from: Timestamp,
        to: Timestamp,
        resolution: Resolution,
    ) -> DomainResult<Vec<NodePoint>>;
    /// The processes that used the most CPU or memory in `[from, to)`.
    async fn top_processes(
        &self,
        server: ServerId,
        from: Timestamp,
        to: Timestamp,
        by: TopBy,
        limit: u32,
    ) -> DomainResult<Vec<ProcessUsage>>;
    /// Stores per-minute container samples.
    async fn write_container_minutes(&self, rows: &[ContainerMinute]) -> DomainResult<()>;
    /// History of the containers of one node in `[from, to)`, oldest point
    /// first; one container when `container` names it. At most `limit`
    /// containers, by name.
    async fn container_series(
        &self,
        server: ServerId,
        container: Option<&str>,
        window: (Timestamp, Timestamp),
        resolution: Resolution,
        limit: u32,
    ) -> DomainResult<Vec<ContainerSeries>>;
    /// Stores service log events (D-128).
    async fn write_logs(&self, events: &[crate::logs::LogEvent]) -> DomainResult<()>;
    /// One page of events matching `q`, newest first.
    async fn query_logs(&self, q: &crate::logs::LogQuery) -> DomainResult<Vec<crate::logs::LogEvent>>;
    /// Event counts per level in buckets of `step_ms` over `q.from..q.to`.
    async fn log_histogram(&self, q: &crate::logs::LogQuery, step_ms: i64)
    -> DomainResult<Vec<crate::logs::LogBucket>>;
}

/// Longest node or container history window one request may ask for.
pub const MAX_WINDOW_MS: i64 = 400 * 24 * 3_600_000;
/// Most containers in one history answer.
pub const MAX_HISTORY_CONTAINERS: u32 = 200;
/// Widest log histogram window.
pub const MAX_HISTOGRAM_WINDOW_MS: i64 = 31 * 24 * 3_600_000;

/// A node's history in a window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeHistory {
    pub server_id: ServerId,
    pub from: i64,
    pub to: i64,
    pub resolution: Resolution,
    pub points: Vec<NodePoint>,
}

/// The containers of a node in a window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContainerHistory {
    pub server_id: ServerId,
    pub from: i64,
    pub to: i64,
    pub resolution: Resolution,
    pub containers: Vec<ContainerSeries>,
}

/// The highest value of one measure in a window and when it happened.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Peak {
    pub t: i64,
    pub value: f64,
}

/// A node report: trends, peaks and the processes that used the most.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeReport {
    #[serde(flatten)]
    pub history: NodeHistory,
    pub cpu_peak: Option<Peak>,
    pub mem_peak: Option<Peak>,
    pub disk_peak: Option<Peak>,
    pub top_cpu: Vec<ProcessUsage>,
    pub top_mem: Vec<ProcessUsage>,
}

/// One histogram bucket with counts per level.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistogramBucket {
    pub t: i64,
    pub counts: std::collections::HashMap<crate::logs::LogLevel, u64>,
}

/// Log counts per level over a window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Histogram {
    pub from: i64,
    pub to: i64,
    pub step_ms: i64,
    pub buckets: Vec<HistogramBucket>,
}

/// Most entries in each list of [`AnalyticsMaps`].
pub const MAX_MAP_ENTRIES: usize = 100_000;

/// An ingest key's token hash and the service it writes for.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct KeyService {
    pub token_hash: String,
    pub service_id: crate::ServiceId,
}

/// A service placed on a node, by its `forge.app` label value
/// (`<environment>/<name>`): container attribution.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AppKey {
    pub server_id: ServerId,
    pub app: String,
    pub service_id: crate::ServiceId,
}

/// A node a service runs on, with the lower-case names an event may use for
/// it (hostname, its first label, the console name).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Placement {
    pub service_id: crate::ServiceId,
    pub server_id: ServerId,
    pub names: Vec<String>,
}

/// What the control plane knows and the analytics worker needs to attribute
/// data: pushed on change and whenever the worker (re)connects.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalyticsMaps {
    /// Nodes that are enrolled and not revoked.
    pub active_nodes: Vec<ServerId>,
    pub key_services: Vec<KeyService>,
    pub app_keys: Vec<AppKey>,
    pub placements: Vec<Placement>,
    /// The current certificate of each active node (D-145). The worker's
    /// upload listener accepts a node's certificate only when its serial is
    /// this one, so a replaced (renewed) or revoked certificate is refused.
    #[serde(default)]
    pub node_certs: Vec<NodeCert>,
}

/// A node's current agent certificate.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodeCert {
    pub server_id: ServerId,
    /// The serial in lower-case hex, as `forge_pki` prints it.
    pub serial: String,
}

impl AnalyticsMaps {
    /// Whether every list is within [`MAX_MAP_ENTRIES`].
    #[must_use]
    pub fn within_limits(&self) -> bool {
        [
            self.active_nodes.len(),
            self.key_services.len(),
            self.app_keys.len(),
            self.placements.len(),
            self.node_certs.len(),
        ]
        .iter()
        .all(|n| *n <= MAX_MAP_ENTRIES)
    }
}

/// Why analytics could not serve a read or take data.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AnalyticsError {
    /// `[analytics] engine = "off"`.
    #[error("analytics is off on this server")]
    Off,
    /// The analytics worker process is not running or not answering.
    #[error("the analytics worker is not running")]
    WorkerDown,
    /// The analytics database is unreachable or refused the request.
    #[error("analytics is unavailable: {0}")]
    Unavailable(String),
    /// The ingest queue is full; retry shortly.
    #[error("log ingest is busy; retry shortly")]
    Busy,
    /// The engine has less free disk than the configured minimum.
    #[error("the analytics database is low on disk space")]
    DiskLow,
    /// Every search slot is in use; retry shortly.
    #[error("too many log searches are running; retry shortly")]
    SearchBusy,
    /// The request itself is wrong (window, filter).
    #[error("{0}")]
    Invalid(String),
    /// An ingest body that cannot be read.
    #[error("{0}")]
    BadBody(String),
}

/// Heartbeat telemetry forwarded as the node sent it (protobuf bytes).
#[derive(Debug, Clone, Copy)]
pub struct NodeTelemetry<'a> {
    /// The node on the authenticated agent link.
    pub server: ServerId,
    /// When the control plane received it.
    pub now: Timestamp,
    /// An encoded `NodeMetrics` (empty: none).
    pub metrics: &'a [u8],
    /// An encoded `ContainerLogBatch` (empty: none).
    pub container_logs: &'a [u8],
}

/// Reads from the analytics database (node and container history, logs).
#[async_trait]
pub trait AnalyticsReader: Send + Sync + std::fmt::Debug {
    async fn node_history(&self, server: ServerId, from: i64, to: i64) -> Result<NodeHistory, AnalyticsError>;
    async fn node_report(&self, server: ServerId, from: i64, to: i64) -> Result<NodeReport, AnalyticsError>;
    /// Every container of `server` (up to `limit`), or the one named.
    async fn container_history(
        &self,
        server: ServerId,
        container: Option<String>,
        window: (i64, i64),
        limit: u32,
    ) -> Result<ContainerHistory, AnalyticsError>;
    /// One page of events; `q` already holds only what the caller may read.
    async fn query_logs(&self, q: &crate::logs::LogQuery) -> Result<Vec<crate::logs::LogEvent>, AnalyticsError>;
    /// Counts per level over `q.from..q.to` (default: the last 24 hours).
    async fn log_histogram(&self, q: &crate::logs::LogQuery) -> Result<Histogram, AnalyticsError>;
}

/// Hands data to the analytics database without decoding it.
#[async_trait]
pub trait TelemetrySink: Send + Sync + std::fmt::Debug {
    /// Stores a Seq/CLEF body sent for `service`; returns the events stored.
    async fn ingest_logs(
        &self,
        service: crate::ServiceId,
        now: Timestamp,
        body: &[u8],
    ) -> Result<usize, AnalyticsError>;
    /// Stores a heartbeat's metrics and container lines; returns the
    /// container batch `seq` to acknowledge (0: none stored).
    async fn forward_telemetry(&self, t: NodeTelemetry<'_>) -> Result<u64, AnalyticsError>;
}

/// One resource sample of a node, folded into a [`MinuteBucket`].
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct NodeSample {
    pub cpu: f32,
    pub mem_used: u64,
    pub mem_total: u64,
    pub swap_used: u64,
    pub disk_used: u64,
    pub disk_total: u64,
    pub rx: u64,
    pub tx: u64,
    pub load1: f32,
}

/// A node's samples within one minute: the agent folds its samples into
/// one of these and writes the closed minute to its spool (D-142); the
/// worker does the same for old agents that still send samples in
/// heartbeats.
#[derive(Debug, Clone, PartialEq)]
pub struct MinuteBucket {
    minute: i64,
    cpu_sum: f64,
    cpu_max: f32,
    last: NodeSample,
    rx_sum: u128,
    tx_sum: u128,
    samples: u32,
}

impl MinuteBucket {
    /// Start of the minute holding `ts_ms`.
    #[must_use]
    pub const fn minute_of(ts_ms: i64) -> i64 {
        ts_ms - ts_ms.rem_euclid(60_000)
    }

    #[must_use]
    pub fn new(minute: i64) -> Self {
        Self { minute, cpu_sum: 0.0, cpu_max: 0.0, last: NodeSample::default(), rx_sum: 0, tx_sum: 0, samples: 0 }
    }

    #[must_use]
    pub const fn minute(&self) -> i64 {
        self.minute
    }

    pub fn add(&mut self, s: &NodeSample) {
        let cpu = if s.cpu.is_finite() { s.cpu.max(0.0) } else { 0.0 };
        self.cpu_sum += f64::from(cpu);
        self.cpu_max = self.cpu_max.max(cpu);
        self.rx_sum += u128::from(s.rx);
        self.tx_sum += u128::from(s.tx);
        self.last = *s;
        self.samples = self.samples.saturating_add(1);
    }

    /// The minute's row: averages of CPU and traffic, the last reading of
    /// the gauges.
    #[must_use]
    pub fn close(&self) -> crate::spool::NodeMinuteRow {
        let n = self.samples.max(1);
        #[allow(clippy::cast_possible_truncation)] // reason: an average of f32 values fits f32
        let cpu_avg = (self.cpu_sum / f64::from(n)) as f32;
        let avg = |s: u128| u64::try_from(s / u128::from(n)).unwrap_or(u64::MAX);

        crate::spool::NodeMinuteRow {
            minute: self.minute,
            cpu_avg,
            cpu_max: self.cpu_max,
            mem_used: self.last.mem_used,
            mem_total: self.last.mem_total,
            swap_used: self.last.swap_used,
            disk_used: self.last.disk_used,
            disk_total: self.last.disk_total,
            rx: avg(self.rx_sum),
            tx: avg(self.tx_sum),
            load1: self.last.load1,
            samples: self.samples,
        }
    }
}

impl NodeMinute {
    /// A spool row of `server`.
    #[must_use]
    pub fn from_row(server_id: ServerId, r: &crate::spool::NodeMinuteRow) -> Self {
        Self {
            server_id,
            minute: Timestamp(r.minute),
            cpu_avg: r.cpu_avg,
            cpu_max: r.cpu_max,
            mem_used: r.mem_used,
            mem_total: r.mem_total,
            swap_used: r.swap_used,
            disk_used: r.disk_used,
            disk_total: r.disk_total,
            rx: r.rx,
            tx: r.tx,
            load1: r.load1,
            samples: r.samples,
        }
    }
}

/// The rows of one agent spool file (D-142), with the file's id, ready to
/// store as one unit.
#[derive(Debug, Clone, PartialEq)]
pub struct SpoolRows {
    /// The node of the uploading agent's certificate.
    pub node: ServerId,
    /// The file id (32 hex digits).
    pub file_id: String,
    pub logs: Vec<crate::logs::LogEvent>,
    pub minutes: Vec<NodeMinute>,
    pub processes: Vec<NodeProcessMinute>,
    pub containers: Vec<ContainerMinute>,
}

impl SpoolRows {
    /// Rows in every table.
    #[must_use]
    pub fn rows(&self) -> u64 {
        (self.logs.len() + self.minutes.len() + self.processes.len() + self.containers.len()) as u64
    }
}

/// What storing a spool file did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileWrite {
    /// The rows were stored (in a store with file ids: every table that did
    /// not already hold them).
    Stored,
    /// Every table already held this file; nothing was written.
    AlreadyStored,
}

/// Stores the rows of one spool file (D-143). A store that records the file
/// id with the rows stores a file sent again only once; the generic
/// [`PerTableSpoolWriter`] writes each table in turn (at least once).
#[async_trait]
pub trait SpoolFileWriter: Send + Sync + std::fmt::Debug {
    async fn write_spool_file(&self, file: SpoolRows) -> DomainResult<FileWrite>;
}

/// [`SpoolFileWriter`] over any [`AnalyticsStore`]: one insert per table.
/// Not atomic across tables: a failure part-way makes the agent send the
/// file again, and the tables already written get those rows twice.
pub struct PerTableSpoolWriter(pub std::sync::Arc<dyn AnalyticsStore>);

impl std::fmt::Debug for PerTableSpoolWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("PerTableSpoolWriter").field(&self.0.engine()).finish()
    }
}

#[async_trait]
impl SpoolFileWriter for PerTableSpoolWriter {
    async fn write_spool_file(&self, f: SpoolRows) -> DomainResult<FileWrite> {
        if !f.logs.is_empty() {
            self.0.write_logs(&f.logs).await?;
        }

        if !f.minutes.is_empty() {
            self.0.write_node_minutes(&f.minutes).await?;
        }

        if !f.processes.is_empty() {
            self.0.write_top_processes(&f.processes).await?;
        }

        if !f.containers.is_empty() {
            self.0.write_container_minutes(&f.containers).await?;
        }

        Ok(FileWrite::Stored)
    }
}

/// One container's minute or hour aggregate, as a store reads it; rows
/// ordered by `(container_name, point.t)` are grouped by [`group_series`].
#[derive(Debug, Clone, PartialEq)]
pub struct ContainerRowOut {
    pub container_name: String,
    pub container_id: String,
    pub image: String,
    pub service_id: Option<crate::ServiceId>,
    /// Empty: none.
    pub stack: String,
    pub point: ContainerPoint,
}

/// Groups rows ordered by `(container_name, t)` into one series per
/// container; the identity fields are the latest seen.
#[must_use]
pub fn group_series(rows: impl IntoIterator<Item = ContainerRowOut>) -> Vec<ContainerSeries> {
    let mut out: Vec<ContainerSeries> = Vec::new();

    for r in rows {
        let stack = (!r.stack.is_empty()).then_some(r.stack);

        match out.last_mut() {
            Some(s) if s.container_name == r.container_name => {
                s.container_id = r.container_id;
                s.image = r.image;
                s.service_id = r.service_id;
                s.stack = stack;
                s.points.push(r.point);
            }
            _ => out.push(ContainerSeries {
                container_name: r.container_name,
                container_id: r.container_id,
                image: r.image,
                service_id: r.service_id,
                stack,
                points: vec![r.point],
            }),
        }
    }

    out
}
