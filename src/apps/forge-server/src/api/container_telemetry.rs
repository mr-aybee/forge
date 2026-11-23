//! Docker container telemetry of a node.
//!
//! - `GET /api/v1/servers/{id}/container-metrics`: per-minute (up to two
//!   days) or hourly history of the node's containers. `view` on the node.
//! - `GET /api/v1/servers/{id}/container-metrics/{name}`: one container.
//! - `GET|PUT /api/v1/servers/{id}/container-telemetry`: whether the node
//!   collects container logs and metrics, and which containers it leaves
//!   out. Reading needs `view`, changing `server.manage`; changes are audited.

use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use forge_core::analytics::{ContainerHistory, MAX_HISTORY_CONTAINERS};
use forge_core::ports::ServerRepository;
use forge_core::{Action, EntityKind, EntityRef, ServerId};
use serde::Deserialize;
use serde_json::json;

use crate::auth::Caller;
use crate::container_telemetry::{AUDIT_SETTINGS_UPDATE, ContainerTelemetrySettings, TelemetryError};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/servers/{id}/container-metrics", get(node_metrics))
        .route("/servers/{id}/container-metrics/{name}", get(container_metrics))
        .route("/servers/{id}/container-telemetry", get(get_settings).put(put_settings))
}

fn server_ref(id: ServerId) -> EntityRef {
    EntityRef::new(EntityKind::Server, id)
}

fn telemetry_error(e: TelemetryError) -> ApiError {
    match e {
        TelemetryError::Invalid(m) => ApiError::bad_request(m),
        TelemetryError::Domain(e) => e.into(),
    }
}

/// Query string of the container history.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ContainerMetricsQuery {
    /// Window start, epoch ms (default: 24 hours before `to`).
    pub from: Option<i64>,
    /// Window end, epoch ms (default: now).
    pub to: Option<i64>,
    /// Most containers returned, by name, 1 to 200 (default 200).
    pub limit: Option<u32>,
}

fn window(state: &AppState, q: &ContainerMetricsQuery) -> (i64, i64) {
    let to = q.to.unwrap_or_else(|| state.clock.now().0);
    (q.from.unwrap_or(to - 24 * 3_600_000), to)
}

/// `GET /api/v1/servers/{id}/container-metrics?from=&to=&limit=`
pub async fn node_metrics(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServerId>,
    Query(q): Query<ContainerMetricsQuery>,
) -> ApiResult<Json<ContainerHistory>> {
    caller.authorize(Action::View, server_ref(id)).await?;
    let limit = q.limit.unwrap_or(MAX_HISTORY_CONTAINERS);
    Ok(Json(state.analytics.container_history(id, None, window(&state, &q), limit).await?))
}

/// `GET /api/v1/servers/{id}/container-metrics/{name}?from=&to=`
pub async fn container_metrics(
    State(state): State<AppState>,
    caller: Caller,
    Path((id, name)): Path<(ServerId, String)>,
    Query(q): Query<ContainerMetricsQuery>,
) -> ApiResult<Json<ContainerHistory>> {
    caller.authorize(Action::View, server_ref(id)).await?;
    if name.is_empty() || name.len() > 256 || name.chars().any(char::is_control) {
        return Err(ApiError::bad_request("a container name is 1 to 256 characters"));
    }
    Ok(Json(state.analytics.container_history(id, Some(name), window(&state, &q), 1).await?))
}

/// `GET /api/v1/servers/{id}/container-telemetry`
pub async fn get_settings(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServerId>,
) -> ApiResult<Json<ContainerTelemetrySettings>> {
    caller.authorize(Action::View, server_ref(id)).await?;
    state.store.get_server(id).await?;
    Ok(Json(state.container_telemetry.settings(id).await.map_err(telemetry_error)?))
}

/// New collection settings of a node.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContainerTelemetryBody {
    /// Collect logs and metrics of every container (default `true`).
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Containers left out: a container name, or `key=value` to match a
    /// label. At most 64 entries of up to 256 characters.
    #[serde(default)]
    pub exclude: Vec<String>,
}

fn yes() -> bool {
    true
}

/// `PUT /api/v1/servers/{id}/container-telemetry`
pub async fn put_settings(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServerId>,
    Json(body): Json<ContainerTelemetryBody>,
) -> ApiResult<Json<ContainerTelemetrySettings>> {
    let body = ContainerTelemetrySettings { enabled: body.enabled, exclude: body.exclude };
    caller.authorize(Action::ServerManage, server_ref(id)).await?;
    let server = state.store.get_server(id).await?;
    let (old, new) = state.container_telemetry.set_settings(id, body).await.map_err(telemetry_error)?;
    caller
        .audit(AUDIT_SETTINGS_UPDATE, &server_ref(id), Some(server.workspace_id), json!({ "from": old, "to": new }))
        .await;
    // The node picks the change up with its next heartbeat answer.
    state.hub.notifier(id).notify_one();
    Ok(Json(new))
}
