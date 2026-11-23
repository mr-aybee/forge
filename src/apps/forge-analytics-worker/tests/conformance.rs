//! One conformance suite over `AnalyticsStore` (D-143): the memory store,
//! DuckDB, and ClickHouse when `FORGE_TEST_CLICKHOUSE_URL` names a server
//! (`http://user@host:8123/db`; a fresh database is created per run, the
//! password comes from `FORGE_TEST_CLICKHOUSE_PASSWORD`). Every store must
//! answer the same questions the same way.
#![allow(clippy::unwrap_used)] // reason: tests

use std::sync::Arc;
use std::time::Duration;

use forge_analytics_duckdb::{DuckDb, DuckDbConfig, Limits};
use forge_analytics_worker::memory::MemoryStore;
use forge_core::analytics::{
    AnalyticsStore, ContainerMinute, NodeMinute, NodeProcessMinute, Resolution, Retention, TopBy,
};
use forge_core::logs::{
    ContainerContext, IngestContext, LogEvent, LogFilter, LogLevel, LogPage, LogQuery, LogSource, LogStream,
    SearchLimits, container_event, parse_batch,
};
use forge_core::{ManualClock, ServerId, ServiceId, Timestamp};

/// 2026-11-22T10:00:00Z, on a whole hour.
const T0: i64 = 1_795_341_600_000;
const MIN: i64 = 60_000;
const RET: Retention =
    Retention { node_metrics_days: 7, top_processes_days: 7, node_rollup_days: 365, app_logs_days: 30 };

struct Fixture {
    svc: ServiceId,
    other: ServiceId,
    node: ServerId,
    node2: ServerId,
}

fn base(f: &Fixture) -> LogQuery {
    LogQuery {
        services: vec![f.svc],
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
        limit: 100,
        limits: SearchLimits::default(),
    }
}

