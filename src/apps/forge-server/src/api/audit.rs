//! Audit log: search, page, histogram, distinct actions and verify.
//!
//! Filtering and keyset pagination happen in the store, so every entry in the
//! log is reachable, not only a recent window.

use axum::Json;
use axum::extract::{Query, RawQuery, State};
use forge_audit::{AuditHistogram, VerifyReport};
use forge_core::model::{AuditActionCount, AuditEntry, AuditFilter, AuditOutcome};
use forge_core::ports::AuditRepository;
use forge_core::*;
use serde::{Deserialize, Serialize};

use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AuditQuery {
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub workspace: Option<WorkspaceId>,
    /// Page size. `/audit`: default 100, max 500. `/audit/page`: default 50, max 200.
    #[serde(default)]
    pub limit: Option<u32>,
    /// Action prefixes, such as `stack.` or `auth.login`. Comma-separated or
    /// repeated (`?action=auth.&action=files.`); an entry matches any of them.
    /// Read from the raw query string, so repeated keys work.
    #[serde(skip_deserializing)]
    #[allow(dead_code)]
    pub action: Option<String>,
    /// Only entries whose actor contains this, case-insensitive (`user:<id>`, `system:forge`…).
    #[serde(default)]
    pub actor: Option<String>,
    /// Free text: entries whose action, actor, resource or details contain
    /// this, case-insensitive.
    #[serde(default)]
    pub q: Option<String>,
    /// Only `success`, `denied` or `failure` (aliases `allow`, `deny`, `error`).
    #[serde(default)]
    pub outcome: Option<String>,
    /// Only entries at or after this time (epoch milliseconds).
    #[serde(default)]
    pub since: Option<i64>,
    /// Only entries before this time (epoch milliseconds, exclusive).
    #[serde(default)]
    pub until: Option<i64>,
    /// Only entries whose resource contains this, case-insensitive (`server:<id>`…).
    #[serde(default)]
    pub resource: Option<String>,
    /// Keyset cursor: only entries with a `seq` below this. Use `next_cursor`.
    #[serde(default)]
    pub before: Option<i64>,
    /// `/audit/page` only: also return `total`, the number of matching entries.
    #[serde(default)]
    pub total: Option<bool>,
    /// `/audit/histogram` only: most buckets, default 60, max 200.
    #[serde(default)]
    pub buckets: Option<u32>,
}

/// One page of `/audit/page`.
#[derive(Debug, Serialize)]
pub struct AuditPage {
    pub entries: Vec<AuditEntry>,
    /// Pass as `before` to get the next (older) page; `null` on the last page.
    pub next_cursor: Option<i64>,
    /// Matching entries across all pages; only when `total=true`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<i64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ActionsQuery {
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub workspace: Option<WorkspaceId>,
}

fn parse_outcome(s: &str) -> Option<AuditOutcome> {
    match s.trim().to_ascii_lowercase().as_str() {
        "success" | "allow" | "allowed" | "ok" => Some(AuditOutcome::Success),
        "denied" | "deny" => Some(AuditOutcome::Denied),
        "failure" | "error" | "failed" => Some(AuditOutcome::Failure),
        _ => None,
    }
}

/// Builds the store filter; `None` when the query can match nothing (unknown outcome).
fn filter_of(q: &AuditQuery, raw: Option<&str>, limit: u32) -> ApiResult<Option<AuditFilter>> {
    if let (Some(s), Some(u)) = (q.since, q.until)
        && s >= u
    {
        return Err(ApiError::bad_request("`since` must be before `until`"));
    }

    let actions: Vec<String> = url::form_urlencoded::parse(raw.unwrap_or_default().as_bytes())
        .filter(|(k, _)| k == "action")
        .flat_map(|(_, v)| v.split(',').map(str::trim).filter(|a| !a.is_empty()).map(str::to_owned).collect::<Vec<_>>())
        .take(50)
        .collect();

    let outcome = match q.outcome.as_deref().filter(|o| !o.trim().is_empty()) {
        None => None,
        Some(o) => match parse_outcome(o) {
            Some(v) => Some(v),
            None => return Ok(None),
        },
    };

    Ok(Some(AuditFilter {
        workspace: q.workspace,
        actions,
        actor: q.actor.clone(),
        outcome,
        resource: q.resource.clone(),
        text: q.q.as_deref().map(str::trim).filter(|t| !t.is_empty()).map(str::to_owned),
        since: q.since,
        until: q.until,
        before: q.before,
        limit,
    }))
}

