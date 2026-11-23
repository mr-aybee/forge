//! Runs against a real ClickHouse when `FORGE_TEST_CLICKHOUSE_URL` is set,
//! e.g. `http://default@127.0.0.1:8123/forge_test_analytics` (password in
//! `FORGE_TEST_CLICKHOUSE_PASSWORD`). Skipped otherwise.
#![allow(clippy::unwrap_used)] // reason: tests

use forge_analytics::{ClickHouse, ClickHouseTarget};
use forge_core::analytics::{AnalyticsStore, NodeMinute, NodeProcessMinute, Resolution, Retention, TopBy};
use forge_core::{ServerId, Timestamp};

#[tokio::test]
async fn writes_and_reads_node_history() {
    let Ok(url) = std::env::var("FORGE_TEST_CLICKHOUSE_URL") else {
        eprintln!("FORGE_TEST_CLICKHOUSE_URL not set; skipped");
        return;
    };
    let pw = std::env::var("FORGE_TEST_CLICKHOUSE_PASSWORD").ok();
    let ch = ClickHouse::new(ClickHouseTarget::from_url(&url, pw.as_deref(), "clickhouse").unwrap()).unwrap();
    let r = Retention { node_metrics_days: 7, top_processes_days: 7, node_rollup_days: 365, app_logs_days: 30 };
    ch.migrate(r).await.unwrap();
    ch.migrate(r).await.unwrap(); // idempotent
    assert_eq!(ch.effective_retention().await.unwrap(), r);
    let shorter = Retention { top_processes_days: 2, app_logs_days: 9, ..r };
    ch.apply_retention(shorter).await.unwrap();
    assert_eq!(ch.effective_retention().await.unwrap(), shorter, "TTLs change at runtime");
    ch.apply_retention(r).await.unwrap();
    let stats = ch.engine_stats().await.unwrap();
    assert!(stats.free_disk_bytes.is_some_and(|b| b > 0));
    ch.health().await.unwrap();

    let s = ServerId::new();
    let now =
        i64::try_from(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis()).unwrap();
    let base = (now / 3_600_000) * 3_600_000 - 3_600_000;
    let rows: Vec<NodeMinute> = (0..3)
        .map(|i| NodeMinute {
            server_id: s,
            minute: Timestamp(base + i * 60_000),
            cpu_avg: 10.0 * (i + 1) as f32,
            cpu_max: 20.0 * (i + 1) as f32,
            mem_used: 100,
            mem_total: 1000,
            swap_used: 0,
            disk_used: 50,
            disk_total: 100,
            rx: 7,
            tx: 9,
            load1: 0.5,
            samples: 2,
        })
        .collect();
    ch.write_node_minutes(&rows).await.unwrap();
    let procs = vec![
        NodeProcessMinute {
            server_id: s,
            minute: Timestamp(base),
            pid: 1,
            name: "a".into(),
            user: "u".into(),
            cpu: 50.0,
            mem: 10,
            cmd: "a --x".into(),
        },
        NodeProcessMinute {
            server_id: s,
            minute: Timestamp(base),
            pid: 2,
            name: "b".into(),
            user: "u".into(),
            cpu: 5.0,
            mem: 900,
            cmd: "b".into(),
        },
    ];
    ch.write_top_processes(&procs).await.unwrap();

    let to = Timestamp(base + 3_600_000);
    let m = ch.node_series(s, Timestamp(base), to, Resolution::Minute).await.unwrap();
    assert_eq!(m.len(), 3);
    assert!((m[2].cpu_max - 60.0).abs() < 0.01);
    let h = ch.node_series(s, Timestamp(base), to, Resolution::Hour).await.unwrap();
    assert_eq!(h.len(), 1);
    assert!((h[0].cpu_avg - 20.0).abs() < 0.01);
    let cpu = ch.top_processes(s, Timestamp(base), to, TopBy::Cpu, 10).await.unwrap();
    assert_eq!(cpu[0].name, "a");
    let mem = ch.top_processes(s, Timestamp(base), to, TopBy::Mem, 10).await.unwrap();
    assert_eq!(mem[0].name, "b");
    // Another node's data is never returned.
    assert!(ch.node_series(ServerId::new(), Timestamp(base), to, Resolution::Minute).await.unwrap().is_empty());
}

