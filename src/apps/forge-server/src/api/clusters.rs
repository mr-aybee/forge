//! Clusters: named groups of nodes inside a workspace, and node assignment.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use forge_core::model::{Cluster, Server};
use forge_core::ports::{ClusterRepository, FabricRepository, ServerRepository, TenancyRepository};
use forge_core::*;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::auth::Caller;
use crate::auth::Ws;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

fn cluster_ref(id: ClusterId) -> EntityRef {
    EntityRef::new(EntityKind::Cluster, id)
}

fn valid_name(name: &str) -> ApiResult<String> {
    let n = name.trim();
    let ok = !n.is_empty()
        && n.len() <= 63
        && n.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ' '));
    if ok { Ok(n.to_owned()) } else { Err(ApiError::bad_request("cluster name: 1-63 letters, digits, space, - _ .")) }
}

async fn check_env(state: &AppState, workspace: WorkspaceId, env: Option<EnvironmentId>) -> ApiResult<()> {
    if let Some(e) = env
        && state.store.get_environment(e).await?.workspace_id != workspace
    {
        return Err(ApiError::bad_request("environment belongs to another workspace"));
    }
    Ok(())
}

pub async fn list(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
) -> ApiResult<Json<Vec<serde_json::Value>>> {
    // Any role in the workspace may list; each cluster is then filtered by
    // `Action::View` on it, so a person with a role on one cluster sees that
    // cluster. Without any role here the request is refused (and recorded).
    if !crate::console_access::is_member(&state.store, &state.store, caller.session.user.id, ws).await? {
        caller.authorize(Action::View, EntityRef::new(EntityKind::Workspace, ws)).await?;
    }
    let servers = state.store.list_servers(Some(ws)).await?;
    let mut out = Vec::new();
    for c in state.store.list_clusters(ws).await? {
        if !caller.can(Action::View, cluster_ref(c.id)).await {
            continue;
        }
        let members: Vec<&Server> = servers.iter().filter(|s| s.cluster_id == Some(c.id)).collect();
        let online = members.iter().filter(|s| s.status == forge_core::model::AgentStatus::Online).count();
        let mut v = serde_json::to_value(&c).map_err(forge_core::DomainError::internal)?;
        v["node_count"] = json!(members.len());
        v["nodes_online"] = json!(online);
        out.push(v);
    }
    Ok(Json(out))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ClusterInput {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub environment_id: Option<EnvironmentId>,
    /// Up to 4 letters or digits; empty derives initials from the name.
    #[serde(default)]
    pub abbr: Option<String>,
    /// Absent keeps the current image, `""` removes it, else a data URL.
    #[serde(default)]
    pub icon: Option<String>,
}

/// Largest accepted badge image (the data URL, base64 included).
const ICON_MAX: usize = 128 * 1024;

fn valid_abbr(s: &str) -> ApiResult<String> {
    let s = s.trim().to_uppercase();
    if s.chars().count() > 4 || !s.chars().all(|c| c.is_alphanumeric()) {
        return Err(ApiError::bad_request("abbreviation: up to 4 letters or digits"));
    }
    Ok(s)
}

/// Accept only base64 data URLs of common image types. SVG is allowed: the UI
/// renders badges with `<img>`, where scripts in an SVG never run.
fn valid_icon(s: &str) -> ApiResult<Option<String>> {
    if s.is_empty() {
        return Ok(None);
    }
    if s.len() > ICON_MAX {
        return Err(ApiError::bad_request("image is larger than 128 KB"));
    }
    let ok =
        ["png", "jpeg", "webp", "gif", "svg+xml"].iter().any(|t| s.starts_with(&format!("data:image/{t};base64,")));
    let body = s.split_once(',').map(|(_, b)| b).unwrap_or("");
    if !ok || !body.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'=') {
        return Err(ApiError::bad_request("image must be a PNG, JPEG, WebP, GIF or SVG data URL"));
    }
    Ok(Some(s.to_owned()))
}

