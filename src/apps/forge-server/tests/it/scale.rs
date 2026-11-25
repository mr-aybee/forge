//! Scale test: a database shaped like a large installation, then the API
//! endpoints the console calls, timed through HTTP.
//!
//! Ignored by default (seeding takes a while). Run with optimizations:
//!
//! ```sh
//! cargo test --profile perf -p forge-server --test it scale:: -- --ignored --nocapture
//! ```
//!
//! Sizes come from `FORGE_SCALE_NODES` (1000) and `FORGE_SCALE_APPS` (15000).
#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use forge_core::model::{AgentStatus, Cluster, Environment, Installation, NodeRole, Server, Service, ServiceInstance};
use forge_core::ports::{
    ClusterRepository, InstallationRepository, ServerRepository, ServiceRepository, TenancyRepository,
};
use forge_core::*;
use serde_json::json;

use crate::common::*;

fn env_usize(k: &str, d: usize) -> usize {
    std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d)
}

async fn time(admin: &Client, path: &str, runs: usize) -> (Duration, Duration, usize) {
    let mut v = Vec::with_capacity(runs);
    let mut size = 0;

    for _ in 0..runs {
        let t = Instant::now();
        let (s, body) = admin.get(path).await;
        v.push(t.elapsed());
        assert!(s.is_success(), "{path}: {s} {body}");
        size = body.as_array().map_or(1, Vec::len);
    }

    v.sort();

    (v[v.len() / 2], v[(v.len() * 99 / 100).min(v.len() - 1)], size)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "scale test: run explicitly with --ignored"]
