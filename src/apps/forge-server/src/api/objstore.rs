//! Object storage (D-170): RustFS containers found on nodes, adopting any of
//! them, settings after install, and access keys kept in RustFS's IAM. The
//! handlers parse, authorize, call `crate::objstore` and audit.
//!
//! * `GET /object-stores/discover[?server_id]`: nodes the caller may manage
//!   (`server.manage`), their Docker state and RustFS containers.
//! * `GET /object-stores/rustfs-env`: the engine variables an operator may set.
//! * `GET|PUT /object-stores/{id}/settings` (`storage.manage`, MFA).
//! * `GET /object-stores/{id}/iam`, `POST …/iam/keys`,
//!   `PUT …/iam/keys/{access_key}`, `POST …/iam/reconcile` (`storage.manage`).

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use forge_core::model::{Job, NewJob, VaultLink};
use forge_core::ports::{ServerRepository, VaultRepository};
use forge_core::storage::{MODE_MANAGED, ObjectStore, StoreMember};
use forge_core::storage_iam::{IamError, KeyGrant};
use forge_core::storage_settings::ENV_ALLOWLIST;
use forge_core::{Action, DomainError, EntityKind, EntityRef, EnvironmentId, ObjectStoreId, ServerId, VaultItemId};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::storage::{authorized, client, store_ref, vault_credential};
use crate::auth::{Caller, Ws};
use crate::error::{ApiError, ApiResult};
use crate::jobs::storage::PreviousSettings;
use crate::objstore::adopt::AdoptPlan;
use crate::objstore::discovery::DiscoveryReport;
use crate::objstore::iam::{IamView, KeyError, ReconcileReport};
use crate::objstore::settings::{SettingsUpdate, SettingsView};
use crate::state::AppState;

pub const AUDIT_SETTINGS: &str = "storage.settings.update";
pub const AUDIT_KEY_CREATE: &str = "storage.key.create";
pub const AUDIT_KEY_POLICY: &str = "storage.key.policy";
pub const AUDIT_RECONCILE: &str = "storage.iam.reconcile";
pub const AUDIT_ADOPT: &str = "storage.adopt";

fn server_ref(id: ServerId) -> EntityRef {
    EntityRef::new(EntityKind::Server, id)
}

impl From<KeyError> for ApiError {
    fn from(e: KeyError) -> Self {
        match e {
            KeyError::Invalid(m) => Self::bad_request(m),
            KeyError::Iam(IamError::Unreachable(m)) => Self::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "store_unreachable",
                format!("RustFS cannot be reached, so its keys cannot change now: {m}"),
            ),
            KeyError::Iam(IamError::NotFound(m)) => Self::not_found(m),
            KeyError::Iam(IamError::Rejected(m)) => Self::new(StatusCode::BAD_GATEWAY, "store_refused", m),
            KeyError::Domain(d) => d.into(),
        }
    }
}

// -------------------------------------------------------------- discovery

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DiscoverQuery {
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub server_id: Option<ServerId>,
}

pub async fn discover(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Query(q): Query<DiscoverQuery>,
) -> ApiResult<Json<DiscoveryReport>> {
    let mut servers: Vec<_> = state
        .store
        .list_servers(Some(ws))
        .await?
        .into_iter()
        .filter(|s| q.server_id.is_none_or(|id| id == s.id))
        .collect();

    if let Some(id) = q.server_id {
        if servers.is_empty() {
            return Err(ApiError::not_found("server not found"));
        }

        caller.authorize(Action::ServerManage, server_ref(id)).await?;
    } else {
        let allowed = caller.can_many(Action::ServerManage, servers.iter().map(|s| server_ref(s.id)).collect()).await;
        let mut it = allowed.into_iter();
        servers.retain(|_| it.next().unwrap_or(false));
    }

    servers.truncate(MAX_SCAN);

    Ok(Json(state.objstore_discovery.scan(&servers, &caller.principal().to_string()).await))
}

