//! Reads, with the ClickHouse adapter's semantics: the same filters, the
//! same grouping of duplicate minutes (`avg`, `max`, latest identity), the
//! same clamps. Only the day files whose time range overlaps the window are
//! opened. Every value from a request is a bound parameter; only fixed SQL,
//! generated aliases and integers the store computed are written into the
//! text.

use std::collections::HashMap;
use std::fmt::Write as _;

use duckdb::Connection;
use duckdb::types::Value;
use forge_core::analytics::{
    ContainerPoint, ContainerRowOut, ContainerSeries, NodePoint, ProcessUsage, Resolution, TopBy, group_series,
};
use forge_core::logs::{LogBucket, LogEvent, LogLevel, LogPage, LogQuery, LogSource, LogStream};
use forge_core::{ServerId, Timestamp};

use crate::Error;
use crate::catalog::{Catalog, Lease};
use crate::parts::{Family, HOUR_MS, Part};
use crate::writer::Rolled;

/// Bound parameters, `$1`, `$2`, ...
#[derive(Debug, Default)]
struct Params(Vec<Value>);

impl Params {
    fn push(&mut self, v: Value) -> String {
        self.0.push(v);
        format!("${}", self.0.len())
    }

    fn text(&mut self, s: impl Into<String>) -> String {
        self.push(Value::Text(s.into()))
    }

    fn int(&mut self, i: i64) -> String {
        self.push(Value::BigInt(i))
    }

    /// `col IN ($a, $b, ..)`, or `FALSE` for an empty list.
    fn in_list(&mut self, col: &str, items: impl IntoIterator<Item = String>) -> String {
        let ps: Vec<String> = items.into_iter().map(|s| self.text(s)).collect();
        if ps.is_empty() { "FALSE".into() } else { format!("{col} IN ({})", ps.join(", ")) }
    }
}

/// The rows of `table` in every leased file, as one relation.
fn union(lease: &Lease, table: &str, cols: &str) -> Option<String> {
    let parts: Vec<String> =
        lease.parts().iter().map(|p| format!("SELECT {cols} FROM {}.{table}", p.alias())).collect();
    (!parts.is_empty()).then(|| parts.join(" UNION ALL "))
}

/// The row bound below which `(day, table)` rows are in the rollups.
#[must_use]
pub fn rolled_upto(rolled: &Rolled, day: i64, table: &'static str) -> i64 {
    rolled.read().unwrap_or_else(std::sync::PoisonError::into_inner).get(&(day, table)).copied().unwrap_or(0)
}

/// Seconds-precision bound like the ClickHouse `DateTime` columns.
fn sec_ms(t: Timestamp) -> i64 {
    t.0.div_euclid(1000) * 1000
}

const LOG_COLUMNS: &str = "service_id, ts, id, node_ts, ingest_ts, level, message, message_template, exception, \
    trace_id, span_id, event_type, server_id, properties, source, container_id, container_name, stack, stream";

