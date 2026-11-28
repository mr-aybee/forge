//! The DuckDB analytics store (D-143): DuckDB's own file format, one file
//! per UTC day and family, run in-process by the analytics worker.
//!
//! - [`parts`]: which file holds which rows.
//! - [`catalog`]: the files on disk, attached on demand under leases.
//! - [`writer`]: the one writer thread; batches spool files and records
//!   their ids with their rows (exactly once per table).
//! - [`read`]: every read of `AnalyticsStore`, ClickHouse semantics.
//! - [`jobs`]: hourly rollups, retention by file, compaction.

pub mod catalog;
pub mod jobs;
pub mod parts;
pub mod read;
pub mod schema;
pub mod store;
pub mod writer;

pub use catalog::Limits;
pub use store::{DuckDb, DuckDbConfig};

/// Why a DuckDB operation failed.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("duckdb: {0}")]
    Db(#[from] duckdb::Error),
    #[error("analytics files: {0}")]
    Io(#[from] std::io::Error),
    #[error("the DuckDB writer has stopped")]
    Stopped,
    #[error("the query took longer than its time limit")]
    Timeout,
    #[error("writing failed: {0}")]
    Write(String),
    #[error("{0}")]
    Invalid(String),
}
