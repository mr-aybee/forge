//! forge-server's side of the analytics worker's control socket (D-141).
//!
//! A small pool of authenticated connections (at most [`MAX_CONNECTIONS`]
//! in use). Each request has a deadline; a connection that fails or times
//! out is dropped, never reused, which also tells the worker to abandon
//! the request. While no worker is connected every call answers
//! [`AnalyticsError::WorkerDown`] at once.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::Duration;

use async_trait::async_trait;
use forge_analytics_proto::tuning::{Setting, Tuning};
use forge_analytics_proto::views::{SettingResult, WorkerReport};
use forge_analytics_proto::{PROTOCOL, RESPONSE_LIMITS, Reply, Request, Response, WorkerError};
use forge_core::analytics::{
    AnalyticsError, AnalyticsMaps, AnalyticsReader, ContainerHistory, Histogram, NodeHistory, NodeReport,
    NodeTelemetry, TelemetrySink,
};
use forge_core::logs::{LogEvent, LogQuery};
use forge_core::{Clock, ServerId, ServiceId, Timestamp};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{Semaphore, watch};

/// Connections to the worker in use at once.
pub const MAX_CONNECTIONS: usize = 16;
/// Longest wait for a free connection.
const POOL_WAIT: Duration = Duration::from_secs(5);
/// Connecting and authenticating.
const CONNECT_WAIT: Duration = Duration::from_secs(5);
/// Control calls (maps, tuning, report, restart).
const CONTROL_WAIT: Duration = Duration::from_secs(30);
/// Applying settings: the memory budget waits up to 15 s for the engine.
const APPLY_WAIT: Duration = Duration::from_secs(60);
/// A Seq/CLEF body: stored and answered within this, or the sender retries.
const INGEST_WAIT: Duration = Duration::from_secs(30);
/// Heartbeat telemetry: the heartbeat must not wait longer.
const TELEMETRY_WAIT: Duration = Duration::from_secs(10);
/// History reads.
const READ_WAIT: Duration = Duration::from_secs(60);
/// Added to the search timeout for log searches.
const SEARCH_MARGIN: Duration = Duration::from_secs(10);

trait Io: AsyncRead + AsyncWrite + Send + Unpin {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin> Io for T {}
type Conn = Box<dyn Io>;

/// Why a call failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CallError {
    #[error("analytics is off")]
    Off,
    #[error("the analytics worker is not running")]
    Down,
    #[error("the analytics worker did not answer in time")]
    Timeout,
    #[error("the analytics worker gave an unexpected answer")]
    Unexpected,
    #[error(transparent)]
    Worker(#[from] WorkerError),
}

impl From<CallError> for AnalyticsError {
    fn from(e: CallError) -> Self {
        match e {
            CallError::Off => AnalyticsError::Off,
            CallError::Down => AnalyticsError::WorkerDown,
            CallError::Timeout | CallError::Unexpected => AnalyticsError::Unavailable(e.to_string()),
            CallError::Worker(w) => w.into(),
        }
    }
}

/// Where the worker listens and how to authenticate.
struct Link {
    /// A socket path, or `127.0.0.1:<port>`.
    endpoint: String,
    secret: String,
    generation: u64,
    idle: Mutex<Vec<Conn>>,
}

impl std::fmt::Debug for Link {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never the secret.
        f.debug_struct("Link").field("endpoint", &self.endpoint).field("generation", &self.generation).finish()
    }
}

async fn dial(endpoint: &str) -> std::io::Result<Conn> {
    #[cfg(unix)]
    if endpoint.starts_with('/') {
        // A path past the kernel's limit is reached through its directory.
        let short = forge_analytics_proto::socket_path::short(std::path::Path::new(endpoint))?;
        return Ok(Box::new(tokio::net::UnixStream::connect(short.path()).await?));
    }
    Ok(Box::new(tokio::net::TcpStream::connect(endpoint).await?))
}

async fn exchange(conn: &mut Conn, req: &Request, blob: &[u8]) -> Result<Reply, CallError> {
    forge_analytics_proto::write_message(conn, req, blob).await.map_err(|_| CallError::Down)?;
    let (resp, _): (Response, _) =
        forge_analytics_proto::read_message(conn, RESPONSE_LIMITS).await.map_err(|_| CallError::Down)?;

    Ok(resp?)
}

