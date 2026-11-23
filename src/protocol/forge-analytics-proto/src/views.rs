//! What the worker reports about itself: status, usage, the ingest queue
//! and the outcome of applying settings.

use forge_core::analytics::Retention;
use serde::{Deserialize, Serialize};

use crate::tuning::Setting;

/// The ingest queue right now.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct QueueStats {
    pub queued_bytes: u64,
    pub queued_requests: usize,
    /// Age of the oldest waiting batch, ms (`null` when empty).
    pub oldest_ms: Option<u64>,
}

/// The outcome of one setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ApplyStatus {
    /// In effect, and the engine (or Forge) reports the new value.
    Applied,
    /// Stored; applied when the analytics database is reachable.
    Pending,
    /// Stored; takes effect when the analytics engine restarts.
    PendingRestart,
    /// Stored, but could not be applied; `message` says why.
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SettingResult {
    pub setting: Setting,
    pub status: ApplyStatus,
    /// The value in effect, as read back (when known).
    pub effective: Option<f64>,
    pub message: Option<String>,
}

/// The engine's resource use now.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Usage {
    /// Managed engine: vCPUs used over the last sample interval.
    pub cpu_vcpu: Option<f64>,
    /// Managed engine: resident memory of its process, bytes.
    pub process_memory_bytes: Option<u64>,
    /// The engine's own tracked memory, bytes.
    pub engine_memory_bytes: Option<u64>,
    /// The engine's memory budget, bytes (`0`: derived from host memory).
    pub server_memory_limit_bytes: Option<u64>,
    /// Memory one search may use, bytes (`null`: engine default).
    pub per_query_memory_bytes: Option<u64>,
    /// Managed engine: CPUs its process may run on, read back from the
    /// kernel (`null`: not running or unknown).
    pub allowed_cpus: Option<u32>,
    pub free_disk_bytes: Option<u64>,
}

/// The analytics database as the worker sees it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoreStatus {
    /// Schema applied and the last write or check succeeded.
    pub ready: bool,
    pub last_error: Option<String>,
    /// Node and container rows waiting to be written.
    pub pending_rows: usize,
    /// Rows dropped because the buffer was full.
    pub dropped_rows: u64,
    pub retention: Retention,
}

/// Everything the settings page shows about the worker's side.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkerReport {
    pub store: StoreStatus,
    pub usage: Usage,
    pub queue: QueueStats,
    /// Why the CPU cap is not in effect (`None`: it is, none is set, or the
    /// engine is external).
    pub process_limits_unavailable: Option<String>,
    /// A setting waits for an engine restart.
    pub pending_restart: bool,
    /// The store's scheduled jobs (DuckDB: rollups, retention, compaction);
    /// empty for engines that run their own (ClickHouse TTLs and views).
    #[serde(default)]
    pub jobs: Vec<JobReport>,
}

/// The last run of one scheduled analytics job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct JobReport {
    /// `rollups`, `retention` or `compaction`.
    pub name: String,
    /// When it last ran, epoch ms.
    pub last_run: i64,
    pub duration_ms: u64,
    /// Why the last run failed (`null`: it succeeded).
    pub error: Option<String>,
    /// What it did: rows folded, files dropped or checkpointed.
    pub done: u64,
}