/// Nodes one discovery scans at most.
const MAX_SCAN: usize = 200;

#[derive(Debug, Serialize)]
pub struct EnvAllowlist {
    pub keys: Vec<&'static str>,
}

pub async fn rustfs_env(_caller: Caller) -> Json<EnvAllowlist> {
    Json(EnvAllowlist { keys: ENV_ALLOWLIST.to_vec() })
}

// --------------------------------------------------------------- adoption

/// Adopt any RustFS container (the `container` field of the adopt request).
#[derive(Debug, Clone)]
pub struct AdoptContainer {
    pub server_id: ServerId,
    pub name: String,
    pub container: String,
    pub owner: Option<EnvironmentId>,
    pub endpoint_host: String,
    pub s3_port: Option<u16>,
    pub console_port: Option<u16>,
}

/// Records an adopted container as a store and starts the `adopt` job,
/// which reads the root credential on the node and seals it in the vault.
pub async fn adopt_container(
    state: &AppState,
    caller: &Caller,
    ws: forge_core::WorkspaceId,
    req: AdoptContainer,
) -> ApiResult<(ObjectStore, StoreMember, Job)> {
    let server = state.store.get_server(req.server_id).await.map_err(|_| ApiError::not_found("server not found"))?;
    if server.workspace_id != ws {
        return Err(ApiError::not_found("server not found"));
    }

    let (node, found) = state.objstore_discovery.scan_node(&server, &caller.principal().to_string()).await;
    let found = found.into_iter().find(|c| c.name == req.container).ok_or_else(|| match node.error {
        Some(e) => ApiError::conflict(format!("cannot read the node's containers: {e}")),
        None => ApiError::not_found(format!("no RustFS container named {} on this node", req.container)),
    })?;
    let plan = AdoptPlan {
        workspace: ws,
        server: server.id,
        name: req.name.clone(),
        owner: req.owner,
        endpoint_host: req.endpoint_host.clone(),
        s3_port: req.s3_port,
        console_port: req.console_port,
        created_by: caller.principal(),
    };
    let (store, member) = state.objstore_adopt.record(&found, &plan).await?;
    let job = submit(state, caller, &store, server.id, "adopt", None).await?;
    caller
        .audit(
            AUDIT_ADOPT,
            &server_ref(server.id),
            Some(ws),
            json!({
                "store_id": store.id, "name": req.name, "container": found.name, "image": found.image,
                "s3_port": store.s3_port, "job_id": job.id, "environment_id": req.owner,
            }),
        )
        .await;

    Ok((store, member, job))
}

/// Submits a `storage.action` job for one member.
pub async fn submit(
    state: &AppState,
    caller: &Caller,
    store: &ObjectStore,
    server: ServerId,
    action: &str,
    previous: Option<&PreviousSettings>,
) -> ApiResult<Job> {
    #[derive(Serialize)]
    struct Payload<'a> {
        store_id: ObjectStoreId,
        server_id: ServerId,
        action: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        previous: Option<&'a PreviousSettings>,
    }
    let payload = serde_json::to_value(Payload { store_id: store.id, server_id: server, action, previous })
        .map_err(DomainError::internal)?;
    Ok(state
        .jobs
        .submit(NewJob {
            kind: "storage.action".into(),
            workspace_id: Some(store.workspace_id),
            resource: server_ref(server),
            principal: caller.principal(),
            context: caller.ctx(),
            idempotency_key: None,
            payload,
            max_attempts: 2,
        })
        .await?)
}

// --------------------------------------------------------------- settings

pub async fn get_settings(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Path(id): Path<ObjectStoreId>,
) -> ApiResult<Json<SettingsView>> {
    let (store, members) = authorized(&state, &caller, ws, id, Action::StorageManage).await?;
    Ok(Json(state.objstore_settings.view(&store, &members).await?))
}

