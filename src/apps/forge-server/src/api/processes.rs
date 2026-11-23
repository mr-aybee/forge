//! `GET /servers/{id}/processes?sort=cpu&limit=100&filter=`: an htop-like
//! snapshot of a node (summary meters plus the top processes), fetched on
//! demand while the console's process panel is open.
//!
//! Needs `server.manage` (command lines can carry sensitive arguments), like
//! container reads. Read-only on the node; not audited per refresh (it is a
//! view, polled every few seconds), but denials are.
//!
//! `POST /servers/{id}/processes/{pid}/signal` and `…/priority` (D-080) end a
//! process (or its tree) or change its priority. They need `process.manage`
//! (an MFA-verified session, by the Cedar guardrail) and are audited, also
//! when the node refuses. The request names the process's start time from
//! the list, so the node never acts on a PID that was reused meanwhile.

use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use forge_core::model::AgentStatus;
use forge_core::ports::ServerRepository;
use forge_core::{Action, EntityKind, EntityRef, ServerId};
use forge_proto::Command;
use forge_proto::messages::{ProcessAction, ProcessList};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/servers/{id}/processes", get(list))
        .route("/servers/{id}/processes/{pid}/signal", post(signal))
        .route("/servers/{id}/processes/{pid}/priority", post(priority))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ProcQuery {
    #[serde(default)]
    pub sort: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub filter: Option<String>,
}

/// Top processes and system meters of a node.
pub async fn list(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServerId>,
    Query(q): Query<ProcQuery>,
) -> ApiResult<Json<Value>> {
    caller.authorize(Action::ServerManage, EntityRef::new(EntityKind::Server, id)).await?;
    let node = state.store.get_server(id).await.map_err(|_| ApiError::not_found("node not found"))?;
    if node.status != AgentStatus::Online {
        return Err(ApiError::conflict("the node is offline"));
    }
    let sort = q.sort.unwrap_or_else(|| "cpu".into());
    if !matches!(sort.as_str(), "cpu" | "mem" | "pid" | "time" | "name") {
        return Err(ApiError::bad_request("sort: cpu, mem, pid, time or name"));
    }
    let filter: String = q.filter.unwrap_or_default().chars().take(100).collect();
    let (body, signed) = state.signer.issue(
        &id.to_string(),
        &caller.principal().to_string(),
        Command::ProcessList(ProcessList { sort, limit: q.limit.unwrap_or(100).clamp(1, 500), filter }),
        state.clock.now(),
        Duration::from_secs(60),
    );
    state.hub.enqueue(id, &body.operation_id, signed).map_err(|e| ApiError::conflict(e.to_string()))?;
    let r = state.hub.wait(&body.operation_id, Duration::from_secs(20)).await.ok_or_else(|| {
        ApiError::new(axum::http::StatusCode::GATEWAY_TIMEOUT, "timeout", "the node did not answer in time")
    })?;
    if !r.ok {
        // Older agents do not know `processes` yet (reinstall or update them).
        return Err(ApiError::conflict(format!("the node refused: {}", r.error)));
    }
    serde_json::from_str(&r.output).map(Json).map_err(|_| ApiError::conflict("the node sent an invalid answer"))
}

