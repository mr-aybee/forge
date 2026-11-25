//! Tags on nodes, clusters, environments and services.
//!
//! Reading needs `view` on the item; changing needs the right that manages
//! it (`server.manage`, `workspace.manage`, or `deploy` on a service's
//! environment). Every change is audited with the old and new set.

use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use forge_core::ports::{ClusterRepository, ServerRepository, ServiceRepository, TenancyRepository};
use forge_core::tags::{TagKind, TagRepository, normalise_tags};
use forge_core::{Action, ClusterId, EntityKind, EntityRef, EnvironmentId, ServerId, ServiceId};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/tags", get(in_use))
        .route("/tags/{kind}", get(of_kind))
        .route("/tags/{kind}/{id}", get(of_item).put(set))
}

fn kind(s: &str) -> ApiResult<TagKind> {
    s.parse().map_err(ApiError::bad_request)
}

/// The entity a tag set belongs to, for authorization (services are
/// authorized on their environment).
async fn target(state: &AppState, kind: TagKind, id: &str) -> ApiResult<EntityRef> {
    let bad = || ApiError::bad_request(format!("invalid {} id", kind.as_str()));
    Ok(match kind {
        TagKind::Server => {
            let sid: ServerId = id.parse().map_err(|_| bad())?;
            state.store.get_server(sid).await.map_err(|_| ApiError::not_found("node not found"))?;
            EntityRef::new(EntityKind::Server, sid)
        }
        TagKind::Cluster => {
            let cid: ClusterId = id.parse().map_err(|_| bad())?;
            state.store.get_cluster(cid).await.map_err(|_| ApiError::not_found("cluster not found"))?;
            EntityRef::new(EntityKind::Cluster, cid)
        }
        TagKind::Environment => {
            let nid: EnvironmentId = id.parse().map_err(|_| bad())?;
            state.store.get_environment(nid).await.map_err(|_| ApiError::not_found("environment not found"))?;
            EntityRef::new(EntityKind::Environment, nid)
        }
        TagKind::Service => {
            let aid: ServiceId = id.parse().map_err(|_| bad())?;
            let app = state.store.get_service(aid).await?.ok_or_else(|| ApiError::not_found("service not found"))?;
            EntityRef::new(EntityKind::Environment, app.environment_id)
        }
    })
}

fn manage_action(kind: TagKind) -> Action {
    match kind {
        TagKind::Server => Action::ServerManage,
        TagKind::Cluster | TagKind::Environment => Action::WorkspaceManage,
        TagKind::Service => Action::Deploy,
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct KindQuery {
    pub kind: String,
}

/// Tags in use for a kind, with counts (for autocomplete and filters).
pub async fn in_use(
    State(state): State<AppState>,
    caller: Caller,
    Query(q): Query<KindQuery>,
) -> ApiResult<Json<Value>> {
    let k = kind(&q.kind)?;

    // Only count items the caller can see.
    let map = visible(&state, &caller, k).await?;
    let mut counts: HashMap<String, u64> = HashMap::new();

    for tags in map.values() {
        for t in tags {
            *counts.entry(t.clone()).or_default() += 1;
        }
    }

    let mut rows: Vec<Value> = counts.into_iter().map(|(tag, count)| json!({ "tag": tag, "count": count })).collect();
    rows.sort_by_key(|r| r["tag"].as_str().unwrap_or_default().to_lowercase());

    Ok(Json(Value::Array(rows)))
}

async fn visible(state: &AppState, caller: &Caller, k: TagKind) -> ApiResult<HashMap<String, Vec<String>>> {
    let all = state.store.tags_of_kind(k).await?;
    let mut out = HashMap::new();

    for (id, tags) in all {
        if let Ok(t) = target(state, k, &id).await
            && caller.can(Action::View, t).await
        {
            out.insert(id, tags);
        }
    }

    Ok(out)
}

/// `{ "<id>": ["tag", …] }` for every item of a kind the caller can see.
pub async fn of_kind(State(state): State<AppState>, caller: Caller, Path(k): Path<String>) -> ApiResult<Json<Value>> {
    let k = kind(&k)?;
    Ok(Json(json!(visible(&state, &caller, k).await?)))
}

pub async fn of_item(
    State(state): State<AppState>,
    caller: Caller,
    Path((k, id)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    let k = kind(&k)?;
    caller.authorize(Action::View, target(&state, k, &id).await?).await?;

    Ok(Json(json!({ "tags": state.store.tags_of(k, &id).await? })))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SetTags {
    pub tags: Vec<String>,
}

/// Replaces an item's tags.
pub async fn set(
    State(state): State<AppState>,
    caller: Caller,
    Path((k, id)): Path<(String, String)>,
    Json(req): Json<SetTags>,
) -> ApiResult<Json<Value>> {
    let k = kind(&k)?;
    let t = target(&state, k, &id).await?;
    caller.authorize(manage_action(k), t.clone()).await?;

    let tags = match normalise_tags(&req.tags) {
        Ok(t) => t,
        Err(e) => {
            caller.audit_denied("tags.update", &t, None, &e).await;
            return Err(ApiError::bad_request(e));
        }
    };

    let before = state.store.tags_of(k, &id).await?;
    state.store.set_tags(k, &id, &tags, state.clock.now()).await?;
    caller.audit("tags.update", &t, None, json!({ "kind": k.as_str(), "id": id, "from": before, "to": tags })).await;

    Ok(Json(json!({ "tags": tags })))
}
