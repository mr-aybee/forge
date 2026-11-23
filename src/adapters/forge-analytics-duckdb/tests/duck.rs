//! DuckDB store: day partitioning, pruning, retention by file, exactly-once
//! spool files across a restart and a crash between families, rollups.
#![allow(clippy::unwrap_used)] // reason: tests

use std::sync::Arc;

use forge_analytics_duckdb::parts::{DAY_MS, Family, HOUR_MS, Part};
use forge_analytics_duckdb::{DuckDb, DuckDbConfig, Limits};
use forge_core::analytics::{AnalyticsStore, FileWrite, NodeMinute, Resolution, Retention, SpoolFileWriter, SpoolRows};
use forge_core::logs::{IngestContext, LogEvent, LogFilter, LogPage, LogQuery, SearchLimits, parse_batch};
use forge_core::{ManualClock, ServerId, ServiceId, Timestamp};

/// 2026-11-22T10:00:00Z
const T0: i64 = 1_795_341_600_000;

const RET: Retention =
    Retention { node_metrics_days: 2, top_processes_days: 2, node_rollup_days: 60, app_logs_days: 3 };

fn open(dir: &std::path::Path, clock: &Arc<ManualClock>) -> Arc<DuckDb> {
    let cfg =
        DuckDbConfig { dir: dir.to_path_buf(), limits: Limits { memory_mb: 128, threads: 1 }, batch_rows: 10_000 };
    DuckDb::open(&cfg, clock.clone()).unwrap()
}

fn logs(service: ServiceId, node: ServerId, ts: i64, n: usize, now: i64) -> Vec<LogEvent> {
    let body: String =
        (0..n).map(|i| format!("{{\"@t\":{},\"@mt\":\"line {i}\"}}\n", ts_text(ts + i as i64))).collect();
    let cx = IngestContext { service_id: service, server_id: Some(node), node_ts: None, now: Timestamp(now) };
    parse_batch(&body, cx).unwrap()
}

fn ts_text(ms: i64) -> String {
    let (y, m, d) = forge_analytics_duckdb::parts::civil(ms.div_euclid(DAY_MS));
    let r = ms.rem_euclid(DAY_MS);
    format!(
        "\"{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{:03}Z\"",
        r / 3_600_000,
        r / 60_000 % 60,
        r / 1000 % 60,
        r % 1000
    )
}

fn minute(node: ServerId, at: i64, cpu: f32) -> NodeMinute {
    NodeMinute {
        server_id: node,
        minute: Timestamp(at),
        cpu_avg: cpu,
        cpu_max: cpu,
        mem_used: 100,
        mem_total: 200,
        swap_used: 0,
        disk_used: 10,
        disk_total: 20,
        rx: 1,
        tx: 2,
        load1: 0.5,
        samples: 6,
    }
}

fn query(service: ServiceId) -> LogQuery {
    LogQuery {
        services: vec![service],
        instances: vec![],
        open_nodes: vec![],
        nodes: vec![],
        container: None,
        stack: None,
        source: None,
        levels: vec![],
        filter: LogFilter::default(),
        from: None,
        to: None,
        page: LogPage::Older(None),
        limit: 1000,
        limits: SearchLimits::default(),
    }
}

fn file(node: ServerId, id: &str, logs: Vec<LogEvent>, minutes: Vec<NodeMinute>) -> SpoolRows {
    SpoolRows { node, file_id: id.into(), logs, minutes, processes: vec![], containers: vec![] }
}

fn id(i: u32) -> String {
    format!("{i:032x}")
}

