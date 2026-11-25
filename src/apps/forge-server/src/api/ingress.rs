//! Cluster ingress profiles: the Nginx layout (standalone, active-standby,
//! active-active) on the cluster's edge nodes.
//!
//! * `GET  /api/v1/clusters/{id}/profile` — profile, last apply status, members
//! * `PUT  /api/v1/clusters/{id}/profile` — validate and save (`workspace.manage`)
//! * `POST /api/v1/clusters/{id}/profile/apply` — every edge: global Nginx
//!   config (+ keepalived)
//! * `POST /api/v1/clusters/{id}/profile/remove` — take the profile off the edges
//! * `GET  /api/v1/clusters/{id}/live` — Nginx / keepalived / VIP state per edge
//!
//! Node operations are audited `agent.command` jobs that need `proxy.manage`
//! on that node. A non-empty global custom block also needs `server.command`
//! (MFA) on every edge, to save and to apply: nginx runs it as root.

use std::time::Duration;

use axum::Json;
use axum::extract::{Path, State};
use forge_catalog::ingress::{ClusterProfile, IngressOp, KeepalivedNode};
use forge_core::model::{ClusterProfileRecord, NodeRole, Server};
use forge_core::ports::{ClusterProfileRepository, ClusterRepository, ServerRepository, SshEndpointRepository};
use forge_core::{Action, ClusterId, EntityKind, EntityRef, ServerId};
use serde_json::{Value, json};

use super::sites::run_on_node;
use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

fn cluster_ref(id: ClusterId) -> EntityRef {
    EntityRef::new(EntityKind::Cluster, id)
}
fn server_ref(id: ServerId) -> EntityRef {
    EntityRef::new(EntityKind::Server, id)
}

/// Cluster members plus the control node (which takes part in every cluster).
async fn members(state: &AppState, cluster: ClusterId) -> ApiResult<Vec<Server>> {
    Ok(state
        .store
        .list_servers(None)
        .await?
        .into_iter()
        .filter(|s| s.cluster_id == Some(cluster) || s.role == NodeRole::Control)
        .collect())
}

/// Best known address of a node: its SSH endpoint host, else its hostname.
/// How to reach a node: its configured SSH endpoint, else the address its
/// agent connects from (live, then the last saved facts), else its hostname
/// (which only works where the control plane can resolve it).
pub async fn node_address(state: &AppState, s: &Server) -> String {
    use forge_core::tags::FactsRepository;

    if let Ok(Some(e)) = state.store.get_ssh_endpoint(s.id).await
        && !e.host.is_empty()
    {
        return e.host;
    }

    if let Some(ip) = crate::facts::live_address(s.id) {
        return ip;
    }

    if let Ok(all) = state.store.all_facts().await
        && let Some(ip) = all.get(&s.id).and_then(|(f, _)| f["address"].as_str())
    {
        return ip.to_owned();
    }

    s.hostname.clone()
}

pub async fn load_profile(state: &AppState, cluster: ClusterId) -> ApiResult<Option<(ClusterProfile, Value)>> {
    let Some(rec) = state.store.get_cluster_profile(cluster).await? else { return Ok(None) };
    let p = serde_json::from_value(rec.profile)
        .map_err(|e| ApiError::bad_request(format!("stored cluster profile is invalid: {e}")))?;

    Ok(Some((p, rec.status)))
}

/// A non-empty global custom block runs as root inside nginx on every edge.
async fn authorize_custom(caller: &Caller, profile: &ClusterProfile) -> ApiResult<()> {
    let custom = profile.ingress.as_ref().is_some_and(|i| !i.nginx.custom.trim().is_empty());
    if custom {
        for e in edge_ids(profile)? {
            caller.authorize(Action::ServerCommand, server_ref(e)).await?;
        }
    }

    Ok(())
}

fn edge_ids(p: &ClusterProfile) -> ApiResult<Vec<ServerId>> {
    p.ingress
        .as_ref()
        .map(|i| {
            i.edges.iter().map(|e| e.server_id.parse().map_err(|_| ApiError::bad_request("invalid edge id"))).collect()
        })
        .unwrap_or_else(|| Ok(Vec::new()))
}

pub async fn get(State(state): State<AppState>, caller: Caller, Path(id): Path<ClusterId>) -> ApiResult<Json<Value>> {
    caller.authorize(Action::View, cluster_ref(id)).await?;
    let cluster = state.store.get_cluster(id).await?;
    let mut out_members = Vec::new();

    for s in members(&state, id).await? {
        out_members.push(json!({
            "id": s.id,
            "name": s.display_name.clone().unwrap_or_else(|| s.hostname.clone()),
            "hostname": s.hostname,
            "role": s.role,
            "status": s.status,
            "os": s.os,
            "address": node_address(&state, &s).await,
        }));
    }

    let (profile, status) = match load_profile(&state, id).await? {
        Some((p, mut st)) => {
            if let Some(o) = st.as_object_mut() {
                o.remove("vrrp_auth");
                o.remove("applied_by");
            }

            (Some(p), st)
        }
        None => (None, json!({})),
    };

    Ok(Json(json!({ "cluster": cluster, "profile": profile, "status": status, "members": out_members })))
}

