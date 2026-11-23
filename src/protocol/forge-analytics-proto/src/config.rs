//! How forge-server starts the worker: the configuration file it writes
//! (mode 0600, no secrets) and passes with `--config`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::tuning::Tuning;

/// Environment variable holding the analytics database password (external
/// ClickHouse); the embedded engine's password lives in its own 0600 file.
pub const PASSWORD_ENV: &str = "FORGE_ANALYTICS_PASSWORD";

/// Files of the embedded engine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbeddedPaths {
    pub root: PathBuf,
    pub binary: PathBuf,
    pub data: PathBuf,
    pub config: PathBuf,
    pub users: PathBuf,
    pub secret: PathBuf,
    /// PID of the running engine, so a worker that was killed outright does
    /// not leave an engine behind holding the port.
    pub pid_file: PathBuf,
}

impl EmbeddedPaths {
    /// The layout under `<data_dir>/analytics`; `binary` overrides the
    /// default `<root>/bin/clickhouse`.
    #[must_use]
    pub fn under(root: &Path, binary: Option<PathBuf>) -> Self {
        Self {
            binary: binary.unwrap_or_else(|| root.join("bin").join("clickhouse")),
            data: root.join("data"),
            config: root.join("config.xml"),
            users: root.join("users.xml"),
            secret: root.join("secret"),
            pid_file: root.join("engine.pid"),
            root: root.to_path_buf(),
        }
    }
}

/// Which analytics store the worker uses. One is active at a time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StoreSelection {
    /// A ClickHouse server; the password comes from [`PASSWORD_ENV`].
    Clickhouse { url: String },
    /// The ClickHouse engine the worker runs and supervises.
    Embedded { paths: EmbeddedPaths, port: u16, database: String },
    /// DuckDB in the worker's own process (D-143): one database file per
    /// UTC day and family under `dir`. Its memory limit and threads come
    /// from the tuning (`memory_budget_mb`, `cpu_quota_vcpu`).
    Duckdb { dir: PathBuf },
    /// Rows in the worker's memory only: for tests and trials. Nothing
    /// survives a restart.
    Memory,
}

/// Where the worker listens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Endpoint {
    /// A Unix socket (mode 0600) in a directory only the service user may enter.
    Unix { path: PathBuf },
    /// Loopback TCP, where Unix sockets are not available (Windows); the
    /// worker picks the port and reports it on stdout.
    Loopback,
}

/// The worker's configuration file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkerConfig {
    pub store: StoreSelection,
    pub endpoint: Endpoint,
    /// The effective settings at start; later changes come over the socket.
    pub tuning: Tuning,
    /// Where agents upload their spool files (D-142); `None`: no uploads.
    #[serde(default)]
    pub agent_listener: Option<AgentListener>,
}

/// The fewest spool files the worker imports at once
/// (`[analytics] worker_max_uploads`), and the default. Below this the
/// worker, not the engine, limits ingest (see `scripts/dev/benchmarks/2026-11-21-chdb-capped-node-scaling.md`).
pub const MIN_UPLOADS: u32 = 8;

/// The most spool files the worker imports at once. Each slot is one upload
/// in flight from an agent, so the cap also bounds the network the uploads
/// use. The embedded engine admits this many concurrent inserts plus a
/// reserve, so a full set of imports is never refused by its query limit.
pub const MAX_UPLOADS: u32 = 32;

/// The worker's agent upload listener (D-142). Its TLS identity is handed
/// over on stdin as an [`AgentTls`] line after the secret, never in this file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentListener {
    /// Address to bind, e.g. `0.0.0.0:55132`.
    pub listen: std::net::SocketAddr,
    /// The Forge CA that signed the agents' certificates (public).
    pub ca_pem: String,
    /// Where the worker keeps the file ids it stored (mode 0700).
    pub state_dir: PathBuf,
    /// Uploads imported at once; more are answered 429 at once.
    pub max_uploads: u32,
}

/// The listener's certificate and key, sent as one JSON line on stdin.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentTls {
    pub cert_pem: String,
    pub key_pem: String,
}

impl std::fmt::Debug for AgentTls {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentTls").finish_non_exhaustive()
    }
}

/// The line the worker prints on stdout once it accepts connections:
/// `ready <endpoint>` (a socket path or `127.0.0.1:<port>`).
pub const READY_PREFIX: &str = "ready ";