#[tokio::test(flavor = "multi_thread")]
async fn rows_go_to_the_ingest_day_file_and_queries_prune_by_range() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(ManualClock::new(Timestamp(T0)));
    let s = open(dir.path(), &clock);
    s.migrate(RET).await.unwrap();
    let (svc, node) = (ServiceId::new(), ServerId::new());
    // Day 1: events of day 1. Day 2: events of day 2 plus a late one of day 1.
    s.write_logs(&logs(svc, node, T0, 3, T0)).await.unwrap();
    clock.set(Timestamp(T0 + DAY_MS));
    let mut late = logs(svc, node, T0 + DAY_MS, 2, T0 + DAY_MS);
    late.extend(logs(svc, node, T0 + 10, 1, T0 + DAY_MS));
    s.write_logs(&late).await.unwrap();
    let files = s.catalog().files(Family::Logs);
    assert_eq!(files.len(), 2, "one file per ingest day");
    assert!(dir.path().join("logs").join(Part::at(Family::Logs, T0).file_name()).exists());
    assert_eq!(s.query_logs(&query(svc)).await.unwrap().len(), 6);
    // A window inside day 2's events only needs day 2's file.
    let day2 = Part::at(Family::Logs, T0 + DAY_MS);
    let only = s.catalog().overlapping(Family::Logs, T0 + DAY_MS, T0 + DAY_MS + 1000);
    assert_eq!(only, vec![day2]);
    // A window on day 1 needs both: day 2's file holds a late day-1 event.
    assert_eq!(s.catalog().overlapping(Family::Logs, T0, T0 + 100).len(), 2);
    let mut q = query(svc);
    (q.from, q.to) = (Some(Timestamp(T0)), Some(Timestamp(T0 + 100)));
    assert_eq!(s.query_logs(&q).await.unwrap().len(), 4, "3 on time plus the late one");
}