#[tokio::test]
async fn writes_pages_and_filters_logs() {
    use forge_core::ServiceId;
    use forge_core::logs::{IngestContext, LogFilter, LogLevel, LogPage, LogQuery, parse_batch};
    let Ok(url) = std::env::var("FORGE_TEST_CLICKHOUSE_URL") else {
        eprintln!("FORGE_TEST_CLICKHOUSE_URL not set; skipped");
        return;
    };
    let pw = std::env::var("FORGE_TEST_CLICKHOUSE_PASSWORD").ok();
    let ch = ClickHouse::new(ClickHouseTarget::from_url(&url, pw.as_deref(), "clickhouse").unwrap()).unwrap();
    ch.migrate(Retention { node_metrics_days: 7, top_processes_days: 7, node_rollup_days: 365, app_logs_days: 30 })
        .await
        .unwrap();

    let app = ServiceId::new();
    let now =
        i64::try_from(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis()).unwrap();
    let cx = IngestContext { service_id: app, server_id: None, node_ts: None, now: Timestamp(now) };
    let body: String = (0..25)
        .map(|i| {
            let level = if i % 5 == 0 { "Error" } else { "Information" };
            format!(
                "{{\"@t\":\"2026-01-01T00:00:{i:02}.5Z\",\"@mt\":\"Request {{Path}} took {{Ms}} ms\",\"@l\":\"{level}\",\"Path\":\"/p{}\",\"Ms\":{i}}}\n",
                i % 3
            )
        })
        .collect();
    let events = parse_batch(&body, cx).unwrap();
    ch.write_logs(&events).await.unwrap();

    let q = |page, levels: Vec<LogLevel>, filter: &str, limit| LogQuery {
        services: vec![app],
        instances: Vec::new(),
        open_nodes: Vec::new(),
        nodes: Vec::new(),
        container: None,
        stack: None,
        source: None,
        levels,
        filter: LogFilter::parse(filter),
        from: None,
        to: None,
        page,
        limit,
        limits: forge_core::logs::SearchLimits::default(),
    };
    let first = ch.query_logs(&q(LogPage::Older(None), vec![], "", 10)).await.unwrap();
    assert_eq!(first.len(), 10);
    assert_eq!(first[0].properties["Ms"], 24); // newest first
    assert_eq!(first[0].message, "Request /p0 took 24 ms");
    let next = ch.query_logs(&q(LogPage::Older(Some(first[9].cursor())), vec![], "", 100)).await.unwrap();
    assert_eq!(next.len(), 15);
    assert!(next[0].ts < first[9].ts);
    let newer = ch.query_logs(&q(LogPage::Newer(next[0].cursor()), vec![], "", 100)).await.unwrap();
    assert_eq!(newer.len(), 10);
    assert_eq!(newer[0].id, first[0].id);

    let errors = ch.query_logs(&q(LogPage::Older(None), vec![LogLevel::Error], "", 100)).await.unwrap();
    assert_eq!(errors.len(), 5);
    let text = ch.query_logs(&q(LogPage::Older(None), vec![], "TOOK 7 MS", 100)).await.unwrap();
    assert_eq!(text.len(), 1);
    let prop = ch.query_logs(&q(LogPage::Older(None), vec![], "Path=\"/p1\"", 100)).await.unwrap();
    assert_eq!(prop.len(), 8);
    let num = ch.query_logs(&q(LogPage::Older(None), vec![], "Ms=3", 100)).await.unwrap();
    assert_eq!(num.len(), 1);

    let mut hq = q(LogPage::Older(None), vec![], "", 1);
    hq.from = Some(Timestamp(1_767_225_600_000));
    hq.to = Some(Timestamp(1_767_225_600_000 + 60_000));
    let h = ch.log_histogram(&hq, 10_000).await.unwrap();
    let total: u64 = h.iter().map(|b| b.count).sum();
    assert_eq!(total, 25);
    assert!(h.iter().any(|b| b.level == LogLevel::Error));
    // Another service's events are never returned.
    let mut other = q(LogPage::Older(None), vec![], "", 100);
    other.services = vec![ServiceId::new()];
    assert!(ch.query_logs(&other).await.unwrap().is_empty());

    // Container lines: one attributed to the service, one to no service.
    use forge_core::logs::{ContainerContext, LogSource, LogStream, container_event};
    let node = forge_core::ServerId::new();
    let cx = |service_id| ContainerContext {
        service_id,
        server_id: node,
        container_id: "abcdef012345".into(),
        container_name: "web-1".into(),
        stack: Some("prod/shop".into()),
        now: Timestamp(now),
    };
    let attributed = container_event(&cx(Some(app)), LogStream::Stdout, now - 5, "hello from the app");
    let loose = container_event(&cx(None), LogStream::Stderr, now - 4, "{\"level\":\"error\",\"msg\":\"boom\"}");
    ch.write_logs(&[attributed.clone(), loose.clone()]).await.unwrap();
    let mut cq = q(LogPage::Older(None), vec![], "", 100);
    cq.source = Some(LogSource::Container);
    let got = ch.query_logs(&cq).await.unwrap();
    assert_eq!(got.len(), 1, "the service sees only its own container line");
    assert_eq!(got[0].container_name.as_deref(), Some("web-1"));
    assert_eq!(got[0].stream, Some(LogStream::Stdout));
    cq.open_nodes = vec![node];
    let got = ch.query_logs(&cq).await.unwrap();
    assert_eq!(got.len(), 2, "node view adds the unattributed line");
    assert_eq!(got[0].service_id, None);
    assert_eq!(got[0].level, LogLevel::Error);
    cq.services = Vec::new();
    cq.stack = Some("prod/shop".into());
    cq.container = Some("abcdef".into());
    assert_eq!(ch.query_logs(&cq).await.unwrap().len(), 1, "unattributed only, by stack and id prefix");
    cq.nodes = vec![forge_core::ServerId::new()];
    assert!(ch.query_logs(&cq).await.unwrap().is_empty(), "node filter");
}

