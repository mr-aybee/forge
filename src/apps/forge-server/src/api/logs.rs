//! Service logs (D-128).
//!
//! - `POST /api/events/raw`: Seq-compatible ingest. CLEF lines (`?clef`)
//!   or `{"Events":[...]}`; the key is the `X-Seq-ApiKey` header.
//! - `GET  /api/v1/logs`: events newest first, paged with `before` /
//!   `after` cursors. `log.read` on each service; container lines that
//!   belong to no service need `view` on their node.
//! - `GET  /api/v1/logs/histogram`: counts per level over a window.
//! - `GET|POST /api/v1/services/{id}/log-keys`, `DELETE
//!   /api/v1/log-keys/{id}`: ingest keys. `log.manage`, audited.

use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{delete, get};
use axum::{Json, Router};
use forge_core::logs::{LogCursor, LogFilter, LogKey, LogLevel, LogPage, LogQuery, LogSource, LogStream};
use forge_core::{Action, EntityKind, EntityRef, LogKeyId, Principal, ServerId, ServiceId, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::logs::{AUDIT_KEY_CREATE, AUDIT_KEY_REVOKE, CreatedKey, LogsError};
use crate::state::AppState;
use forge_core::analytics::Histogram;

/// Default and largest page.
const DEFAULT_LIMIT: u32 = 100;
const MAX_LIMIT: u32 = 500;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/logs", get(events))
        .route("/logs/histogram", get(histogram))
        .route("/services/{id}/log-keys", get(list_keys).post(create_key))
        .route("/log-keys/{id}", delete(revoke_key))
}

fn app_ref(id: ServiceId) -> EntityRef {
    EntityRef::new(EntityKind::Service, id)
}

fn logs_error(e: LogsError) -> ApiError {
    match e {
        LogsError::BadKey => {
            ApiError::new(StatusCode::UNAUTHORIZED, "invalid_api_key", "the log ingest key is missing or revoked")
        }
        LogsError::Analytics(e) => e.into(),
        LogsError::Invalid(m) => ApiError::bad_request(m),
        LogsError::Domain(e) => e.into(),
    }
}

/// Query string of the event list and the histogram.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct LogsQuery {
    /// One service (kept for older links; same as `services=<id>`).
    #[schemars(with = "Option<String>")]
    pub service: Option<ServiceId>,
    /// Comma-separated service ids: every event of these services.
    pub services: Option<String>,
    /// Comma-separated `<service id>:<node id>` pairs: only events of that
    /// service that came from that node. OR'ed with `services`; with
    /// neither, every service the caller may read.
    pub instances: Option<String>,
    /// Comma-separated node ids: only events from these nodes.
    pub node: Option<String>,
    /// Container name, or a prefix of its id.
    pub container: Option<String>,
    /// Stack name (`<environment>/<stack>`, or a compose project).
    pub stack: Option<String>,
    /// `ingest` (sent by services) or `container` (collected from Docker).
    pub source: Option<String>,
    /// Comma-separated level names; omitted: every level.
    pub levels: Option<String>,
    /// Search: free text plus `Name="value"` property filters.
    pub q: Option<String>,
    /// Window start and end, epoch ms.
    pub from: Option<i64>,
    pub to: Option<i64>,
    /// Older than this cursor (the `next` of a previous page).
    pub before: Option<String>,
    /// Newer than this cursor (the `newest` of a previous page): live tail.
    pub after: Option<String>,
    /// Events per page, 1 to 500 (default 100).
    pub limit: Option<u32>,
}

/// One event as the console shows it.
#[derive(Debug, Serialize)]
pub struct LogEventView {
    pub id: forge_core::logs::EventId,
    /// `null` for a container line that belongs to no service.
    pub service_id: Option<ServiceId>,
    /// The service's name (`null` with `service_id`).
    pub service: Option<String>,
    pub ts: i64,
    pub node_ts: Option<i64>,
    pub ingest_ts: i64,
    pub level: LogLevel,
    pub message: String,
    pub message_template: String,
    pub exception: Option<String>,
    pub trace_id: Option<String>,
    pub span_id: Option<String>,
    /// Eight hex digits.
    pub event_type: String,
    /// The node that sent the event, when Forge could tell (`null` otherwise).
    pub server_id: Option<forge_core::ServerId>,
    /// That node's console name.
    pub node: Option<String>,
    pub properties: serde_json::Map<String, serde_json::Value>,
    /// `ingest` or `container`.
    pub source: LogSource,
    /// Container lines: short container id, name, stack and stream.
    pub container_id: Option<String>,
    pub container_name: Option<String>,
    pub stack: Option<String>,
    pub stream: Option<LogStream>,
}

