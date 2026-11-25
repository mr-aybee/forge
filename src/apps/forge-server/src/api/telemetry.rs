//! Node telemetry: live resource metrics and inter-node connectivity.
//!
//! - `GET  /api/v1/servers/{id}/metrics?since=<ms>`: samples newer than
//!   `since` (in-memory history). Each call marks the node as watched, which
//!   makes its agent report every couple of seconds.
//! - `GET  /api/v1/servers/{id}/connectivity`: the node's peers and the last
//!   probe runs.
//! - `POST /api/v1/servers/{id}/connectivity`: probes every peer *from this
//!   node* (an `agent.command` job, authorized and audited by the job engine).

use std::time::Duration;

use axum::Json;
use axum::extract::{Path, Query, State};
use forge_core::model::{AgentStatus, NodeRole, Server};
use forge_core::ports::{ServerRepository, SshEndpointRepository};
use forge_core::{Action, EntityKind, EntityRef, ServerId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::metrics::{ProbeRun, Sample};
use crate::state::AppState;

fn server_ref(id: ServerId) -> EntityRef {
    EntityRef::new(EntityKind::Server, id)
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct MetricsQuery {
    #[serde(default)]
    pub since: i64,
}

#[derive(Debug, Serialize)]
pub struct MetricsResponse {
    pub server_id: ServerId,
    pub online: bool,
    /// Expected seconds between samples while watched.
    pub live_interval_secs: u64,
    pub samples: Vec<Sample>,
    /// Mounted volumes from the latest sample.
    pub volumes: Vec<crate::metrics::Volume>,
}

pub async fn metrics(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServerId>,
    Query(q): Query<MetricsQuery>,
) -> ApiResult<Json<MetricsResponse>> {
    caller.authorize(Action::View, server_ref(id)).await?;
    let node = state.store.get_server(id).await.map_err(|_| ApiError::not_found("server not found"))?;
    state.telemetry.watch(id);

    // Wake a parked long-poll so the faster pace starts immediately.
    state.hub.notifier(id).notify_one();

    Ok(Json(MetricsResponse {
        server_id: id,
        online: node.status == AgentStatus::Online,
        live_interval_secs: crate::metrics::LIVE_HOLD.as_secs(),
        samples: state.telemetry.since(id, q.since),
        volumes: state.telemetry.volumes(id),
    }))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct HistoryQuery {
    /// Window start, epoch ms (default: 24 hours before `to`).
    pub from: Option<i64>,
    /// Window end, epoch ms (default: now).
    pub to: Option<i64>,
}

fn window(state: &AppState, q: &HistoryQuery) -> (i64, i64) {
    let to = q.to.unwrap_or_else(|| state.clock.now().0);
    (q.from.unwrap_or(to - 24 * 3_600_000), to)
}

/// `GET /api/v1/servers/{id}/history?from=&to=`: per-minute (up to two days)
/// or hourly points from the analytics database.
pub async fn history(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServerId>,
    Query(q): Query<HistoryQuery>,
) -> ApiResult<Json<forge_core::analytics::NodeHistory>> {
    caller.authorize(Action::View, server_ref(id)).await?;
    let (from, to) = window(&state, &q);

    Ok(Json(state.analytics.node_history(id, from, to).await?))
}

/// `GET /api/v1/servers/{id}/report?from=&to=`: history, peaks and the top
/// processes by CPU and memory.
pub async fn report(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServerId>,
    Query(q): Query<HistoryQuery>,
) -> ApiResult<Json<forge_core::analytics::NodeReport>> {
    caller.authorize(Action::View, server_ref(id)).await?;
    let (from, to) = window(&state, &q);

    Ok(Json(state.analytics.node_report(id, from, to).await?))
}

/// `GET /api/v1/analytics/status`: engine, readiness, buffered rows and the
/// analytics worker process.
pub async fn analytics_status(
    State(state): State<AppState>,
    caller: Caller,
) -> ApiResult<Json<crate::analytics_control::AnalyticsStatus>> {
    caller.authorize(Action::View, EntityRef::platform()).await?;
    Ok(Json(state.analytics_control.status().await))
}

/// `GET /api/v1/metrics/latest`: the newest sample of every node the caller
/// can see (for utilization columns in node lists). Does not mark anything as
/// watched.
pub async fn latest(State(state): State<AppState>, caller: Caller) -> ApiResult<Json<Value>> {
    let mut out = serde_json::Map::new();

    for s in state.store.list_servers(None).await? {
        if let Some(sample) = state.telemetry.latest(s.id)
            && caller.can(Action::View, server_ref(s.id)).await
        {
            out.insert(s.id.to_string(), serde_json::to_value(sample).unwrap_or(Value::Null));
        }
    }

    Ok(Json(Value::Object(out)))
}

#[derive(Debug, Serialize)]
pub struct Peer {
    pub server_id: ServerId,
    pub hostname: String,
    pub role: NodeRole,
    pub status: AgentStatus,
    /// What the source node dials: `host:port` of the peer's SSH endpoint,
    /// or its hostname.
    pub target: String,
}

/// Peers of `node`: the other members of its cluster plus the control node
/// (which takes part in every cluster); for the control node, every other
/// node. Limited to what the caller may see.
async fn peers(state: &AppState, caller: &Caller, node: &Server) -> ApiResult<Vec<Peer>> {
    let mut out = Vec::new();

    for s in state.store.list_servers(None).await? {
        let member = node.role == NodeRole::Control
            || s.role == NodeRole::Control
            || (node.cluster_id.is_some() && s.cluster_id == node.cluster_id);

        if s.id == node.id || !member || s.status == AgentStatus::Revoked {
            continue;
        }

        if !caller.can(Action::View, server_ref(s.id)).await {
            continue;
        }

        let target = match state.store.get_ssh_endpoint(s.id).await? {
            Some(e) if e.host.contains(':') => format!("[{}]:{}", e.host, e.port),
            Some(e) => format!("{}:{}", e.host, e.port),
            // Workers already reach the control plane at its agent endpoint.
            None if s.role == NodeRole::Control => {
                agent_endpoint(state.config.node_agent_url()).unwrap_or_else(|| s.hostname.clone())
            }
            None => s.hostname.clone(),
        };

        out.push(Peer { server_id: s.id, hostname: s.hostname, role: s.role, status: s.status, target });
    }

    out.sort_by(|a, b| (a.role != NodeRole::Control, &a.hostname).cmp(&(b.role != NodeRole::Control, &b.hostname)));

    Ok(out)
}

/// `https://forge-cp:3843/…` → `forge-cp:3843`.
fn agent_endpoint(url: &str) -> Option<String> {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority = rest.split('/').next()?;
    if authority.is_empty() || authority.contains('@') {
        return None;
    }

    Some(if authority.contains(':') && !authority.ends_with(']') {
        authority.to_owned()
    } else {
        format!("{authority}:443")
    })
}

pub async fn connectivity(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServerId>,
) -> ApiResult<Json<Value>> {
    caller.authorize(Action::View, server_ref(id)).await?;
    let node = state.store.get_server(id).await.map_err(|_| ApiError::not_found("server not found"))?;
    let peers = peers(&state, &caller, &node).await?;

    Ok(Json(json!({ "peers": peers, "runs": state.telemetry.probes(id) })))
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProbeRequest {
    /// Echo requests per peer (1–10, default 4).
    #[serde(default)]
    pub count: Option<u32>,
}

pub async fn probe(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServerId>,
    body: Option<Json<ProbeRequest>>,
) -> ApiResult<Json<Value>> {
    caller.authorize(Action::ServerManage, server_ref(id)).await?;
    let node = state.store.get_server(id).await.map_err(|_| ApiError::not_found("server not found"))?;
    if node.status != AgentStatus::Online {
        return Err(ApiError::bad_request("the node's agent must be online to run checks"));
    }

    let peers = peers(&state, &caller, &node).await?;
    if peers.is_empty() {
        return Err(ApiError::bad_request("this node has no peers: add it to a cluster first"));
    }

    let count = body.and_then(|b| b.0.count).unwrap_or(4).clamp(1, 10);
    let targets: Vec<&str> = peers.iter().map(|p| p.target.as_str()).collect();
    let r = super::sites::run_on_node(
        &state,
        &caller,
        id,
        json!({ "type": "probe", "targets": targets, "count": count }),
        Duration::from_secs(40),
    )
    .await?;

    if r.get("status").and_then(Value::as_str) != Some("succeeded") {
        return Ok(Json(r));
    }

    // Attach the peer identity to each result.
    let results: Vec<Value> = r
        .pointer("/result/results")
        .and_then(Value::as_array)
        .map(|rs| {
            rs.iter()
                .zip(&peers)
                .map(|(res, p)| {
                    let mut v = res.clone();
                    if let Some(o) = v.as_object_mut() {
                        o.insert("server_id".into(), json!(p.server_id));
                        o.insert("hostname".into(), json!(p.hostname));
                        o.insert("role".into(), json!(p.role));
                    }

                    v
                })
                .collect()
        })
        .unwrap_or_default();
    let run = ProbeRun { t: state.clock.now().0, results: Value::Array(results) };
    state.telemetry.record_probe(id, run.clone());

    Ok(Json(json!({ "status": "succeeded", "job_id": r.get("job_id"), "run": run })))
}
