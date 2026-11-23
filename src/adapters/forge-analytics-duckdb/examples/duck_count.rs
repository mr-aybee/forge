//! Counts stored container lines in a stopped DuckDB analytics store (the
//! capacity benchmark): `total distinct` (distinct by node, time, message,
//! container and stream), like `scripts/dev/capacity-bench/rows.sh` for
//! ClickHouse. The worker must be stopped: DuckDB locks its files.
//!
//! Further arguments are SQL statements to time, with `{logs}` standing for
//! every day file's `app_logs`; each prints its row count and time.
//!
//! Usage: duck_count <dir>/logs ["SELECT .. FROM {logs} .."]..

use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let dir = args.next().ok_or("usage: duck_count <logs dir> [sql]..")?;
    let c = duckdb::Connection::open_in_memory()?;
    let mut parts = Vec::new();
    for (i, e) in std::fs::read_dir(&dir)?.enumerate() {
        let p = e?.path();
        if p.extension().is_some_and(|x| x == "duckdb") {
            let path = p.to_string_lossy().replace('\'', "''");
            c.execute_batch(&format!("ATTACH '{path}' AS d{i} (READ_ONLY)"))?;
            parts.push(format!("SELECT * FROM d{i}.app_logs"));
        }
    }
    if parts.is_empty() {
        println!("0\t0");
        return Ok(());
    }
    let logs = format!("({})", parts.join(" UNION ALL "));
    let sql = format!(
        "SELECT count(*), count(DISTINCT (server_id, ts, message, container_id, stream)) FROM {logs} WHERE source = 'container'"
    );
    let (total, distinct): (i64, i64) = c.query_row(&sql, [], |r| Ok((r.get(0)?, r.get(1)?)))?;
    println!("{total}\t{distinct}");
    for q in args {
        let started = Instant::now();
        let mut st = c.prepare(&q.replace("{logs}", &logs))?;
        let n = st.query_map([], |_| Ok(()))?.count();
        println!("{n} rows\t{} ms\t{q}", started.elapsed().as_millis());
    }
    Ok(())
}
