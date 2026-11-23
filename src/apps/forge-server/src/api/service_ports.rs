//! The service port catalogue over the API (D-100): what listens where, for
//! `forge tunnel open` target resolution and the console's per-service port
//! lists. Read-only; see [`crate::service_ports`].

use axum::Json;
use axum::extract::{Path, Query, State};
use forge_core::tunnels::{TunnelGrantRepository, TunnelTargetKind};
use forge_core::*;
use serde::Deserialize;

use crate::auth::{Caller, Ws};
use crate::error::{ApiError, ApiResult};
use crate::service_ports::ServicePort;
use crate::state::AppState;

fn internal(e: String) -> ApiError {
    tracing::error!(error = %e, "service port catalogue failed");
    ApiError::new(axum::http::StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal error")
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct CatalogueQuery {
    /// A service by name, id, `<environment>/<name>` or `<stack>/<service>`.
    #[serde(default)]
    pub service: Option<String>,
    /// `app`, `stack`, `package` or `ssh`.
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub kind: Option<TunnelTargetKind>,
    /// Environment name or id.
    #[serde(default)]
    pub environment: Option<String>,
}

/// `GET /api/v1/service-ports?service=&kind=&environment=`: the workspace's
/// catalogue, limited to nodes the caller may view.
pub async fn list(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Query(q): Query<CatalogueQuery>,
) -> ApiResult<Json<Vec<ServicePort>>> {
    // Whoever may view the workspace, or holds any role inside it, sees the
    // catalogue (each entry still needs `view` on its node). A person with
    // no role here still sees the entries a tunnel grant gives them: the
    // grant exists for exactly those people, and the CLI has to resolve the
    // target before it can open the tunnel.
    let now = state.clock.now();
    let user = caller.session.user.id;
    let grants: Vec<forge_core::tunnels::TunnelGrant> =
        if caller.can(Action::View, EntityRef::new(EntityKind::Workspace, ws)).await {
            Vec::new()
        } else {
            let mine = state.store.list_tunnel_grants(Some(user)).await?;
            let mine: Vec<_> = mine.into_iter().filter(|g| g.is_active(now)).collect();
            if mine.is_empty() && !crate::console_access::is_member(&state.store, &state.store, user, ws).await? {
                caller.authorize(Action::View, EntityRef::new(EntityKind::Workspace, ws)).await?;
            }
            mine
        };
    let granted = |e: &ServicePort| grants.iter().any(|g| g.covers(user, e.kind, &e.id, e.server_id, e.port, now));
    let all = crate::service_ports::catalogue(&state, ws).await.map_err(internal)?;
    let wanted: Vec<ServicePort> = all
        .into_iter()
        .filter(|e| q.kind.is_none_or(|k| e.kind == k))
        .filter(|e| q.service.as_deref().is_none_or(|s| e.matches(s)))
        .filter(|e| {
            q.environment.as_deref().is_none_or(|n| {
                e.environment.as_deref() == Some(n) || e.environment_id.is_some_and(|id| id.to_string() == n)
            })
        })
        .collect();
    let allowed = caller
        .can_many(Action::View, wanted.iter().map(|e| EntityRef::new(EntityKind::Server, e.server_id)).collect())
        .await;
    Ok(Json(wanted.into_iter().zip(allowed).filter_map(|(e, ok)| (ok || granted(&e)).then_some(e)).collect()))
}

/// `GET /api/v1/servers/{id}/service-ports`: one node's listening services.
pub async fn for_server(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServerId>,
) -> ApiResult<Json<serde_json::Value>> {
    caller.authorize(Action::View, EntityRef::new(EntityKind::Server, id)).await?;
    let server = forge_core::ports::ServerRepository::get_server(&state.store, id)
        .await
        .map_err(|_| ApiError::not_found("server not found"))?;
    let all = crate::service_ports::catalogue(&state, server.workspace_id).await.map_err(internal)?;
    let ports: Vec<ServicePort> = all.into_iter().filter(|e| e.server_id == id).collect();
    Ok(Json(serde_json::json!({ "server_id": id, "ports": ports })))
}