/// Opens and authenticates one connection.
async fn open(link: &Link) -> Result<Conn, CallError> {
    let fut = async {
        let mut conn = dial(&link.endpoint).await.map_err(|_| CallError::Down)?;
        let hello = Request::Hello { secret: link.secret.clone(), protocol: PROTOCOL };
        exchange(&mut conn, &hello, &[]).await?;
        Ok(conn)
    };
    tokio::time::timeout(CONNECT_WAIT, fut).await.map_err(|_| CallError::Timeout)?
}

/// The client. One per server; the supervisor connects and disconnects it.
#[derive(Debug)]
pub struct WorkerClient {
    enabled: bool,
    link: RwLock<Option<Arc<Link>>>,
    generation: AtomicU64,
    slots: Arc<Semaphore>,
    tuning: watch::Receiver<Tuning>,
    clock: Arc<dyn Clock>,
}

impl WorkerClient {
    /// `enabled` is false when analytics is off: every call answers `Off`.
    #[must_use]
    pub fn new(enabled: bool, tuning: watch::Receiver<Tuning>, clock: Arc<dyn Clock>) -> Self {
        Self {
            enabled,
            link: RwLock::new(None),
            generation: AtomicU64::new(0),
            slots: Arc::new(Semaphore::new(MAX_CONNECTIONS)),
            tuning,
            clock,
        }
    }

    #[must_use]
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Counts (re)connections; maps are pushed again when it changes.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// Whether a worker is connected.
    #[must_use]
    pub fn connected(&self) -> bool {
        self.link.read().unwrap_or_else(PoisonError::into_inner).is_some()
    }

    /// Points the client at a freshly started worker.
    pub fn connect_to(&self, endpoint: String, secret: String) {
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        let link = Link { endpoint, secret, generation, idle: Mutex::new(Vec::new()) };
        *self.link.write().unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(link));
    }

    /// Forgets the worker (it stopped); pooled connections are dropped.
    pub fn disconnect(&self) {
        *self.link.write().unwrap_or_else(PoisonError::into_inner) = None;
    }

    fn current(&self) -> Result<Arc<Link>, CallError> {
        if !self.enabled {
            return Err(CallError::Off);
        }

        self.link.read().unwrap_or_else(PoisonError::into_inner).clone().ok_or(CallError::Down)
    }

    /// One request on a pooled connection, within `wait`.
    async fn call(&self, req: &Request, blob: &[u8], wait: Duration) -> Result<Reply, CallError> {
        let link = self.current()?;
        let _slot = tokio::time::timeout(POOL_WAIT, self.slots.acquire())
            .await
            .map_err(|_| CallError::Worker(WorkerError::Busy))?
            .map_err(|_| CallError::Down)?;
        let pooled = link.idle.lock().unwrap_or_else(PoisonError::into_inner).pop();
        let mut conn = match pooled {
            Some(c) => c,
            None => open(&link).await?,
        };

        let reply = tokio::time::timeout(wait, exchange(&mut conn, req, blob)).await.map_err(|_| CallError::Timeout)?;

        // Only a connection that completed an exchange goes back, and only
        // to the worker it was opened for.
        if reply.is_ok() || matches!(reply, Err(CallError::Worker(_))) {
            let same = self.current().is_ok_and(|l| l.generation == link.generation);
            if same {
                link.idle.lock().unwrap_or_else(PoisonError::into_inner).push(conn);
            }
        }

        reply
    }

    /// Liveness on a connection of its own, so a busy pool never makes a
    /// healthy worker look hung. Returns the worker's pid.
    pub async fn ping(&self, wait: Duration) -> Result<u32, CallError> {
        let link = self.current()?;
        let fut = async {
            let mut conn = open(&link).await?;
            match exchange(&mut conn, &Request::Ping, &[]).await? {
                Reply::Pong { pid } => Ok(pid),
                _ => Err(CallError::Unexpected),
            }
        };

        tokio::time::timeout(wait, fut).await.map_err(|_| CallError::Timeout)?
    }

    pub async fn push_maps(&self, maps: AnalyticsMaps) -> Result<(), CallError> {
        expect_ok(self.call(&Request::PushMaps(maps), &[], CONTROL_WAIT).await?)
    }

    /// Replaces the agent listener's certificate of the running worker (D-145).
    pub async fn set_agent_tls(&self, tls: forge_analytics_proto::config::AgentTls) -> Result<(), CallError> {
        expect_ok(self.call(&Request::SetAgentTls(tls), &[], CONTROL_WAIT).await?)
    }

    pub async fn set_tuning(&self, t: Tuning) -> Result<(), CallError> {
        expect_ok(self.call(&Request::SetTuning(t), &[], CONTROL_WAIT).await?)
    }

    pub async fn report(&self) -> Result<WorkerReport, CallError> {
        match self.call(&Request::Report, &[], CONTROL_WAIT).await? {
            Reply::Report(r) => Ok(*r),
            _ => Err(CallError::Unexpected),
        }
    }

    pub async fn apply(&self, settings: Vec<Setting>, tuning: Tuning) -> Result<Vec<SettingResult>, CallError> {
        match self.call(&Request::Apply { settings, tuning }, &[], APPLY_WAIT).await? {
            Reply::Applied(r) => Ok(r),
            _ => Err(CallError::Unexpected),
        }
    }

    pub async fn restart_engine(&self) -> Result<(), CallError> {
        expect_ok(self.call(&Request::RestartEngine, &[], CONTROL_WAIT).await?)
    }

    fn search_wait(&self) -> Duration {
        Duration::from_secs(u64::from(self.tuning.borrow().search_timeout_secs)) + SEARCH_MARGIN
    }
}