async fn api_stays_fast_with_1000_nodes_and_15000_apps() {
    let nodes = env_usize("FORGE_SCALE_NODES", 1000);
    let apps = env_usize("FORGE_SCALE_APPS", 15000);
    let clusters = 10.max(nodes / 100);
    let ns_per_cluster = 30;

    let h = start().await;
    let s = &h.app.state.store;
    let ws: WorkspaceId = h.default_org.parse().unwrap();
    let now = Timestamp(1);
    let t0 = Instant::now();

    let mut cluster_ids = Vec::new();
    let mut environments = Vec::new();

    for c in 0..clusters {
        let cl = Cluster {
            id: ClusterId::new(),
            workspace_id: ws,
            name: format!("cluster-{c}"),
            description: String::new(),
            environment_id: None,
            abbr: String::new(),
            icon: None,
            mode: Default::default(),
            created_at: now,
        };
        s.create_cluster(&cl).await.unwrap();

        for n in 0..ns_per_cluster {
            let ns = Environment {
                id: EnvironmentId::new(),
                workspace_id: ws,
                cluster_id: Some(cl.id),
                project_id: None,
                name: format!("ns-{c}-{n}"),
                protected: false,
                created_at: now,
            };
            s.create_environment(&ns).await.unwrap();
            environments.push(ns.id);
        }

        cluster_ids.push(cl.id);
    }

    let mut server_ids = Vec::new();

    for i in 0..nodes {
        let cl = cluster_ids[i % cluster_ids.len()];
        let sv = Server {
            id: ServerId::new(),
            workspace_id: ws,
            environment_id: None,
            cluster_id: Some(cl),
            role: NodeRole::Worker,
            display_name: None,
            hostname: format!("node-{i:05}"),
            os: "linux".into(),
            arch: "x86_64".into(),
            agent_version: "0".into(),
            status: AgentStatus::Online,
            last_seen_at: Some(now),
            cert_serial: None,
            cert_not_after: None,
            created_at: now,
        };
        s.create_server(&sv).await.unwrap();
        s.assign_server(sv.id, Some(cl), None).await.unwrap();

        for pkg in ["nginx", "redis", "python"] {
            s.put_installation(&Installation {
                id: InstallationId::new(),
                workspace_id: ws,
                server_id: sv.id,
                package: pkg.into(),
                version: "distro".into(),
                params: BTreeMap::new(),
                status: "installed".into(),
                state: Some("running".into()),
                env: BTreeMap::new(),
                last_job_id: None,
                last_error: None,
                versions: None,
                update_policy: "notify".into(),
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();
        }

        server_ids.push(sv.id);
    }

    for i in 0..apps {
        let ns = environments[i % environments.len()];
        let app = Service {
            id: ServiceId::new(),
            workspace_id: ws,
            environment_id: ns,
            name: format!("app-{i:05}"),
            runtime: "service".into(),
            spec: json!({ "environment": "ns", "command": "python3 -m http.server", "working_dir": "", "image": "",
                          "ports": [], "volumes": [], "env_files": [], "restart": true }),
            env: vec![],
            created_by: Principal::User(UserId::new()),
            created_at: now,
            updated_at: now,
        };
        s.put_service(&app).await.unwrap();

        for k in 0..2 {
            s.put_instance(&ServiceInstance {
                service_id: app.id,
                server_id: server_ids[(i * 2 + k) % server_ids.len()],
                role: if k == 0 { "active".into() } else { "standby".into() },
                status: "deployed".into(),
                state: Some("running".into()),
                last_job_id: None,
                last_error: None,
                updated_at: now,
            })
            .await
            .unwrap();
        }
    }

    println!(
        "seeded {nodes} nodes, {clusters} clusters, {} environments, {apps} apps, {} placements, {} installs in {:.1}s",
        environments.len(),
        apps * 2,
        nodes * 3,
        t0.elapsed().as_secs_f64()
    );

    // Where the time goes, below HTTP and JSON.
    let t = Instant::now();
    let all = s.list_servers(None).await.unwrap();
    println!("store: list_servers ({}) {:.1} ms", all.len(), t.elapsed().as_secs_f64() * 1e3);
    let t = Instant::now();
    let all_apps = s.list_services(None).await.unwrap();
    println!("store: list_services ({}) {:.1} ms", all_apps.len(), t.elapsed().as_secs_f64() * 1e3);
    let t = Instant::now();
    let inst = s.list_all_instances(None).await.unwrap();
    println!("store: list_all_instances ({}) {:.1} ms", inst.len(), t.elapsed().as_secs_f64() * 1e3);
    let t = Instant::now();
    let refs: Vec<EntityRef> = all.iter().map(|x| EntityRef::new(EntityKind::Server, x.id)).collect();
    let _ = forge_core::ports::ResourceResolver::resolve_many(s, &refs).await;
    println!("store: resolve_many ({}) {:.1} ms", refs.len(), t.elapsed().as_secs_f64() * 1e3);
    let t = Instant::now();
    let body = serde_json::to_vec(&all_apps).unwrap();
    println!(
        "json: {} apps -> {:.1} MB in {:.1} ms",
        all_apps.len(),
        body.len() as f64 / 1e6,
        t.elapsed().as_secs_f64() * 1e3
    );

    // Every lookup the API makes per request must use an index (no full scans).
    let plans = [
        "SELECT * FROM servers WHERE id = 'x'",
        "SELECT * FROM servers WHERE workspace_id = 'x' AND hostname = 'y'",
        "UPDATE servers SET status = 'offline' WHERE status = 'online' AND last_seen_at < 5",
        "SELECT * FROM services WHERE environment_id = 'x' ORDER BY name",
        "SELECT i.* FROM service_instances i JOIN services a ON a.id = i.service_id WHERE a.environment_id = 'x'",
        "SELECT a.* FROM services a JOIN environments n ON n.id = a.environment_id WHERE n.cluster_id = 'x'",
        "SELECT * FROM service_instances WHERE service_id = 'x'",
        "SELECT * FROM installations WHERE server_id = 'x'",
        "SELECT * FROM installations WHERE server_id = 'x' AND package = 'p' AND version = 'v'",
        "SELECT * FROM environments WHERE workspace_id = 'x' AND cluster_id = 'y' ORDER BY name",
        "SELECT * FROM role_bindings WHERE user_id = 'x'",
        "SELECT * FROM sessions WHERE id_hash = 'x'",
        "SELECT * FROM api_keys WHERE secret_hash = 'x'",
        "SELECT * FROM jobs WHERE kind = 'k' AND idempotency_key = 'i'",
        "SELECT * FROM audit_log WHERE workspace_id = 'x' ORDER BY seq DESC LIMIT 100",
        "SELECT * FROM clusters WHERE workspace_id = 'x' ORDER BY name",
    ];
    let mut scans = Vec::new();

    for q in plans {
        let rows = sqlx::query(&format!("EXPLAIN QUERY PLAN {q}")).fetch_all(s.pool()).await.unwrap();
        let plan: Vec<String> = rows.iter().map(|r| sqlx::Row::get::<String, _>(r, "detail")).collect();
        let full = plan.iter().any(|d| d.starts_with("SCAN ") && !d.contains("USING") && !d.contains("COVERING"));
        println!(
            "{} {q}
      {}",
            if full { "SCAN " } else { "index" },
            plan.join(" | ")
        );

        if full {
            scans.push(q);
        }
    }

    assert!(scans.is_empty(), "full table scans: {scans:?}");

    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let cl0 = cluster_ids[0];
    let ns0 = environments[0];
    let checks: Vec<(String, u64)> = vec![
        ("/api/v1/servers".into(), 500),
        ("/api/v1/clusters".into(), 300),
        ("/api/v1/services".into(), 1500),
        (format!("/api/v1/services?environment_id={ns0}"), 200),
        (format!("/api/v1/services?cluster_id={cl0}"), 300),
        ("/api/v1/installations".into(), 500),
        (format!("/api/v1/environments?cluster_id={cl0}"), 200),
        (format!("/api/v1/clusters/{cl0}/fabric"), 500),
        ("/api/v1/jobs".into(), 300),
        ("/api/v1/audit?limit=200".into(), 300),
    ];
    println!("{:<60} {:>9} {:>9} {:>7}  budget", "endpoint", "p50 ms", "p99 ms", "items");
    let mut over = Vec::new();

    for (path, budget_ms) in &checks {
        let (p50, p99, n) = time(&admin, path, 10).await;
        println!(
            "{:<60} {:>9.1} {:>9.1} {:>7}  {budget_ms} ms",
            path,
            p50.as_secs_f64() * 1e3,
            p99.as_secs_f64() * 1e3,
            n
        );

        if p50 > Duration::from_millis(*budget_ms) {
            over.push(format!("{path}: p50 {:.0} ms > {budget_ms} ms", p50.as_secs_f64() * 1e3));
        }
    }

    assert!(over.is_empty(), "over budget:\n{}", over.join("\n"));
}
