//! Analytics settings at runtime (D-136), shown under **Administration →
//! Settings → Analytics**.
//!
//! * `GET /api/v1/analytics/settings`: every setting with its `forge.toml`
//!   value, override and effective value; usage; the last apply result.
//! * `PUT /api/v1/analytics/settings`: changes settings and reports, per
//!   setting, whether it was applied, is pending or failed.
//! * `POST /api/v1/analytics/restart`: restarts the managed engine now or
//!   at a given time. `DELETE` cancels a scheduled restart.
//!
//! Reading needs `view` on the platform. Changes need `analytics.configure`
//! on the platform (platform admins).

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use forge_core::{Action, EntityRef};
use serde::{Deserialize, Serialize};

use crate::analytics_control::{ApplyReport, Change, Confirmations, NeedsConfirmation, SettingsView, UpdateError};
use crate::analytics_tuning::Patch;
use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// Audit action of a settings change.
pub const AUDIT_SETTINGS_UPDATE: &str = "analytics.settings.update";
/// Audit action of a scheduled or immediate engine restart.
pub const AUDIT_RESTART: &str = "analytics.restart";
/// Audit action of cancelling a scheduled restart.
pub const AUDIT_RESTART_CANCEL: &str = "analytics.restart.cancel";

/// `GET /api/v1/analytics/settings`
pub async fn get(State(state): State<AppState>, caller: Caller) -> ApiResult<Json<SettingsView>> {
    caller.authorize(Action::View, EntityRef::platform()).await?;
    Ok(Json(state.analytics_control.view().await))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateRequest {
    /// The settings to change: a number sets an override, `null` returns to
    /// the `forge.toml` value; absent settings are left alone.
    pub settings: Patch,
    #[serde(default)]
    pub confirm: Confirmations,
}

/// The audit record of a change.
#[derive(Debug, Serialize)]
struct ChangeAudit<'a> {
    changes: &'a [Change],
    report: &'a ApplyReport,
}

/// 409 body when a change needs confirmation: a problem document plus the
/// confirmations to send.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct ConfirmationRequired {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub title: &'static str,
    pub status: u16,
    pub detail: String,
    pub confirm: Vec<NeedsConfirmation>,
}

/// Error code: a CPU or memory setting of an engine Forge does not run.
pub const ERR_MANAGED_EXTERNALLY: &str = "managed_externally";

async fn guard(caller: &Caller, action: &str) -> ApiResult<EntityRef> {
    let platform = EntityRef::platform();
    caller.authorize(Action::AnalyticsConfigure, platform.clone()).await?;
    caller.require_mfa(action, &platform).await?;

    Ok(platform)
}

/// `PUT /api/v1/analytics/settings`
pub async fn put(State(state): State<AppState>, caller: Caller, Json(req): Json<UpdateRequest>) -> ApiResult<Response> {
    let platform = guard(&caller, AUDIT_SETTINGS_UPDATE).await?;

    match state.analytics_control.update(&req.settings, req.confirm).await {
        Ok((changes, report)) => {
            let details = serde_json::to_value(ChangeAudit { changes: &changes, report: &report })
                .map_err(|e| ApiError::from(forge_core::DomainError::Internal(e.to_string())))?;
            caller.audit(AUDIT_SETTINGS_UPDATE, &platform, None, details).await;
            Ok(Json(report).into_response())
        }
        Err(UpdateError::Confirm(confirm)) => {
            let detail = confirm.iter().map(|c| c.message.as_str()).collect::<Vec<_>>().join(" ");
            let body = ConfirmationRequired {
                kind: "https://docs.orbyn.dev/errors/confirmation_required",
                title: "confirmation_required",
                status: StatusCode::CONFLICT.as_u16(),
                detail,
                confirm,
            };
            Ok((StatusCode::CONFLICT, Json(body)).into_response())
        }
        Err(UpdateError::Invalid(e)) => {
            caller.audit_denied(AUDIT_SETTINGS_UPDATE, &platform, None, &e.to_string()).await;
            Err(ApiError::bad_request(e.to_string()))
        }
        Err(e @ UpdateError::NotManaged(_)) => {
            caller.audit_denied(AUDIT_SETTINGS_UPDATE, &platform, None, &e.to_string()).await;
            Err(ApiError::new(StatusCode::CONFLICT, ERR_MANAGED_EXTERNALLY, e.to_string()))
        }
        Err(UpdateError::Empty) => Err(ApiError::bad_request("name at least one setting to change")),
        Err(UpdateError::Store(e)) => Err(e.into()),
    }
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RestartRequest {
    /// When to restart, epoch ms; omitted or past: now. At most 7 days ahead.
    pub at: Option<i64>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct RestartScheduled {
    /// When the restart happens, epoch ms.
    pub at: i64,
}

/// `POST /api/v1/analytics/restart`
pub async fn restart(
    State(state): State<AppState>,
    caller: Caller,
    body: Option<Json<RestartRequest>>,
) -> ApiResult<Json<RestartScheduled>> {
    let platform = guard(&caller, AUDIT_RESTART).await?;
    let req = body.map(|Json(b)| b).unwrap_or_default();
    match state.analytics_control.schedule_restart(req.at) {
        Ok(at) => {
            let details = serde_json::to_value(RestartScheduled { at })
                .map_err(|e| ApiError::from(forge_core::DomainError::Internal(e.to_string())))?;
            caller.audit(AUDIT_RESTART, &platform, None, details).await;
            Ok(Json(RestartScheduled { at }))
        }
        Err(e) => Err(ApiError::bad_request(e.to_string())),
    }
}

/// `DELETE /api/v1/analytics/restart`
pub async fn cancel_restart(State(state): State<AppState>, caller: Caller) -> ApiResult<StatusCode> {
    let platform = guard(&caller, AUDIT_RESTART_CANCEL).await?;

    if !state.analytics_control.cancel_restart() {
        return Err(ApiError::not_found("no analytics restart is scheduled"));
    }

    caller.audit(AUDIT_RESTART_CANCEL, &platform, None, serde_json::Value::Null).await;

    Ok(StatusCode::NO_CONTENT)
}
