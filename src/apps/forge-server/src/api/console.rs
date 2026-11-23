//! `GET /me/capabilities`: which console areas the signed-in caller may
//! open, so the navigation hides what the server would refuse.

use axum::Json;
use axum::extract::State;
use serde::Serialize;

use crate::auth::{Caller, Ws};
use crate::console_access::{ConsoleAccess, ConsoleCapabilities};
use crate::error::ApiResult;
use crate::state::AppState;

/// One boolean per gated navigation entry.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct MyCapabilities {
    #[serde(flatten)]
    pub capabilities: ConsoleCapabilities,
}

/// The caller's navigation capabilities. Hints for the console only: every
/// endpoint still authorizes its own requests.
pub async fn capabilities(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
) -> ApiResult<Json<MyCapabilities>> {
    let access = ConsoleAccess { bindings: &state.store, policy: &state.policy, authz: state.authz.as_ref() };
    let capabilities = access.for_user(caller.session.user.id, ws).await?;
    Ok(Json(MyCapabilities { capabilities }))
}
