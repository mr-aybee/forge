//! Times the store's reads on a stopped store directory (dev tool): CPU and
//! wall time per call of each read the console makes.
//!
//! Usage: duck_bench <store dir> [calls]

use std::sync::Arc;
use std::time::{Duration, Instant};

use forge_analytics_duckdb::{DuckDb, DuckDbConfig, Limits};
use forge_core::analytics::{AnalyticsStore, Resolution};
use forge_core::logs::{LogFilter, LogPage, LogQuery, SearchLimits};
use forge_core::{ServerId, ServiceId, SystemClock, Timestamp};

fn cpu_ms() -> f64 {
    let s = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
    let f: Vec<&str> = s.rsplit(')').next().unwrap_or("").split_whitespace().collect();
    let t = |i: usize| f.get(i).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
    (t(11) + t(12)) * 10.0
}

/// Services, nodes and the newest event time in the store.
type Ids = (Vec<ServiceId>, Vec<ServerId>, i64);

fn ids(dir: &str) -> Result<Ids, Box<dyn std::error::Error>> {
    let c = duckdb::Connection::open_in_memory()?;
    let mut parts = Vec::new();
    for (i, e) in std::fs::read_dir(format!("{dir}/logs"))?.enumerate() {
        let p = e?.path();
        if p.extension().is_some_and(|x| x == "duckdb") {
            c.execute_batch(&format!("ATTACH '{}' AS d{i} (READ_ONLY)", p.to_string_lossy().replace('\'', "''")))?;
            parts.push(format!("SELECT * FROM d{i}.app_logs"));
        }
    }
    let src = parts.join(" UNION ALL ");
    let mut st = c.prepare(&format!("SELECT DISTINCT service_id FROM ({src}) WHERE service_id IS NOT NULL"))?;
    let svc = st.query_map([], |r| r.get::<_, String>(0))?.filter_map(|r| r.ok()?.parse().ok()).collect();
    let mut st = c.prepare(&format!("SELECT DISTINCT server_id FROM ({src}) WHERE server_id IS NOT NULL"))?;
    let nodes = st.query_map([], |r| r.get::<_, String>(0))?.filter_map(|r| r.ok()?.parse().ok()).collect();
    let max: i64 = c.query_row(&format!("SELECT max(ts) FROM ({src})"), [], |r| r.get(0))?;
    Ok((svc, nodes, max))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let dir = args.next().ok_or("usage: duck_bench <store dir> [calls]")?;
    let calls: u32 = args.next().and_then(|v| v.parse().ok()).unwrap_or(50);
    let (services, nodes, max) = ids(&dir)?;
    let cfg = DuckDbConfig {
        dir: dir.clone().into(),
        limits: Limits {
            memory_mb: 256,
            threads: std::env::var("THREADS").ok().and_then(|v| v.parse().ok()).unwrap_or(2),
        },
        batch_rows: 20_000,
    };
    let s = tokio::task::spawn_blocking(move || DuckDb::open(&cfg, Arc::new(SystemClock))).await??;
    let node = *nodes.first().ok_or("no nodes")?;
    let q = LogQuery {
        services,
        instances: vec![],
        open_nodes: nodes.clone(),
        nodes: vec![],
        container: None,
        stack: None,
        source: None,
        levels: vec![],
        filter: LogFilter::parse("timeout"),
        from: Some(Timestamp(max - 900_000)),
        to: Some(Timestamp(max)),
        page: LogPage::Older(None),
        limit: 100,
        limits: SearchLimits::default(),
    };
    let (from, to) = (Timestamp(max - 3_600_000), Timestamp(max));
    for name in ["health", "logs_search", "logs_histogram", "node_history", "container_metrics"] {
        let (c0, t0) = (cpu_ms(), Instant::now());
        for _ in 0..calls {
            match name {
                "health" => s.health().await?,
                "logs_search" => drop(s.query_logs(&q).await?),
                "logs_histogram" => drop(s.log_histogram(&q, 30_000).await?),
                "node_history" => drop(s.node_series(node, from, to, Resolution::Minute).await?),
                _ => drop(s.container_series(node, None, (from, to), Resolution::Minute, 200).await?),
            }
        }
        let n = f64::from(calls);
        println!(
            "{name}: {:.1} ms CPU, {:.1} ms wall per call",
            (cpu_ms() - c0) / n,
            t0.elapsed().as_secs_f64() * 1000.0 / n
        );
    }
    // Raw statements on the same instance (SQL given after the call count).
    let conn = s.catalog().connect()?;
    for sql in args {
        let (c0, t0) = (cpu_ms(), Instant::now());
        for _ in 0..calls {
            let mut st = conn.prepare(&sql)?;
            let _ = st.query_map([], |_| Ok(()))?.count();
        }
        let n = f64::from(calls);
        println!("{:.1} ms CPU, {:.1} ms wall: {sql}", (cpu_ms() - c0) / n, t0.elapsed().as_secs_f64() * 1000.0 / n);
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    Ok(())
}