fn app_logs(f: &Fixture) -> Vec<LogEvent> {
    let body = [
        r#"{"@t":"2026-11-22T10:00:01Z","@mt":"one","user":"ann","n":5}"#,
        r#"{"@t":"2026-11-22T10:00:02Z","@mt":"Two BOOM","@l":"Error","user":"bob"}"#,
        r#"{"@t":"2026-11-22T10:00:03Z","@mt":"three","@l":"Warning","n":"5"}"#,
        r#"{"@t":"2026-11-22T10:01:30Z","@mt":"four","@x":"Boom trace"}"#,
    ]
    .join("\n");
    let cx = |svc, node| IngestContext { service_id: svc, server_id: Some(node), node_ts: None, now: Timestamp(T0) };
    let mut out = parse_batch(&body, cx(f.svc, f.node)).unwrap();
    out.extend(parse_batch(r#"{"@t":"2026-11-22T10:00:04Z","@mt":"node two"}"#, cx(f.svc, f.node2)).unwrap());
    out.extend(parse_batch(r#"{"@t":"2026-11-22T10:00:05Z","@mt":"other app"}"#, cx(f.other, f.node)).unwrap());
    // An unattributed container line on node 2.
    out.push(container_event(
        &ContainerContext {
            service_id: None,
            server_id: f.node2,
            container_id: "abcdef012345".into(),
            container_name: "cache".into(),
            stack: Some("shop".into()),
            now: Timestamp(T0),
        },
        LogStream::Stderr,
        T0 + 6_000,
        "{\"level\":\"error\",\"msg\":\"evicted\"}",
    ));
    out
}

fn minute(node: ServerId, at: i64, cpu: f32) -> NodeMinute {
    NodeMinute {
        server_id: node,
        minute: Timestamp(at),
        cpu_avg: cpu,
        cpu_max: cpu + 1.0,
        mem_used: 1_000,
        mem_total: 4_000,
        swap_used: 0,
        disk_used: 50,
        disk_total: 100,
        rx: 10,
        tx: 20,
        load1: 0.25,
        samples: 6,
    }
}

fn process(node: ServerId, at: i64, name: &str, cpu: f32, mem: u64) -> NodeProcessMinute {
    NodeProcessMinute {
        server_id: node,
        minute: Timestamp(at),
        // One pid per name: ClickHouse keeps one row per (node, minute, pid).
        pid: u32::from(name.as_bytes()[0]),
        name: name.into(),
        user: "root".into(),
        cpu,
        mem,
        cmd: format!("/usr/bin/{name}"),
    }
}

fn container(node: ServerId, name: &str, at: i64, cpu: f32) -> ContainerMinute {
    ContainerMinute {
        server_id: node,
        minute: Timestamp(at),
        container_name: name.into(),
        container_id: format!("{name}-id"),
        image: "img:1".into(),
        service_id: None,
        stack: Some("shop".into()),
        cpu,
        mem_used: 64,
        mem_limit: 128,
        rx: 1,
        tx: 2,
        block_read: 3,
        block_write: 4,
        restarts: 1,
    }
}

fn messages(page: &[LogEvent]) -> Vec<&str> {
    page.iter().map(|e| e.message.as_str()).collect()
}

/// Polls a read until it sees `want` rows (asynchronous inserts settle).
async fn settled(store: &dyn AnalyticsStore, q: &LogQuery, want: usize) -> Vec<LogEvent> {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let page = store.query_logs(q).await.unwrap();
        if page.len() >= want || std::time::Instant::now() > deadline {
            return page;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn suite(store: Arc<dyn AnalyticsStore>) {
    let f = Fixture { svc: ServiceId::new(), other: ServiceId::new(), node: ServerId::new(), node2: ServerId::new() };
    store.migrate(RET).await.unwrap();
    store.write_logs(&app_logs(&f)).await.unwrap();

    // Scope: a whole service, newest first.
    let q = base(&f);
    let page = settled(store.as_ref(), &q, 5).await;
    assert_eq!(messages(&page), ["four", "node two", "three", "Two BOOM", "one"], "{}", store.engine());
    assert_eq!(page[1].server_id, Some(f.node2));
    assert_eq!(page[0].exception.as_deref(), Some("Boom trace"));
    // One node of a service, plus unattributed lines of a node.
    let q = LogQuery { services: vec![], instances: vec![(f.svc, f.node2)], open_nodes: vec![f.node2], ..base(&f) };
    let page = store.query_logs(&q).await.unwrap();
    assert_eq!(messages(&page), ["evicted", "node two"]);
    assert_eq!(page[0].container_name.as_deref(), Some("cache"));
    assert_eq!(page[0].level, LogLevel::Error);
    // Nothing in scope matches nothing.
    assert!(store.query_logs(&LogQuery { services: vec![], ..base(&f) }).await.unwrap().is_empty());
    // Filters.
    let q = LogQuery { levels: vec![LogLevel::Error, LogLevel::Warning], ..base(&f) };
    assert_eq!(messages(&store.query_logs(&q).await.unwrap()), ["three", "Two BOOM"]);
    let q = LogQuery { filter: LogFilter::parse("boom"), ..base(&f) };
    assert_eq!(messages(&store.query_logs(&q).await.unwrap()), ["four", "Two BOOM"], "message or exception, any case");
    let q = LogQuery { filter: LogFilter::parse("user=bob"), ..base(&f) };
    assert_eq!(messages(&store.query_logs(&q).await.unwrap()), ["Two BOOM"]);
    let q = LogQuery { filter: LogFilter::parse("n=5"), ..base(&f) };
    assert_eq!(messages(&store.query_logs(&q).await.unwrap()), ["three", "one"], "a number and its string");
    let q = LogQuery { nodes: vec![f.node2], ..base(&f) };
    assert_eq!(messages(&store.query_logs(&q).await.unwrap()), ["node two"]);
    let open = LogQuery { services: vec![], open_nodes: vec![f.node2], ..base(&f) };
    for q in [
        LogQuery { container: Some("cache".into()), ..open.clone() },
        LogQuery { container: Some("abcdef".into()), ..open.clone() },
        LogQuery { stack: Some("shop".into()), ..open.clone() },
        LogQuery { source: Some(LogSource::Container), ..open.clone() },
    ] {
        assert_eq!(messages(&store.query_logs(&q).await.unwrap()), ["evicted"], "{q:?}");
    }
    let q = LogQuery { from: Some(Timestamp(T0 + 2_000)), to: Some(Timestamp(T0 + 4_000)), ..base(&f) };
    assert_eq!(messages(&store.query_logs(&q).await.unwrap()), ["three", "Two BOOM"], "from inclusive, to exclusive");
    // Paging both ways.
    let first = store.query_logs(&LogQuery { limit: 2, ..base(&f) }).await.unwrap();
    assert_eq!(messages(&first), ["four", "node two"]);
    let older = LogQuery { limit: 2, page: LogPage::Older(Some(first[1].cursor())), ..base(&f) };
    let second = store.query_logs(&older).await.unwrap();
    assert_eq!(messages(&second), ["three", "Two BOOM"]);
    let newer = LogQuery { limit: 1, page: LogPage::Newer(second[0].cursor()), ..base(&f) };
    assert_eq!(messages(&store.query_logs(&newer).await.unwrap()), ["node two"], "the nearest newer event");
    // Histogram: per level and minute.
    let q = LogQuery { from: Some(Timestamp(T0)), to: Some(Timestamp(T0 + 2 * MIN)), ..base(&f) };
    let mut h = store.log_histogram(&q, MIN).await.unwrap();
    h.sort_by_key(|b| (b.t, b.level.as_str()));
    let got: Vec<(i64, &str, u64)> = h.iter().map(|b| (b.t - T0, b.level.as_str(), b.count)).collect();
    assert_eq!(got, [(0, "Error", 1), (0, "Information", 2), (0, "Warning", 1), (MIN, "Information", 1)]);

    // Node history, one row per minute.
    store.write_node_minutes(&[minute(f.node, T0, 10.0), minute(f.node, T0 + MIN, 30.0)]).await.unwrap();
    let pts = store.node_series(f.node, Timestamp(T0), Timestamp(T0 + 10 * MIN), Resolution::Minute).await.unwrap();
    assert_eq!(pts.iter().map(|p| p.t - T0).collect::<Vec<_>>(), [0, MIN]);
    assert!((pts[1].cpu_avg - 30.0).abs() < 0.01 && (pts[1].cpu_max - 31.0).abs() < 0.01);
    assert_eq!((pts[0].mem_used, pts[0].mem_total, pts[0].disk_used, pts[0].rx), (1_000, 4_000, 50, 10));
    let none = store.node_series(f.node2, Timestamp(T0), Timestamp(T0 + MIN), Resolution::Minute).await.unwrap();
    assert!(none.is_empty());

    // Top processes over a window.
    store
        .write_top_processes(&[
            process(f.node, T0, "db", 50.0, 900),
            process(f.node, T0 + MIN, "db", 50.0, 900),
            process(f.node, T0, "web", 70.0, 100),
        ])
        .await
        .unwrap();
    let window = (Timestamp(T0), Timestamp(T0 + 10 * MIN));
    let cpu = store.top_processes(f.node, window.0, window.1, TopBy::Cpu, 10).await.unwrap();
    assert_eq!(cpu.iter().map(|p| (p.name.as_str(), p.minutes)).collect::<Vec<_>>(), [("web", 1), ("db", 2)]);
    assert_eq!(cpu[1].cmd, "/usr/bin/db");
    let mem = store.top_processes(f.node, window.0, window.1, TopBy::Mem, 1).await.unwrap();
    assert_eq!((mem.len(), mem[0].name.as_str(), mem[0].mem_avg), (1, "db", 900));

    // Containers: by name, limited, identity of the latest minute.
    store
        .write_container_minutes(&[
            container(f.node, "web", T0, 5.0),
            container(f.node, "web", T0 + MIN, 15.0),
            container(f.node, "api", T0, 1.0),
        ])
        .await
        .unwrap();
    let all = store.container_series(f.node, None, window, Resolution::Minute, 10).await.unwrap();
    assert_eq!(
        all.iter().map(|s| (s.container_name.as_str(), s.points.len())).collect::<Vec<_>>(),
        [("api", 1), ("web", 2)]
    );
    assert_eq!((all[1].image.as_str(), all[1].stack.as_deref()), ("img:1", Some("shop")));
    assert!((all[1].points[1].cpu_avg - 15.0).abs() < 0.01);
    let one = store.container_series(f.node, None, window, Resolution::Minute, 1).await.unwrap();
    assert_eq!(one.len(), 1);
    assert_eq!(one[0].container_name, "api");
    let named = store.container_series(f.node, Some("web"), window, Resolution::Minute, 10).await.unwrap();
    assert_eq!(named.len(), 1);
    assert_eq!(named[0].points.len(), 2);

    assert_eq!(store.effective_retention().await.unwrap(), RET);
}

#[tokio::test(flavor = "multi_thread")]
async fn memory_store_conforms() {
    suite(Arc::new(MemoryStore::default())).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn duckdb_store_conforms() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = DuckDbConfig {
        dir: dir.path().to_path_buf(),
        limits: Limits { memory_mb: 128, threads: 1 },
        batch_rows: 10_000,
    };
    let clock = Arc::new(ManualClock::new(Timestamp(T0)));
    suite(DuckDb::open(&cfg, clock).unwrap()).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn clickhouse_store_conforms() {
    let Ok(url) = std::env::var("FORGE_TEST_CLICKHOUSE_URL") else {
        eprintln!("FORGE_TEST_CLICKHOUSE_URL is not set; skipping the ClickHouse run");
        return;
    };
    let url = format!("{}_{}", url.trim_end_matches('/'), uuid::Uuid::new_v4().simple());
    let password = std::env::var("FORGE_TEST_CLICKHOUSE_PASSWORD").ok();
    let target = forge_analytics::ClickHouseTarget::from_url(&url, password.as_deref(), "clickhouse").unwrap();
    suite(Arc::new(forge_analytics::ClickHouse::new(target).unwrap())).await;
}