pub async fn create(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Json(req): Json<ClusterInput>,
) -> ApiResult<(StatusCode, Json<Cluster>)> {
    caller.authorize(Action::WorkspaceManage, EntityRef::new(EntityKind::Workspace, ws)).await?;
    check_env(&state, ws, req.environment_id).await?;
    let c = Cluster {
        id: ClusterId::new(),
        workspace_id: ws,
        name: valid_name(&req.name)?,
        description: req.description.chars().take(500).collect(),
        environment_id: req.environment_id,
        abbr: valid_abbr(req.abbr.as_deref().unwrap_or(""))?,
        icon: valid_icon(req.icon.as_deref().unwrap_or(""))?,
        created_at: state.clock.now(),
    };
    state.store.create_cluster(&c).await?;
    caller.audit("cluster.create", &cluster_ref(c.id), Some(ws), json!({ "name": c.name })).await;
    Ok((StatusCode::CREATED, Json(c)))
}

pub async fn get(State(state): State<AppState>, caller: Caller, Path(id): Path<ClusterId>) -> ApiResult<Json<Cluster>> {
    caller.authorize(Action::View, cluster_ref(id)).await?;
    Ok(Json(state.store.get_cluster(id).await?))
}

pub async fn update(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ClusterId>,
    Json(req): Json<ClusterInput>,
) -> ApiResult<Json<Cluster>> {
    caller.authorize(Action::WorkspaceManage, cluster_ref(id)).await?;
    let mut c = state.store.get_cluster(id).await?;
    check_env(&state, c.workspace_id, req.environment_id).await?;
    c.name = valid_name(&req.name)?;
    c.description = req.description.chars().take(500).collect();
    c.environment_id = req.environment_id;
    if let Some(a) = &req.abbr {
        c.abbr = valid_abbr(a)?;
    }
    if let Some(i) = &req.icon {
        c.icon = valid_icon(i)?;
    }
    state.store.update_cluster(&c).await?;
    caller.audit("cluster.update", &cluster_ref(id), Some(c.workspace_id), json!({ "name": c.name })).await;
    Ok(Json(c))
}

