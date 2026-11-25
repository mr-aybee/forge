//! Forge network fabric: each cluster's subnet, address plan and internal
//! domain, managed by Forge itself (no Swarm / K3s IPAM).
//!
//! * `GET    /api/v1/clusters/{id}/fabric` — spec, layout, leases, pins, status
//! * `POST   /api/v1/clusters/{id}/fabric/suggest` — next free subnet + default domain
//! * `PUT    /api/v1/clusters/{id}/fabric` — validate, allocate and save (`workspace.manage`)
//! * `DELETE /api/v1/clusters/{id}/fabric` — remove (`workspace.manage`)
//!
//! Leases are recomputed whenever the spec or pins change and whenever a node
//! joins or leaves the cluster ([`reallocate`]); existing addresses are kept.

use std::net::Ipv4Addr;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;

use forge_catalog::fabric::{Cidr, DEFAULT_POOL, FabricSpec, Lease, allocate, suggest_subnet};
use forge_core::model::FabricRecord;
use forge_core::ports::{
    ClusterRepository, FabricRepository, ServerRepository, ServiceRepository, SettingsRepository, TenancyRepository,
};
use forge_core::{Action, ClusterId, EntityKind, EntityRef, ServerId, WorkspaceId};
use serde::Deserialize;
use serde_json::{Value, json};

use super::fabric_reconcile::{AppliedBy, reconcile_cluster, reset};
use super::ingress::node_address;
use crate::auth::{Caller, Ws};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// Settings key of the address pool clusters are carved from.
pub const POOL_SETTING: &str = "fabric.pool";

fn cluster_ref(id: ClusterId) -> EntityRef {
    EntityRef::new(EntityKind::Cluster, id)
}

fn bad(e: impl std::fmt::Display) -> ApiError {
    ApiError::bad_request(e.to_string())
}

async fn pool(state: &AppState) -> ApiResult<Cidr> {
    let v = state.store.all_settings().await?.into_iter().find(|(k, _)| k == POOL_SETTING).map(|(_, v)| v);
    v.as_deref().unwrap_or(DEFAULT_POOL).parse().map_err(bad)
}

/// Node and service ids that get addresses in this cluster.
async fn owners(
    state: &AppState,
    cluster: ClusterId,
) -> ApiResult<(Vec<forge_core::model::Server>, Vec<(String, String, String)>)> {
    let nodes: Vec<_> =
        state.store.list_servers(None).await?.into_iter().filter(|s| s.cluster_id == Some(cluster)).collect();
    let c = state.store.get_cluster(cluster).await?;
    let mut services = Vec::new();

    for ns in state.store.list_environments(c.workspace_id, Some(cluster)).await? {
        for a in state.store.list_services(Some(ns.id)).await? {
            services.push((a.id.to_string(), a.name.clone(), ns.name.clone()));
        }
    }

    Ok((nodes, services))
}

/// Subnets that a new cluster subnet must not overlap: other clusters'
/// subnets, and the networks nodes already sit on (the /24 of each node
/// address that is an IPv4 literal).
async fn taken(state: &AppState, ws: WorkspaceId, except: Option<ClusterId>) -> ApiResult<Vec<(Cidr, String)>> {
    let mut out = Vec::new();

    for f in state.store.list_fabrics(Some(ws)).await? {
        if Some(f.cluster_id) == except {
            continue;
        }

        if let Ok(s) = serde_json::from_value::<FabricSpec>(f.spec) {
            let name = state.store.get_cluster(f.cluster_id).await.map(|c| c.name).unwrap_or_default();
            out.push((s.subnet, format!("cluster {name}")));
        }
    }

    for s in state.store.list_servers(Some(ws)).await? {
        if let Ok(ip) = node_address(state, &s).await.parse::<Ipv4Addr>()
            && let Ok(c) = Cidr::new(ip, 24)
        {
            out.push((c, format!("network of {}", s.display_name.clone().unwrap_or_else(|| s.hostname.clone()))));
        }
    }

    Ok(out)
}

