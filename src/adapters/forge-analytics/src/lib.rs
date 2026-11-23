//! Analytics database adapters (D-127).
//!
//! [`ClickHouse`] talks to ClickHouse through its HTTP interface: a remote
//! server, ClickHouse Cloud, or the embedded engine Forge runs on
//! `127.0.0.1`. Values are passed as ClickHouse query parameters
//! (`{name:Type}` with `param_name=`), never spliced into SQL.

//!
//! [`ClickHouseSpoolWriter`] stores agent spool files with one insert per
//! table for many files at once, deduplicated per file and table (D-145).

mod clickhouse;
mod schema;
pub mod spool;

pub use clickhouse::{ClickHouse, ClickHouseTarget};
pub use spool::ClickHouseSpoolWriter;
