//! Hourly jobs (D-143), run on the writer thread:
//!
//! - **rollups:** minute rows not yet folded (row ids at or past the bound
//!   in `meta.rolled`) are aggregated into mergeable hourly states in the
//!   month's rollup file. Late rows simply add states for their hour.
//! - **retention:** whole files past their family's retention are detached
//!   and deleted; no `DELETE`, no `VACUUM`.
//! - **compaction:** closed day files are checkpointed (their WAL folded in)
//!   and detached when idle.

use std::collections::HashSet;
use std::time::Duration;

use duckdb::params;
use forge_core::analytics::Retention;

use crate::Error;
use crate::parts::{DAY_MS, Family, HOUR_MS, Part};
use crate::writer::Writer;

/// The minute tables that have rollups.
pub const MINUTE_TABLES: [&str; 2] = ["node_minute", "container_minute"];

/// A file no lease used for this long is detached.
const IDLE: Duration = Duration::from_secs(600);

fn rollup_sql(table: &str, src: &str, dst: &str) -> String {
    let h = HOUR_MS;

    match table {
        "node_minute" => format!(
            "INSERT INTO {dst}.node_1h SELECT server_id, minute - minute % {h} AS hour, count(*),
                sum(cpu_avg::DOUBLE), max(cpu_max), sum(mem_used::DOUBLE), max(mem_total), sum(disk_used::DOUBLE),
                max(disk_total), sum(rx::DOUBLE), sum(tx::DOUBLE), sum(load1::DOUBLE)
             FROM {src}.node_minute WHERE rowid >= $1 AND rowid < $2 AND minute - minute % {h} >= $3
                AND minute - minute % {h} < $4
             GROUP BY server_id, hour"
        ),
        _ => format!(
            "INSERT INTO {dst}.container_1h SELECT server_id, container_name, minute - minute % {h} AS hour, count(*),
                max(minute), arg_max(container_id, minute), arg_max(image, minute), arg_max(service_id, minute),
                arg_max(stack, minute), sum(cpu::DOUBLE), max(cpu), sum(mem_used::DOUBLE), max(mem_limit),
                sum(rx::DOUBLE), sum(tx::DOUBLE), sum(block_read::DOUBLE), sum(block_write::DOUBLE), max(restarts)
             FROM {src}.container_minute WHERE rowid >= $1 AND rowid < $2 AND minute - minute % {h} >= $3
                AND minute - minute % {h} < $4
             GROUP BY server_id, container_name, hour"
        ),
    }
}

/// Folds new minute rows into the hourly rollups; returns the rows folded.
pub fn rollups(w: &mut Writer) -> Result<u64, Error> {
    let mut folded = 0u64;

    for part in w.catalog.files(Family::Metrics) {
        let lease = w.catalog.lease(&[part], false)?;
        if lease.parts().is_empty() {
            continue;
        }

        let src = part.alias();

        for table in MINUTE_TABLES {
            let upto = crate::read::rolled_upto(&w.rolled, part.index, table);
            let end: i64 = w.conn.query_row(&format!("SELECT count(*) FROM {src}.{table}"), [], |r| r.get(0))?;
            if end <= upto {
                continue;
            }

            let months: Vec<i64> = {
                let mut st = w.conn.prepare(&format!(
                    "SELECT DISTINCT minute - minute % {HOUR_MS} FROM {src}.{table} WHERE rowid >= $1 AND rowid < $2"
                ))?;
                let hours = st.query_map(params![upto, end], |r| r.get::<_, i64>(0))?;
                let mut m: Vec<i64> = Vec::new();

                for h in hours {
                    let idx = Part::at(Family::Rollups, h?).index;
                    if !m.contains(&idx) {
                        m.push(idx);
                    }
                }

                m
            };

            // The bound moves only after the states are in; readers wait
            // on the lock so they never count a row twice.
            let mut rolled = w.rolled.write().unwrap_or_else(std::sync::PoisonError::into_inner);

            for index in months {
                let dst = Part { family: Family::Rollups, index };
                let dst_lease = w.catalog.lease(&[dst], true)?;
                w.conn.execute_batch("BEGIN")?;
                let r = w
                    .conn
                    .execute(&rollup_sql(table, &src, &dst.alias()), params![upto, end, dst.start_ms(), dst.end_ms()]);

                match r {
                    Ok(_) => w.conn.execute_batch("COMMIT")?,
                    Err(e) => {
                        let _ = w.conn.execute_batch("ROLLBACK");
                        return Err(e.into());
                    }
                }

                drop(dst_lease);
            }

            // A crash before this line applies the range again next time:
            // every average and maximum stays the same (see `schema`).
            w.conn.execute("DELETE FROM meta.rolled WHERE part = ? AND tbl = ?", params![part.index, table])?;
            w.conn.execute("INSERT INTO meta.rolled VALUES (?, ?, ?)", params![part.index, table, end])?;
            rolled.insert((part.index, table), end);
            folded += u64::try_from(end - upto).unwrap_or(0);
        }
    }

    Ok(folded)
}

/// The first part of `family` kept under `r`.
#[must_use]
pub fn keep_from(family: Family, r: &Retention, now: i64) -> i64 {
    let today = now.div_euclid(DAY_MS);

    match family {
        Family::Logs => today - i64::from(r.app_logs_days.max(1)),
        Family::Metrics => today - i64::from(r.node_metrics_days.max(1)),
        Family::Processes => today - i64::from(r.top_processes_days.max(1)),
        // A month is dropped once its last hour is past the retention.
        Family::Rollups => Part::at(Family::Rollups, now - i64::from(r.node_rollup_days.max(1)) * DAY_MS).index,
    }
}

/// Drops the files past `r`; returns how many. Minute files are folded
/// into the rollups first, so no minute is lost from the hourly history.
pub fn retention(w: &mut Writer, r: &Retention) -> Result<u64, Error> {
    rollups(w)?;
    let now = w.clock.now().0;
    let mut dropped = 0u64;

    for fam in Family::ALL {
        let keep = keep_from(fam, r, now);
        let gone: Vec<Part> = w.catalog.files(fam).into_iter().filter(|p| p.index < keep).collect();
        dropped += w.catalog.expire(fam, keep) as u64;

        if fam == Family::Metrics {
            let mut rolled = w.rolled.write().unwrap_or_else(std::sync::PoisonError::into_inner);

            for p in gone {
                w.conn.execute("DELETE FROM meta.rolled WHERE part = ?", params![p.index])?;
                rolled.retain(|(i, _), _| *i != p.index);
            }
        }
    }

    Ok(dropped)
}

/// Checkpoints closed day files and detaches idle ones; returns the files
/// checkpointed.
pub fn compaction(w: &mut Writer) -> Result<u64, Error> {
    let keep: HashSet<Part> = w.current_parts()?.into_iter().collect();
    let today = w.today();
    let mut n = 0;

    for p in w.catalog.attached() {
        if !p.family.monthly() && p.index < today {
            w.catalog.checkpoint(p)?;
            n += 1;
        }
    }

    w.catalog.detach_idle(IDLE, &keep);

    Ok(n)
}