#[tokio::test(flavor = "multi_thread")]
async fn retention_drops_whole_files() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(ManualClock::new(Timestamp(T0)));
    let s = open(dir.path(), &clock);
    s.migrate(RET).await.unwrap();
    let (svc, node) = (ServiceId::new(), ServerId::new());
    for d in 0..6 {
        clock.set(Timestamp(T0 + d * DAY_MS));
        s.write_logs(&logs(svc, node, T0 + d * DAY_MS, 2, T0 + d * DAY_MS)).await.unwrap();
        s.write_node_minutes(&[minute(node, T0 + d * DAY_MS, 1.0)]).await.unwrap();
    }
    assert_eq!(s.catalog().files(Family::Logs).len(), 6);
    s.apply_retention(RET).await.unwrap();
    // Today is day 5: logs keep days 2..=5 (3 days back), metrics 3..=5.
    assert_eq!(s.catalog().files(Family::Logs).len(), 4);
    assert_eq!(s.catalog().files(Family::Metrics).len(), 3);
    let first = Part::at(Family::Logs, T0);
    assert!(!dir.path().join("logs").join(first.file_name()).exists(), "the file is deleted");
    assert!(!dir.path().join("logs").join(format!("{}.wal", first.file_name())).exists());
    assert_eq!(s.query_logs(&query(svc)).await.unwrap().len(), 8);
    // The minutes were folded into the rollups before their files went.
    let hours =
        s.node_series(node, Timestamp(T0 - HOUR_MS), Timestamp(T0 + 7 * DAY_MS), Resolution::Hour).await.unwrap();
    assert_eq!(hours.len(), 6, "every day's hour survives in the rollups");
    let reports = s.reports();
    assert!(reports.iter().any(|r| r.name == "retention" && r.error.is_none() && r.done >= 3), "{reports:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_file_sent_again_is_stored_once_even_after_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(ManualClock::new(Timestamp(T0)));
    let (svc, node) = (ServiceId::new(), ServerId::new());
    let full = file(node, &id(1), logs(svc, node, T0, 5, T0), vec![minute(node, T0, 2.0)]);
    {
        let s = open(dir.path(), &clock);
        s.migrate(RET).await.unwrap();
        assert_eq!(s.write_spool_file(full.clone()).await.unwrap(), FileWrite::Stored);
        assert_eq!(s.write_spool_file(full.clone()).await.unwrap(), FileWrite::AlreadyStored);
        // A crash between families: only the logs of file 2 were committed.
        let part = file(node, &id(2), logs(svc, node, T0 + 100, 3, T0), vec![]);
        assert_eq!(s.write_spool_file(part).await.unwrap(), FileWrite::Stored);
    }
    let s = open(dir.path(), &clock);
    assert_eq!(s.write_spool_file(full).await.unwrap(), FileWrite::AlreadyStored, "remembered across a restart");
    // The agent sends file 2 again in full: its logs are skipped, its minutes stored.
    let again = file(node, &id(2), logs(svc, node, T0 + 100, 3, T0), vec![minute(node, T0 + 60_000, 4.0)]);
    assert_eq!(s.write_spool_file(again).await.unwrap(), FileWrite::Stored);
    assert_eq!(s.query_logs(&query(svc)).await.unwrap().len(), 8, "5 + 3, nothing twice");
    let pts = s.node_series(node, Timestamp(T0), Timestamp(T0 + HOUR_MS), Resolution::Minute).await.unwrap();
    assert_eq!(pts.len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_files_are_batched_and_each_acknowledged() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(ManualClock::new(Timestamp(T0)));
    let s = open(dir.path(), &clock);
    s.migrate(RET).await.unwrap();
    let svc = ServiceId::new();
    let mut tasks = Vec::new();
    for i in 0..40u32 {
        let (s, node) = (s.clone(), ServerId::new());
        let f = file(node, &id(i), logs(svc, node, T0 + i64::from(i), 10, T0), vec![minute(node, T0, 1.0)]);
        tasks.push(tokio::spawn(async move { s.write_spool_file(f).await }));
    }
    for t in tasks {
        assert_eq!(t.await.unwrap().unwrap(), FileWrite::Stored);
    }
    assert_eq!(s.query_logs(&query(svc)).await.unwrap().len(), 400);
}

#[tokio::test(flavor = "multi_thread")]
async fn hourly_history_merges_rollups_with_new_minutes() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(ManualClock::new(Timestamp(T0)));
    let s = open(dir.path(), &clock);
    s.migrate(RET).await.unwrap();
    let node = ServerId::new();
    s.write_node_minutes(&[minute(node, T0, 10.0), minute(node, T0 + 60_000, 20.0)]).await.unwrap();
    s.run_jobs_once().await;
    // A late minute of the same hour after the rollup ran.
    s.write_node_minutes(&[minute(node, T0 + 120_000, 30.0)]).await.unwrap();
    let h = s.node_series(node, Timestamp(T0), Timestamp(T0 + HOUR_MS), Resolution::Hour).await.unwrap();
    assert_eq!(h.len(), 1);
    assert!((h[0].cpu_avg - 20.0).abs() < 0.01, "average of 10, 20, 30: {}", h[0].cpu_avg);
    assert!((h[0].cpu_max - 30.0).abs() < 0.01);
    // Folding the late row changes nothing.
    s.run_jobs_once().await;
    let again = s.node_series(node, Timestamp(T0), Timestamp(T0 + HOUR_MS), Resolution::Hour).await.unwrap();
    assert_eq!(h, again);
}

#[tokio::test(flavor = "multi_thread")]
async fn reads_beyond_the_thread_count_wait_their_turn() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(ManualClock::new(Timestamp(T0)));
    let s = open(dir.path(), &clock);
    s.migrate(RET).await.unwrap();
    let (svc, node) = (ServiceId::new(), ServerId::new());
    s.write_logs(&logs(svc, node, T0, 50, T0)).await.unwrap();
    assert_eq!(forge_analytics_duckdb::store::read_slots(Limits { memory_mb: 128, threads: 1 }), 1);
    // One slot: 16 searches at once all finish, one after another.
    let tasks: Vec<_> = (0..16)
        .map(|_| {
            let s = s.clone();
            tokio::spawn(async move { s.query_logs(&query(svc)).await.map(|p| p.len()) })
        })
        .collect();
    for t in tasks {
        assert_eq!(t.await.unwrap().unwrap(), 50);
    }
    // More threads, more slots; still correct.
    s.set_limits(Limits { memory_mb: 128, threads: 3 }).unwrap();
    assert_eq!(s.query_logs(&query(svc)).await.unwrap().len(), 50);
}
