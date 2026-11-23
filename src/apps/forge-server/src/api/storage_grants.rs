//! Bucket grants (D-108): one person's access to one bucket of a store,
//! given by someone who may manage the store. The rules live in
//! [`crate::bucket_grants`]; the object endpoints enforce them
//! ([`super::storage`]).
//!
//! * `GET    /api/v1/object-stores/{id}/grants?all=&limit=`: the grants on a
//!   store; live ones unless `all=true`.
//! * `POST   /api/v1/object-stores/{id}/grants`: `{user, bucket, access,
//!   lifetime_minutes | expires_at, note}`. The bucket must exist. Audited
//!   as `storage.grant.create`.
//! * `DELETE /api/v1/object-stores/{id}/grants/{grant_id}`: revokes a grant;
//!   the next request made through it is refused. Audited as
//!   `storage.grant.revoke`.
//! * `GET    /api/v1/object-stores/grants/mine`: the caller's live grants,
//!   reduced to what they open (no note, no issuer).
//!
//! The three store routes need `storage.manage` on the store (its node, or
//! the workspace for a connected store), which the MFA guardrail covers: so
//! does reading the list, like the list of everyone's tunnel grants. A grant
//! is not a role and cannot be passed on: holding one never lets a person
//! see or change grants.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use forge_core::storage::{BucketAccess, ObjectStore};
use forge_core::{Action, BucketGrantId, EntityRef, ObjectStoreId, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::storage as stores;
use crate::auth::{Caller, Ws};
use crate::bucket_grants::{AUDIT_CREATE, AUDIT_REVOKE, Expiry, GrantView, MyGrantView, NewBucketGrant};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/object-stores/grants/mine", get(mine))
        .route("/object-stores/{id}/grants", get(list).post(create))
        .route("/object-stores/{id}/grants/{grant_id}", axum::routing::delete(revoke))
}

/// A list of grants.
#[derive(Debug, Serialize)]
pub struct GrantList {
    pub grants: Vec<GrantView>,
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct GrantsQuery {
    /// Include expired and revoked grants.
    #[serde(default)]
    pub all: bool,
    /// Grants to return, newest first (default 200, at most 500).
    #[serde(default)]
    pub limit: Option<u32>,
}

/// The caller's own grants, in the reduced form their holder gets.
#[derive(Debug, Serialize)]
pub struct MyGrantList {
    pub grants: Vec<MyGrantView>,
}

/// Input kept in an audit event: no control characters, at most 128.
fn audit_text(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).take(128).collect()
}

/// Revokes every live grant for `bucket` of `store` and audits each as
/// `storage.grant.revoke` with `reason`: the bucket was deleted, or a bucket
/// of that name was created, so the grants must not reach the new one.
/// Returns how many were revoked. The caller has authorized the bucket
/// change itself; ending the grants of a bucket that is gone needs no more.
pub(super) async fn revoke_for_bucket(
    state: &AppState,
    caller: &Caller,
    store: &ObjectStore,
    target: &EntityRef,
    bucket: &str,
    reason: &'static str,
) -> ApiResult<usize> {
    let revoked = state.bucket_grants.revoke_bucket(store, bucket).await?;
    for g in &revoked {
        caller
            .audit(
                AUDIT_REVOKE,
                target,
                Some(store.workspace_id),
                json!({
                    "grant_id": g.id, "store_id": store.id, "name": store.name, "user_id": g.user_id,
                    "user": g.user, "bucket": g.bucket, "access": g.access, "reason": reason,
                }),
            )
            .await;
    }
    Ok(revoked.len())
}