#[derive(Debug, Serialize)]
pub struct SettingsSaved {
    pub store: SettingsView,
    pub jobs: Vec<Job>,
}

/// Body of `PUT /object-stores/{id}/settings`: the full desired state.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(transparent)]
pub struct SettingsBody(pub SettingsUpdate);

pub async fn put_settings(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Path(id): Path<ObjectStoreId>,
    Json(SettingsBody(req)): Json<SettingsBody>,
) -> ApiResult<(StatusCode, Json<SettingsSaved>)> {
    let (store, members) = authorized(&state, &caller, ws, id, Action::StorageManage).await?;
    if crate::jobs::storage::BUSY.contains(&store.status.as_str()) {
        return Err(ApiError::conflict("an action is running on this store; try again when it ends"));
    }

    // Another image runs with the store's root credential and data: it needs
    // administrator rights on every member node too.
    if req.docker.as_ref().is_some_and(|d| d.image != forge_core::storage_settings::DEFAULT_IMAGE) {
        for m in &members {
            caller.authorize(Action::ServerManage, server_ref(m.server_id)).await?;
        }
    }

    let applied = state.objstore_settings.apply(&store, &members, &req).await?;
    let mut jobs = Vec::new();

    for m in &members {
        jobs.push(submit(&state, &caller, &applied.store, m.server_id, applied.action, Some(&applied.previous)).await?);
    }

    let view = state.objstore_settings.view(&applied.store, &members).await?;
    caller
        .audit(
            AUDIT_SETTINGS,
            &store_ref(&applied.store, &members),
            Some(ws),
            json!({
                "store_id": store.id, "version": req.version, "s3_port": req.s3_port,
                "console_port": req.console_port, "region": req.region,
                "previous": { "version": applied.previous.version, "s3_port": applied.previous.s3_port,
                              "console_port": applied.previous.console_port },
                // Variable names only; values may be addresses an operator keeps private.
                "env_keys": req.docker.as_ref().map(|d| d.env.keys().cloned().collect::<Vec<_>>()),
                "jobs": jobs.iter().map(|j| j.id).collect::<Vec<_>>(),
            }),
        )
        .await;

    Ok((StatusCode::ACCEPTED, Json(SettingsSaved { store: view, jobs })))
}

// -------------------------------------------------------------------- IAM

/// IAM is RustFS's: managed RustFS stores only.
fn iam_supported(store: &ObjectStore) -> ApiResult<()> {
    if store.mode != MODE_MANAGED || store.provider != "rustfs" {
        return Err(ApiError::conflict(
            "access keys are managed in RustFS for managed RustFS stores; manage a connected store's keys at its provider",
        ));
    }

    Ok(())
}

/// The store's own (root) access key, never listed as an app key.
async fn root_key(state: &AppState, store: &ObjectStore) -> ApiResult<String> {
    Ok(state.store.get_vault_item(store.credential_id).await?.map(|i| i.username).unwrap_or_default())
}