/// Most services or nodes one query may name.
const MAX_SELECTED: usize = 200;

fn id_list<T: std::str::FromStr>(s: Option<&str>, what: &'static str) -> ApiResult<Vec<T>> {
    let parts: Vec<&str> = s.unwrap_or_default().split(',').map(str::trim).filter(|p| !p.is_empty()).collect();
    if parts.len() > MAX_SELECTED {
        return Err(ApiError::bad_request(format!("`{what}` names at most {MAX_SELECTED} entries")));
    }
    parts
        .into_iter()
        .map(|p| p.parse().map_err(|_| ApiError::bad_request(format!("`{what}`: invalid id `{p}`"))))
        .collect()
}

fn instance_list(s: Option<&str>) -> ApiResult<Vec<(ServiceId, forge_core::ServerId)>> {
    let pairs: Vec<String> = id_list::<String>(s, "instances")?;
    pairs
        .iter()
        .map(|p| {
            let (a, n) =
                p.split_once(':').ok_or_else(|| ApiError::bad_request("`instances`: use `<service id>:<node id>`"))?;
            Ok((
                a.parse().map_err(|_| ApiError::bad_request(format!("`instances`: invalid service id `{a}`")))?,
                n.parse().map_err(|_| ApiError::bad_request(format!("`instances`: invalid node id `{n}`")))?,
            ))
        })
        .collect()
}

#[derive(Debug, Serialize)]
pub struct LogPageView {
    /// Newest first.
    pub events: Vec<LogEventView>,
    /// Pass as `before` for older events; `null` at the end of the log.
    pub next: Option<String>,
    /// Pass as `after` for events newer than this page.
    pub newest: Option<String>,
}

fn cursor(s: Option<&str>) -> ApiResult<Option<LogCursor>> {
    s.filter(|s| !s.is_empty())
        .map(|s| s.parse::<LogCursor>().map_err(|_| ApiError::bad_request("invalid cursor")))
        .transpose()
}

fn levels(s: Option<&str>) -> ApiResult<Vec<LogLevel>> {
    let mut out = Vec::new();
    for part in s.unwrap_or_default().split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let l = part.parse::<LogLevel>().map_err(ApiError::bad_request)?;
        if !out.contains(&l) {
            out.push(l);
        }
    }
    Ok(out)
}

/// The services the caller may read logs of, with their names.
async fn readable(state: &AppState, caller: &Caller, one: Option<ServiceId>) -> ApiResult<HashMap<ServiceId, String>> {
    if let Some(id) = one {
        caller.authorize(Action::LogRead, app_ref(id)).await?;
    }
    let apps = state.logs.services(one).await.map_err(logs_error)?;
    if one.is_some() && apps.is_empty() {
        return Err(ApiError::not_found("service not found"));
    }
    let allowed = caller.can_many(Action::LogRead, apps.iter().map(|a| app_ref(a.id)).collect()).await;
    Ok(apps.into_iter().zip(allowed).filter(|(_, ok)| *ok).map(|(a, _)| (a.id, a.name)).collect())
}

/// Longest `container` or `stack` filter.
const MAX_FILTER: usize = 256;

fn text_filter(s: Option<&str>, what: &'static str) -> ApiResult<Option<String>> {
    let Some(v) = s.map(str::trim).filter(|v| !v.is_empty()) else { return Ok(None) };
    if v.len() > MAX_FILTER || v.chars().any(char::is_control) {
        return Err(ApiError::bad_request(format!("`{what}` is at most {MAX_FILTER} characters")));
    }
    Ok(Some(v.to_owned()))
}

fn source(s: Option<&str>) -> ApiResult<Option<LogSource>> {
    s.map(str::trim)
        .filter(|v| !v.is_empty())
        .map(|v| LogSource::parse(v).ok_or_else(|| ApiError::bad_request("`source` is `ingest` or `container`")))
        .transpose()
}

/// The nodes whose unattributed container lines the caller may read
/// (`view` on the node), limited to `only` when it names any.
async fn open_nodes(state: &AppState, caller: &Caller, only: &[ServerId]) -> ApiResult<Vec<ServerId>> {
    let ids: Vec<ServerId> =
        if only.is_empty() { state.logs.node_ids().await.map_err(logs_error)? } else { only.to_vec() };
    let allowed =
        caller.can_many(Action::View, ids.iter().map(|id| EntityRef::new(EntityKind::Server, *id)).collect()).await;
    Ok(open_scope(true, ids, &allowed))
}

