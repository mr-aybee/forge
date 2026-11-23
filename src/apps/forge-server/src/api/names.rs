//! Local names (D-084): the fleet's names with their addresses and ports,
//! and a node's DNS label.

use std::collections::{BTreeSet, HashMap};

use axum::Json;
use axum::extract::{Path, Query, State};
use forge_core::ports::{ServerRepository, SettingsRepository};
use forge_core::*;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

fn server_ref(id: ServerId) -> EntityRef {
    EntityRef::new(EntityKind::Server, id)
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct NamesQuery {
    /// Only this workspace's names.
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub workspace: Option<WorkspaceId>,
}

/// `GET /api/v1/names`: every name the caller may see (nodes and services
/// on nodes they can view), with addresses, published ports and each
/// node's sync state.
pub async fn list(
    State(state): State<AppState>,
    caller: Caller,
    Query(q): Query<NamesQuery>,
) -> ApiResult<Json<Value>> {
    let servers = state.store.list_servers(q.workspace).await?;
    let allowed = caller.can_many(Action::View, servers.iter().map(|s| server_ref(s.id)).collect()).await;
    let visible: HashMap<ServerId, WorkspaceId> =
        servers.iter().zip(allowed).filter(|(_, ok)| *ok).map(|(s, _)| (s.id, s.workspace_id)).collect();
    let workspaces: BTreeSet<String> = visible.values().map(ToString::to_string).collect();
    let mut names = Vec::new();
    let mut nodes = Vec::new();
    for ws in workspaces.iter().filter_map(|w| w.parse::<WorkspaceId>().ok()) {
        let fleet = crate::names::fleet(&state, ws).await.map_err(|e| ApiError::from(DomainError::internal(e)))?;
        for r in fleet.rows.iter().filter(|r| visible.contains_key(&r.server_id)) {
            let mut v = serde_json::to_value(r).unwrap_or_default();
            v["workspace_id"] = json!(ws);
            names.push(v);
        }
        for s in fleet.servers.iter().filter(|s| visible.contains_key(&s.id)) {
            let l = fleet.labels.get(&s.id);
            nodes.push(json!({
                "server_id": s.id,
                "label": l.map(|l| l.label.clone()),
                "label_set": l.is_some_and(|l| l.explicit),
                "name": crate::names::node_name(&fleet, s.id).map(|r| r.name.clone()),
                "sync": crate::names::node_sync(s.id),
            }));
        }
    }
    let (suffix, suffix_source) = crate::names::suffix_with_source(&state).await;
    Ok(Json(json!({
        "enabled": state.config.names.enabled,
        "suffix": suffix,
        "suffix_source": suffix_source,
        "names": names,
        "nodes": nodes,
    })))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DnsNameRequest {
    /// The node's DNS label (`web-1`); empty goes back to the label derived
    /// from its console name or hostname.
    pub label: String,
}

/// `PUT /api/v1/servers/{id}/dns-name`: sets a node's DNS label (unique in
/// its workspace). Every node's hosts file follows within one pass.
pub async fn set_dns_name(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServerId>,
    Json(req): Json<DnsNameRequest>,
) -> ApiResult<Json<Value>> {
    caller.authorize(Action::ServerRename, server_ref(id)).await?;
    let server = state.store.get_server(id).await?;
    let label = req.label.trim();
    if !label.is_empty() {
        forge_catalog::names::check_label(label).map_err(ApiError::bad_request)?;
    }
    let before =
        crate::names::fleet(&state, server.workspace_id).await.map_err(|e| ApiError::from(DomainError::internal(e)))?;
    let old = before.labels.get(&id).cloned();
    let new = (!label.is_empty()).then_some(label);
    let stored = crate::names::stored_labels(&state, server.workspace_id)
        .await
        .map_err(|e| ApiError::from(DomainError::internal(e)))?;
    // Set and derived labels share one environment: never take another node's.
    let holder = |l: &str| stored.iter().find(|(o, (sl, _))| **o != id && sl == l).map(|(o, _)| *o);
    let in_use = |l: &str, other: ServerId| {
        let shown = before
            .servers
            .iter()
            .find(|s| s.id == other)
            .map_or_else(|| other.to_string(), |s| s.display_name.clone().unwrap_or_else(|| s.hostname.clone()));
        ApiError::conflict(format!("`{l}` is in use by node {shown}; rename that node first"))
    };
    let (chosen, explicit) = match new {
        Some(l) => {
            if let Some(other) = holder(l) {
                return Err(in_use(l, other));
            }
            (l.to_owned(), true)
        }
        // Back to a derived label that no other node holds.
        None => {
            let taken = stored.iter().filter(|(o, _)| **o != id).map(|(_, (l, _))| l.clone()).collect();
            (crate::names::free_label(&server, &taken), false)
        }
    };
    match state.store.set_node_label(id, server.workspace_id, &chosen, explicit, state.clock.now()).await {
        Err(DomainError::Conflict(_)) => {
            return Err(ApiError::conflict(format!("another node of this workspace is already named `{chosen}`")));
        }
        r => r?,
    }
    let after =
        crate::names::fleet(&state, server.workspace_id).await.map_err(|e| ApiError::from(DomainError::internal(e)))?;
    let name = crate::names::node_name(&after, id).map(|r| r.name.clone());
    caller
        .audit(
            "server.dns_rename",
            &server_ref(id),
            Some(server.workspace_id),
            json!({
                "from": old.as_ref().map(|l| &l.label),
                "to": after.labels.get(&id).map(|l| &l.label),
                "set": explicit,
                "name": name,
            }),
        )
        .await;
    crate::names::poke();
    Ok(Json(json!({
        "server_id": id,
        "label": after.labels.get(&id).map(|l| l.label.clone()),
        "label_set": explicit,
        "name": name,
    })))
}

/// DNS details for one node (`GET /servers/{id}`).
pub async fn of_node(state: &AppState, server: &forge_core::model::Server) -> Value {
    let Ok(fleet) = crate::names::fleet(state, server.workspace_id).await else { return Value::Null };
    let l = fleet.labels.get(&server.id);
    json!({
        "label": l.map(|l| l.label.clone()),
        "label_set": l.is_some_and(|l| l.explicit),
        "name": crate::names::node_name(&fleet, server.id).map(|r| r.name.clone()),
        "sync": crate::names::node_sync(server.id),
    })
}

/// The suffix answer: the one in use, where it comes from, and the config
/// file's value (what clearing the setting goes back to).
async fn suffix_view(state: &AppState) -> Value {
    let (suffix, source) = crate::names::suffix_with_source(state).await;
    json!({
        "suffix": suffix,
        "source": source,
        "config_suffix": state.config.names.suffix,
        "default_suffix": forge_catalog::names::DEFAULT_SUFFIX,
    })
}

/// `GET /api/v1/names/suffix`: the suffix every local name ends in and
/// where it comes from (`setting`, `config` or `default`). `view` on the
/// workspace.
pub async fn get_suffix(
    State(state): State<AppState>,
    caller: Caller,
    crate::auth::Ws(ws): crate::auth::Ws,
) -> ApiResult<Json<Value>> {
    // Any role in the workspace: every local name a member sees ends in it.
    if !crate::console_access::is_member(&state.store, &state.store, caller.session.user.id, ws).await? {
        caller.authorize(Action::View, EntityRef::new(EntityKind::Workspace, ws)).await?;
    }
    Ok(Json(suffix_view(&state).await))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SuffixRequest {
    /// The new suffix (`lab.local`, `home.arpa`, `corp.internal`); empty
    /// clears the runtime setting and goes back to the config file's value.
    pub suffix: String,
}

/// `PUT /api/v1/names/suffix`: sets the suffix of every local name in the
/// fleet. `system.update` on the platform (it changes every node) and an
/// MFA-verified session. Every node's hosts block is replaced on the next
/// pass; the old names stop resolving.
pub async fn put_suffix(
    State(state): State<AppState>,
    caller: Caller,
    Json(req): Json<SuffixRequest>,
) -> ApiResult<Json<Value>> {
    let platform = EntityRef::platform();
    if state.config.mfa_required() && !caller.ctx().mfa {
        caller.audit_denied("names.suffix.update", &platform, None, "MFA required").await;
        return Err(ApiError::new(
            axum::http::StatusCode::FORBIDDEN,
            "step_up_required",
            "changing the local name suffix needs an MFA-verified session",
        ));
    }
    caller.authorize(Action::SystemUpdate, platform.clone()).await?;
    let new = req.suffix.trim().to_owned();
    if !new.is_empty()
        && let Err(e) = forge_catalog::names::check_suffix(&new)
    {
        caller.audit_denied("names.suffix.update", &platform, None, &e).await;
        return Err(ApiError::bad_request(e));
    }
    let (old, old_source) = crate::names::suffix_with_source(&state).await;
    if new.is_empty() {
        state.store.delete_setting(crate::names::SUFFIX_KEY).await?;
    } else {
        state.store.put_setting(crate::names::SUFFIX_KEY, &new, state.clock.now()).await?;
    }
    let view = suffix_view(&state).await;
    caller
        .audit(
            "names.suffix.update",
            &platform,
            None,
            json!({ "from": old, "from_source": old_source, "to": view["suffix"], "source": view["source"] }),
        )
        .await;
    crate::names::poke();
    Ok(Json(view))
}