fn log_where(q: &LogQuery, p: &mut Params) -> String {
    let apps = p.in_list("service_id", q.services.iter().map(ToString::to_string));
    let inst = p.in_list("(service_id || ':' || server_id)", q.instances.iter().map(|(a, s)| format!("{a}:{s}")));
    let open = p.in_list("server_id", q.open_nodes.iter().map(ToString::to_string));
    let mut sql = format!(
        "({apps} OR (service_id IS NOT NULL AND server_id IS NOT NULL AND {inst}) OR (service_id IS NULL AND {open}))"
    );

    if !q.nodes.is_empty() {
        let nodes = p.in_list("server_id", q.nodes.iter().map(ToString::to_string));
        let _ = write!(sql, " AND {nodes}");
    }

    if let Some(c) = &q.container {
        let c = p.text(c.clone());
        let _ = write!(sql, " AND (container_name = {c} OR starts_with(container_id, {c}))");
    }

    if let Some(st) = &q.stack {
        let st = p.text(st.clone());
        let _ = write!(sql, " AND stack = {st}");
    }

    if let Some(src) = q.source {
        let src = p.text(src.as_str());
        let _ = write!(sql, " AND source = {src}");
    }

    if !q.levels.is_empty() {
        let l = p.in_list("level", q.levels.iter().map(|l| l.as_str().to_owned()));
        let _ = write!(sql, " AND {l}");
    }

    if let Some(t) = &q.filter.text {
        let t = p.text(t.to_lowercase());
        let _ = write!(sql, " AND (contains(lower(message), {t}) OR contains(lower(exception), {t}))");
    }

    for (k, v) in &q.filter.properties {
        // As in ClickHouse: a JSON string equal to the value or, when the
        // value reads as a JSON scalar, that raw value.
        let quoted = serde_json::Value::String(v.clone()).to_string();

        let raw = match serde_json::from_str::<serde_json::Value>(v) {
            Ok(j @ (serde_json::Value::Number(_) | serde_json::Value::Bool(_) | serde_json::Value::Null)) => {
                j.to_string()
            }
            _ => quoted.clone(),
        };

        let path = p.text(json_path(k));
        let (raw, quoted) = (p.text(raw), p.text(quoted));
        let _ = write!(sql, " AND CAST(json_extract(properties, {path}) AS VARCHAR) IN ({raw}, {quoted})");
    }

    if let Some(f) = q.from {
        let f = p.int(f.0);
        let _ = write!(sql, " AND ts >= {f}");
    }

    if let Some(t) = q.to {
        let t = p.int(t.0);
        let _ = write!(sql, " AND ts < {t}");
    }

    sql
}

/// A JSON path naming one top-level key exactly.
fn json_path(key: &str) -> String {
    format!("$.{}", serde_json::Value::String(key.to_owned()))
}

fn opt(s: Option<String>) -> Option<String> {
    s.filter(|s| !s.is_empty())
}

fn log_event(r: &duckdb::Row<'_>) -> duckdb::Result<LogEvent> {
    let props: String = r.get(13)?;
    let properties = match serde_json::from_str::<serde_json::Value>(&props) {
        Ok(serde_json::Value::Object(m)) => m,
        // A bag that cannot be read is shown empty rather than failing the page.
        _ => serde_json::Map::new(),
    };

    let level: String = r.get(5)?;
    let source: String = r.get(14)?;
    let stream: String = r.get(18)?;
    let id: String = r.get(2)?;

    Ok(LogEvent {
        service_id: r.get::<_, Option<String>>(0)?.and_then(|s| s.parse().ok()),
        ts: Timestamp(r.get(1)?),
        id: id.parse().unwrap_or_default(),
        node_ts: r.get::<_, Option<i64>>(3)?.map(Timestamp),
        ingest_ts: Timestamp(r.get(4)?),
        level: LogLevel::parse_lenient(&level).unwrap_or(LogLevel::Information),
        message: r.get(6)?,
        message_template: r.get(7)?,
        exception: opt(r.get(8)?),
        trace_id: opt(r.get(9)?),
        span_id: opt(r.get(10)?),
        event_type: r.get(11)?,
        server_id: r.get::<_, Option<String>>(12)?.and_then(|s| s.parse().ok()),
        properties,
        source: LogSource::parse(&source).unwrap_or_default(),
        container_id: opt(r.get(15)?),
        container_name: opt(r.get(16)?),
        stack: opt(r.get(17)?),
        stream: LogStream::parse(&stream),
    })
}

fn log_window(q: &LogQuery) -> (i64, i64) {
    (q.from.map_or(i64::MIN, |t| t.0), q.to.map_or(i64::MAX, |t| t.0))
}