fn expect_ok(r: Reply) -> Result<(), CallError> {
    match r {
        Reply::Ok => Ok(()),
        _ => Err(CallError::Unexpected),
    }
}

#[async_trait]
impl AnalyticsReader for WorkerClient {
    async fn node_history(&self, server: ServerId, from: i64, to: i64) -> Result<NodeHistory, AnalyticsError> {
        match self.call(&Request::NodeHistory { server, from, to }, &[], READ_WAIT).await? {
            Reply::NodeHistory(h) => Ok(h),
            _ => Err(CallError::Unexpected.into()),
        }
    }

    async fn node_report(&self, server: ServerId, from: i64, to: i64) -> Result<NodeReport, AnalyticsError> {
        match self.call(&Request::NodeReport { server, from, to }, &[], READ_WAIT).await? {
            Reply::NodeReport(r) => Ok(*r),
            _ => Err(CallError::Unexpected.into()),
        }
    }

    async fn container_history(
        &self,
        server: ServerId,
        container: Option<String>,
        window: (i64, i64),
        limit: u32,
    ) -> Result<ContainerHistory, AnalyticsError> {
        let req = Request::ContainerHistory { server, container, from: window.0, to: window.1, limit };
        match self.call(&req, &[], READ_WAIT).await? {
            Reply::ContainerHistory(h) => Ok(h),
            _ => Err(CallError::Unexpected.into()),
        }
    }

    async fn query_logs(&self, q: &LogQuery) -> Result<Vec<LogEvent>, AnalyticsError> {
        let req = Request::QueryLogs { query: q.clone(), now: self.clock.now() };
        match self.call(&req, &[], self.search_wait()).await? {
            Reply::Logs(e) => Ok(e),
            _ => Err(CallError::Unexpected.into()),
        }
    }

    async fn log_histogram(&self, q: &LogQuery) -> Result<Histogram, AnalyticsError> {
        let req = Request::LogHistogram { query: q.clone(), now: self.clock.now() };
        match self.call(&req, &[], self.search_wait()).await? {
            Reply::Histogram(h) => Ok(h),
            _ => Err(CallError::Unexpected.into()),
        }
    }
}

#[async_trait]
impl TelemetrySink for WorkerClient {
    async fn ingest_logs(&self, service: ServiceId, now: Timestamp, body: &[u8]) -> Result<usize, AnalyticsError> {
        match self.call(&Request::IngestLogs { service, now }, body, INGEST_WAIT).await? {
            Reply::Ingested { count } => Ok(count),
            _ => Err(CallError::Unexpected.into()),
        }
    }

    async fn forward_telemetry(&self, t: NodeTelemetry<'_>) -> Result<u64, AnalyticsError> {
        let metrics_len =
            u32::try_from(t.metrics.len()).map_err(|_| AnalyticsError::BadBody("the metrics are too large".into()))?;
        let mut blob = Vec::with_capacity(t.metrics.len() + t.container_logs.len());
        blob.extend_from_slice(t.metrics);
        blob.extend_from_slice(t.container_logs);
        let req = Request::Telemetry { server: t.server, now: t.now, metrics_len };
        match self.call(&req, &blob, TELEMETRY_WAIT).await? {
            Reply::TelemetryAck { ack } => Ok(ack),
            _ => Err(CallError::Unexpected.into()),
        }
    }
}