/// Recompute and store the leases of a cluster's fabric (no-op without one).
/// Called after spec / pin changes and when cluster membership changes.
pub async fn reallocate(state: &AppState, cluster: ClusterId) -> ApiResult<Option<FabricRecord>> {
    let Some(mut rec) = state.store.get_fabric(cluster).await? else { return Ok(None) };
    let spec: FabricSpec = serde_json::from_value(rec.spec.clone()).map_err(bad)?;
    let pins: Vec<Lease> = serde_json::from_value(rec.pins.clone()).unwrap_or_default();
    let previous: Vec<Lease> = serde_json::from_value(rec.leases.clone()).unwrap_or_default();
    let (nodes, services) = owners(state, cluster).await?;
    let node_ids: Vec<String> = nodes.iter().map(|n| n.id.to_string()).collect();
    let svc_ids: Vec<String> = services.iter().map(|s| s.0.clone()).collect();
    let leases = allocate(&spec, &node_ids, &svc_ids, &previous, &pins).map_err(bad)?;
    rec.leases = serde_json::to_value(&leases).map_err(bad)?;
    rec.updated_at = state.clock.now();
    state.store.put_fabric(&rec).await?;

    Ok(Some(rec))
}

pub async fn get(State(state): State<AppState>, caller: Caller, Path(id): Path<ClusterId>) -> ApiResult<Json<Value>> {
    caller.authorize(Action::View, cluster_ref(id)).await?;
    let rec = state.store.get_fabric(id).await?;
    let (nodes, services) = owners(&state, id).await?;
    let mut members = Vec::new();

    for n in &nodes {
        members.push(json!({
            "id": n.id,
            "name": n.display_name.clone().unwrap_or_else(|| n.hostname.clone()),
            "address": node_address(&state, n).await,
            "status": n.status,
            "os": n.os,
        }));
    }

    let services: Vec<Value> =
        services.iter().map(|(id, name, ns)| json!({ "id": id, "name": name, "environment": ns })).collect();

    let Some(rec) = rec else {
        return Ok(Json(json!({ "spec": null, "members": members, "services": services })));
    };

    let spec: Option<FabricSpec> = serde_json::from_value(rec.spec.clone()).ok();

    // The retired routed mode is shown as the mesh it becomes on apply.
    let mut shown = rec.spec.clone();
    if shown.get("mode").and_then(Value::as_str) == Some("routed") {
        shown["mode"] = json!("mesh");
    }

    Ok(Json(json!({
        "spec": shown,
        "layout": spec.map(|s| s.layout()),
        "pins": rec.pins,
        "leases": rec.leases,
        "status": rec.status,
        "updated_at": rec.updated_at,
        "members": members,
        "services": services,
    })))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SuggestInput {
    /// 16 (class B sized, default) or 24 (class C sized).
    #[serde(default = "default_prefix")]
    pub prefix: u8,
    /// Cluster name for the default domain (the new-cluster wizard has no id yet).
    #[serde(default)]
    pub name: Option<String>,
}
fn default_prefix() -> u8 {
    16
}

pub async fn suggest(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ClusterId>,
    Json(req): Json<SuggestInput>,
) -> ApiResult<Json<Value>> {
    caller.authorize(Action::View, cluster_ref(id)).await?;
    let c = state.store.get_cluster(id).await?;

    suggestion(&state, c.workspace_id, Some(id), &c.name, req.prefix).await
}

/// `POST /api/v1/fabric/suggest`: the same, for a cluster not created yet.
pub async fn suggest_new(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Json(req): Json<SuggestInput>,
) -> ApiResult<Json<Value>> {
    caller.authorize(Action::View, EntityRef::new(EntityKind::Workspace, ws)).await?;
    suggestion(&state, ws, None, req.name.as_deref().unwrap_or("cluster"), req.prefix).await
}

async fn suggestion(
    state: &AppState,
    ws: WorkspaceId,
    except: Option<ClusterId>,
    name: &str,
    prefix: u8,
) -> ApiResult<Json<Value>> {
    if !(16..=24).contains(&prefix) {
        return Err(ApiError::bad_request("prefix must be between 16 and 24"));
    }

    let pool = pool(state).await?;
    let taken = taken(state, ws, except).await?;
    let nets: Vec<Cidr> = taken.iter().map(|t| t.0).collect();
    let subnet = suggest_subnet(pool, prefix, &nets)
        .ok_or_else(|| ApiError::conflict(format!("no free /{prefix} left in the pool {pool}")))?;

    // `<cluster>.<suffix>`: the same names the fleet's hosts block uses (D-084).
    let slug = forge_catalog::names::label_from(name).unwrap_or_else(|| "cluster".into());
    let taken: Vec<Value> = taken.iter().map(|(c, why)| json!({ "subnet": c, "by": why })).collect();

    Ok(Json(json!({
        "subnet": subnet,
        "pool": pool,
        "domain": format!("{slug}.{}", crate::names::suffix(state).await),
        "taken": taken,
    })))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct FabricInput {
    #[schemars(with = "serde_json::Value")]
    pub spec: FabricSpec,
    #[serde(default)]
    #[schemars(with = "Vec<serde_json::Value>")]
    pub pins: Vec<Lease>,
}

pub async fn put(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ClusterId>,
    Json(req): Json<FabricInput>,
) -> ApiResult<Json<Value>> {
    let target = cluster_ref(id);
    caller.authorize(Action::WorkspaceManage, target.clone()).await?;
    let c = state.store.get_cluster(id).await?;
    req.spec.validate().map_err(bad)?;

    if let Some((t, why)) =
        taken(&state, c.workspace_id, Some(id)).await?.into_iter().find(|(t, _)| t.overlaps(&req.spec.subnet))
    {
        return Err(ApiError::conflict(format!("{} overlaps {t} ({why})", req.spec.subnet)));
    }

    let previous = state.store.get_fabric(id).await?;
    let moved = previous
        .as_ref()
        .and_then(|p| serde_json::from_value::<FabricSpec>(p.spec.clone()).ok())
        .is_some_and(|old| old.subnet != req.spec.subnet);
    let rec = FabricRecord {
        cluster_id: id,
        spec: serde_json::to_value(&req.spec).map_err(bad)?,
        pins: serde_json::to_value(&req.pins).map_err(bad)?,
        // A new subnet invalidates every old lease.
        leases: if moved { json!([]) } else { previous.as_ref().map(|p| p.leases.clone()).unwrap_or(json!([])) },
        status: previous.as_ref().map(|p| p.status.clone()).unwrap_or(json!({})),
        updated_at: state.clock.now(),
    };
    state.store.put_fabric(&rec).await?;

    let saved = match reallocate(&state, id).await {
        Ok(r) => r,
        Err(e) => {
            // Keep the previous state when the new plan does not fit.
            match &previous {
                Some(p) => state.store.put_fabric(p).await?,
                None => state.store.delete_fabric(id).await?,
            }

            return Err(e);
        }
    };

    caller
        .audit(
            "fabric.update",
            &target,
            Some(c.workspace_id),
            json!({ "subnet": req.spec.subnet, "mode": req.spec.mode, "domain": req.spec.domain, "pins": req.pins.len(), "moved": moved }),
        )
        .await;

    Ok(Json(json!({ "leases": saved.map(|r| r.leases) })))
}

/// Members must allow `server.command` (MFA) for the caller: applying
/// changes host networking on each of them.
async fn authorize_members(
    state: &AppState,
    caller: &Caller,
    cluster: ClusterId,
) -> ApiResult<Vec<forge_core::model::Server>> {
    let members: Vec<_> =
        state.store.list_servers(None).await?.into_iter().filter(|s| s.cluster_id == Some(cluster)).collect();

    for m in &members {
        caller.authorize(Action::ServerCommand, EntityRef::new(EntityKind::Server, m.id)).await?;
    }

    Ok(members)
}

/// `POST /clusters/{id}/fabric/apply`: bring every member to the plan. The
/// reconciler then keeps them there, acting as this caller.
pub async fn apply(State(state): State<AppState>, caller: Caller, Path(id): Path<ClusterId>) -> ApiResult<Json<Value>> {
    let target = cluster_ref(id);
    caller.authorize(Action::WorkspaceManage, target.clone()).await?;
    let c = state.store.get_cluster(id).await?;
    let members = authorize_members(&state, &caller, id).await?;
    let mut rec =
        state.store.get_fabric(id).await?.ok_or_else(|| ApiError::not_found("this cluster has no network yet"))?;
    let by = AppliedBy { principal: caller.principal(), context: caller.ctx(), at: state.clock.now() };

    if !rec.status.is_object() {
        rec.status = json!({});
    }

    rec.status["applied_by"] = serde_json::to_value(&by).map_err(bad)?;

    // A network saved in the retired routed mode becomes a mesh when applied.
    if rec.spec.get("mode").and_then(Value::as_str) == Some("routed") {
        rec.spec["mode"] = json!("mesh");
    }

    reset(&mut rec, None);
    state.store.put_fabric(&rec).await?;
    caller.audit("fabric.apply", &target, Some(c.workspace_id), json!({ "nodes": members.len() })).await;
    let s = state.clone();
    tokio::spawn(async move { reconcile_cluster(&s, id).await });

    Ok(Json(json!({ "nodes": members.len() })))
}

/// `POST /clusters/{id}/fabric/nodes/{node}/resync`: retry one node now.
pub async fn resync(
    State(state): State<AppState>,
    caller: Caller,
    Path((id, node)): Path<(ClusterId, ServerId)>,
) -> ApiResult<StatusCode> {
    let target = cluster_ref(id);
    caller.authorize(Action::WorkspaceManage, target.clone()).await?;
    caller.authorize(Action::ServerCommand, EntityRef::new(EntityKind::Server, node)).await?;
    let c = state.store.get_cluster(id).await?;
    let mut rec =
        state.store.get_fabric(id).await?.ok_or_else(|| ApiError::not_found("this cluster has no network yet"))?;
    reset(&mut rec, Some(&node.to_string()));
    state.store.put_fabric(&rec).await?;
    caller.audit("fabric.resync", &target, Some(c.workspace_id), json!({ "server_id": node })).await;
    let s = state.clone();
    tokio::spawn(async move { reconcile_cluster(&s, id).await });

    Ok(StatusCode::ACCEPTED)
}

/// Removes the plan; when it was applied, also takes the mesh off every
/// online member (best effort; offline nodes keep a dead interface until removed by hand).
pub async fn delete(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ClusterId>,
) -> ApiResult<Json<Value>> {
    let target = cluster_ref(id);
    caller.authorize(Action::WorkspaceManage, target.clone()).await?;
    let c = state.store.get_cluster(id).await?;
    let applied = state.store.get_fabric(id).await?.is_some_and(|r| r.status.get("applied_by").is_some());
    let mut left = Vec::new();

    if applied {
        let iface = forge_catalog::fabric::interface_name(&id.to_string());

        for m in authorize_members(&state, &caller, id).await? {
            let op = json!({ "type": "fabric", "op": { "op": "remove", "iface": iface } });
            let ok = m.status == forge_core::model::AgentStatus::Online
                && super::sites::run_on_node(&state, &caller, m.id, op, std::time::Duration::from_secs(60))
                    .await
                    .is_ok_and(|r| r["status"] == "succeeded");

            if !ok {
                left.push(m.display_name.clone().unwrap_or(m.hostname.clone()));
            }
        }
    }

    state.store.delete_fabric(id).await?;
    caller.audit("fabric.delete", &target, Some(c.workspace_id), json!({ "not_cleaned": left })).await;

    Ok(Json(json!({ "not_cleaned": left })))
}