/// One page of events, newest first.
pub fn query_logs(cat: &std::sync::Arc<Catalog>, c: &Connection, q: &LogQuery) -> Result<Vec<LogEvent>, Error> {
    if q.is_empty() {
        return Ok(Vec::new());
    }

    let (from, to) = log_window(q);
    let lease = cat.lease(&cat.overlapping(Family::Logs, from, to), false)?;
    let Some(src) = union(&lease, "app_logs", "*") else { return Ok(Vec::new()) };
    let mut p = Params::default();
    let mut wh = log_where(q, &mut p);

    let (cursor, order) = match q.page {
        LogPage::Older(c) => (c.map(|c| (c, "<")), "DESC"),
        LogPage::Newer(c) => (Some((c, ">")), "ASC"),
    };

    if let Some((c, op)) = cursor {
        let (ts, id) = (p.int(c.ts), p.text(c.id.to_string()));

        // `op` is one of two fixed operators.
        let _ = write!(wh, " AND (ts {op} {ts} OR (ts = {ts} AND id {op} {id}))");
    }

    let limit = p.int(i64::from(q.limit.clamp(1, 1000)));
    let sql = format!("SELECT {LOG_COLUMNS} FROM ({src}) WHERE {wh} ORDER BY ts {order}, id {order} LIMIT {limit}");
    let mut st = c.prepare(&sql)?;
    let rows = st.query_map(duckdb::params_from_iter(p.0.iter()), log_event)?;
    let mut out = rows.collect::<Result<Vec<_>, _>>()?;

    if matches!(q.page, LogPage::Newer(_)) {
        out.reverse(); // newest first, like every page
    }

    Ok(out)
}

/// Counts per level in buckets of `step_ms`.
pub fn log_histogram(
    cat: &std::sync::Arc<Catalog>,
    c: &Connection,
    q: &LogQuery,
    step_ms: i64,
) -> Result<Vec<LogBucket>, Error> {
    if q.is_empty() {
        return Ok(Vec::new());
    }

    let (from, to) = log_window(q);
    let lease = cat.lease(&cat.overlapping(Family::Logs, from, to), false)?;

    let Some(src) = union(
        &lease,
        "app_logs",
        "ts, level, service_id, server_id, container_name, container_id, stack, source, message, exception, properties",
    ) else {
        return Ok(Vec::new());
    };

    let mut p = Params::default();
    let wh = log_where(q, &mut p);
    let step = p.int(step_ms.max(1000));
    let sql = format!(
        "SELECT (ts // {step}) * {step} AS t, level, count(*) FROM ({src}) WHERE {wh} GROUP BY t, level ORDER BY t"
    );
    let mut st = c.prepare(&sql)?;
    let rows = st.query_map(duckdb::params_from_iter(p.0.iter()), |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?))
    })?;
    let mut out = Vec::new();

    for row in rows {
        let (t, level, count) = row?;
        if let Some(level) = LogLevel::parse_lenient(&level) {
            out.push(LogBucket { t, level, count: u64::try_from(count).unwrap_or(0) });
        }
    }

    Ok(out)
}