/// The nodes whose unattributed lines a query reads: none when the query
/// names services or instances (`everything` false), else each candidate
/// the caller may view (`allowed`, in the same order), sorted.
fn open_scope(everything: bool, candidates: Vec<ServerId>, allowed: &[bool]) -> Vec<ServerId> {
    if !everything {
        return Vec::new();
    }
    let mut out: Vec<ServerId> =
        candidates.into_iter().zip(allowed.iter()).filter(|(_, ok)| **ok).map(|(id, _)| id).collect();
    out.sort_unstable();
    out.dedup();
    out
}

async fn build_query(
    state: &AppState,
    caller: &Caller,
    q: &LogsQuery,
) -> ApiResult<(LogQuery, HashMap<ServiceId, String>)> {
    let mut whole: Vec<ServiceId> = id_list(q.services.as_deref(), "services")?;
    whole.extend(q.service);
    let instances = instance_list(q.instances.as_deref())?;
    // A single named service keeps the old behaviour (403 when not allowed).
    let one = match (whole.as_slice(), instances.is_empty()) {
        ([id], true) => Some(*id),
        _ => None,
    };
    let names = readable(state, caller, one).await?;
    // Every named service must be one the caller may read; the answer is the
    // same as for a service that does not exist.
    if whole.iter().chain(instances.iter().map(|(a, _)| a)).any(|a| !names.contains_key(a)) {
        return Err(ApiError::not_found("service not found"));
    }
    let before = cursor(q.before.as_deref())?;
    let after = cursor(q.after.as_deref())?;
    let page = match (before, after) {
        (Some(_), Some(_)) => return Err(ApiError::bad_request("use `before` or `after`, not both")),
        (b, None) => LogPage::Older(b),
        (None, Some(a)) => LogPage::Newer(a),
    };
    let nodes: Vec<ServerId> = id_list(q.node.as_deref(), "node")?;
    // Unattributed container lines join only a query that names no service.
    let everything = whole.is_empty() && instances.is_empty();
    let open = if everything { open_nodes(state, caller, &nodes).await? } else { Vec::new() };
    let mut services = if everything { names.keys().copied().collect() } else { whole };
    services.sort_unstable();
    services.dedup();
    let mut instances = instances;
    instances.sort_unstable();
    instances.dedup();
    Ok((
        LogQuery {
            services,
            instances,
            open_nodes: open,
            nodes,
            container: text_filter(q.container.as_deref(), "container")?,
            stack: text_filter(q.stack.as_deref(), "stack")?,
            source: source(q.source.as_deref())?,
            levels: levels(q.levels.as_deref())?,
            filter: LogFilter::parse(q.q.as_deref().unwrap_or_default()),
            from: q.from.map(Timestamp),
            to: q.to.map(Timestamp),
            page,
            limit: q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT),
            // Set by the analytics worker from the runtime settings.
            limits: forge_core::logs::SearchLimits::default(),
        },
        names,
    ))
}

/// `GET /api/v1/logs`
pub async fn events(
    State(state): State<AppState>,
    caller: Caller,
    Query(q): Query<LogsQuery>,
) -> ApiResult<Json<LogPageView>> {
    let (query, names) = build_query(&state, &caller, &q).await?;
    let events = state.analytics.query_logs(&query).await?;
    let full = u32::try_from(events.len()).unwrap_or(u32::MAX) >= query.limit;
    let next = match query.page {
        LogPage::Older(_) if full => events.last().map(|e| e.cursor().to_string()),
        _ => None,
    };
    let newest = events.first().map(|e| e.cursor().to_string()).or_else(|| match query.page {
        // An empty tail keeps the caller's position.
        LogPage::Newer(c) => Some(c.to_string()),
        LogPage::Older(_) => None,
    });
    let mut nodes: Vec<forge_core::ServerId> = events.iter().filter_map(|e| e.server_id).collect();
    nodes.sort_unstable();
    nodes.dedup();
    let node_names = state.logs.node_names(&nodes).await;
    let events = events
        .into_iter()
        .map(|e| LogEventView {
            node: e.server_id.and_then(|id| node_names.get(&id).cloned()),
            server_id: e.server_id,
            service: e.service_id.and_then(|id| names.get(&id).cloned()),
            id: e.id,
            service_id: e.service_id,
            ts: e.ts.0,
            node_ts: e.node_ts.map(|t| t.0),
            ingest_ts: e.ingest_ts.0,
            level: e.level,
            message: e.message,
            message_template: e.message_template,
            exception: e.exception,
            trace_id: e.trace_id,
            span_id: e.span_id,
            event_type: format!("{:08x}", e.event_type),
            properties: e.properties,
            source: e.source,
            container_id: e.container_id,
            container_name: e.container_name,
            stack: e.stack,
            stream: e.stream,
        })
        .collect();
    Ok(Json(LogPageView { events, next, newest }))
}