pub async fn put(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ClusterId>,
    Json(profile): Json<ClusterProfile>,
) -> ApiResult<Json<Value>> {
    caller.authorize(Action::WorkspaceManage, cluster_ref(id)).await?;
    let cluster = state.store.get_cluster(id).await?;
    profile.validate().map_err(ApiError::bad_request)?;
    let ids: Vec<ServerId> = members(&state, id).await?.iter().map(|s| s.id).collect();
    if let Some(bad) = edge_ids(&profile)?.into_iter().find(|e| !ids.contains(e)) {
        return Err(ApiError::bad_request(format!("node {bad} is not a member of this cluster")));
    }

    authorize_custom(&caller, &profile).await?;
    let mut status = state.store.get_cluster_profile(id).await?.map_or_else(|| json!({}), |r| r.status);
    if status.get("vrrp_auth").and_then(Value::as_str).is_none() {
        status["vrrp_auth"] = json!(forge_secrets::crypto::random_token::<6>());
    }

    status["dirty"] = json!(true);
    state
        .store
        .put_cluster_profile(&ClusterProfileRecord {
            cluster_id: id,
            profile: serde_json::to_value(&profile).map_err(forge_core::DomainError::internal)?,
            status,
            updated_at: state.clock.now(),
        })
        .await?;
    caller
        .audit("cluster.profile.update", &cluster_ref(id), Some(cluster.workspace_id), json!({ "profile": profile }))
        .await;

    Ok(Json(json!({ "saved": true })))
}

async fn edges_op(
    state: &AppState,
    caller: &Caller,
    profile: &ClusterProfile,
    auth: &str,
    remove: bool,
) -> ApiResult<Value> {
    let Some(ingress) = &profile.ingress else { return Err(ApiError::bad_request("this profile has no edge nodes")) };
    let ids = edge_ids(profile)?;
    if ids.is_empty() {
        return Err(ApiError::bad_request("this profile has no edge nodes"));
    }

    for e in &ids {
        caller.authorize(Action::ProxyManage, server_ref(*e)).await?;
    }

    if !remove {
        authorize_custom(caller, profile).await?;
    }

    let mut out = serde_json::Map::new();

    for (index, e) in ids.iter().enumerate() {
        let op = if remove {
            IngressOp::Remove
        } else {
            IngressOp::Apply {
                nginx: Box::new(ingress.nginx.clone()),
                keepalived: ingress
                    .uses_vrrp()
                    .then(|| Box::new(KeepalivedNode { spec: ingress.clone(), index, auth: auth.to_owned() })),
            }
        };

        let r = run_on_node(
            state,
            caller,
            *e,
            json!({ "type": "ingress", "op": serde_json::to_value(&op).map_err(forge_core::DomainError::internal)? }),
            Duration::from_secs(120),
        )
        .await?;

        let status = match (remove, r["status"].as_str()) {
            (_, Some("failed")) => "failed",
            (false, Some("succeeded")) => "applied",
            (true, Some("succeeded")) => "removed",
            _ => "pending",
        };

        out.insert(
            e.to_string(),
            json!({ "status": status, "error": r["error"], "job_id": r["job_id"], "updated_at": state.clock.now().0 }),
        );
    }

    Ok(Value::Object(out))
}

pub(crate) async fn save_status(
    state: &AppState,
    id: ClusterId,
    profile: &ClusterProfile,
    status: Value,
) -> ApiResult<()> {
    state
        .store
        .put_cluster_profile(&ClusterProfileRecord {
            cluster_id: id,
            profile: serde_json::to_value(profile).map_err(forge_core::DomainError::internal)?,
            status,
            updated_at: state.clock.now(),
        })
        .await?;
    Ok(())
}

