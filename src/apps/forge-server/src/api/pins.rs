//! `/me/pins`: the caller's own pins (projects, services, nodes, clusters)
//! for the console sidebar.
//!
//! A user only reads and writes their own pins. Pinning needs `view` on the
//! entity; listing hides pins whose entity is gone or that the caller can no
//! longer view (they stay stored). Changes are audited as
//! `user.pin.add`, `user.pin.remove` and `user.pin.reorder`.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use forge_core::pins::{MAX_PINS, PinKind};
use forge_core::{Action, EntityKind, EntityRef, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::pins::{AUDIT_PIN_ADD, AUDIT_PIN_REMOVE, AUDIT_PIN_REORDER, PinError};
use crate::state::AppState;

/// One pin as the console shows it.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct PinItem {
    /// `project`, `service`, `node` or `cluster`.
    #[schemars(with = "String")]
    pub kind: PinKind,
    pub id: String,
    /// The entity's current name.
    pub label: String,
    /// Context: the environment of a service, the cluster of a node.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sublabel: Option<String>,
    pub position: i64,
    /// Epoch milliseconds.
    #[schemars(with = "i64")]
    pub created_at: Timestamp,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct PinList {
    pub items: Vec<PinItem>,
}

/// One entry of a reorder request.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PinRef {
    #[schemars(with = "String")]
    pub kind: PinKind,
    pub id: String,
}

/// The wanted order; pins not listed keep their order after these.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PinOrder {
    pub items: Vec<PinRef>,
}

fn parse_kind(s: &str) -> ApiResult<PinKind> {
    s.parse().map_err(|e: forge_core::DomainError| ApiError::bad_request(e.to_string()))
}

fn me(caller: &Caller) -> EntityRef {
    EntityRef::new(EntityKind::User, caller.session.user.id)
}

/// The caller's pins, in order, without the ones they cannot view.
pub async fn list(State(state): State<AppState>, caller: Caller) -> ApiResult<Json<PinList>> {
    let resolved = state.pins.list(caller.session.user.id).await?;
    let allowed = caller.can_many(Action::View, resolved.iter().map(|r| r.target.entity.clone()).collect()).await;
    let items = resolved
        .into_iter()
        .zip(allowed)
        .filter(|(_, ok)| *ok)
        .map(|(r, _)| PinItem {
            kind: r.pin.kind,
            id: r.pin.entity_id,
            label: r.target.label,
            sublabel: r.target.sublabel,
            position: r.pin.position,
            created_at: r.pin.created_at,
        })
        .collect();

    Ok(Json(PinList { items }))
}

/// Pins an entity the caller can view, at the end. Idempotent.
pub async fn add(
    State(state): State<AppState>,
    caller: Caller,
    Path((kind, id)): Path<(String, String)>,
) -> ApiResult<StatusCode> {
    let kind = parse_kind(&kind)?;
    let target = state.pins.target(kind, &id).await?;
    let visible = caller.can_many(Action::View, vec![target.entity.clone()]).await;
    if !visible.first().copied().unwrap_or(false) {
        caller.audit_denied(AUDIT_PIN_ADD, &target.entity, None, "caller cannot view the entity").await;

        // Same answer as a missing entity, so pinning does not reveal what exists.
        return Err(PinError::NotFound(kind).into());
    }

    if state.pins.add(caller.session.user.id, kind, &id).await? {
        caller.audit(AUDIT_PIN_ADD, &me(&caller), None, json!({ "kind": kind, "id": id })).await;
    }

    Ok(StatusCode::NO_CONTENT)
}

/// Unpins an entity. Idempotent.
pub async fn remove(
    State(state): State<AppState>,
    caller: Caller,
    Path((kind, id)): Path<(String, String)>,
) -> ApiResult<StatusCode> {
    let kind = parse_kind(&kind)?;
    if state.pins.remove(caller.session.user.id, kind, &id).await? {
        caller.audit(AUDIT_PIN_REMOVE, &me(&caller), None, json!({ "kind": kind, "id": id })).await;
    }

    Ok(StatusCode::NO_CONTENT)
}

/// Reorders the caller's pins. Unknown entries are ignored.
pub async fn reorder(
    State(state): State<AppState>,
    caller: Caller,
    Json(body): Json<PinOrder>,
) -> ApiResult<StatusCode> {
    // A user holds at most MAX_PINS; entries past a generous bound cannot
    // all be pins and are ignored like any other unknown entry.
    let wanted: Vec<(PinKind, String)> = body.items.into_iter().take(4 * MAX_PINS).map(|p| (p.kind, p.id)).collect();
    state.pins.reorder(caller.session.user.id, &wanted).await?;
    caller.audit(AUDIT_PIN_REORDER, &me(&caller), None, json!({ "count": wanted.len() })).await;

    Ok(StatusCode::NO_CONTENT)
}
