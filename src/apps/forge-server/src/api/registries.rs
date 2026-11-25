//! Container registries (D-158): list (`view`), add, change and remove
//! (`registry.manage`, MFA). Secrets are write-only: no answer, log or audit
//! entry ever carries one.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use forge_core::registries::{Registry, RegistryKind};
use forge_core::{Action, EntityKind, EntityRef, RegistryId};
use serde::Deserialize;
use serde_json::json;

use crate::auth::{Caller, Ws};
use crate::error::ApiResult;
use crate::registries::RegistryInput;
use crate::state::AppState;

pub const AUDIT_CREATE: &str = "registry.create";
pub const AUDIT_UPDATE: &str = "registry.update";
pub const AUDIT_DELETE: &str = "registry.delete";

fn ws_ref(ws: forge_core::WorkspaceId) -> EntityRef {
    EntityRef::new(EntityKind::Workspace, ws)
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RegistryBody {
    pub name: String,
    /// `docker_hub`, `ghcr`, `gitlab`, `ecr` or `custom`.
    #[schemars(with = "String")]
    pub kind: RegistryKind,
    /// `host[:port]`, e.g. `ghcr.io`.
    pub host: String,
    #[serde(default)]
    pub username: String,
    /// Password or token. Required on create; absent on update keeps it
    /// (only while host, kind and user name stay the same).
    #[serde(default)]
    pub secret: Option<String>,
    /// Images may be pushed to it (also needs `registry.manage`).
    #[serde(default)]
    pub allow_push: bool,
}

impl From<RegistryBody> for RegistryInput {
    fn from(b: RegistryBody) -> Self {
        Self {
            name: b.name,
            kind: b.kind,
            host: b.host,
            username: b.username,
            secret: b.secret,
            allow_push: b.allow_push,
        }
    }
}

pub async fn list(State(state): State<AppState>, caller: Caller, Ws(ws): Ws) -> ApiResult<Json<Vec<Registry>>> {
    caller.authorize(Action::View, ws_ref(ws)).await?;
    Ok(Json(state.registries.repo.list_registries(ws).await?))
}

pub async fn create(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Json(body): Json<RegistryBody>,
) -> ApiResult<(StatusCode, Json<Registry>)> {
    caller.authorize(Action::RegistryManage, ws_ref(ws)).await?;
    let r = state.registries.create(ws, &caller.principal(), body.into()).await?;
    caller.audit(AUDIT_CREATE, &ws_ref(ws), Some(ws), json!({ "id": r.id, "name": r.name, "host": r.host })).await;

    Ok((StatusCode::CREATED, Json(r)))
}

pub async fn update(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<RegistryId>,
    Json(body): Json<RegistryBody>,
) -> ApiResult<Json<Registry>> {
    let ws = state.registries.get(id).await?.workspace_id;
    caller.authorize(Action::RegistryManage, ws_ref(ws)).await?;
    let secret_changed = body.secret.is_some();
    let r = state.registries.update(id, body.into()).await?;
    caller
        .audit(
            AUDIT_UPDATE,
            &ws_ref(ws),
            Some(ws),
            json!({ "id": r.id, "name": r.name, "host": r.host, "secret_changed": secret_changed }),
        )
        .await;

    Ok(Json(r))
}

pub async fn delete(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<RegistryId>,
) -> ApiResult<StatusCode> {
    let r = state.registries.get(id).await?;
    caller.authorize(Action::RegistryManage, ws_ref(r.workspace_id)).await?;
    state.registries.delete(id).await?;
    caller
        .audit(
            AUDIT_DELETE,
            &ws_ref(r.workspace_id),
            Some(r.workspace_id),
            json!({ "id": id, "name": r.name, "host": r.host }),
        )
        .await;

    Ok(StatusCode::NO_CONTENT)
}
