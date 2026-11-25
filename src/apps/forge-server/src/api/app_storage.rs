//! Object storage attached to a service (D-106).
//!
//! - `GET    /services/{id}/storage`: the buckets attached to the
//!   service and the variables each adds at deploy. `view` on the
//!   service's environment.
//! - `POST   /services/{id}/storage`: attaches a bucket of a managed
//!   store: issues a key limited to that bucket and records it for the
//!   service. `deploy` on the environment; and `storage.manage` (MFA) on
//!   the store unless the store belongs to that environment (D-107) and the
//!   bucket is not attached to a service of another environment. Audited as
//!   `service.storage.attach`.
//! - `DELETE /services/{id}/storage/{access_key}`: revokes the key and
//!   removes the attachment. `deploy` on the environment, whoever owns the
//!   store: it only takes access away from the caller's own service.
//!   Audited as `service.storage.detach`.
//!
//! The service gets the variables with its next deploy
//! (`crate::app_storage::deploy_env`); nothing is written to its own
//! environment, and the secret never leaves the vault.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use forge_core::model::{Service, VaultLink};
use forge_core::ports::VaultRepository;
use forge_core::storage::ObjectStoreRepository;
use forge_core::{Action, DomainError, EntityKind, EntityRef, ObjectStoreId, ServiceId};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::storage as stores;
use crate::app_storage::{self, Access, Attachment, DEFAULT_PREFIX, EnvNames};
use crate::auth::{Caller, Ws};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// Audit action of an attach.
pub const AUDIT_ATTACH: &str = "service.storage.attach";
/// Audit action of a detach.
pub const AUDIT_DETACH: &str = "service.storage.detach";

fn ns_of(app: &Service) -> EntityRef {
    EntityRef::new(EntityKind::Environment, app.environment_id)
}

fn app_ref(id: ServiceId) -> EntityRef {
    EntityRef::new(EntityKind::Service, id)
}

