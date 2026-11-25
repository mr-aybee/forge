//! Silences and maintenance windows (#11, D-112).
//!
//! A silence holds back the notifications of the alarms it covers for a
//! bounded time; alarms are still evaluated and recorded.
//!
//! * Creating, extending and ending a silence needs `alert.manage` on
//!   **everything its scope names**: the workspace for a workspace-wide
//!   silence, the cluster, the environment, or each node or service. A
//!   person with the operator role on one environment can silence that
//!   environment and nothing wider.
//! * Reading needs `alert.view`: on the workspace for every silence, or on
//!   what a silence's scope names for that silence.
//! * A silence looked up by id that the caller may not read answers the same
//!   `404` as an id of nothing.
//! * Every change is audited with what the silence matches and when it
//!   holds.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use forge_alerts::silence::SilenceInput;
use forge_core::alerts::Severity;
use forge_core::silences::{Silence, SilenceScope, SilenceState};
use forge_core::{Action, AlertRuleId, DomainError, EntityKind, EntityRef, SilenceId, Timestamp, WorkspaceId};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::auth::{Caller, Ws};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// Audit action names (one definition).
pub const AUDIT_SILENCE_CREATE: &str = "alert.silence.create";
pub const AUDIT_SILENCE_EXTEND: &str = "alert.silence.extend";
pub const AUDIT_SILENCE_END: &str = "alert.silence.end";

const MISSING: &str = "silence not found";

fn ws_ref(ws: WorkspaceId) -> EntityRef {
    EntityRef::new(EntityKind::Workspace, ws)
}

/// A silence with its state now.
#[derive(Debug, Serialize)]
pub struct SilenceView {
    #[serde(flatten)]
    pub silence: Silence,
    /// `scheduled`, `active` or `expired`, by the server's clock.
    pub state: SilenceState,
}

fn view(state: &AppState, silence: Silence) -> SilenceView {
    let now = state.clock.now();
    SilenceView { state: silence.state(now), silence }
}

/// What the audit log keeps of a silence: what it matches and when, and
/// for a change the window it had before.
#[derive(Serialize)]
struct SilenceAudit<'a> {
    silence: SilenceId,
    scope: &'a SilenceScope,
    rules: &'a [AlertRuleId],
    min_severity: Option<Severity>,
    max_severity: Option<Severity>,
    starts_at: Timestamp,
    ends_at: Timestamp,
    reason: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    previous_starts_at: Option<Timestamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    previous_ends_at: Option<Timestamp>,
}

fn silence_audit(s: &Silence, previous: Option<&Silence>, with_start: bool) -> serde_json::Value {
    json!(SilenceAudit {
        silence: s.id,
        scope: &s.scope,
        rules: &s.rules,
        min_severity: s.min_severity,
        max_severity: s.max_severity,
        starts_at: s.starts_at,
        ends_at: s.ends_at,
        reason: &s.reason,
        previous_starts_at: previous.filter(|_| with_start).map(|p| p.starts_at),
        previous_ends_at: previous.map(|p| p.ends_at),
    })
}

/// `alert.manage` on everything the scope names. A refusal is recorded by
/// the authorization layer. An entity that does not exist is refused like
/// one the caller holds no role on, so ids cannot be probed.
async fn authorize_scope(caller: &Caller, ws: WorkspaceId, scope: &SilenceScope) -> ApiResult<()> {
    for r in scope.refs(ws) {
        match caller.authorize(Action::AlertManage, r.clone()).await {
            Ok(()) => {}
            Err(e) if e.status == StatusCode::NOT_FOUND => {
                return Err(ApiError::forbidden(format!("alert.manage on {r}")));
            }
            Err(e) => return Err(e),
        }
    }

    Ok(())
}

/// Whether the caller may read silences with these scopes: `alert.view` on
/// the workspace covers all; otherwise on everything a scope names.
async fn readable(caller: &Caller, ws: WorkspaceId, silences: &[Silence]) -> Vec<bool> {
    if caller.can(Action::AlertView, ws_ref(ws)).await {
        return vec![true; silences.len()];
    }

    let refs: Vec<Vec<EntityRef>> = silences.iter().map(|s| s.scope.refs(s.workspace_id)).collect();
    let flat: Vec<EntityRef> = refs.iter().flatten().cloned().collect();
    let mut allowed = caller.can_many(Action::AlertView, flat).await.into_iter();

    refs.iter().map(|r| !r.is_empty() && allowed.by_ref().take(r.len()).all(|ok| ok)).collect()
}