/// The grants on a store. `storage.manage` (MFA) on the store.
pub async fn list(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Path(id): Path<ObjectStoreId>,
    Query(q): Query<GrantsQuery>,
) -> ApiResult<Json<GrantList>> {
    let (store, _) = stores::authorized(&state, &caller, ws, id, Action::StorageManage).await?;
    let grants = state.bucket_grants.on_store(&store, q.all, q.limit).await?;
    Ok(Json(GrantList { grants }))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NewGrantRequest {
    /// The person: a user id, a username or an email address.
    #[serde(alias = "user_id")]
    pub user: String,
    /// A bucket of this store. It must exist.
    pub bucket: String,
    /// `read` (list and download, the default) or `readwrite` (also upload
    /// and delete objects).
    #[serde(default = "default_access")]
    #[schemars(with = "super::schema_types::BucketAccess")]
    pub access: BucketAccess,
    /// Lifetime in minutes from now. Without it and without `expires_at`
    /// the grant never expires.
    #[serde(default)]
    pub lifetime_minutes: Option<u32>,
    /// When the grant ends, in milliseconds since the Unix epoch. Not
    /// together with `lifetime_minutes`.
    #[serde(default)]
    pub expires_at: Option<i64>,
    /// Why the grant exists (at most 200 characters).
    #[serde(default)]
    pub note: String,
}

const fn default_access() -> BucketAccess {
    BucketAccess::Read
}

fn expiry(req: &NewGrantRequest) -> ApiResult<Expiry> {
    match (req.lifetime_minutes, req.expires_at) {
        (Some(_), Some(_)) => Err(ApiError::bad_request("give lifetime_minutes or expires_at, not both")),
        (Some(minutes), None) => Ok(Expiry::In { minutes }),
        (None, Some(at)) => Ok(Expiry::At(Timestamp(at))),
        (None, None) => Ok(Expiry::Never),
    }
}

/// Grants one person access to one bucket. `storage.manage` (MFA) on the
/// store.
pub async fn create(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Path(id): Path<ObjectStoreId>,
    Json(req): Json<NewGrantRequest>,
) -> ApiResult<(StatusCode, Json<GrantView>)> {
    let (store, members) = stores::authorized(&state, &caller, ws, id, Action::StorageManage).await?;
    let target = stores::store_ref(&store, &members);
    let asked = json!({
        "store_id": store.id, "name": store.name, "user": audit_text(&req.user), "bucket": audit_text(&req.bucket),
        "access": req.access,
    });
    let grant = match issue(&state, &caller, &store, req).await {
        Ok(grant) => grant,
        Err(e) => {
            // A refused grant is on record too: who was named and for which
            // bucket, with the error's code, not its text.
            let error = format!("{} ({})", e.code, e.status.as_u16());
            caller.audit_failed(AUDIT_CREATE, &target, Some(ws), asked, &error).await;
            return Err(e);
        }
    };
    caller
        .audit(
            AUDIT_CREATE,
            &target,
            Some(ws),
            json!({
                "grant_id": grant.id, "store_id": store.id, "name": store.name, "user_id": grant.user_id,
                "user": grant.user, "bucket": grant.bucket, "access": grant.access,
                "expires_at": grant.expires_at, "never_expires": grant.never_expires, "note": grant.note,
            }),
        )
        .await;
    Ok((StatusCode::CREATED, Json(grant)))
}

/// Checks the request against the store and issues the grant.
async fn issue(state: &AppState, caller: &Caller, store: &ObjectStore, req: NewGrantRequest) -> ApiResult<GrantView> {
    let expiry = expiry(&req)?;
    // A grant names a bucket that is there: a typo is refused now, instead
    // of leaving a grant that opens whatever bucket gets that name later.
    let buckets = stores::client(state, store).await?.list_buckets().await.map_err(stores::s3_error)?;
    if !buckets.iter().any(|b| b.name == req.bucket) {
        return Err(ApiError::not_found(format!("no bucket `{}` in this store", req.bucket)));
    }
    let new = NewBucketGrant {
        user: req.user,
        bucket: req.bucket,
        access: req.access,
        expiry,
        note: req.note,
        created_by: caller.principal(),
    };
    Ok(state.bucket_grants.issue(store, new).await?)
}

/// Revokes a grant. `storage.manage` (MFA) on the store.
pub async fn revoke(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Path((id, grant_id)): Path<(ObjectStoreId, BucketGrantId)>,
) -> ApiResult<StatusCode> {
    let (store, members) = stores::authorized(&state, &caller, ws, id, Action::StorageManage).await?;
    let grant = state.bucket_grants.revoke(&store, grant_id).await?;
    caller
        .audit(
            AUDIT_REVOKE,
            &stores::store_ref(&store, &members),
            Some(ws),
            json!({
                "grant_id": grant.id, "store_id": store.id, "name": store.name, "user_id": grant.user_id,
                "user": grant.user, "bucket": grant.bucket, "access": grant.access, "reason": "revoked",
            }),
        )
        .await;
    Ok(StatusCode::NO_CONTENT)
}

/// The caller's own live grants: the buckets they may open, how and until
/// when, without who granted them or why. Every signed-in account may ask;
/// an AI connector gets an empty list, because grants do not count for it.
pub async fn mine(State(state): State<AppState>, caller: Caller, Ws(ws): Ws) -> ApiResult<Json<MyGrantList>> {
    let grants = if stores::grants_apply(&caller) {
        state.bucket_grants.mine(caller.session.user.id, ws).await?
    } else {
        Vec::new()
    };
    Ok(Json(MyGrantList { grants }))
}
