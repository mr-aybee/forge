//! The control messages between forge-server and the analytics worker.
//!
//! The first frame on every connection is [`Request::Hello`]; the worker
//! closes a connection whose secret is wrong or whose protocol differs.
//! Each request gets exactly one [`Response`]. Requests on one connection
//! are answered in order; forge-server keeps a small pool of connections.

use forge_core::analytics::{AnalyticsMaps, ContainerHistory, Histogram, NodeHistory, NodeReport};
use forge_core::logs::{LogEvent, LogQuery};
use forge_core::{ServerId, ServiceId, Timestamp};
use serde::{Deserialize, Serialize};

use crate::tuning::{Setting, Tuning};
use crate::views::{SettingResult, WorkerReport};

/// Version of this protocol; a worker built for another answers
/// [`WorkerError::ProtocolMismatch`].
pub const PROTOCOL: u32 = 2;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", content = "args", rename_all = "snake_case")]
pub enum Request {
    /// Authenticates the connection with the secret the server handed the
    /// worker on stdin.
    Hello {
        secret: String,
        protocol: u32,
    },
    /// Liveness: answered from the connection task, never queued.
    Ping,
    /// Replaces the control-plane maps.
    PushMaps(AnalyticsMaps),
    /// The effective settings changed.
    SetTuning(Tuning),
    /// Status, usage and the ingest queue.
    Report,
    /// Applies `settings` from `tuning` to the engine and reports each one.
    Apply {
        settings: Vec<Setting>,
        tuning: Tuning,
    },
    /// Restarts the managed engine now.
    RestartEngine,
    /// A new certificate and key for the agent upload listener (D-145): new
    /// connections use it at once, open ones keep theirs. forge-server sends
    /// it before the current one expires, so a long-running worker never
    /// serves an expired certificate.
    SetAgentTls(crate::config::AgentTls),
    /// A Seq/CLEF body for `service`; the body is the frame's blob.
    IngestLogs {
        service: ServiceId,
        now: Timestamp,
    },
    /// One heartbeat's telemetry. The blob holds an encoded `NodeMetrics`
    /// (`metrics_len` bytes, possibly 0) followed by an encoded
    /// `ContainerLogBatch` (the rest, possibly empty).
    Telemetry {
        server: ServerId,
        now: Timestamp,
        metrics_len: u32,
    },
    NodeHistory {
        server: ServerId,
        from: i64,
        to: i64,
    },
    NodeReport {
        server: ServerId,
        from: i64,
        to: i64,
    },
    ContainerHistory {
        server: ServerId,
        container: Option<String>,
        from: i64,
        to: i64,
        limit: u32,
    },
    QueryLogs {
        query: LogQuery,
        now: Timestamp,
    },
    LogHistogram {
        query: LogQuery,
        now: Timestamp,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Reply {
    Ok,
    Pong {
        pid: u32,
    },
    Report(Box<WorkerReport>),
    Applied(Vec<SettingResult>),
    Ingested {
        count: usize,
    },
    /// The container batch `seq` to acknowledge (0: none stored).
    TelemetryAck {
        ack: u64,
    },
    NodeHistory(NodeHistory),
    NodeReport(Box<NodeReport>),
    ContainerHistory(ContainerHistory),
    Logs(Vec<LogEvent>),
    Histogram(Histogram),
}

/// Why the worker refused a request. A closed set of codes; free text only
/// where it carries the engine's own reason or a validation message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "code", content = "detail", rename_all = "snake_case")]
pub enum WorkerError {
    #[error("analytics is off")]
    Off,
    #[error("analytics is unavailable: {reason}")]
    Unavailable { reason: String },
    #[error("log ingest is busy; retry shortly")]
    Busy,
    #[error("the analytics database is low on disk space")]
    DiskLow,
    #[error("too many log searches are running; retry shortly")]
    SearchBusy,
    #[error("{reason}")]
    Invalid { reason: String },
    #[error("{reason}")]
    BadBody { reason: String },
    #[error("the analytics engine is not run by Forge")]
    NotManaged,
    #[error("the connection is not authenticated")]
    Unauthorized,
    #[error("protocol {got} is not supported; this worker speaks {expected}")]
    ProtocolMismatch { expected: u32, got: u32 },
    #[error("unexpected message: {reason}")]
    Protocol { reason: String },
    #[error("the analytics worker is stopping")]
    Stopped,
}

/// One answer.
pub type Response = Result<Reply, WorkerError>;

impl From<forge_core::analytics::AnalyticsError> for WorkerError {
    fn from(e: forge_core::analytics::AnalyticsError) -> Self {
        use forge_core::analytics::AnalyticsError as A;
        match e {
            A::Off => WorkerError::Off,
            A::WorkerDown => WorkerError::Stopped,
            A::Unavailable(reason) => WorkerError::Unavailable { reason },
            A::Busy => WorkerError::Busy,
            A::DiskLow => WorkerError::DiskLow,
            A::SearchBusy => WorkerError::SearchBusy,
            A::Invalid(reason) => WorkerError::Invalid { reason },
            A::BadBody(reason) => WorkerError::BadBody { reason },
        }
    }
}

impl From<WorkerError> for forge_core::analytics::AnalyticsError {
    fn from(e: WorkerError) -> Self {
        use forge_core::analytics::AnalyticsError as A;
        match e {
            WorkerError::Off => A::Off,
            WorkerError::Unavailable { reason } => A::Unavailable(reason),
            WorkerError::Busy => A::Busy,
            WorkerError::DiskLow => A::DiskLow,
            WorkerError::SearchBusy => A::SearchBusy,
            WorkerError::Invalid { reason } => A::Invalid(reason),
            WorkerError::BadBody { reason } => A::BadBody(reason),
            WorkerError::NotManaged => A::Invalid("the analytics engine is not run by Forge".into()),
            WorkerError::Unauthorized
            | WorkerError::ProtocolMismatch { .. }
            | WorkerError::Protocol { .. }
            | WorkerError::Stopped => A::WorkerDown,
        }
    }
}