async fn authorize(caller: &Caller, workspace: Option<WorkspaceId>) -> ApiResult<()> {
    let scope = match workspace {
        Some(o) => EntityRef::new(EntityKind::AuditLog, o),
        None => EntityRef::platform(),
    };

    caller.authorize(Action::AuditRead, scope).await?;

    Ok(())
}

/// Matching entries, newest first, as a plain array (the original shape).
pub async fn list(
    State(state): State<AppState>,
    caller: Caller,
    Query(q): Query<AuditQuery>,
    RawQuery(raw): RawQuery,
) -> ApiResult<Json<Vec<AuditEntry>>> {
    authorize(&caller, q.workspace).await?;
    let limit = q.limit.unwrap_or(100).clamp(1, 500);

    let Some(f) = filter_of(&q, raw.as_deref(), limit)? else {
        return Ok(Json(Vec::new()));
    };

    Ok(Json(state.store.search_entries(&f).await?))
}

/// One page of matching entries with a keyset cursor for the next page.
pub async fn page(
    State(state): State<AppState>,
    caller: Caller,
    Query(q): Query<AuditQuery>,
    RawQuery(raw): RawQuery,
) -> ApiResult<Json<AuditPage>> {
    authorize(&caller, q.workspace).await?;
    let limit = q.limit.unwrap_or(50).clamp(1, 200);

    let Some(mut f) = filter_of(&q, raw.as_deref(), limit)? else {
        let total = q.total.unwrap_or(false).then_some(0);
        return Ok(Json(AuditPage { entries: Vec::new(), next_cursor: None, total }));
    };

    // One extra row tells whether another page exists.
    f.limit = limit + 1;
    let mut entries = state.store.search_entries(&f).await?;
    let more = entries.len() > limit as usize;
    entries.truncate(limit as usize);
    let next_cursor = if more { entries.last().map(|e| e.seq) } else { None };
    let total = if q.total.unwrap_or(false) { Some(state.store.count_entries(&f).await?) } else { None };

    Ok(Json(AuditPage { entries, next_cursor, total }))
}

/// Matching entries over time by outcome, for the console chart.
pub async fn histogram(
    State(state): State<AppState>,
    caller: Caller,
    Query(q): Query<AuditQuery>,
    RawQuery(raw): RawQuery,
) -> ApiResult<Json<AuditHistogram>> {
    authorize(&caller, q.workspace).await?;
    let buckets = q.buckets.unwrap_or(60).clamp(1, forge_audit::MAX_BUCKETS);

    let Some(f) = filter_of(&q, raw.as_deref(), 1)? else {
        // An unknown outcome matches nothing: the same window, every bucket empty.
        let window = AuditFilter { workspace: q.workspace, since: q.since, until: q.until, ..AuditFilter::default() };
        let mut h = state.audit.histogram(&window, buckets).await?;

        for b in &mut h.buckets {
            (b.success, b.denied, b.failure) = (0, 0, 0);
        }

        return Ok(Json(h));
    };

    Ok(Json(state.audit.histogram(&f, buckets).await?))
}

/// Every distinct action in the log with its count, for filter pickers.
pub async fn actions(
    State(state): State<AppState>,
    caller: Caller,
    Query(q): Query<ActionsQuery>,
) -> ApiResult<Json<Vec<AuditActionCount>>> {
    authorize(&caller, q.workspace).await?;
    Ok(Json(state.store.distinct_actions(q.workspace).await?))
}

pub async fn verify(State(state): State<AppState>, caller: Caller) -> ApiResult<Json<VerifyReport>> {
    caller.authorize(Action::AuditRead, EntityRef::platform()).await?;
    Ok(Json(state.audit.verify().await?))
}