/// Signals a process can be sent per node OS.
const LINUX_SIGNALS: [&str; 4] = ["term", "kill", "hup", "int"];
const WINDOWS_SIGNALS: [&str; 2] = ["term", "kill"];
/// Windows priority classes Forge sets (never `realtime`).
const PRIORITY_CLASSES: [&str; 5] = ["idle", "below_normal", "normal", "above_normal", "high"];

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SignalInput {
    /// The process's `started` value from the list (seconds since the epoch).
    /// The node refuses when the PID now belongs to another process.
    pub start_time: u64,
    /// Linux: `term`, `kill`, `hup` or `int`. Windows: `term` (End task, asks
    /// the process to close) or `kill` (End process, forced).
    pub signal: String,
    /// The process's descendants too, after the process itself.
    #[serde(default)]
    pub tree: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PriorityInput {
    /// The process's `started` value from the list (seconds since the epoch).
    pub start_time: u64,
    /// Linux: the nice value, -20 (highest priority) to 19.
    #[serde(default)]
    pub nice: Option<i32>,
    /// Windows: `idle`, `below_normal`, `normal`, `above_normal` or `high`.
    #[serde(default)]
    pub class: Option<String>,
}

/// Sends the action to the node, waits for its answer and audits it.
async fn act(
    state: &AppState,
    caller: &Caller,
    id: ServerId,
    event: &str,
    action: ProcessAction,
    details: Value,
) -> ApiResult<Json<Value>> {
    let node = state.store.get_server(id).await.map_err(|_| ApiError::not_found("node not found"))?;
    if node.status != AgentStatus::Online {
        return Err(ApiError::conflict("the node is offline"));
    }
    let target = EntityRef::new(EntityKind::Server, id);
    let (body, signed) = state.signer.issue(
        &id.to_string(),
        &caller.principal().to_string(),
        Command::ProcessAction(action),
        state.clock.now(),
        Duration::from_secs(60),
    );
    state.hub.enqueue(id, &body.operation_id, signed).map_err(|e| ApiError::conflict(e.to_string()))?;
    let Some(r) = state.hub.wait(&body.operation_id, Duration::from_secs(30)).await else {
        caller.audit_failed(event, &target, Some(node.workspace_id), details, "the node did not answer in time").await;
        return Err(ApiError::new(StatusCode::GATEWAY_TIMEOUT, "timeout", "the node did not answer in time"));
    };
    if !r.ok {
        caller.audit_failed(event, &target, Some(node.workspace_id), details, &r.error).await;
        // Older agents do not know `process_action` (update them), or the
        // operator removed it from the node's allowed commands.
        return Err(ApiError::conflict(format!("the node refused: {}", r.error)));
    }
    let out: Value = serde_json::from_str(&r.output).unwrap_or(Value::Null);
    let mut audited = details;
    audited["affected"] = out["affected"].clone();
    caller.audit(event, &target, Some(node.workspace_id), audited).await;
    Ok(Json(out))
}

/// Ends a process, or with `tree` the process and its descendants.
pub async fn signal(
    State(state): State<AppState>,
    caller: Caller,
    Path((id, pid)): Path<(ServerId, u32)>,
    Json(req): Json<SignalInput>,
) -> ApiResult<Json<Value>> {
    caller.authorize(Action::ProcessManage, EntityRef::new(EntityKind::Server, id)).await?;
    let node = state.store.get_server(id).await.map_err(|_| ApiError::not_found("node not found"))?;
    let windows = node.os == "windows";
    let allowed: &[&str] = if windows { &WINDOWS_SIGNALS } else { &LINUX_SIGNALS };
    if !allowed.contains(&req.signal.as_str()) {
        return Err(ApiError::bad_request(format!("signal: {} on this node", allowed.join(", "))));
    }
    if pid == 0 {
        return Err(ApiError::bad_request("pid must not be 0"));
    }
    let details = json!({ "pid": pid, "start_time": req.start_time, "signal": req.signal, "tree": req.tree });
    let action = ProcessAction {
        op: "signal".into(),
        pid,
        start_time: req.start_time,
        signal: req.signal,
        tree: req.tree,
        ..Default::default()
    };
    act(&state, &caller, id, "server.process.signal", action, details).await
}

/// Changes a process's priority: `nice` on Linux, `class` on Windows.
pub async fn priority(
    State(state): State<AppState>,
    caller: Caller,
    Path((id, pid)): Path<(ServerId, u32)>,
    Json(req): Json<PriorityInput>,
) -> ApiResult<Json<Value>> {
    caller.authorize(Action::ProcessManage, EntityRef::new(EntityKind::Server, id)).await?;
    let node = state.store.get_server(id).await.map_err(|_| ApiError::not_found("node not found"))?;
    if pid == 0 {
        return Err(ApiError::bad_request("pid must not be 0"));
    }
    let base = ProcessAction { pid, start_time: req.start_time, ..Default::default() };
    let (action, details) = if node.os == "windows" {
        let class = req.class.ok_or_else(|| ApiError::bad_request("class: a Windows node needs a priority class"))?;
        if !PRIORITY_CLASSES.contains(&class.as_str()) {
            return Err(ApiError::bad_request(format!("class: {}", PRIORITY_CLASSES.join(", "))));
        }
        let d = json!({ "pid": pid, "start_time": req.start_time, "class": class });
        (ProcessAction { op: "priority_class".into(), class, ..base }, d)
    } else {
        let nice = req.nice.ok_or_else(|| ApiError::bad_request("nice: a Linux node needs a nice value"))?;
        if !(-20..=19).contains(&nice) {
            return Err(ApiError::bad_request("nice: -20 to 19"));
        }
        let d = json!({ "pid": pid, "start_time": req.start_time, "nice": nice });
        (ProcessAction { op: "nice".into(), nice, ..base }, d)
    };
    act(&state, &caller, id, "server.process.priority", action, details).await
}
