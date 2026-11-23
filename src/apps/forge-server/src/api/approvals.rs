//! Approvals (issue #6 Ask mode, D-073): a change an AI connector asked for
//! waits until its owner approves it in the console. Approving runs the
//! request once, through the same REST router, as the approving person's
//! own console session (their permissions and MFA), and records the result
//! for the agent to read. The agent cannot approve: approval needs a browser
//! session with CSRF, and connector tokens never reach these endpoints
//! except to read their own request's status.

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, Request, StatusCode, header};
use axum::{Extension, Json};
use forge_core::model::{Approval, SessionKind};
use forge_core::ports::ApprovalRepository;
use forge_core::{Action, EntityRef};
use serde::Deserialize;
use serde_json::{Value, json};
use tower::ServiceExt as _;

use super::mcp::{McpRouter, explain};
use crate::auth::{ApprovalExec, CSRF_HEADER, Caller};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// What the console and the agent see. The stored request is the tool's
/// arguments: data from an AI agent, shown as data.
fn view(a: &Approval, now: i64) -> Value {
    let status = if a.status == "pending" && a.expires_at.0 <= now { "expired" } else { a.status.as_str() };
    json!({
        "id": a.id, "kind": a.kind, "status": status, "requester": a.requester, "connection_id": a.connection_id,
        "action": a.action, "request": a.request, "summary": a.summary, "mfa_required": a.mfa_required,
        "created_at": a.created_at, "expires_at": a.expires_at, "decided_by": a.decided_by,
        "decided_at": a.decided_at, "result": a.result, "error": a.error, "user_id": a.user_id,
    })
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListQuery {
    /// Everyone's (`user.manage`).
    #[serde(default)]
    pub all: bool,
    /// Only this status (`pending`, `executed`, `denied`, `failed`, `expired`, `cancelled`).
    #[serde(default)]
    pub status: Option<String>,
}

/// `GET /api/v1/approvals`: your approvals, newest first.
pub async fn list(State(state): State<AppState>, caller: Caller, Query(q): Query<ListQuery>) -> ApiResult<Json<Value>> {
    if caller.session.connection.is_some() {
        return Err(ApiError::forbidden("connectors read only their own requests (forge_approval_status)"));
    }
    let user = if q.all {
        caller.authorize(Action::UserManage, EntityRef::platform()).await?;
        None
    } else {
        Some(caller.session.user.id)
    };
    let now = state.clock.now().0;
    let items: Vec<Value> = state
        .store
        .list_approvals(user, 200)
        .await?
        .iter()
        .map(|a| view(a, now))
        .filter(|v| q.status.as_deref().is_none_or(|s| v["status"] == s))
        .collect();
    Ok(Json(json!({ "approvals": items })))
}

async fn load(state: &AppState, caller: &Caller, id: &str) -> ApiResult<Approval> {
    let a = state.store.get_approval(id).await?.ok_or_else(|| ApiError::not_found("approval not found"))?;
    let own = a.user_id == caller.session.user.id
        && caller.session.connection.as_ref().is_none_or(|c| a.connection_id.as_deref() == Some(c.id.as_str()));
    if own {
        return Ok(a);
    }
    if caller.session.connection.is_none() && caller.can(Action::UserManage, EntityRef::platform()).await {
        return Ok(a);
    }
    Err(ApiError::not_found("approval not found"))
}

/// `GET /api/v1/approvals/{id}`: one approval; also what
/// `forge_approval_status` reads for the agent that asked.
pub async fn get(State(state): State<AppState>, caller: Caller, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let a = load(&state, &caller, &id).await?;
    Ok(Json(view(&a, state.clock.now().0)))
}

/// Only the owner, in their own console session, decides.
fn console_owner(caller: &Caller, a: &Approval) -> ApiResult<()> {
    if caller.session.session.kind != SessionKind::Web || caller.via_api_key() || caller.session.connection.is_some() {
        return Err(ApiError::forbidden("approve requests in the Forge console"));
    }
    if a.user_id != caller.session.user.id {
        return Err(ApiError::forbidden("only the person whose connector asked can approve it"));
    }
    Ok(())
}

/// Denying is the safe direction, so the owner may also deny from the CLI
/// (`forge approvals deny`); connector tokens still cannot decide.
fn owner_not_connector(caller: &Caller, a: &Approval) -> ApiResult<()> {
    if caller.session.connection.is_some() {
        return Err(ApiError::forbidden("connectors cannot decide approvals"));
    }
    if a.user_id != caller.session.user.id {
        return Err(ApiError::forbidden("only the person whose connector asked can decide it"));
    }
    Ok(())
}

/// Records a refused decision, with the approval and the reason, and
/// returns the error for the caller.
async fn refused(caller: &Caller, a: &Approval, e: ApiError) -> ApiError {
    caller
        .audit_denied(
            "mcp.approval.decide",
            &EntityRef::platform(),
            None,
            &format!("{} ({}): {}", a.id, a.action, e.message),
        )
        .await;
    e
}

/// The request may still run: its connection is active, connectors are
/// still enabled, and the connection's mode, under the server's current
/// `[mcp] max_mode`, still allows the tool. Each may have changed since the
/// request was queued.
async fn still_permitted(state: &AppState, a: &Approval) -> ApiResult<()> {
    if !state.config.mcp.enabled {
        return Err(ApiError::conflict("AI connectors are turned off on this Forge"));
    }
    let Some(conn_id) = a.connection_id.as_deref() else {
        return Err(ApiError::conflict("this request has no connection"));
    };
    let gone = || ApiError::conflict("the connector that asked was disconnected");
    let c = state.identity.get_connection(conn_id).await?.filter(|c| c.revoked_at.is_none()).ok_or_else(gone)?;
    // A revoked client ends its connections.
    if state.identity.connector_client(&c.client_id).await.is_err() {
        return Err(gone());
    }
    if !super::mcp::still_allowed(&a.action, c.mode.min(state.config.mcp.max_mode)) {
        return Err(ApiError::conflict("the connector's mode no longer allows this change"));
    }
    Ok(())
}

/// `POST /api/v1/approvals/{id}/deny`: the owner, in the console or the CLI.
pub async fn deny(State(state): State<AppState>, caller: Caller, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let a = load(&state, &caller, &id).await?;
    if let Err(e) = owner_not_connector(&caller, &a) {
        return Err(refused(&caller, &a, e).await);
    }
    if !state.store.decide_approval(&id, "denied", caller.session.user.id, state.clock.now()).await? {
        return Err(refused(&caller, &a, ApiError::conflict("this request was already decided or expired")).await);
    }
    caller
        .audit_denied("mcp.approval.decide", &EntityRef::platform(), None, &format!("{} denied: {}", a.action, a.id))
        .await;
    let a = load(&state, &caller, &id).await?;
    Ok(Json(view(&a, state.clock.now().0)))
}

/// `POST /api/v1/approvals/{id}/approve`: runs the request once as you.
pub async fn approve(
    State(state): State<AppState>,
    caller: Caller,
    Extension(router): Extension<McpRouter>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let a = load(&state, &caller, &id).await?;
    if let Err(e) = console_owner(&caller, &a) {
        return Err(refused(&caller, &a, e).await);
    }
    if let Err(e) = still_permitted(&state, &a).await {
        return Err(refused(&caller, &a, e).await);
    }
    if a.mfa_required && state.config.mfa_required() && !caller.ctx().mfa {
        let e = ApiError::new(
            StatusCode::FORBIDDEN,
            "step_up_required",
            "this action needs an MFA-verified session: verify it's you, then approve",
        );
        return Err(refused(&caller, &a, e).await);
    }
    // Single use: only one approve wins, and only before it expires.
    if !state.store.decide_approval(&id, "approved", caller.session.user.id, state.clock.now()).await? {
        return Err(refused(&caller, &a, ApiError::conflict("this request was already decided or expired")).await);
    }
    caller
        .audit(
            "mcp.approval.decide",
            &EntityRef::platform(),
            None,
            json!({ "approval": a.id, "action": a.action, "approved": true, "requester": a.requester }),
        )
        .await;
    let (status, result, error) = match run(&router, &headers, &a).await {
        Ok(v) => ("executed", Some(v), None),
        Err(e) => ("failed", None, Some(e)),
    };
    state.store.finish_approval(&id, status, result.as_ref(), error.as_deref()).await?;
    let a = load(&state, &caller, &id).await?;
    Ok(Json(view(&a, state.clock.now().0)))
}

/// Replays the stored request through the router as the approver: only
/// their session cookie and CSRF header, plus the [`ApprovalExec`] marker
/// that names the approval and connector in the handler's audit entry.
async fn run(router: &McpRouter, headers: &HeaderMap, a: &Approval) -> Result<Value, String> {
    let method = a.request["method"].as_str().unwrap_or("POST");
    let uri = a.request["uri"].as_str().ok_or("the request has no target")?;
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(c) = headers.get(header::COOKIE) {
        b = b.header(header::COOKIE, c);
    }
    if let Some(c) = headers.get(CSRF_HEADER) {
        b = b.header(CSRF_HEADER, c);
    }
    let body = match a.request.get("body") {
        Some(v) if !v.is_null() => {
            b = b.header(header::CONTENT_TYPE, "application/json");
            Body::from(v.to_string())
        }
        _ => Body::empty(),
    };
    let mut req = b.body(body).map_err(|e| e.to_string())?;
    req.extensions_mut().insert(ApprovalExec { approval_id: a.id.clone(), requester: a.requester.clone() });
    let resp = match router.0.clone().oneshot(req).await {
        Ok(r) => r,
        Err(e) => match e {},
    };
    let status = resp.status();
    let bytes =
        axum::body::to_bytes(resp.into_body(), 1024 * 1024).await.map_err(|_| "the result is too large".to_owned())?;
    let v: Value = serde_json::from_slice(&bytes).unwrap_or_else(|_| json!(String::from_utf8_lossy(&bytes)));
    if status.is_success() { Ok(v) } else { Err(explain(status, &v)) }
}
