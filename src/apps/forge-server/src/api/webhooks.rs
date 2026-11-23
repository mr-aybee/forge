//! Deploy webhooks (D-159), managed in the console and API: list and
//! deliveries (`view`), create, change, rotate and remove (`webhook.manage`,
//! MFA, plus the right to deploy the target). The token and secret appear
//! once, in the create and rotate answers.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use forge_core::webhooks::{Delivery, Signature, Webhook, WebhookTarget};
use forge_core::{Action, EntityKind, EntityRef, WebhookId, WorkspaceId};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::auth::{Caller, Ws};
use crate::error::ApiResult;
use crate::state::AppState;
use crate::webhooks::{Minted, WebhookInput};

pub const AUDIT_CREATE: &str = "webhook.create";
pub const AUDIT_UPDATE: &str = "webhook.update";
pub const AUDIT_ROTATE: &str = "webhook.rotate";
pub const AUDIT_DELETE: &str = "webhook.delete";

fn ws_ref(ws: WorkspaceId) -> EntityRef {
    EntityRef::new(EntityKind::Workspace, ws)
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct WebhookBody {
    pub name: String,
    /// `{"kind": "service", "id"}` or `{"kind": "stack", "id"}`.
    #[schemars(with = "Value")]
    pub target: WebhookTarget,
    /// `none`, `github`, `gitlab` or `forge`.
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub signature: Signature,
    /// Tags a delivery may deploy, e.g. `v*`; absent: the tag stays.
    #[serde(default)]
    pub tag_pattern: Option<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
}

const fn yes() -> bool {
    true
}

impl From<WebhookBody> for WebhookInput {
    fn from(b: WebhookBody) -> Self {
        Self { name: b.name, target: b.target, signature: b.signature, tag_pattern: b.tag_pattern, enabled: b.enabled }
    }
}

/// The creator must be able to deploy the target: a delivery runs as them.
async fn authorize_target(state: &AppState, caller: &Caller, target: WebhookTarget) -> ApiResult<WorkspaceId> {
    match target {
        WebhookTarget::Service { id } => {
            let app = super::services::load_own(state, id).await?;
            caller.authorize(Action::Deploy, super::services::svc_ref(id)).await?;
            Ok(app.workspace_id)
        }
        WebhookTarget::Stack { id } => {
            let stack = super::stacks::load(state, id).await?;
            caller.authorize(Action::StackManage, super::stacks::ns_ref(stack.environment_id)).await?;
            Ok(stack.workspace_id)
        }
    }
}

pub async fn list(State(state): State<AppState>, caller: Caller, Ws(ws): Ws) -> ApiResult<Json<Vec<Webhook>>> {
    caller.authorize(Action::View, ws_ref(ws)).await?;
    Ok(Json(state.webhooks.repo.list_webhooks(ws).await?))
}

pub async fn create(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Json(body): Json<WebhookBody>,
) -> ApiResult<(StatusCode, Json<Minted>)> {
    caller.authorize(Action::WebhookManage, ws_ref(ws)).await?;
    let target_ws = authorize_target(&state, &caller, body.target).await?;
    if target_ws != ws {
        return Err(crate::error::ApiError::bad_request("the target belongs to another workspace"));
    }
    let minted = state.webhooks.create(ws, &caller.principal(), body.into()).await?;
    let w = &minted.webhook;
    caller
        .audit(
            AUDIT_CREATE,
            &ws_ref(ws),
            Some(ws),
            json!({ "id": w.id, "name": w.name, "target": w.target, "signature": w.signature }),
        )
        .await;
    Ok((StatusCode::CREATED, Json(minted)))
}

async fn owned(state: &AppState, caller: &Caller, id: WebhookId) -> ApiResult<Webhook> {
    let w = state.webhooks.get(id).await?;
    caller.authorize(Action::WebhookManage, ws_ref(w.workspace_id)).await?;
    authorize_target(state, caller, w.target).await?;
    Ok(w)
}

pub async fn update(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<WebhookId>,
    Json(body): Json<WebhookBody>,
) -> ApiResult<Json<Webhook>> {
    let w = owned(&state, &caller, id).await?;
    let updated = state.webhooks.update(id, body.into()).await?;
    caller
        .audit(
            AUDIT_UPDATE,
            &ws_ref(w.workspace_id),
            Some(w.workspace_id),
            json!({ "id": id, "enabled": updated.enabled, "tag_pattern": updated.tag_pattern }),
        )
        .await;
    Ok(Json(updated))
}

pub async fn rotate(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<WebhookId>,
) -> ApiResult<Json<Minted>> {
    let w = owned(&state, &caller, id).await?;
    let minted = state.webhooks.rotate(id).await?;
    caller.audit(AUDIT_ROTATE, &ws_ref(w.workspace_id), Some(w.workspace_id), json!({ "id": id })).await;
    Ok(Json(minted))
}

pub async fn delete(State(state): State<AppState>, caller: Caller, Path(id): Path<WebhookId>) -> ApiResult<StatusCode> {
    let w = owned(&state, &caller, id).await?;
    state.webhooks.delete(id).await?;
    caller
        .audit(AUDIT_DELETE, &ws_ref(w.workspace_id), Some(w.workspace_id), json!({ "id": id, "name": w.name }))
        .await;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn deliveries(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<WebhookId>,
) -> ApiResult<Json<Vec<Delivery>>> {
    let w = state.webhooks.get(id).await?;
    caller.authorize(Action::View, ws_ref(w.workspace_id)).await?;
    Ok(Json(state.webhooks.repo.list_deliveries(id, forge_core::webhooks::DELIVERIES_KEPT).await?))
}