/// `GET /api/v1/logs/histogram`
pub async fn histogram(
    State(state): State<AppState>,
    caller: Caller,
    Query(q): Query<LogsQuery>,
) -> ApiResult<Json<Histogram>> {
    let (query, _) = build_query(&state, &caller, &q).await?;
    Ok(Json(state.analytics.log_histogram(&query).await?))
}

/// `GET /api/v1/services/{id}/log-keys`
pub async fn list_keys(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServiceId>,
) -> ApiResult<Json<Vec<LogKey>>> {
    caller.authorize(Action::LogManage, app_ref(id)).await?;
    Ok(Json(state.logs.list_keys(id).await.map_err(logs_error)?))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct NewLogKey {
    /// What sends with this key, e.g. `api-prod`. 1 to 80 characters.
    pub name: String,
}

/// `POST /api/v1/services/{id}/log-keys`: the token is in this response only.
pub async fn create_key(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServiceId>,
    Json(body): Json<NewLogKey>,
) -> ApiResult<(StatusCode, Json<CreatedKey>)> {
    caller.authorize(Action::LogManage, app_ref(id)).await?;
    let app = state.logs.services(Some(id)).await.map_err(logs_error)?.into_iter().next();
    let app = app.ok_or_else(|| ApiError::not_found("service not found"))?;
    let Principal::User(user) = caller.principal();
    let created = state.logs.create_key(id, &body.name, Some(user)).await.map_err(logs_error)?;
    caller
        .audit(
            AUDIT_KEY_CREATE,
            &app_ref(id),
            Some(app.workspace_id),
            json!({ "service": app.name, "key_id": created.key.id, "name": created.key.name, "prefix": created.key.prefix }),
        )
        .await;
    Ok((StatusCode::CREATED, Json(created)))
}

/// `DELETE /api/v1/log-keys/{id}`
pub async fn revoke_key(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<LogKeyId>,
) -> ApiResult<StatusCode> {
    let key = state.logs.get_key(id).await.map_err(logs_error)?.ok_or_else(|| ApiError::not_found("no such key"))?;
    caller.authorize(Action::LogManage, app_ref(key.service_id)).await?;
    let workspace =
        state.logs.services(Some(key.service_id)).await.map_err(logs_error)?.first().map(|a| a.workspace_id);
    state.logs.revoke_key(id).await.map_err(logs_error)?;
    caller
        .audit(
            AUDIT_KEY_REVOKE,
            &app_ref(key.service_id),
            workspace,
            json!({ "key_id": key.id, "name": key.name, "prefix": key.prefix }),
        )
        .await;
    Ok(StatusCode::NO_CONTENT)
}

/// Seq's answer to a stored batch.
#[derive(Debug, Serialize)]
pub struct IngestResponse {
    #[serde(rename = "MinimumLevelAccepted")]
    pub minimum_level_accepted: Option<LogLevel>,
}

/// `POST /api/events/raw`: the Seq ingestion endpoint. Not under `/api/v1`
/// and without a session: the ingest key is the only credential, and it can
/// only add events to its own service.
pub async fn ingest(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> ApiResult<(StatusCode, Json<IngestResponse>)> {
    let token = headers.get("x-seq-apikey").and_then(|v| v.to_str().ok()).unwrap_or_default().trim();
    state.logs.ingest(token, &body).await.map_err(logs_error)?;
    Ok((StatusCode::CREATED, Json(IngestResponse { minimum_level_accepted: None })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unattributed_lines_need_view_on_their_node() {
        let (a, b, c) = (ServerId::new(), ServerId::new(), ServerId::new());
        let open = open_scope(true, vec![a, b, c, a], &[true, false, true, true]);
        assert!(open.contains(&a) && open.contains(&c), "viewable nodes");
        assert!(!open.contains(&b), "a node the caller may not view stays closed");
        assert_eq!(open.len(), 2);
        assert!(open_scope(false, vec![a], &[true]).is_empty(), "a query that names services reads no node");
        assert!(open_scope(true, vec![a, b], &[]).is_empty(), "no answer means no access");
    }

    #[test]
    fn filters_are_checked() {
        assert_eq!(source(Some("container")).unwrap_or_default(), Some(LogSource::Container));
        assert!(source(Some("syslog")).is_err());
        assert_eq!(text_filter(Some("  web-1 "), "container").unwrap_or_default().as_deref(), Some("web-1"));
        assert!(text_filter(Some(&"x".repeat(MAX_FILTER + 1)), "stack").is_err());
        assert!(id_list::<ServerId>(Some("nope"), "node").is_err());
    }
}