/// One attachment as the console shows it.
#[derive(Debug, Serialize)]
pub struct AttachmentView {
    #[serde(flatten)]
    pub attachment: Attachment,
    /// The store's name; `null` when the store no longer exists.
    pub store_name: Option<String>,
    /// The S3 endpoint the service is given.
    pub endpoint: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct StorageView {
    pub attachments: Vec<AttachmentView>,
    /// The caller may attach and detach here (`deploy` on the environment).
    /// A store of another environment, or a shared one, also needs
    /// `storage.manage`, checked per store.
    pub can_change: bool,
}

async fn view(state: &AppState, caller: &Caller, app: &Service) -> ApiResult<StorageView> {
    let mut attachments = Vec::new();

    for (attachment, _) in app_storage::attachments(&state.store, app).await? {
        let store = state.store.get_object_store(attachment.store_id).await?;
        attachments.push(AttachmentView {
            store_name: store.as_ref().map(|s| s.name.clone()),
            endpoint: store.map(|s| s.endpoint),
            attachment,
        });
    }

    Ok(StorageView { attachments, can_change: caller.can(Action::Deploy, ns_of(app)).await })
}

pub async fn list(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServiceId>,
) -> ApiResult<Json<StorageView>> {
    let app = super::services::load_own(&state, id).await?;
    caller.authorize(Action::View, ns_of(&app)).await?;

    Ok(Json(view(&state, &caller, &app).await?))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AttachRequest {
    /// A managed store (access keys are issued for managed RustFS stores).
    #[schemars(with = "String")]
    pub store_id: ObjectStoreId,
    pub bucket: String,
    /// Create the bucket when it doesn't exist yet.
    #[serde(default)]
    pub create_bucket: bool,
    /// `read` or `readwrite` (default).
    #[serde(default = "default_access")]
    #[schemars(with = "super::schema_types::BucketAccess")]
    pub access: Access,
    /// Prefix of the injected variables (default `S3`): `<PREFIX>_ENDPOINT`,
    /// `_REGION`, `_BUCKET`, `_ACCESS_KEY_ID`, `_SECRET_ACCESS_KEY`.
    #[serde(default = "default_prefix")]
    pub prefix: String,
}

const fn default_access() -> Access {
    Access::ReadWrite
}

fn default_prefix() -> String {
    DEFAULT_PREFIX.to_owned()
}

/// A name for the store's key: the service's name, within the 32
/// characters a key name may have.
fn key_name(app: &Service) -> String {
    app.name.chars().take(32).collect()
}

pub async fn attach(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Path(id): Path<ServiceId>,
    Json(req): Json<AttachRequest>,
) -> ApiResult<(StatusCode, Json<StorageView>)> {
    let app = super::services::load_own(&state, id).await?;
    caller.authorize(Action::Deploy, ns_of(&app)).await?;
    let (store, members) = stores::authorized_for_attach(&state, &caller, ws, req.store_id, &app, &req.bucket).await?;
    stores::keys_supported(&store)?;

    if !forge_s3::valid_bucket(&req.bucket) {
        return Err(ApiError::bad_request(
            "bucket name: 3-63 lowercase letters, digits, dots and hyphens, starting and ending with a letter or digit",
        ));
    }

    if !app_storage::valid_prefix(&req.prefix) {
        return Err(ApiError::bad_request(
            "prefix: an uppercase letter, then uppercase letters, digits and underscores (at most 24), not ending in an underscore",
        ));
    }

    // The variables must be free: in the service's own environment and
    // among its other attachments.
    let names = EnvNames::of(&req.prefix);
    if let Some(taken) = names.all().into_iter().find(|n| app.env.iter().any(|e| e.key == *n)) {
        return Err(ApiError::conflict(format!(
            "the service already defines `{taken}`; choose another prefix or remove that variable"
        )));
    }

    if app_storage::attachments(&state.store, &app).await?.iter().any(|(a, _)| a.prefix == req.prefix) {
        return Err(ApiError::conflict(format!(
            "another bucket is attached with the prefix `{}`; choose another prefix",
            req.prefix
        )));
    }

    let client = stores::client(&state, &store).await?;
    if req.create_bucket && client.list_objects(&req.bucket, "", None, 1).await.is_err() {
        client.create_bucket(&req.bucket).await.map_err(stores::s3_error)?;

        // A new bucket: bucket grants left for this name do not carry over.
        let target = stores::store_ref(&store, &members);
        let reason = crate::bucket_grants::REASON_BUCKET_RECREATED;
        super::storage_grants::revoke_for_bucket(&state, &caller, &store, &target, &req.bucket, reason).await?;
    }

    // The bucket must exist, so a typo does not create a useless key.
    client.list_objects(&req.bucket, "", None, 1).await.map_err(stores::s3_error)?;
    let (access_key, secret_key) = client
        .add_service_account(
            &key_name(&app),
            &format!("Forge key of service {} for bucket {}", app.name, req.bucket),
            &forge_s3::bucket_policy(&req.bucket, req.access.writes()),
        )
        .await
        .map_err(stores::s3_error)?;

    // Two steps that can't be one transaction (the store, then the vault):
    // when recording the key fails, the key is revoked again.
    let recorded = async {
        let mut links: Vec<VaultLink> =
            members.iter().map(|m| VaultLink { kind: "node".into(), id: m.server_id.to_string() }).collect();
        links.push(VaultLink { kind: "service".into(), id: app.id.to_string() });
        let item_id = stores::vault_credential(
            &state,
            &caller,
            ws,
            format!("{} / {} ({})", store.name, req.bucket, app.name),
            &access_key,
            &secret_key,
            &store.endpoint,
            "Access key of a service, limited to one bucket, issued by Forge Object Storage.",
            links,
        )
        .await?;
        let mut item = state
            .store
            .get_vault_item(item_id)
            .await?
            .ok_or_else(|| DomainError::internal("the new key's vault item is missing"))?;
        item.tags = vec![
            "object-storage".into(),
            format!("{}{}", app_storage::TAG_STORE, store.id),
            format!("{}{}", app_storage::TAG_BUCKET, req.bucket),
            format!("{}{}", app_storage::TAG_ACCESS, req.access.as_str()),
        ];
        item.tags.extend(app_storage::attachment_tags(app.id, &req.prefix));
        state.store.put_vault_item(&item).await?;
        ApiResult::Ok(())
    }
    .await;

    if let Err(e) = recorded {
        if let Err(revoke) = client.delete_service_account(&access_key).await {
            tracing::warn!(store = %store.id, access_key, error = %revoke, "could not revoke a key that was not recorded");
        }

        return Err(e);
    }

    caller
        .audit(
            AUDIT_ATTACH,
            &app_ref(app.id),
            Some(app.workspace_id),
            json!({ "service": app.name, "store_id": store.id, "store": store.name, "bucket": req.bucket,
                    "access": req.access, "prefix": req.prefix, "access_key": access_key,
                    "bucket_created": req.create_bucket }),
        )
        .await;

    Ok((StatusCode::CREATED, Json(view(&state, &caller, &app).await?)))
}

pub async fn detach(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Path((id, access_key)): Path<(ServiceId, String)>,
) -> ApiResult<Json<StorageView>> {
    let app = super::services::load_own(&state, id).await?;
    caller.authorize(Action::Deploy, ns_of(&app)).await?;
    let (attachment, item) = app_storage::attachments(&state.store, &app)
        .await?
        .into_iter()
        .find(|(a, _)| a.access_key == access_key)
        .ok_or_else(|| ApiError::not_found("no bucket is attached to this service with that key"))?;

    // `deploy` on the service's environment is all a detach needs, whoever
    // owns the store: it revokes the service's own key and nothing else.
    // A store that is gone has no key left to revoke: only the record goes.
    if let Some(store) = state.store.get_object_store(attachment.store_id).await?.filter(|s| s.workspace_id == ws) {
        stores::client(&state, &store).await?.delete_service_account(&access_key).await.map_err(stores::s3_error)?;
    }

    state.store.delete_vault_item(item.id).await?;
    caller
        .audit(
            AUDIT_DETACH,
            &app_ref(app.id),
            Some(app.workspace_id),
            json!({ "service": app.name, "store_id": attachment.store_id, "bucket": attachment.bucket,
                    "prefix": attachment.prefix, "access_key": access_key }),
        )
        .await;

    Ok(Json(view(&state, &caller, &app).await?))
}
