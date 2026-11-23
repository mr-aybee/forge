//! Observations (D-085): current health evidence and its history.
//!
//! - `GET /api/v1/observations?resource=<kind>:<id>[&server=<id>][&subject_prefix=<p>][&limit=<n>]`:
//!   current values of one resource, with the effective value computed on the
//!   server clock (a stale value reads as `unknown`).
//! - `GET /api/v1/observations/history?resource=<kind>:<id>&subject=<s>[&server=<id>][&since=<ms>][&limit=<n>]`:
//!   stored history of one key, newest first.
//!
//! Both need `health.view` on the resource. Read-only, so not audited.

use axum::Json;
use axum::extract::{Query, State};
use forge_core::observation::{
    Latest, ObservationFilter, ObservationKey, ObservationQuery, ObservedValue, ReasonCode, Subject,
};
use forge_core::{Action, EntityKind, EntityRef, ServerId, Timestamp};
use serde::{Deserialize, Serialize};

use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

const DEFAULT_LIMIT: u32 = 200;
const MAX_LIMIT: u32 = 1_000;

/// Resource kinds that carry observations.
const OBSERVABLE: [EntityKind; 3] = [EntityKind::Server, EntityKind::Service, EntityKind::Cluster];

fn parse_resource(s: &str) -> ApiResult<EntityRef> {
    let r: EntityRef = s.parse().map_err(|e: String| ApiError::bad_request(e))?;
    if !OBSERVABLE.contains(&r.kind) {
        return Err(ApiError::bad_request(format!(
            "resource kind `{}` has no observations; use server, service or cluster",
            r.kind.as_str()
        )));
    }
    Ok(r)
}

fn limit(n: Option<u32>) -> u32 {
    n.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListQuery {
    /// `<kind>:<id>`, e.g. `server:0192…` or `service:0192…`.
    pub resource: String,
    /// Only instances on this node.
    #[schemars(with = "Option<String>")]
    pub server: Option<ServerId>,
    /// Only subjects starting with this, e.g. `health.`.
    pub subject_prefix: Option<String>,
    pub limit: Option<u32>,
}

/// One current value as the API shows it.
#[derive(Debug, Serialize)]
pub struct ObservationView {
    #[serde(flatten)]
    pub latest: Latest,
    /// The value to act on now: `value` while fresh, `unknown` after.
    pub effective: ObservedValue,
    pub stale: bool,
}

impl ObservationView {
    fn new(latest: Latest, now: Timestamp) -> Self {
        let stale = !latest.is_fresh(now);
        let effective = latest.effective(now);
        Self { latest, effective, stale }
    }
}

#[derive(Debug, Serialize)]
pub struct ListResponse {
    pub now: Timestamp,
    pub observations: Vec<ObservationView>,
}

pub async fn list(
    State(state): State<AppState>,
    caller: Caller,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<ListResponse>> {
    let resource = parse_resource(&q.resource)?;
    caller.authorize(Action::HealthView, resource.clone()).await?;
    let filter = ObservationFilter { resource: Some(resource), server: q.server, subject_prefix: q.subject_prefix };
    let now = state.clock.now();
    let rows = ObservationQuery::latest(&state.store, &filter, limit(q.limit)).await?;
    Ok(Json(ListResponse { now, observations: rows.into_iter().map(|l| ObservationView::new(l, now)).collect() }))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct HistoryQuery {
    pub resource: String,
    pub subject: String,
    #[schemars(with = "Option<String>")]
    pub server: Option<ServerId>,
    /// Epoch ms; default: everything kept.
    #[serde(default)]
    pub since: i64,
    pub limit: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct HistoryEntry {
    pub value: ObservedValue,
    pub reason: Option<ReasonCode>,
    pub detail: Option<String>,
    pub generation: Option<String>,
    pub observed_at: Timestamp,
    pub received_at: Timestamp,
    pub skewed: bool,
}

#[derive(Debug, Serialize)]
pub struct HistoryResponse {
    pub key: ObservationKey,
    pub entries: Vec<HistoryEntry>,
}

pub async fn history(
    State(state): State<AppState>,
    caller: Caller,
    Query(q): Query<HistoryQuery>,
) -> ApiResult<Json<HistoryResponse>> {
    let resource = parse_resource(&q.resource)?;
    let subject = Subject::new(q.subject).map_err(|e| ApiError::bad_request(e.to_string()))?;
    caller.authorize(Action::HealthView, resource.clone()).await?;
    let key = ObservationKey { resource, server: q.server, subject };
    let rows = ObservationQuery::history(&state.store, &key, Timestamp(q.since), limit(q.limit)).await?;
    let entries = rows
        .into_iter()
        .map(|l| HistoryEntry {
            value: l.value,
            reason: l.reason,
            detail: l.detail,
            generation: l.generation,
            observed_at: l.observed_at,
            received_at: l.received_at,
            skewed: l.skewed,
        })
        .collect();
    Ok(Json(HistoryResponse { key, entries }))
}