pub async fn apply(State(state): State<AppState>, caller: Caller, Path(id): Path<ClusterId>) -> ApiResult<Json<Value>> {
    caller.authorize(Action::View, cluster_ref(id)).await?;
    let cluster = state.store.get_cluster(id).await?;
    let (profile, mut status) =
        load_profile(&state, id).await?.ok_or_else(|| ApiError::bad_request("save a profile first"))?;
    let auth = status["vrrp_auth"].as_str().unwrap_or_default().to_owned();
    let edges = edges_op(&state, &caller, &profile, &auth, false).await?;
    status["edges"] = edges.clone();
    status["dirty"] = json!(false);
    status["applied_at"] = json!(state.clock.now().0);

    // DNS: publish now; the health loop keeps it current as this caller.
    let mut dns = Value::Null;
    if profile.ingress.as_ref().is_some_and(|i| i.dns.is_some()) {
        let by = super::fabric_reconcile::AppliedBy {
            principal: caller.principal(),
            context: caller.ctx(),
            at: state.clock.now(),
        };
        status["applied_by"] = serde_json::to_value(&by).map_err(forge_core::DomainError::internal)?;
        dns = super::ingress_dns::publish(&state, id, &profile, &status, &by).await;
        status["dns"] = dns.clone();
    }

    save_status(&state, id, &profile, status).await?;
    caller
        .audit(
            "cluster.profile.apply",
            &cluster_ref(id),
            Some(cluster.workspace_id),
            json!({ "cluster": cluster.name }),
        )
        .await;

    Ok(Json(json!({ "edges": edges, "dns": dns })))
}

/// The profile without the edges that report `removed`. Everything else
/// (mode, Nginx, VRRP, DNS) is kept, so adding an edge again restores it; an
/// empty edge list means the cluster has no edge.
fn release_removed(mut profile: ClusterProfile, results: &Value) -> ClusterProfile {
    if let Some(ingress) = profile.ingress.as_mut() {
        ingress.edges.retain(|e| results[&e.server_id]["status"].as_str() != Some("removed"));
    }

    profile
}

pub async fn remove(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ClusterId>,
) -> ApiResult<Json<Value>> {
    caller.authorize(Action::View, cluster_ref(id)).await?;
    let cluster = state.store.get_cluster(id).await?;
    let (profile, mut status) =
        load_profile(&state, id).await?.ok_or_else(|| ApiError::bad_request("this cluster has no profile"))?;
    let edges = edges_op(&state, &caller, &profile, "", true).await?;

    // Issue #25: an edge the profile was taken off holds no ingress role any
    // more, so it can leave the cluster. Edges whose removal failed or is
    // still pending keep the role, so the operator can retry the removal.
    let profile = release_removed(profile, &edges);
    status["edges"] = edges.clone();
    status["dirty"] = json!(true);
    save_status(&state, id, &profile, status).await?;
    caller
        .audit(
            "cluster.profile.remove",
            &cluster_ref(id),
            Some(cluster.workspace_id),
            json!({ "cluster": cluster.name }),
        )
        .await;

    Ok(Json(json!({ "edges": edges })))
}

/// Live state. Read-only commands; nodes the caller cannot manage are skipped.
pub async fn live(State(state): State<AppState>, caller: Caller, Path(id): Path<ClusterId>) -> ApiResult<Json<Value>> {
    caller.authorize(Action::View, cluster_ref(id)).await?;

    let Some((profile, _)) = load_profile(&state, id).await? else {
        return Ok(Json(json!({ "edges": {} })));
    };

    let vips: Vec<String> =
        profile.ingress.as_ref().and_then(|i| i.vrrp.as_ref()).map(|v| v.vips.clone()).unwrap_or_default();
    let mut edges = serde_json::Map::new();

    for e in edge_ids(&profile)? {
        if !caller.can(Action::ServerManage, server_ref(e)).await {
            continue;
        }

        let op = IngressOp::Status { vips: vips.clone() };
        let r = run_on_node(
            &state,
            &caller,
            e,
            json!({ "type": "ingress", "op": serde_json::to_value(&op).map_err(forge_core::DomainError::internal)? }),
            Duration::from_secs(15),
        )
        .await?;
        edges.insert(e.to_string(), json!({ "status": r["status"], "error": r["error"], "state": r["result"] }));
    }

    Ok(Json(json!({ "edges": edges })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(edges: &[&str]) -> ClusterProfile {
        let edges: Vec<Value> = edges.iter().map(|e| json!({ "server_id": e })).collect();
        serde_json::from_value(json!({ "ingress": { "mode": "standalone", "edges": edges } })).unwrap_or_default()
    }

    /// Issue #25: removed edges lose the role; failed or pending ones keep it.
    #[test]
    fn removing_the_profile_releases_only_removed_edges() {
        let results =
            json!({ "a": { "status": "removed" }, "b": { "status": "failed" }, "c": { "status": "pending" } });
        let left = release_removed(profile(&["a", "b", "c"]), &results);
        let ids: Vec<&str> = left.ingress.iter().flat_map(|i| i.edges.iter().map(|e| e.server_id.as_str())).collect();
        assert_eq!(ids, ["b", "c"]);
        let all = json!({ "a": { "status": "removed" } });
        let left = release_removed(profile(&["a"]), &all);
        let ingress = left.ingress.expect("settings are kept");
        assert!(ingress.edges.is_empty(), "no edge left");
        assert_eq!(ingress.mode, profile(&["a"]).ingress.expect("profile").mode);
    }
}