#[tokio::test]
async fn writes_and_reads_container_metrics() {
    use forge_core::analytics::ContainerMinute;
    let Ok(url) = std::env::var("FORGE_TEST_CLICKHOUSE_URL") else {
        eprintln!("FORGE_TEST_CLICKHOUSE_URL not set; skipped");
        return;
    };
    let pw = std::env::var("FORGE_TEST_CLICKHOUSE_PASSWORD").ok();
    let ch = ClickHouse::new(ClickHouseTarget::from_url(&url, pw.as_deref(), "clickhouse").unwrap()).unwrap();
    ch.migrate(Retention { node_metrics_days: 7, top_processes_days: 7, node_rollup_days: 365, app_logs_days: 30 })
        .await
        .unwrap();
    let s = ServerId::new();
    let now =
        i64::try_from(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis()).unwrap();
    let base = (now / 3_600_000) * 3_600_000 - 3_600_000;
    let row = |name: &str, i: i64| ContainerMinute {
        server_id: s,
        minute: Timestamp(base + i * 60_000),
        container_name: name.into(),
        container_id: format!("id{i}"),
        image: "nginx:1".into(),
        service_id: None,
        stack: Some("prod/shop".into()),
        cpu: 10.0 * (i + 1) as f32,
        mem_used: 100,
        mem_limit: 1000,
        rx: 1,
        tx: 2,
        block_read: 3,
        block_write: 4,
        restarts: u32::try_from(i).unwrap(),
    };
    let rows: Vec<ContainerMinute> = (0..3).map(|i| row("web", i)).chain((0..2).map(|i| row("db", i))).collect();
    ch.write_container_minutes(&rows).await.unwrap();
    let w = (Timestamp(base), Timestamp(base + 3_600_000));
    let all = ch.container_series(s, None, w, Resolution::Minute, 10).await.unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].container_name, "db");
    assert_eq!(all[1].points.len(), 3);
    assert_eq!(all[1].container_id, "id2", "latest identity");
    assert_eq!(all[1].stack.as_deref(), Some("prod/shop"));
    let one = ch.container_series(s, Some("web"), w, Resolution::Hour, 10).await.unwrap();
    assert_eq!(one.len(), 1);
    assert_eq!(one[0].points.len(), 1);
    assert!((one[0].points[0].cpu_max - 30.0).abs() < 0.01);
    assert_eq!(one[0].points[0].restarts, 2);
    let limited = ch.container_series(s, None, w, Resolution::Minute, 1).await.unwrap();
    assert_eq!(limited.len(), 1, "bounded by limit");
}

/// Runs `sql` against the test database; its output.
async fn run_sql(url: &str, pw: Option<&str>, sql: &str) -> String {
    let t = ClickHouseTarget::from_url(url, pw, "clickhouse").unwrap();
    reqwest::Client::new()
        .post(format!("{}/?database={}", t.endpoint, t.database))
        .header("X-ClickHouse-User", &t.user)
        .header("X-ClickHouse-Key", &t.password)
        .body(sql.to_owned())
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap()
}

