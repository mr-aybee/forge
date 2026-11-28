//! Route throughput through the Nginx edges, from the last minute each edge
//! reports with its metrics.
//!
//! `GET /api/v1/traffic?cluster_id=` returns, in bytes per second:
//! * `total` — everything passing through the edges (clients in / out);
//! * `edges` — per edge node, with per-endpoint figures (`sites`);
//! * `nodes` — per backend node: what the edges send to it and receive back,
//!   matched from the upstream address Nginx connected to;
//! * `unmatched` — upstreams that are not a Forge node (external addresses).
//!
//! Only nodes the caller may view are included.

use std::collections::{BTreeMap, HashMap};
use std::net::IpAddr;
use std::time::Duration;

use axum::Json;
use axum::extract::{Query, State};
use forge_core::ports::ServerRepository;
use forge_core::{Action, ClusterId, EntityKind, EntityRef, ServerId};
use serde::{Deserialize, Serialize};

use crate::auth::Caller;
use crate::error::ApiResult;
use crate::state::AppState;

/// Reports older than this are ignored (the agent sends one every 10–30 s).
const FRESH_MS: i64 = 120_000;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct Q {
    #[schemars(with = "Option<String>")]
    pub cluster_id: Option<ClusterId>,
}

#[derive(Debug, Default, Serialize)]
pub struct Flow {
    pub requests: u64,
    pub in_bps: u64,
    pub out_bps: u64,
}

#[derive(Debug, Default, Serialize)]
pub struct EdgeFlow {
    #[serde(flatten)]
    pub flow: Flow,
    pub sites: BTreeMap<String, Flow>,
}

#[derive(Debug, Default, Serialize)]
pub struct NodeFlow {
    pub requests: u64,
    /// Edges → this node (requests).
    pub to_bps: u64,
    /// This node → edges (responses).
    pub from_bps: u64,
}

#[derive(Debug, Default, Serialize)]
pub struct Traffic {
    pub window_secs: u32,
    pub total: Flow,
    pub edges: BTreeMap<ServerId, EdgeFlow>,
    pub nodes: BTreeMap<ServerId, NodeFlow>,
    pub unmatched: BTreeMap<String, NodeFlow>,
}

fn ip_of(addr: &str) -> Option<IpAddr> {
    addr.parse::<std::net::SocketAddr>().map(|s| s.ip()).ok().or_else(|| addr.parse().ok())
}

pub async fn get(State(state): State<AppState>, caller: Caller, Query(q): Query<Q>) -> ApiResult<Json<Traffic>> {
    let servers = state.store.list_servers(None).await?;
    let in_scope: Vec<_> = servers.iter().filter(|s| q.cluster_id.is_none() || s.cluster_id == q.cluster_id).collect();
    let allowed = caller
        .can_many(Action::View, in_scope.iter().map(|s| EntityRef::new(EntityKind::Server, s.id)).collect())
        .await;
    let visible: Vec<_> = in_scope.into_iter().zip(allowed).filter_map(|(s, ok)| ok.then_some(s)).collect();

    // Every address a node may be reached at: its fabric / routing address,
    // its hostname, and what those resolve to.
    let r = super::routing::Resolver::new(&state).await?;
    let mut by_ip: HashMap<IpAddr, ServerId> = HashMap::new();
    let lookups = visible.iter().flat_map(|s| {
        let mut names = vec![s.hostname.clone()];
        names.extend(r.addresses_of(&s.id.to_string()));
        names.into_iter().map(move |n| (s.id, n))
    });
    let resolved = resolve_all(lookups.collect()).await;

    for (id, ips) in resolved {
        for ip in ips {
            by_ip.entry(ip).or_insert(id);
        }
    }

    let now = state.clock.now().0;
    let mut out = Traffic { window_secs: 60, ..Traffic::default() };
    let visible_ids: Vec<ServerId> = visible.iter().map(|s| s.id).collect();

    for (edge, sites) in state.telemetry.traffic(now - FRESH_MS) {
        if !visible_ids.contains(&edge) {
            continue;
        }

        let e = out.edges.entry(edge).or_default();

        for site in sites {
            e.flow.requests += site.requests;
            e.flow.in_bps += site.in_bytes_per_sec;
            e.flow.out_bps += site.out_bytes_per_sec;
            let f = e.sites.entry(site.site.clone()).or_default();
            f.requests += site.requests;
            f.in_bps += site.in_bytes_per_sec;
            f.out_bps += site.out_bytes_per_sec;

            for u in site.upstreams {
                let slot = match ip_of(&u.address).and_then(|ip| by_ip.get(&ip)) {
                    Some(id) => out.nodes.entry(*id).or_default(),
                    None => out.unmatched.entry(u.address.clone()).or_default(),
                };

                slot.requests += u.requests;
                slot.to_bps += u.sent_bytes_per_sec;
                slot.from_bps += u.received_bytes_per_sec;
            }
        }

        out.total.requests += e.flow.requests;
        out.total.in_bps += e.flow.in_bps;
        out.total.out_bps += e.flow.out_bps;
    }

    Ok(Json(out))
}

/// Resolves `(server, name)` pairs concurrently (2 s each); IP literals are
/// taken as they are.
async fn resolve_all(names: Vec<(ServerId, String)>) -> Vec<(ServerId, Vec<IpAddr>)> {
    let mut set = tokio::task::JoinSet::new();

    for (id, name) in names {
        set.spawn(async move {
            if let Ok(ip) = name.parse::<IpAddr>() {
                return (id, vec![ip]);
            }

            let ips =
                match tokio::time::timeout(Duration::from_secs(2), tokio::net::lookup_host((name.as_str(), 0))).await {
                    Ok(Ok(it)) => it.map(|a| a.ip()).collect(),
                    _ => Vec::new(),
                };

            (id, ips)
        });
    }

    let mut out = Vec::new();

    while let Some(r) = set.join_next().await {
        if let Ok(v) = r {
            out.push(v);
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::ip_of;

    #[test]
    fn upstream_addresses_parse() {
        assert_eq!(ip_of("10.64.0.16:8101").map(|i| i.to_string()).as_deref(), Some("10.64.0.16"));
        assert_eq!(ip_of("[fd00::1]:80").map(|i| i.to_string()).as_deref(), Some("fd00::1"));
        assert!(ip_of("node-1:8101").is_none());
    }
}