/// Live minute rows of `table` not yet in the rollups, per leased file.
fn live(lease: &Lease, rolled: &HashMap<(i64, &'static str), i64>, table: &'static str, cols: &str) -> Vec<String> {
    lease
        .parts()
        .iter()
        .map(|p| {
            let upto = rolled.get(&(p.index, table)).copied().unwrap_or(0);
            format!("SELECT {cols} FROM {}.{table} WHERE rowid >= {upto}", p.alias())
        })
        .collect()
}

/// The rollup files for hours in `[from, to)`.
fn rollup_parts(cat: &Catalog, from: i64, to: i64) -> Vec<Part> {
    cat.files(Family::Rollups).into_iter().filter(|p| p.start_ms() < to && p.end_ms() > from).collect()
}

fn u64_of(r: &duckdb::Row<'_>, i: usize) -> duckdb::Result<u64> {
    Ok(r.get::<_, Option<u64>>(i)?.unwrap_or(0))
}

/// A node's history.
pub fn node_series(
    cat: &std::sync::Arc<Catalog>,
    rolled: &Rolled,
    c: &Connection,
    server: ServerId,
    window: (Timestamp, Timestamp),
    resolution: Resolution,
) -> Result<Vec<NodePoint>, Error> {
    let (from, to) = (sec_ms(window.0), sec_ms(window.1));
    let mut p = Params::default();
    let (s, f, t) = (p.text(server.to_string()), p.int(from), p.int(to));

    match resolution {
        Resolution::Minute => {
            let lease = cat.lease(&cat.overlapping(Family::Metrics, from, to), false)?;
            let Some(src) = union(&lease, "node_minute", "*") else { return Ok(Vec::new()) };
            let sql = format!(
                "SELECT minute, avg(cpu_avg)::REAL, max(cpu_max), CAST(trunc(avg(mem_used)) AS UBIGINT), max(mem_total),
                    CAST(trunc(avg(disk_used)) AS UBIGINT), max(disk_total), CAST(trunc(avg(rx)) AS UBIGINT),
                    CAST(trunc(avg(tx)) AS UBIGINT), avg(load1)::REAL
                 FROM ({src}) WHERE server_id = {s} AND minute >= {f} AND minute < {t}
                 GROUP BY minute ORDER BY minute"
            );
            run_points(c, &sql, &p, lease)
        }
        Resolution::Hour => {
            let guard = rolled.read().unwrap_or_else(std::sync::PoisonError::into_inner);

            // Minute files that can hold rows of these hours.
            let minutes = cat.lease(&cat.overlapping(Family::Metrics, from, to.saturating_add(HOUR_MS)), false)?;
            let states = cat.lease(&rollup_parts(cat, from, to), false)?;
            let mut parts = live(
                &minutes,
                &guard,
                "node_minute",
                &format!(
                    "server_id, minute - minute % {HOUR_MS} AS hour, 1 AS n, cpu_avg::DOUBLE AS cpu_sum, cpu_max,
                    mem_used::DOUBLE AS mem_sum, mem_total, disk_used::DOUBLE AS disk_sum, disk_total,
                    rx::DOUBLE AS rx_sum, tx::DOUBLE AS tx_sum, load1::DOUBLE AS load_sum"
                ),
            );
            parts.extend(states.parts().iter().map(|r| format!("SELECT * FROM {}.node_1h", r.alias())));

            if parts.is_empty() {
                return Ok(Vec::new());
            }

            let src = parts.join(" UNION ALL ");
            let sql = format!(
                "SELECT hour, (sum(cpu_sum) / sum(n))::REAL, max(cpu_max), CAST(trunc(sum(mem_sum) / sum(n)) AS UBIGINT),
                    max(mem_total), CAST(trunc(sum(disk_sum) / sum(n)) AS UBIGINT), max(disk_total),
                    CAST(trunc(sum(rx_sum) / sum(n)) AS UBIGINT), CAST(trunc(sum(tx_sum) / sum(n)) AS UBIGINT),
                    (sum(load_sum) / sum(n))::REAL
                 FROM ({src}) WHERE server_id = {s} AND hour >= {f} AND hour < {t}
                 GROUP BY hour ORDER BY hour"
            );
            let out = run_points(c, &sql, &p, (minutes, states));
            drop(guard);
            out
        }
    }
}

fn run_points<L>(c: &Connection, sql: &str, p: &Params, _lease: L) -> Result<Vec<NodePoint>, Error> {
    let mut st = c.prepare(sql)?;
    let rows = st.query_map(duckdb::params_from_iter(p.0.iter()), |r| {
        Ok(NodePoint {
            t: r.get(0)?,
            cpu_avg: r.get::<_, Option<f32>>(1)?.unwrap_or(0.0),
            cpu_max: r.get::<_, Option<f32>>(2)?.unwrap_or(0.0),
            mem_used: u64_of(r, 3)?,
            mem_total: u64_of(r, 4)?,
            disk_used: u64_of(r, 5)?,
            disk_total: u64_of(r, 6)?,
            rx: u64_of(r, 7)?,
            tx: u64_of(r, 8)?,
            load1: r.get::<_, Option<f32>>(9)?.unwrap_or(0.0),
        })
    })?;

    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// The processes that used the most CPU or memory.
pub fn top_processes(
    cat: &std::sync::Arc<Catalog>,
    c: &Connection,
    server: ServerId,
    window: (Timestamp, Timestamp),
    by: TopBy,
    limit: u32,
) -> Result<Vec<ProcessUsage>, Error> {
    let (from, to) = (sec_ms(window.0), sec_ms(window.1));
    let lease = cat.lease(&cat.overlapping(Family::Processes, from, to), false)?;
    let Some(src) = union(&lease, "top_process", "*") else { return Ok(Vec::new()) };
    let mut p = Params::default();
    let (s, f, t) = (p.text(server.to_string()), p.int(from), p.int(to));
    let l = p.int(i64::from(limit.clamp(1, 100)));

    let order = match by {
        TopBy::Cpu => "cpu_avg",
        TopBy::Mem => "mem_avg",
    };

    // `order` is one of two fixed column names.
    let sql = format!(
        "SELECT name, \"user\", avg(cpu)::REAL AS cpu_avg, max(cpu), CAST(trunc(avg(mem)) AS UBIGINT) AS mem_avg,
            max(mem), count(DISTINCT minute)::UINTEGER, any_value(cmd)
         FROM ({src}) WHERE server_id = {s} AND minute >= {f} AND minute < {t}
         GROUP BY name, \"user\" ORDER BY {order} DESC LIMIT {l}"
    );
    let mut st = c.prepare(&sql)?;
    let rows = st.query_map(duckdb::params_from_iter(p.0.iter()), |r| {
        Ok(ProcessUsage {
            name: r.get(0)?,
            user: r.get(1)?,
            cpu_avg: r.get::<_, Option<f32>>(2)?.unwrap_or(0.0),
            cpu_max: r.get::<_, Option<f32>>(3)?.unwrap_or(0.0),
            mem_avg: u64_of(r, 4)?,
            mem_max: u64_of(r, 5)?,
            minutes: r.get(6)?,
            cmd: r.get::<_, Option<String>>(7)?.unwrap_or_default(),
        })
    })?;

    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// Filters of a container history query.
#[derive(Debug)]
pub struct ContainerQuery<'a> {
    pub server: ServerId,
    pub container: Option<&'a str>,
    pub window: (Timestamp, Timestamp),
    pub resolution: Resolution,
    pub limit: u32,
}

/// The containers of one node.
pub fn container_series(
    cat: &std::sync::Arc<Catalog>,
    rolled: &Rolled,
    c: &Connection,
    q: &ContainerQuery<'_>,
) -> Result<Vec<ContainerSeries>, Error> {
    let (from, to) = (sec_ms(q.window.0), sec_ms(q.window.1));
    let guard = rolled.read().unwrap_or_else(std::sync::PoisonError::into_inner);

    let (src, select, col, _leases) = match q.resolution {
        Resolution::Minute => {
            let lease = cat.lease(&cat.overlapping(Family::Metrics, from, to), false)?;
            let Some(src) = union(&lease, "container_minute", "*") else { return Ok(Vec::new()) };
            let select = "arg_max(container_id, minute), arg_max(image, minute), arg_max(service_id, minute),
                arg_max(stack, minute), avg(cpu)::REAL, max(cpu), CAST(trunc(avg(mem_used)) AS UBIGINT),
                max(mem_limit), CAST(trunc(avg(rx)) AS UBIGINT), CAST(trunc(avg(tx)) AS UBIGINT),
                CAST(trunc(avg(block_read)) AS UBIGINT), CAST(trunc(avg(block_write)) AS UBIGINT), max(restarts)";
            (src, select, "minute", (lease, None))
        }
        Resolution::Hour => {
            let minutes = cat.lease(&cat.overlapping(Family::Metrics, from, to.saturating_add(HOUR_MS)), false)?;
            let states = cat.lease(&rollup_parts(cat, from, to), false)?;
            let mut parts = live(
                &minutes,
                &guard,
                "container_minute",
                &format!(
                    "server_id, container_name, minute - minute % {HOUR_MS} AS hour, 1 AS n, minute AS last_minute,
                    container_id, image, service_id, stack, cpu::DOUBLE AS cpu_sum, cpu AS cpu_max,
                    mem_used::DOUBLE AS mem_sum, mem_limit, rx::DOUBLE AS rx_sum, tx::DOUBLE AS tx_sum,
                    block_read::DOUBLE AS br_sum, block_write::DOUBLE AS bw_sum, restarts"
                ),
            );
            parts.extend(states.parts().iter().map(|r| format!("SELECT * FROM {}.container_1h", r.alias())));

            if parts.is_empty() {
                return Ok(Vec::new());
            }

            let select = "arg_max(container_id, last_minute), arg_max(image, last_minute),
                arg_max(service_id, last_minute), arg_max(stack, last_minute), (sum(cpu_sum) / sum(n))::REAL,
                max(cpu_max), CAST(trunc(sum(mem_sum) / sum(n)) AS UBIGINT), max(mem_limit),
                CAST(trunc(sum(rx_sum) / sum(n)) AS UBIGINT), CAST(trunc(sum(tx_sum) / sum(n)) AS UBIGINT),
                CAST(trunc(sum(br_sum) / sum(n)) AS UBIGINT), CAST(trunc(sum(bw_sum) / sum(n)) AS UBIGINT),
                max(restarts)";
            (parts.join(" UNION ALL "), select, "hour", (minutes, Some(states)))
        }
    };

    let mut p = Params::default();
    let (s, f, t) = (p.text(q.server.to_string()), p.int(from), p.int(to));
    let scope = format!("server_id = {s} AND {col} >= {f} AND {col} < {t}");

    let filter = match q.container {
        Some(name) => format!(" AND container_name = {}", p.text(name)),
        None => {
            let l = p.int(i64::from(q.limit.clamp(1, 500)));
            format!(
                " AND container_name IN (SELECT DISTINCT container_name FROM src WHERE {scope}
                    ORDER BY container_name LIMIT {l})"
            )
        }
    };

    // `select` and `col` are fixed text chosen above.
    let sql = format!(
        "WITH src AS ({src}) SELECT container_name, {col}, {select} FROM src WHERE {scope}{filter}
         GROUP BY container_name, {col} ORDER BY container_name, {col}"
    );
    let mut st = c.prepare(&sql)?;
    let rows = st.query_map(duckdb::params_from_iter(p.0.iter()), |r| {
        Ok(ContainerRowOut {
            container_name: r.get(0)?,
            container_id: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
            image: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
            service_id: r.get::<_, Option<String>>(4)?.and_then(|s| s.parse().ok()),
            stack: r.get::<_, Option<String>>(5)?.unwrap_or_default(),
            point: ContainerPoint {
                t: r.get(1)?,
                cpu_avg: r.get::<_, Option<f32>>(6)?.unwrap_or(0.0),
                cpu_max: r.get::<_, Option<f32>>(7)?.unwrap_or(0.0),
                mem_used: u64_of(r, 8)?,
                mem_limit: u64_of(r, 9)?,
                rx: u64_of(r, 10)?,
                tx: u64_of(r, 11)?,
                block_read: u64_of(r, 12)?,
                block_write: u64_of(r, 13)?,
                restarts: r.get::<_, Option<u32>>(14)?.unwrap_or(0),
            },
        })
    })?;
    let rows = rows.collect::<Result<Vec<_>, _>>()?;
    drop(guard);

    Ok(group_series(rows))
}