pub async fn delete(State(state): State<AppState>, caller: Caller, Path(id): Path<ClusterId>) -> ApiResult<StatusCode> {
    caller.authorize(Action::WorkspaceManage, cluster_ref(id)).await?;
    let c = state.store.get_cluster(id).await?;
    // Environments always live in a cluster (D-056): they must move first.
    let environments =
        forge_core::ports::TenancyRepository::list_environments(&state.store, c.workspace_id, Some(id)).await?;
    if !environments.is_empty() {
        let names: Vec<&str> = environments.iter().map(|n| n.name.as_str()).collect();
        return Err(ApiError::conflict(format!(
            "move this cluster's environments to another cluster first: {}",
            names.join(", ")
        )));
    }
    forge_core::ports::ClusterProfileRepository::delete_cluster_profile(&state.store, id).await?;
    forge_core::ports::FabricRepository::delete_fabric(&state.store, id).await?;
    state.store.delete_cluster(id).await?;
    caller.audit("cluster.delete", &cluster_ref(id), Some(c.workspace_id), json!({ "name": c.name })).await;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AssignNode {
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub cluster_id: Option<ClusterId>,
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub environment_id: Option<EnvironmentId>,
}

/// `PATCH /servers/{id}`: move a node into a cluster and/or environment.
pub async fn assign(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServerId>,
    Json(req): Json<AssignNode>,
) -> ApiResult<Json<Server>> {
    caller.authorize(Action::ServerManage, EntityRef::new(EntityKind::Server, id)).await?;
    let server = state.store.get_server(id).await?;
    if let Some(c) = req.cluster_id {
        if state.store.get_cluster(c).await?.workspace_id != server.workspace_id {
            return Err(ApiError::bad_request("cluster belongs to another workspace"));
        }
        // Moving a node into a cluster requires management rights on the cluster too.
        caller.authorize(Action::ServerEnroll, cluster_ref(c)).await?;
    }
    check_env(&state, server.workspace_id, req.environment_id).await?;
    // Enrolling into an environment, and taking a node out of a cluster or
    // environment, need `server.enroll` there (as an enrollment token does).
    if req.environment_id != server.environment_id {
        if let Some(n) = req.environment_id {
            caller.authorize(Action::ServerEnroll, EntityRef::new(EntityKind::Environment, n)).await?;
        }
        if let Some(n) = server.environment_id {
            caller.authorize(Action::ServerEnroll, EntityRef::new(EntityKind::Environment, n)).await?;
        }
    }
    if let Some(old) = server.cluster_id.filter(|old| Some(*old) != req.cluster_id) {
        caller.authorize(Action::ServerEnroll, cluster_ref(old)).await?;
    }
    let leaving = server.cluster_id.filter(|old| Some(*old) != req.cluster_id);
    if let Some(old) = leaving {
        handover_blockers(&state, id, old).await?;
    }
    state.store.assign_server(id, req.cluster_id, req.environment_id).await?;
    // Keep the fabric address plans in step with membership.
    if let Some(c) = req.cluster_id
        && let Err(e) = super::fabric::reallocate(&state, c).await
    {
        state.store.assign_server(id, server.cluster_id, server.environment_id).await?;
        return Err(e);
    }
    if let Some(old) = leaving {
        super::fabric::reallocate(&state, old).await?;
        // Handover: the node leaves the old cluster's network (its WireGuard
        // interface) in the background; the new cluster's reconciler adds it
        // to the new network.
        let (s, principal, ctx) = (state.clone(), caller.principal(), caller.ctx());
        let iface = forge_catalog::fabric::interface_name(&old.to_string());
        if state.store.get_fabric(old).await?.is_some() {
            tokio::spawn(async move {
                let cmd = json!({ "type": "fabric", "op": { "op": "remove", "iface": iface } });
                if let Err(e) =
                    super::sites::run_as(&s, principal, ctx, id, cmd, std::time::Duration::from_secs(120)).await
                {
                    tracing::warn!(error = %e.message, %id, "handover: removing the old cluster network failed");
                }
            });
        }
    }
    // Every member of the old and the new fabric re-plans now (#24).
    if req.cluster_id != server.cluster_id {
        let affected: Vec<ClusterId> = leaving.into_iter().chain(req.cluster_id).collect();
        super::fabric_reconcile::membership_changed(&state, &affected).await;
    }
    caller
        .audit(
            "server.assign",
            &EntityRef::new(EntityKind::Server, id),
            Some(server.workspace_id),
            json!({ "cluster_id": req.cluster_id, "environment_id": req.environment_id }),
        )
        .await;
    Ok(Json(state.store.get_server(id).await?))
}

/// Whether `node` is listed in the profile's ingress edges. An empty edge
/// list (after **Remove profile**) means no edge (#25).
fn is_edge(profile: &Value, node: ServerId) -> bool {
    let id = node.to_string();
    profile["ingress"]["edges"]
        .as_array()
        .is_some_and(|edges| edges.iter().any(|e| e["server_id"].as_str() == Some(id.as_str())))
}

/// Roles a node has in its cluster that must move before the node does.
async fn handover_blockers(state: &AppState, node: ServerId, cluster: ClusterId) -> ApiResult<()> {
    let mut roles = Vec::new();
    if let Some(p) = forge_core::ports::ClusterProfileRepository::get_cluster_profile(&state.store, cluster).await?
        && is_edge(&p.profile, node)
    {
        roles.push("ingress edge (Cluster → Ingress)");
    }
    if roles.is_empty() {
        return Ok(());
    }
    Err(ApiError::conflict(format!(
        "the node is still the cluster's {}; move that role to another node first",
        roles.join(" and ")
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn badge_inputs_are_validated() {
        assert_eq!(valid_abbr(" la ").ok().as_deref(), Some("LA"));
        assert!(valid_abbr("TOOLONG").is_err());
        assert!(valid_abbr("A<B").is_err());
        assert_eq!(valid_icon("").ok(), Some(None));
        assert!(valid_icon("data:image/png;base64,iVBORw0KGgo=").is_ok());
        assert!(valid_icon("data:text/html;base64,PGI+").is_err());
        assert!(valid_icon("data:image/png;base64,<script>").is_err());
        assert!(valid_icon("javascript:alert(1)").is_err());
    }
}