/// `sql` against the test database, as one number.
async fn count(url: &str, pw: Option<&str>, sql: &str) -> u64 {
    let text = run_sql(url, pw, sql).await;
    text.trim().parse().unwrap_or_else(|_| panic!("{sql}: {text}"))
}

/// A spool file sent again is stored once per table (D-145): after a
/// partial failure the writer only writes the missing tables, and after a
/// worker restart the engine drops the insert by its deduplication token,
/// rollups included.
#[tokio::test]
async fn spool_files_are_stored_once_per_table() {
    use forge_analytics::ClickHouseSpoolWriter;
    use forge_core::analytics::{FileWrite, SpoolFileWriter, SpoolRows};
    use forge_core::logs::{IngestContext, parse_batch};
    let Ok(base) = std::env::var("FORGE_TEST_CLICKHOUSE_URL") else {
        eprintln!("FORGE_TEST_CLICKHOUSE_URL not set; skipped");
        return;
    };
    let url = format!("{}_{}", base.trim_end_matches('/'), uuid::Uuid::new_v4().simple());
    let pw = std::env::var("FORGE_TEST_CLICKHOUSE_PASSWORD").ok();
    let ch = ClickHouse::new(ClickHouseTarget::from_url(&url, pw.as_deref(), "clickhouse").unwrap()).unwrap();
    ch.migrate(Retention { node_metrics_days: 7, top_processes_days: 7, node_rollup_days: 365, app_logs_days: 30 })
        .await
        .unwrap();
    // No merges: the replacing and aggregating tables would fold duplicates
    // away and hide them from the counts.
    for t in ["app_logs", "node_metrics_1m", "node_metrics_1h"] {
        run_sql(&url, pw.as_deref(), &format!("SYSTEM STOP MERGES {t}")).await;
    }
    let s = ServerId::new();
    let now =
        i64::try_from(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis()).unwrap();
    let hour = (now / 3_600_000) * 3_600_000;
    let cx = IngestContext {
        service_id: forge_core::ServiceId::new(),
        server_id: Some(s),
        node_ts: None,
        now: Timestamp(now),
    };
    let logs = parse_batch("{\"@mt\":\"one\"}\n{\"@mt\":\"two\"}\n{\"@mt\":\"three\"}", cx).unwrap();
    let minutes: Vec<NodeMinute> = (0..2)
        .map(|i| NodeMinute {
            server_id: s,
            minute: Timestamp(hour + i * 60_000),
            cpu_avg: 10.0,
            cpu_max: 20.0,
            mem_used: 100,
            mem_total: 200,
            swap_used: 0,
            disk_used: 1,
            disk_total: 2,
            rx: 1,
            tx: 1,
            load1: 0.5,
            samples: 6,
        })
        .collect();
    let file = |id: &str| SpoolRows {
        node: s,
        file_id: id.into(),
        logs: logs.clone(),
        minutes: minutes.clone(),
        processes: vec![],
        containers: vec![],
    };
    let id = format!("{:032x}", now);
    let counts = || async {
        (
            count(&url, pw.as_deref(), "SELECT count() FROM app_logs").await,
            count(&url, pw.as_deref(), "SELECT count() FROM node_metrics_1m").await,
            count(&url, pw.as_deref(), "SELECT count() FROM node_metrics_1h").await,
        )
    };
    let w = ClickHouseSpoolWriter::start(ch.clone(), 20_000);
    assert_eq!(w.write_spool_file(file(&id)).await.unwrap(), FileWrite::Stored);
    let first = counts().await;
    assert_eq!(first, (3, 2, 1));
    // The worker restarted: a new writer remembers nothing, the token holds.
    let w = ClickHouseSpoolWriter::start(ch.clone(), 20_000);
    assert_eq!(w.write_spool_file(file(&id)).await.unwrap(), FileWrite::Stored);
    assert_eq!(counts().await, first, "the same file again: no rows added, rollups included");
    // Another file id is new data.
    assert_eq!(w.write_spool_file(file(&format!("{:032x}", now + 1))).await.unwrap(), FileWrite::Stored);
    assert_eq!(counts().await, (6, 4, 2));
    // Rows without a file id (the worker's own buffers) are written as before.
    let more = parse_batch("{\"@mt\":\"four\"}", cx).unwrap();
    ch.write_logs(&more).await.unwrap();
    assert_eq!(counts().await, (7, 4, 2));
    // Migrations stay idempotent with the deduplication window applied.
    ch.migrate(Retention { node_metrics_days: 7, top_processes_days: 7, node_rollup_days: 365, app_logs_days: 30 })
        .await
        .unwrap();
}