/// A silence the caller may read. Unknown and unreadable ids both answer
/// `404`.
async fn silence_for(state: &AppState, caller: &Caller, id: SilenceId) -> ApiResult<Silence> {
    let s = state.notifier.silences.get(id).await.map_err(|e| match e {
        DomainError::NotFound(_) => ApiError::not_found(MISSING),
        other => other.into(),
    })?;

    if readable(caller, s.workspace_id, std::slice::from_ref(&s)).await.first().copied().unwrap_or(false) {
        Ok(s)
    } else {
        Err(ApiError::not_found(MISSING))
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SilenceQuery {
    /// The workspace (default: this deployment's workspace).
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub workspace: Option<WorkspaceId>,
    /// Only `scheduled`, `active` or `expired` silences.
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub state: Option<SilenceState>,
    /// 1–500, default 100.
    #[serde(default)]
    pub limit: Option<u32>,
}

/// `GET /api/v1/silences`: latest start first.
pub async fn list_silences(
    State(state): State<AppState>,
    caller: Caller,
    Ws(default): Ws,
    Query(q): Query<SilenceQuery>,
) -> ApiResult<Json<Vec<SilenceView>>> {
    let ws = q.workspace.unwrap_or(default);
    let limit = q.limit.unwrap_or(100).clamp(1, 500);

    // The read filter runs after the query: page on until the list is full
    // or the silences run out; bounded.
    let mut out: Vec<Silence> = Vec::new();
    let mut before = None;

    for _ in 0..5 {
        let page = state.notifier.silences.list(ws, q.state, before, limit).await?;
        let full = page.len() == limit as usize;
        before = page.last().map(|s| (s.starts_at, s.id));
        let ok = readable(&caller, ws, &page).await;
        out.extend(page.into_iter().zip(ok).filter(|(_, ok)| *ok).map(|(s, _)| s));

        if out.len() >= limit as usize || !full {
            break;
        }
    }

    if out.is_empty() {
        // Nothing to show: an empty list only for someone who may read the
        // workspace's alerts; anyone else gets the refusal (recorded), so a
        // caller without any role here cannot tell "none" from "not yours".
        caller.authorize(Action::AlertView, ws_ref(ws)).await?;
    }

    out.truncate(limit as usize);

    Ok(Json(out.into_iter().map(|s| view(&state, s)).collect()))
}

/// `GET /api/v1/silences/{id}`.
pub async fn get_silence(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<SilenceId>,
) -> ApiResult<Json<SilenceView>> {
    // Read access is decided per silence (`Action::AlertView` on the
    // workspace or on what its scope names).
    let s = silence_for(&state, &caller, id).await?;
    Ok(Json(view(&state, s)))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SilenceBody {
    /// Default: this deployment's workspace. Fixed after creation.
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub workspace_id: Option<WorkspaceId>,
    /// Which alarms: `{"kind": "workspace"}`, `{"kind": "cluster", "id": …}`,
    /// `{"kind": "environment", "id": …}`, `{"kind": "servers", "ids": […]}`
    /// or `{"kind": "services", "ids": […]}`.
    #[schemars(with = "serde_json::Value")]
    pub scope: SilenceScope,
    /// Only alarms of these rules (empty: every rule). At most 100.
    #[serde(default)]
    #[schemars(with = "Vec<String>")]
    pub rules: Vec<AlertRuleId>,
    /// Only alarms of this severity and worse: `info`, `warning`, `major`,
    /// `critical`.
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub min_severity: Option<Severity>,
    /// Only alarms of this severity and milder. A worse alarm still notifies.
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub max_severity: Option<Severity>,
    /// Start, in epoch milliseconds. Left out: now. A later time makes a
    /// maintenance window.
    #[serde(default)]
    #[schemars(with = "Option<i64>")]
    pub starts_at: Option<Timestamp>,
    /// End, in epoch milliseconds. Give this or `duration_secs`.
    #[serde(default)]
    #[schemars(with = "Option<i64>")]
    pub ends_at: Option<Timestamp>,
    /// Length from the start, in seconds (60 to 2592000). Give this or
    /// `ends_at`.
    #[serde(default)]
    pub duration_secs: Option<u64>,
    /// Why alarms are quiet: 1 to 500 characters.
    pub reason: String,
}

/// `base + secs`, in epoch milliseconds, without overflow.
fn plus_secs(base: Timestamp, secs: u64) -> Timestamp {
    let ms = i64::try_from(secs).unwrap_or(i64::MAX).saturating_mul(1000);
    Timestamp(base.0.saturating_add(ms))
}

/// The one end a request names: a time, or a length from `base`.
fn end_of(ends_at: Option<Timestamp>, secs: Option<u64>, base: Timestamp, length: &str) -> ApiResult<Timestamp> {
    match (ends_at, secs) {
        (Some(t), None) => Ok(t),
        (None, Some(secs)) => Ok(plus_secs(base, secs)),
        _ => Err(ApiError::bad_request(format!("give exactly one of ends_at and {length}"))),
    }
}

/// `POST /api/v1/silences`.
pub async fn create_silence(
    State(state): State<AppState>,
    caller: Caller,
    Ws(default): Ws,
    Json(body): Json<SilenceBody>,
) -> ApiResult<(StatusCode, Json<SilenceView>)> {
    let ws = body.workspace_id.unwrap_or(default);

    // `Action::AlertManage` on everything the scope names.
    authorize_scope(&caller, ws, &body.scope).await?;

    if ws != default {
        return Err(ApiError::bad_request("no such workspace"));
    }

    let now = state.clock.now();
    let base = body.starts_at.map_or(now, |t| Timestamp(t.0.max(now.0)));
    let ends_at = end_of(body.ends_at, body.duration_secs, base, "duration_secs")?;
    let input = SilenceInput {
        scope: body.scope,
        rules: body.rules,
        min_severity: body.min_severity,
        max_severity: body.max_severity,
        starts_at: body.starts_at,
        ends_at,
        reason: body.reason,
        manages_workspace: caller.can(Action::AlertManage, ws_ref(ws)).await,
    };
    let silence = state.notifier.silences.create(ws, input, caller.principal()).await?;
    caller.audit(AUDIT_SILENCE_CREATE, &ws_ref(ws), Some(ws), silence_audit(&silence, None, false)).await;

    Ok((StatusCode::CREATED, Json(view(&state, silence))))
}

/// A silence the caller may change: `alert.manage` on the workspace, or
/// readable and `alert.manage` on everything its scope names. The workspace
/// check comes first so a silence that names a deleted node can still be
/// ended. A silence the caller may not read answers `404`, and the refusal
/// is recorded under `audit_action`.
async fn silence_to_manage(state: &AppState, caller: &Caller, id: SilenceId, audit_action: &str) -> ApiResult<Silence> {
    let s = match silence_for(state, caller, id).await {
        Ok(s) => s,
        Err(e) => {
            if e.status == StatusCode::NOT_FOUND {
                // Only a silence that exists is worth recording; a failed
                // lookup here changes nothing about the answer.
                if let Ok(hidden) = state.notifier.silences.get(id).await {
                    let ws = hidden.workspace_id;
                    caller.audit_denied(audit_action, &ws_ref(ws), Some(ws), "silence not readable").await;
                }
            }

            return Err(e);
        }
    };

    if !caller.can(Action::AlertManage, ws_ref(s.workspace_id)).await {
        authorize_scope(caller, s.workspace_id, &s.scope).await?;
    }

    Ok(s)
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExtendBody {
    /// The new end, in epoch milliseconds; later than the current end. Give
    /// this or `extend_secs`.
    #[serde(default)]
    #[schemars(with = "Option<i64>")]
    pub ends_at: Option<Timestamp>,
    /// Seconds to add to the current end. Give this or `ends_at`.
    #[serde(default)]
    pub extend_secs: Option<u64>,
}

/// `POST /api/v1/silences/{id}/extend`: moves the end of a silence that has
/// not ended to a later time (at most 30 days after its start).
pub async fn extend_silence(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<SilenceId>,
    Json(body): Json<ExtendBody>,
) -> ApiResult<Json<SilenceView>> {
    // `Action::AlertManage` on everything the scope names.
    let old = silence_to_manage(&state, &caller, id, AUDIT_SILENCE_EXTEND).await?;
    let ends_at = end_of(body.ends_at, body.extend_secs, old.ends_at, "extend_secs")?;
    let silence = state.notifier.silences.extend(id, ends_at).await?;
    let details = silence_audit(&silence, Some(&old), false);
    caller.audit(AUDIT_SILENCE_EXTEND, &ws_ref(old.workspace_id), Some(old.workspace_id), details).await;

    Ok(Json(view(&state, silence)))
}

/// `POST /api/v1/silences/{id}/end`: ends a silence now. Alarms it covered
/// that still fire notify within seconds. The silence stays as history.
pub async fn end_silence(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<SilenceId>,
) -> ApiResult<Json<SilenceView>> {
    // `Action::AlertManage` on everything the scope names.
    let old = silence_to_manage(&state, &caller, id, AUDIT_SILENCE_END).await?;
    let silence = state.notifier.silences.end(id, caller.principal()).await?;
    let details = silence_audit(&silence, Some(&old), true);
    caller.audit(AUDIT_SILENCE_END, &ws_ref(old.workspace_id), Some(old.workspace_id), details).await;

    Ok(Json(view(&state, silence)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_names_exactly_one_end() {
        let base = Timestamp(1_000);
        assert_eq!(end_of(Some(Timestamp(5)), None, base, "x").ok(), Some(Timestamp(5)));
        assert_eq!(end_of(None, Some(60), base, "x").ok(), Some(Timestamp(61_000)));
        assert!(end_of(None, None, base, "x").is_err());
        assert!(end_of(Some(Timestamp(5)), Some(60), base, "x").is_err());

        // A huge length saturates; the service refuses it as too long.
        assert_eq!(plus_secs(base, u64::MAX), Timestamp(i64::MAX));
    }
}