pub async fn iam_view(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Path(id): Path<ObjectStoreId>,
) -> ApiResult<Json<IamView>> {
    let (store, _) = authorized(&state, &caller, ws, id, Action::StorageManage).await?;
    iam_supported(&store)?;
    let root = root_key(&state, &store).await?;
    let c = client(&state, &store).await?;

    Ok(Json(state.objstore_iam.view(&store, &c, &root).await?))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateKey {
    pub name: String,
    #[schemars(with = "Vec<serde_json::Value>")]
    pub grants: Vec<KeyGrant>,
}

#[derive(Debug, Serialize)]
pub struct KeyCreated {
    pub access_key: String,
    pub name: String,
    pub grants: Vec<KeyGrant>,
    pub vault_item_id: Option<VaultItemId>,
    pub record_id: forge_core::StoreKeyId,
}

/// Creates a key in RustFS with a policy for exactly its grants. The secret
/// goes straight into the vault; it is never in the response.
pub async fn iam_create(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Path(id): Path<ObjectStoreId>,
    Json(req): Json<CreateKey>,
) -> ApiResult<(StatusCode, Json<KeyCreated>)> {
    let (store, members) = authorized(&state, &caller, ws, id, Action::StorageManage).await?;
    iam_supported(&store)?;
    let c = client(&state, &store).await?;
    let by = caller.principal().to_string();
    let (record, secret) = state.objstore_iam.create(&store, &c, &req.name, &req.grants, &by).await?;
    let buckets: Vec<&str> = record.grants.iter().flatten().map(|g| g.bucket.as_str()).collect();
    let item = vault_credential(
        &state,
        &caller,
        ws,
        format!("{} / {} ({})", store.name, buckets.join(", "), record.name),
        &record.access_key,
        &secret,
        &store.endpoint,
        "Access key kept in the store's RustFS IAM, issued by Forge Object Storage.",
        members.iter().map(|m| VaultLink { kind: "node".into(), id: m.server_id.to_string() }).collect(),
    )
    .await?;

    // Tagged like every Forge key, so rotate and revoke find it.
    if let Some(first) = record.grants.iter().flatten().next() {
        super::storage::tag_key_item(&state, item, &store, &first.bucket, first.access.as_str()).await?;
    }

    let record = state.objstore_iam.attach_vault(record, item).await?;
    let grants = record.grants.clone().unwrap_or_default();
    caller
        .audit(
            AUDIT_KEY_CREATE,
            &store_ref(&store, &members),
            Some(ws),
            json!({ "store_id": store.id, "access_key": record.access_key, "name": record.name, "grants": grants }),
        )
        .await;

    Ok((
        StatusCode::CREATED,
        Json(KeyCreated {
            access_key: record.access_key,
            name: record.name,
            grants,
            vault_item_id: record.vault_item_id,
            record_id: record.id,
        }),
    ))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetGrants {
    #[schemars(with = "Vec<serde_json::Value>")]
    pub grants: Vec<KeyGrant>,
}

#[derive(Debug, Serialize)]
pub struct GrantsSet {
    pub access_key: String,
    pub grants: Vec<KeyGrant>,
}

pub async fn iam_set(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Path((id, access_key)): Path<(ObjectStoreId, String)>,
    Json(req): Json<SetGrants>,
) -> ApiResult<Json<GrantsSet>> {
    let (store, members) = authorized(&state, &caller, ws, id, Action::StorageManage).await?;
    iam_supported(&store)?;

    if root_key(&state, &store).await? == access_key {
        return Err(ApiError::conflict("the store's root key has no policy to change"));
    }

    let c = client(&state, &store).await?;
    let by = caller.principal().to_string();
    let grants = state.objstore_iam.set_grants(&store, &c, &access_key, &req.grants, &by).await?;
    caller
        .audit(
            AUDIT_KEY_POLICY,
            &store_ref(&store, &members),
            Some(ws),
            json!({ "store_id": store.id, "access_key": access_key, "grants": grants }),
        )
        .await;

    Ok(Json(GrantsSet { access_key, grants }))
}

pub async fn iam_reconcile(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Path(id): Path<ObjectStoreId>,
) -> ApiResult<Json<ReconcileReport>> {
    let (store, members) = authorized(&state, &caller, ws, id, Action::StorageManage).await?;
    iam_supported(&store)?;
    let root = root_key(&state, &store).await?;
    let c = client(&state, &store).await?;
    let by = caller.principal().to_string();
    let report = state.objstore_iam.reconcile(&store, &c, &root, &by).await?;
    caller
        .audit(
            AUDIT_RECONCILE,
            &store_ref(&store, &members),
            Some(ws),
            json!({ "store_id": store.id, "imported": report.imported, "updated": report.updated, "removed": report.removed }),
        )
        .await;

    Ok(Json(report))
}
