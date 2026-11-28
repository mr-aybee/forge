//! Tables of each family's files and of `meta.duckdb`. Times are epoch ms
//! (`BIGINT`), ids are text. A new file gets its family's tables when it is
//! created; a change to a table is a new statement after these that every
//! file applies when attached (`CREATE .. IF NOT EXISTS`, `ALTER .. ADD
//! COLUMN IF NOT EXISTS`).

use crate::parts::Family;

/// Version of the layout, kept in `meta.settings`.
pub const VERSION: u32 = 1;

/// The id log of the spool files a file's rows came from (D-143): written
/// in the same transaction as the rows.
const SPOOL_FILES: &str =
    "CREATE TABLE IF NOT EXISTS {db}.spool_files (node VARCHAR, file_id VARCHAR, stored_at BIGINT)";

const LOGS: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS {db}.app_logs (
        service_id VARCHAR, ts BIGINT, id VARCHAR, node_ts BIGINT, ingest_ts BIGINT, level VARCHAR,
        message VARCHAR, message_template VARCHAR, exception VARCHAR, trace_id VARCHAR, span_id VARCHAR,
        event_type UINTEGER, server_id VARCHAR, properties VARCHAR, source VARCHAR, container_id VARCHAR,
        container_name VARCHAR, stack VARCHAR, stream VARCHAR)",
    SPOOL_FILES,
];

const METRICS: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS {db}.node_minute (
        server_id VARCHAR, minute BIGINT, cpu_avg REAL, cpu_max REAL, mem_used UBIGINT, mem_total UBIGINT,
        swap_used UBIGINT, disk_used UBIGINT, disk_total UBIGINT, rx UBIGINT, tx UBIGINT, load1 REAL,
        samples UINTEGER)",
    "CREATE TABLE IF NOT EXISTS {db}.container_minute (
        server_id VARCHAR, container_name VARCHAR, minute BIGINT, container_id VARCHAR, image VARCHAR,
        service_id VARCHAR, stack VARCHAR, cpu REAL, mem_used UBIGINT, mem_limit UBIGINT, rx UBIGINT,
        tx UBIGINT, block_read UBIGINT, block_write UBIGINT, restarts UINTEGER)",
    SPOOL_FILES,
];

const PROCESSES: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS {db}.top_process (
        server_id VARCHAR, minute BIGINT, pid UINTEGER, name VARCHAR, \"user\" VARCHAR, cpu REAL,
        mem UBIGINT, cmd VARCHAR)",
    SPOOL_FILES,
];

/// Hourly rollups keep mergeable states: sums and a count for averages,
/// maxima, and the latest identity with its minute. Late rows add rows for
/// an hour; reads merge them, so applying a range twice leaves every
/// average and maximum unchanged.
const ROLLUPS: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS {db}.node_1h (
        server_id VARCHAR, hour BIGINT, n BIGINT, cpu_sum DOUBLE, cpu_max REAL, mem_sum DOUBLE,
        mem_total UBIGINT, disk_sum DOUBLE, disk_total UBIGINT, rx_sum DOUBLE, tx_sum DOUBLE, load_sum DOUBLE)",
    "CREATE TABLE IF NOT EXISTS {db}.container_1h (
        server_id VARCHAR, container_name VARCHAR, hour BIGINT, n BIGINT, last_minute BIGINT,
        container_id VARCHAR, image VARCHAR, service_id VARCHAR, stack VARCHAR, cpu_sum DOUBLE, cpu_max REAL,
        mem_sum DOUBLE, mem_limit UBIGINT, rx_sum DOUBLE, tx_sum DOUBLE, br_sum DOUBLE, bw_sum DOUBLE,
        restarts UINTEGER)",
];

const META: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS meta.settings (key VARCHAR, value VARCHAR)",
    // The event-time range each file holds: widened before its rows commit.
    "CREATE TABLE IF NOT EXISTS meta.ranges (family VARCHAR, part BIGINT, min_t BIGINT, max_t BIGINT)",
    // Rows of a metrics file already folded into the rollups (row ids below `upto`).
    "CREATE TABLE IF NOT EXISTS meta.rolled (part BIGINT, tbl VARCHAR, upto BIGINT)",
];

/// The statements that create `family`'s tables in catalog `db`.
#[must_use]
pub fn family(family: Family, db: &str) -> Vec<String> {
    let list = match family {
        Family::Logs => LOGS,
        Family::Metrics => METRICS,
        Family::Processes => PROCESSES,
        Family::Rollups => ROLLUPS,
    };

    list.iter().map(|s| s.replace("{db}", db)).collect()
}

/// The statements that create `meta`'s tables.
#[must_use]
pub fn meta() -> &'static [&'static str] {
    META
}
