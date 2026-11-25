//! Object storage: stores Forge installs on nodes (RustFS), stores it
//! connects (AWS S3, other S3-compatible services, a node it does not
//! manage), their buckets and objects.
//!
//! * `GET    /api/v1/object-stores` — stores the caller can see, with members
//!   (`?environment_id=` for the stores one environment owns).
//! * `POST   /api/v1/object-stores` — install a managed store on a node.
//! * `POST   /api/v1/object-stores/connect` — connect an existing S3 service.
//! * `GET    /api/v1/object-stores/{id}` — one store.
//! * `PUT    /api/v1/object-stores/{id}/owner` — `{environment_id, force}`:
//!   assign the store to an environment, or clear the owner with `null`.
//! * `POST   /api/v1/object-stores/{id}/actions` — `{action}`: start, stop,
//!   restart, status, install (repair), uninstall; `disconnect` for
//!   connected stores.
//! * `POST   /api/v1/object-stores/{id}/test` — S3 connectivity check.
//! * `GET|POST /api/v1/object-stores/{id}/buckets`, `DELETE …/buckets/{bucket}`.
//! * `GET|POST …/grants`, `DELETE …/grants/{grant_id}` and
//!   `GET /api/v1/object-stores/grants/mine`: bucket grants
//!   ([`super::storage_grants`]).
//! * `GET    …/buckets/{bucket}/objects?prefix=&token=` — one page.
//! * `GET|PUT|DELETE …/buckets/{bucket}/object?key=` — download (streamed),
//!   upload (streamed, `Content-Length` required) and delete.
//!
//! Managed stores are authorized on their member node, connected stores on
//! the workspace: `storage.manage` (MFA) for install, lifecycle, connect and
//! bucket deletion; `storage.data` for browsing and transfers. A store that
//! belongs to an environment (D-107) is also open, for `view` and
//! `storage.data` only, to callers who hold that action on the owning
//! environment ([`owner_grants`]). Such a caller gets a reduced view of the
//! store ([`LimitedStore`]) and, of its keys, only those attached to
//! services of that environment. A person with a bucket grant (D-108) and no
//! such role reaches the objects of the granted bucket only
//! ([`authorized_for_bucket`]), sees the store in the same reduced view and,
//! of its buckets, the granted ones ([`reachable`]). S3 calls run
//! from the control plane with the credential unsealed from the vault for
//! that request only; secrets never reach the browser or the logs.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;
use std::time::Duration;

use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use forge_core::model::{Job, JobStatus, NewJob, Service, VaultItem, VaultLink};
use forge_core::ports::{JobRepository, ServerRepository, ServiceRepository, TenancyRepository, VaultRepository};
use forge_core::storage::{
    BucketAccess, BucketGrant, MODE_EXTERNAL, MODE_MANAGED, ObjectStore, ObjectStoreRepository, StoreMember,
    TOPOLOGY_DISTRIBUTED, TOPOLOGY_STANDALONE,
};
use forge_core::storage_iam::StoreKeyRepository;
use forge_core::storage_settings::StoreSettingsRepository;
use forge_core::{
    Action, BucketGrantId, DomainError, EntityKind, EntityRef, EnvironmentId, ObjectStoreId, ServerId, ServiceId,
    VaultItemId, WorkspaceId,
};
use forge_s3::{Client, S3Config, S3Error};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::Semaphore;

use crate::app_storage;
use crate::auth::{Caller, Ws};
use crate::bucket_grants::{REASON_BUCKET_DELETED, REASON_BUCKET_RECREATED};
use crate::error::{ApiError, ApiResult};
use crate::jobs::storage::{ACTIONS, busy_status, spec, vault_aad};
use crate::state::AppState;

/// Largest single upload (S3's single-PUT limit).
const MAX_UPLOAD: u64 = 5 * 1024 * 1024 * 1024;
/// Concurrent uploads and downloads through the control plane.
const MAX_TRANSFERS: usize = 8;
/// Audit action of a change of owner (assign to an environment, or clear).
pub const AUDIT_SET_OWNER: &str = "storage.owner.set";
/// Error code: the store has buckets attached to services of environments
/// other than the new owner; the request needs `force`.
pub const ERR_ATTACHED_ELSEWHERE: &str = "attached_elsewhere";
/// Error code: the bucket is attached to a service of another environment,
/// so attaching it needs `storage.manage`.
pub const ERR_BUCKET_ATTACHED_ELSEWHERE: &str = "bucket_attached_elsewhere";

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/object-stores", get(list).post(install))
        .route("/object-stores/connect", post(connect))
        .route("/object-stores/discover", get(super::objstore::discover))
        .route("/object-stores/rustfs-env", get(super::objstore::rustfs_env))
        .route("/object-stores/{id}/settings", get(super::objstore::get_settings).put(super::objstore::put_settings))
        .route("/object-stores/{id}/iam", get(super::objstore::iam_view))
        .route("/object-stores/{id}/iam/keys", post(super::objstore::iam_create))
        .route("/object-stores/{id}/iam/keys/{access_key}", put(super::objstore::iam_set))
        .route("/object-stores/{id}/iam/reconcile", post(super::objstore::iam_reconcile))
        .route("/object-stores/{id}", get(get_one))
        .route("/object-stores/{id}/owner", put(set_owner))
        .route("/object-stores/{id}/actions", post(action))
        .route("/object-stores/{id}/test", post(test))
        .route("/object-stores/{id}/buckets", get(buckets).post(create_bucket))
        .route("/object-stores/{id}/buckets/{bucket}", axum::routing::delete(delete_bucket))
        .route("/object-stores/{id}/buckets/{bucket}/objects", get(objects))
        .route("/object-stores/{id}/buckets/{bucket}/object", get(download).put(upload).delete(delete_object))
        .route("/object-stores/adopt", post(adopt))
        .route("/object-stores/{id}/keys", get(list_keys).post(create_key))
        .route("/object-stores/{id}/keys/{access_key}", axum::routing::delete(delete_key))
        .route("/object-stores/{id}/keys/{access_key}/rotate", post(rotate_key))
}

fn transfers() -> &'static Semaphore {
    static S: OnceLock<Semaphore> = OnceLock::new();
    S.get_or_init(|| Semaphore::new(MAX_TRANSFERS))
}

/// Whether the control plane may send store requests to `ip`. Link-local
/// (cloud metadata services), unspecified, multicast and broadcast addresses
/// never; loopback only when `storage_allow_loopback` is set.
fn endpoint_ip_allowed(ip: std::net::IpAddr, allow_loopback: bool) -> bool {
    use std::net::IpAddr;

    match ip {
        IpAddr::V4(v4) => {
            !(v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_multicast()
                || v4.is_broadcast()
                || v4.octets()[0] == 0
                || (v4.is_loopback() && !allow_loopback))
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return endpoint_ip_allowed(IpAddr::V4(v4), allow_loopback);
            }

            let seg = v6.segments();
            let link_local = seg[0] & 0xffc0 == 0xfe80;

            // AWS's IPv6 instance metadata service.
            let metadata = seg == [0xfd00, 0x0ec2, 0, 0, 0, 0, 0, 0x0254];
            !(link_local
                || metadata
                || v6.is_unspecified()
                || v6.is_multicast()
                || (v6.is_loopback() && !allow_loopback))
        }
    }
}

/// Resolves store host names and drops forbidden addresses on every
/// connection, so DNS rebinding cannot point a store at the metadata
/// service or the control plane itself.
#[derive(Debug, Clone, Copy)]
struct GuardedResolver {
    allow_loopback: bool,
}

impl reqwest::dns::Resolve for GuardedResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let allow = self.allow_loopback;
        let host = name.as_str().to_owned();

        Box::pin(async move {
            let addrs: Vec<std::net::SocketAddr> = tokio::net::lookup_host((host.as_str(), 0)).await?.collect();
            let ok: Vec<std::net::SocketAddr> =
                addrs.into_iter().filter(|a| endpoint_ip_allowed(a.ip(), allow)).collect();

            if ok.is_empty() {
                return Err(format!("{host} resolves only to addresses Forge does not call for object storage").into());
            }

            let iter: reqwest::dns::Addrs = Box::new(ok.into_iter());
            Ok(iter)
        })
    }
}

fn http(allow_loopback: bool) -> ApiResult<reqwest::Client> {
    static STRICT: OnceLock<reqwest::Client> = OnceLock::new();
    static LOOPBACK: OnceLock<reqwest::Client> = OnceLock::new();
    let cell = if allow_loopback { &LOOPBACK } else { &STRICT };
    if let Some(c) = cell.get() {
        return Ok(c.clone());
    }

    let c = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .read_timeout(Duration::from_secs(120))
        .redirect(reqwest::redirect::Policy::none())
        .dns_resolver(std::sync::Arc::new(GuardedResolver { allow_loopback }))
        .build()
        .map_err(|_| DomainError::internal("cannot create the S3 HTTP client"))?;

    Ok(cell.get_or_init(|| c).clone())
}

fn server_ref(id: ServerId) -> EntityRef {
    EntityRef::new(EntityKind::Server, id)
}

/// The entity a store is authorized on.
pub(super) fn store_ref(store: &ObjectStore, members: &[StoreMember]) -> EntityRef {
    match members.first() {
        Some(m) if store.mode == MODE_MANAGED => server_ref(m.server_id),
        _ => EntityRef::new(EntityKind::Workspace, store.workspace_id),
    }
}

fn ns_ref(id: EnvironmentId) -> EntityRef {
    EntityRef::new(EntityKind::Environment, id)
}

/// The actions the roles of the owning environment carry onto its store
/// (D-107): seeing it and working with its buckets and objects. Everything
/// that needs `storage.manage` stays on the node or workspace.
const fn owner_grants(action: Action) -> bool {
    matches!(action, Action::View | Action::StorageData)
}

/// Whether the caller holds `action` on the store through the environment
/// that owns it. Never true for a store without an owner.
async fn allowed_by_owner(caller: &Caller, store: &ObjectStore, action: Action) -> bool {
    match store.environment_id {
        Some(ns) if owner_grants(action) => caller.can(action, ns_ref(ns)).await,
        _ => false,
    }
}

/// The environment a store is to belong to: one of this workspace, where the
/// caller may deploy. `None` (a shared store) needs nothing more.
async fn checked_owner(
    state: &AppState,
    caller: &Caller,
    ws: WorkspaceId,
    id: Option<EnvironmentId>,
) -> ApiResult<Option<EnvironmentId>> {
    let Some(id) = id else { return Ok(None) };
    let found = match state.store.get_environment(id).await {
        Ok(ns) => Some(ns),
        Err(DomainError::NotFound(_)) => None,
        Err(e) => return Err(e.into()),
    };

    // Another workspace's environment answers like one that does not exist.
    if found.is_none_or(|ns| ns.workspace_id != ws) {
        return Err(ApiError::not_found("environment not found"));
    }

    caller.authorize(Action::Deploy, ns_ref(id)).await?;

    Ok(Some(id))
}

/// How a caller reached a store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Via {
    /// The node (managed) or workspace (connected) rule.
    Scope,
    /// Only the environment that owns the store (D-107).
    Owner,
}

/// What the audit event of a refused request says about an owned store:
/// the resource of the event is the node or workspace, not the store.
fn denial_details(store: &ObjectStore) -> Value {
    match store.environment_id {
        Some(ns) => json!({ "store_id": store.id, "environment_id": ns }),
        None => Value::Null,
    }
}

/// One member as a caller on the owner path sees it.
#[derive(Debug, Serialize)]
pub struct LimitedMember {
    pub hostname: String,
    pub status: String,
}

/// A store as a caller sees it whose access comes only from the owning
/// environment or from bucket grants: enough to find and use it, nothing
/// about how it is run (no credential, data paths, ports, capacity, jobs or
/// errors). A grant holder works through Forge alone, so that view also
/// leaves out the owner, the endpoint and the members.
#[derive(Debug, Serialize)]
pub struct LimitedStore {
    pub id: ObjectStoreId,
    pub environment_id: Option<EnvironmentId>,
    pub name: String,
    pub slug: String,
    pub provider: String,
    pub mode: String,
    pub topology: String,
    pub status: String,
    pub endpoint: String,
    pub members: Vec<LimitedMember>,
    /// Always `true`: tells the console to hide what only operators use.
    pub limited: bool,
    /// The caller reaches the store through bucket grants only (D-108):
    /// the granted buckets and their objects, nothing else.
    pub by_grant: bool,
}

/// A store in a response: everything, or the reduced view.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum StoreView {
    Full(Value),
    Limited(LimitedStore),
}

/// How a store is shown to one caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shown {
    /// Reached by a role: everything, or the owner path's reduced view.
    Role(Via),
    /// Reached by bucket grants only.
    Grant,
}

fn view_for(
    shown: Shown,
    s: &ObjectStore,
    members: &[StoreMember],
    hostnames: &HashMap<ServerId, String>,
) -> StoreView {
    let by_grant = match shown {
        Shown::Role(Via::Scope) => return StoreView::Full(view(s, members, hostnames)),
        Shown::Role(Via::Owner) => false,
        Shown::Grant => true,
    };

    StoreView::Limited(LimitedStore {
        id: s.id,
        environment_id: s.environment_id.filter(|_| !by_grant),
        name: s.name.clone(),
        slug: s.slug.clone(),
        provider: s.provider.clone(),
        mode: s.mode.clone(),
        topology: s.topology.clone(),
        status: s.status.clone(),
        endpoint: if by_grant { String::new() } else { s.endpoint.clone() },
        members: members
            .iter()
            .filter(|_| !by_grant)
            .map(|m| LimitedMember {
                // A node that is gone has no name left to show.
                hostname: hostnames.get(&m.server_id).cloned().unwrap_or_default(),
                status: m.status.clone(),
            })
            .collect(),
        limited: true,
        by_grant,
    })
}

fn view(s: &ObjectStore, members: &[StoreMember], hostnames: &HashMap<ServerId, String>) -> Value {
    json!({
        "id": s.id,
        "limited": false,
        "environment_id": s.environment_id,
        "name": s.name,
        "slug": s.slug,
        "provider": s.provider,
        "mode": s.mode,
        "region": s.region,
        "path_style": s.path_style,
        "version": s.version,
        "deployed_version": s.deployed_version,
        "runtime": s.runtime,
        "topology": s.topology,
        "s3_port": s.s3_port,
        "console_port": s.console_port,
        "endpoint": s.endpoint,
        "credential_id": s.credential_id,
        "status": s.status,
        "last_error": s.last_error,
        "last_job_id": s.last_job_id,
        "created_at": s.created_at,
        "updated_at": s.updated_at,
        "members": members.iter().map(|m| json!({
            "server_id": m.server_id,
            "hostname": hostnames.get(&m.server_id).cloned().unwrap_or_default(),
            "data_paths": m.data_paths,
            "endpoint_host": m.endpoint_host,
            "require_mount": m.require_mount,
            "status": m.status,
            "capacity_bytes": m.capacity_bytes,
            "free_bytes": m.free_bytes,
            "dedicated": m.dedicated,
            "observed_at": m.observed_at,
        })).collect::<Vec<_>>(),
    })
}

async fn hostnames(state: &AppState) -> ApiResult<HashMap<ServerId, String>> {
    Ok(state
        .store
        .list_servers(None)
        .await?
        .into_iter()
        .map(|s| (s.id, s.display_name.clone().unwrap_or(s.hostname)))
        .collect())
}

/// Loads a store of this workspace with its members.
pub(super) async fn load(
    state: &AppState,
    ws: WorkspaceId,
    id: ObjectStoreId,
) -> ApiResult<(ObjectStore, Vec<StoreMember>)> {
    let store = state
        .store
        .get_object_store(id)
        .await?
        .filter(|s| s.workspace_id == ws)
        .ok_or_else(|| ApiError::not_found("object store not found"))?;
    let members = state.store.list_store_members(id).await?;

    Ok((store, members))
}

/// Loads and authorizes `action` on a store, and says how: on its node or
/// workspace, or else through the environment that owns it (`view` and
/// `storage.data` only). A denial is audited on the node or workspace, with
/// the store and its owner in the details.
pub(super) async fn authorized_via(
    state: &AppState,
    caller: &Caller,
    ws: WorkspaceId,
    id: ObjectStoreId,
    action: Action,
) -> ApiResult<(ObjectStore, Vec<StoreMember>, Via)> {
    let (store, members) = load(state, ws, id).await?;
    let target = store_ref(&store, &members);
    let via = match by_role(caller, &store, &target, action).await {
        Some(via) => via,
        None => {
            caller.authorize_with(action, target, denial_details(&store)).await?;
            Via::Scope
        }
    };

    Ok((store, members, via))
}

/// The role path that gives the caller `action` on a store, if one does: its
/// node or workspace, else the environment that owns it.
async fn by_role(caller: &Caller, store: &ObjectStore, target: &EntityRef, action: Action) -> Option<Via> {
    if caller.can(action, target.clone()).await {
        Some(Via::Scope)
    } else if allowed_by_owner(caller, store, action).await {
        Some(Via::Owner)
    } else {
        None
    }
}

/// Whether bucket grants count for this caller (D-108): for a person's
/// sessions and for API keys and service-account tokens, which act as the
/// account that holds the grant. Never for an AI connector, which gets what
/// its user's roles allow and nothing an administrator handed to the person:
/// neither on its own connection, nor when a person approved its request
/// and the request is replayed with that person's session (D-073).
pub(super) fn grants_apply(caller: &Caller) -> bool {
    grants_count(caller.session.session.connection_id.is_some(), caller.approved_replay())
}

/// The rule of [`grants_apply`]: no connector behind the request, in either
/// form.
const fn grants_count(connector_session: bool, approved_connector_request: bool) -> bool {
    !connector_session && !approved_connector_request
}

/// What a grant holder is told when a store request fails. The store's own
/// answer can name its address, its node or its credential, which a grant
/// does not show: the holder learns that the bucket or object is not there,
/// that the request was not valid, or that the store cannot be reached, and
/// the log keeps the rest.
fn for_grant_holder(e: ApiError) -> ApiError {
    if e.status == StatusCode::NOT_FOUND {
        return ApiError::not_found("no such bucket or object");
    }

    if e.status == StatusCode::BAD_REQUEST {
        return ApiError::bad_request("the bucket name or the key is not valid");
    }

    tracing::warn!(code = e.code, error = %e.message, "a store request made through a bucket grant failed");

    ApiError::new(StatusCode::BAD_GATEWAY, "store_unreachable", "the store can't be reached")
}

/// [`for_grant_holder`] when the request came `by_grant`; the error as it
/// is for everyone a role let in.
fn shown_error(by_grant: bool) -> impl Fn(ApiError) -> ApiError {
    move |e| if by_grant { for_grant_holder(e) } else { e }
}

/// How a caller reached a store as a whole.
#[derive(Debug)]
enum Reach {
    Role(Via),
    /// No role: the live bucket grants the caller holds on the store (at
    /// least one).
    Grants(Vec<BucketGrant>),
}

/// Loads a store to show it or to list its buckets (`view`,
/// `storage.data`): by a role like [`authorized_via`], or else because the
/// caller holds a live bucket grant on it. Use it for those two requests
/// only: what a grant opens beyond them is decided per bucket by
/// [`authorized_for_bucket`], and everything else needs a role.
async fn reachable(
    state: &AppState,
    caller: &Caller,
    ws: WorkspaceId,
    id: ObjectStoreId,
    action: Action,
) -> ApiResult<(ObjectStore, Vec<StoreMember>, Reach)> {
    let (store, members) = load(state, ws, id).await?;
    let target = store_ref(&store, &members);
    if let Some(via) = by_role(caller, &store, &target, action).await {
        return Ok((store, members, Reach::Role(via)));
    }

    if grants_apply(caller) {
        let grants = state.bucket_grants.held_on(caller.session.user.id, store.id).await?;
        if !grants.is_empty() {
            return Ok((store, members, Reach::Grants(grants)));
        }
    }

    caller.authorize_with(action, target, denial_details(&store)).await?;

    Ok((store, members, Reach::Role(Via::Scope)))
}

/// A store loaded for a request on the objects of one bucket.
struct BucketScope {
    store: ObjectStore,
    members: Vec<StoreMember>,
    /// The grant that allowed the request, when no role did. It goes into
    /// the audit event of what the request does.
    grant_id: Option<BucketGrantId>,
}

/// Loads a store for a request on the objects of `bucket` that needs `need`
/// (`read`: list and download; `readwrite`: upload and delete). Allowed by
/// `storage.data` on the node or workspace, or on the owning environment
/// (D-107); else by a live grant of this caller for this store and bucket
/// with at least that access (D-108). The grant is read for this request, so
/// a revocation or an expiry holds from the next one. A denial is audited as
/// `storage.data` on the node or workspace, with the store, the bucket and
/// the access that was needed.
async fn authorized_for_bucket(
    state: &AppState,
    caller: &Caller,
    ws: WorkspaceId,
    id: ObjectStoreId,
    bucket: &str,
    need: BucketAccess,
) -> ApiResult<BucketScope> {
    let (store, members) = load(state, ws, id).await?;
    let target = store_ref(&store, &members);
    let mut grant_id = None;

    if by_role(caller, &store, &target, Action::StorageData).await.is_none() {
        if grants_apply(caller) {
            let grant = state.bucket_grants.allowance(caller.session.user.id, store.id, bucket, need).await?;
            grant_id = grant.map(|g| g.id);
        }

        if grant_id.is_none() {
            let denied = json!({
                "store_id": store.id, "environment_id": store.environment_id, "bucket": bucket, "access": need,
            });
            caller.authorize_with(Action::StorageData, target, denied).await?;
        }
    }

    Ok(BucketScope { store, members, grant_id })
}

/// [`authorized_via`] for handlers that treat both paths alike.
pub(super) async fn authorized(
    state: &AppState,
    caller: &Caller,
    ws: WorkspaceId,
    id: ObjectStoreId,
    action: Action,
) -> ApiResult<(ObjectStore, Vec<StoreMember>)> {
    let (store, members, _) = authorized_via(state, caller, ws, id, action).await?;
    Ok((store, members))
}

/// A bucket of a store attached to a service outside one environment.
#[derive(Debug, Clone, Serialize)]
pub struct ForeignAttachment {
    pub service_id: ServiceId,
    /// `None` when the service no longer exists.
    pub service: Option<String>,
    pub environment_id: Option<EnvironmentId>,
    pub bucket: String,
    pub access_key: String,
}

/// The attachments of `store` whose service is not in `environment`. An
/// service that no longer exists counts: nobody can say it was one of
/// that environment.
async fn foreign_attachments(
    state: &AppState,
    store: &ObjectStore,
    environment: EnvironmentId,
) -> ApiResult<Vec<ForeignAttachment>> {
    let attachments = app_storage::store_attachments(&state.store, store.workspace_id, store.id).await?;
    if attachments.is_empty() {
        return Ok(Vec::new());
    }

    let own: HashSet<ServiceId> =
        state.store.list_services(Some(environment)).await?.into_iter().map(|a| a.id).collect();
    let mut out = Vec::new();

    for (service_id, a) in attachments.into_iter().filter(|(id, _)| !own.contains(id)) {
        let app = state.store.get_service(service_id).await?;
        out.push(ForeignAttachment {
            service_id,
            environment_id: app.as_ref().map(|x| x.environment_id),
            service: app.map(|x| x.name),
            bucket: a.bucket,
            access_key: a.access_key,
        });
    }

    Ok(out)
}

/// `service (bucket)` for each attachment, at most ten, for an error message.
fn named(list: &[ForeignAttachment]) -> String {
    let mut names: Vec<String> = list
        .iter()
        .take(10)
        .map(|f| format!("{} ({})", f.service.as_deref().unwrap_or("a deleted service"), f.bucket))
        .collect();

    if list.len() > names.len() {
        names.push(format!("and {} more", list.len() - names.len()));
    }

    names.join(", ")
}

/// Loads a store to attach `bucket` to `app` (D-107). A store that belongs
/// to the service's environment needs `deploy` there and nothing more,
/// unless the bucket is already attached to a service of another environment:
/// that bucket, like any other store, needs `storage.manage` (MFA).
pub(super) async fn authorized_for_attach(
    state: &AppState,
    caller: &Caller,
    ws: WorkspaceId,
    id: ObjectStoreId,
    app: &Service,
    bucket: &str,
) -> ApiResult<(ObjectStore, Vec<StoreMember>)> {
    let (store, members) = load(state, ws, id).await?;
    let target = store_ref(&store, &members);

    if store.environment_id != Some(app.environment_id) {
        caller.authorize_with(Action::StorageManage, target, denial_details(&store)).await?;
        return Ok((store, members));
    }

    caller.authorize(Action::Deploy, ns_ref(app.environment_id)).await?;
    let elsewhere: Vec<ForeignAttachment> = foreign_attachments(state, &store, app.environment_id)
        .await?
        .into_iter()
        .filter(|f| f.bucket == bucket)
        .collect();

    if !elsewhere.is_empty() {
        let details = json!({
            "store_id": store.id, "environment_id": store.environment_id, "bucket": bucket, "attached_to": elsewhere,
        });

        // The denial is audited by the check; the answer says why.
        if caller.authorize_with(Action::StorageManage, target, details).await.is_err() {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                ERR_BUCKET_ATTACHED_ELSEWHERE,
                format!(
                    "bucket `{bucket}` is already attached to a service of another environment: {}. Attaching it needs the storage.manage permission on the store and an MFA-verified session",
                    named(&elsewhere)
                ),
            ));
        }
    }

    Ok((store, members))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct StoreQuery {
    /// Only the stores this environment owns.
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub environment_id: Option<EnvironmentId>,
}

pub async fn list(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Query(q): Query<StoreQuery>,
) -> ApiResult<Json<Vec<StoreView>>> {
    let stores = state.store.list_object_stores(Some(ws)).await?;
    let mut rows = Vec::new();

    for s in stores {
        if q.environment_id.is_some_and(|n| s.environment_id != Some(n)) {
            continue;
        }

        let members = state.store.list_store_members(s.id).await?;
        rows.push((s, members));
    }

    let refs = rows.iter().map(|(s, m)| store_ref(s, m)).collect();
    let allowed = caller.can_many(Action::View, refs).await;
    let mut via: Vec<Option<Shown>> = allowed.iter().map(|ok| ok.then_some(Shown::Role(Via::Scope))).collect();

    // Owned stores the caller does not see on the node or workspace: asked
    // once more on the owning environment, in one batch.
    let by_owner: Vec<(usize, EntityRef)> = rows
        .iter()
        .zip(&allowed)
        .enumerate()
        .filter(|(_, (_, seen))| !**seen)
        .filter_map(|(i, ((s, _), _))| s.environment_id.map(|n| (i, ns_ref(n))))
        .collect();
    let owner_refs = by_owner.iter().map(|(_, r)| r.clone()).collect();

    for ((i, _), ok) in by_owner.iter().zip(caller.can_many(Action::View, owner_refs).await) {
        via[*i] = ok.then_some(Shown::Role(Via::Owner));
    }

    // Stores no role shows: those the caller holds a live bucket grant on.
    // Not in a list by environment: a grant says nothing about who owns the
    // store, and the reduced view of a grant holder leaves the owner out.
    if q.environment_id.is_none() && grants_apply(&caller) && via.iter().any(Option::is_none) {
        let granted: HashSet<ObjectStoreId> =
            state.bucket_grants.held(caller.session.user.id).await?.into_iter().map(|g| g.store_id).collect();

        for (shown, (s, _)) in via.iter_mut().zip(&rows) {
            if shown.is_none() && granted.contains(&s.id) {
                *shown = Some(Shown::Grant);
            }
        }
    }

    let names = hostnames(&state).await?;

    Ok(Json(rows.iter().zip(via).filter_map(|((s, m), via)| Some(view_for(via?, s, m, &names))).collect()))
}

pub async fn get_one(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Path(id): Path<ObjectStoreId>,
) -> ApiResult<Json<StoreView>> {
    let (store, members, reach) = reachable(&state, &caller, ws, id, Action::View).await?;
    let shown = match reach {
        Reach::Role(via) => Shown::Role(via),
        Reach::Grants(_) => Shown::Grant,
    };

    Ok(Json(view_for(shown, &store, &members, &hostnames(&state).await?)))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct OwnerRequest {
    /// The environment the store is to belong to; `null` makes it a shared
    /// store again.
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub environment_id: Option<EnvironmentId>,
    /// Assign the store although buckets of it are attached to services of
    /// other environments. Without it such a request answers `409`.
    #[serde(default)]
    pub force: bool,
}

/// Assigns a store to an environment, or clears its owner. `storage.manage`
/// (MFA) on the store, and `deploy` on the environment it is assigned to.
/// The roles of the new owner reach every bucket of the store, so a store
/// with buckets attached to services of other environments is assigned only
/// with `force`, and the audit event names those services.
pub async fn set_owner(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Path(id): Path<ObjectStoreId>,
    Json(req): Json<OwnerRequest>,
) -> ApiResult<Json<Value>> {
    let (mut store, members) = authorized(&state, &caller, ws, id, Action::StorageManage).await?;
    let owner = checked_owner(&state, &caller, ws, req.environment_id).await?;
    let affected = match owner {
        Some(ns) => foreign_attachments(&state, &store, ns).await?,
        None => Vec::new(),
    };

    if !affected.is_empty() && !req.force {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            ERR_ATTACHED_ELSEWHERE,
            format!(
                "{} bucket(s) of this store are attached to services of other environments: {}. The roles of the new environment would reach those buckets too. Send `force: true` to assign the store anyway",
                affected.len(),
                named(&affected)
            ),
        ));
    }

    let now = state.clock.now();
    state.store.set_object_store_owner(store.id, owner, now).await?;
    caller
        .audit(
            AUDIT_SET_OWNER,
            &store_ref(&store, &members),
            Some(ws),
            json!({
                "store_id": store.id, "name": store.name, "from": store.environment_id, "to": owner,
                "forced": !affected.is_empty(), "affected": affected,
            }),
        )
        .await;
    store.environment_id = owner;
    store.updated_at = now;

    Ok(Json(view(&store, &members, &hostnames(&state).await?)))
}

fn random_alnum(len: usize) -> ApiResult<String> {
    use ring::rand::SecureRandom;
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut buf = vec![0u8; len];
    ring::rand::SystemRandom::new().fill(&mut buf).map_err(|_| DomainError::internal("no randomness"))?;

    // 248 = 4 * 62: rejection-free modulo bias is negligible for keys; keep
    // it exact anyway by re-drawing bytes >= 248.
    let mut out = String::with_capacity(len);
    let rng = ring::rand::SystemRandom::new();

    for mut b in buf {
        while b >= 248 {
            let mut one = [0u8; 1];
            rng.fill(&mut one).map_err(|_| DomainError::internal("no randomness"))?;
            b = one[0];
        }

        out.push(char::from(ALPHABET[usize::from(b) % ALPHABET.len()]));
    }

    Ok(out)
}

fn clean_name(name: &str) -> ApiResult<String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 80 {
        return Err(ApiError::bad_request("name: 1 to 80 characters"));
    }

    Ok(name.to_owned())
}

/// Seals a store credential into a new vault item.
#[allow(clippy::too_many_arguments)]
pub(super) async fn vault_credential(
    state: &AppState,
    caller: &Caller,
    ws: WorkspaceId,
    name: String,
    access_key: &str,
    secret_key: &str,
    url: &str,
    notes: &str,
    links: Vec<VaultLink>,
) -> ApiResult<VaultItemId> {
    let id = VaultItemId::new();
    let sealed = state.envelope.seal(secret_key.as_bytes(), &vault_aad(id)).map_err(DomainError::internal)?;
    let now = state.clock.now();
    let item = VaultItem {
        id,
        workspace_id: ws,
        name,
        username: access_key.to_owned(),
        url: url.to_owned(),
        notes: notes.to_owned(),
        tags: vec!["object-storage".into()],
        links,
        sealed,
        created_by: caller.principal(),
        created_at: now,
        updated_at: now,
        rotated_at: now,
    };
    state.store.put_vault_item(&item).await?;

    Ok(id)
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct InstallRequest {
    pub name: String,
    /// Instance name on the node (service, container, paths).
    pub slug: String,
    #[serde(default = "default_provider")]
    pub provider: String,
    #[serde(default = "default_version")]
    pub version: String,
    /// `native` or `docker`.
    #[serde(default = "default_runtime")]
    pub runtime: String,
    /// `standalone` (one node) or `distributed` (HA across nodes).
    #[serde(default = "default_topology")]
    pub topology: String,
    /// The nodes and their drives. A standalone store has one member.
    #[serde(default)]
    pub members: Vec<MemberRequest>,
    /// Standalone shorthand for `members: [{server_id, data_paths: [data_path], endpoint_host}]`.
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub server_id: Option<ServerId>,
    #[serde(default)]
    pub data_path: Option<String>,
    #[serde(default)]
    pub endpoint_host: Option<String>,
    #[serde(default)]
    pub require_mount: bool,
    #[serde(default = "default_s3_port")]
    pub s3_port: u16,
    #[serde(default)]
    pub console_port: Option<u16>,
    /// The environment the store belongs to (D-107); omit for a shared store.
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub environment_id: Option<EnvironmentId>,
    /// S3 signing region (`RUSTFS_REGION`, D-170).
    #[serde(default = "default_region")]
    pub region: String,
    /// Docker stores: container settings (D-170).
    #[serde(default)]
    #[schemars(with = "Option<serde_json::Value>")]
    pub docker: Option<forge_core::storage_settings::RustfsSettings>,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct MemberRequest {
    #[schemars(with = "String")]
    pub server_id: ServerId,
    /// One data path per drive.
    pub data_paths: Vec<String>,
    /// Host the other members and the control plane reach this node on
    /// (default: its hostname).
    #[serde(default)]
    pub endpoint_host: Option<String>,
}

fn default_topology() -> String {
    TOPOLOGY_STANDALONE.into()
}

/// Nodes a distributed store may span.
const MAX_MEMBERS: usize = 32;

fn default_provider() -> String {
    "rustfs".into()
}
fn default_version() -> String {
    "latest".into()
}
fn default_runtime() -> String {
    "native".into()
}
const fn default_s3_port() -> u16 {
    9000
}

async fn submit(
    state: &AppState,
    caller: &Caller,
    store: &ObjectStore,
    server: ServerId,
    action: &str,
) -> ApiResult<Job> {
    submit_with(state, caller, store, server, action, false).await
}

async fn submit_with(
    state: &AppState,
    caller: &Caller,
    store: &ObjectStore,
    server: ServerId,
    action: &str,
    delete_data: bool,
) -> ApiResult<Job> {
    let mut payload = json!({ "store_id": store.id, "server_id": server, "action": action });

    if delete_data {
        payload["delete_data"] = json!(true);
    }

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

pub async fn install(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Json(req): Json<InstallRequest>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let name = clean_name(&req.name)?;

    let distributed = match req.topology.as_str() {
        TOPOLOGY_STANDALONE => false,
        TOPOLOGY_DISTRIBUTED => true,
        _ => return Err(ApiError::bad_request("topology: standalone or distributed")),
    };

    let mut requested = req.members.clone();
    if requested.is_empty()
        && let (Some(server_id), Some(path)) = (req.server_id, req.data_path.clone())
    {
        requested.push(MemberRequest { server_id, data_paths: vec![path], endpoint_host: req.endpoint_host.clone() });
    }

    match (distributed, requested.len()) {
        (_, 0) => return Err(ApiError::bad_request("choose a node and its data path")),
        (false, n) if n > 1 => return Err(ApiError::bad_request("a standalone store has one node")),
        (true, 1) => return Err(ApiError::bad_request("a distributed store needs at least 2 nodes")),
        (true, n) if n > MAX_MEMBERS => {
            return Err(ApiError::bad_request(format!("a distributed store spans at most {MAX_MEMBERS} nodes")));
        }
        _ => {}
    }

    // Every member node is checked before anything is recorded.
    for m in &requested {
        caller.authorize(Action::StorageManage, server_ref(m.server_id)).await?;
    }

    let owner = checked_owner(&state, &caller, ws, req.environment_id).await?;
    let mut seen = HashSet::new();
    if !requested.iter().all(|m| seen.insert(m.server_id)) {
        return Err(ApiError::bad_request("each node may be listed once"));
    }

    if distributed {
        let drives = requested[0].data_paths.len();
        if requested.iter().any(|m| m.data_paths.len() != drives) {
            return Err(ApiError::bad_request("every node of a distributed store needs the same number of drives"));
        }

        if drives * requested.len() < forge_catalog::storage::MIN_DISTRIBUTED_DRIVES {
            return Err(ApiError::bad_request(format!(
                "a distributed store needs at least {} drives in total (nodes × drives per node)",
                forge_catalog::storage::MIN_DISTRIBUTED_DRIVES
            )));
        }
    }

    let now = state.clock.now();
    let id = ObjectStoreId::new();
    let mut members = Vec::new();
    let mut hosts = HashSet::new();

    for m in &requested {
        let server = state.store.get_server(m.server_id).await.map_err(|_| ApiError::not_found("server not found"))?;
        if server.workspace_id != ws {
            return Err(ApiError::not_found("server not found"));
        }

        let label = server.display_name.clone().unwrap_or_else(|| server.hostname.clone());

        match (server.os.as_str(), req.runtime.as_str()) {
            ("linux", "native" | "docker") => {}
            ("windows", "native") if !distributed => {}
            ("windows", _) if distributed => {
                return Err(ApiError::bad_request(format!("{label}: distributed stores run on Linux nodes only")));
            }
            ("windows", _) => {
                return Err(ApiError::bad_request(format!("{label}: Windows nodes run object stores natively only")));
            }
            (os, _) => return Err(ApiError::bad_request(format!("{label}: object stores are not supported on {os}"))),
        }

        if server.os == "windows" && !matches!(server.arch.as_str(), "x86_64" | "amd64") {
            return Err(ApiError::bad_request("RustFS for Windows is available for x86_64 only"));
        }

        for p in &m.data_paths {
            if server.os == "linux" && !p.starts_with('/') || server.os == "windows" && p.starts_with('/') {
                return Err(ApiError::bad_request(format!(
                    "{label}: data path `{p}` does not fit a {} node",
                    server.os
                )));
            }
        }

        // Default: the address the node's agent connects from. The host name
        // is the last resort only: without DNS neither the control plane nor
        // the other members can resolve it, and the store never forms (#1).
        let known = crate::facts::node_address(&state, server.id).await?;
        let host = m
            .endpoint_host
            .as_deref()
            .map(str::trim)
            .filter(|h| !h.is_empty())
            .or(known.as_deref())
            .unwrap_or(&server.hostname);

        if !forge_catalog::storage::valid_host(host) {
            return Err(ApiError::bad_request(format!("{label}: endpoint host: a hostname or IP address")));
        }

        if !hosts.insert(host.to_ascii_lowercase()) {
            return Err(ApiError::bad_request(format!("endpoint host `{host}` is used by two nodes")));
        }

        // Instances on one node need distinct names, ports and data paths.
        let mut taken_ports = HashSet::new();

        for other_m in state.store.list_node_store_members(server.id).await? {
            if let Some(p) = other_m.data_paths.iter().find(|p| m.data_paths.contains(p)) {
                return Err(ApiError::conflict(format!("{label}: {p} is already used by another object store")));
            }

            if let Some(other) = state.store.get_object_store(other_m.store_id).await? {
                if other.slug == req.slug {
                    return Err(ApiError::conflict(format!("{label}: instance name `{}` is taken", req.slug)));
                }

                taken_ports.insert(other.s3_port);
                taken_ports.extend(other.console_port);
            }
        }

        if taken_ports.contains(&req.s3_port) || req.console_port.is_some_and(|p| taken_ports.contains(&p)) {
            return Err(ApiError::conflict(format!("{label}: another object store on this node uses that port")));
        }

        members.push(StoreMember {
            store_id: id,
            server_id: server.id,
            data_paths: m.data_paths.clone(),
            endpoint_host: host.to_owned(),
            require_mount: req.require_mount,
            status: "pending".into(),
            capacity_bytes: None,
            free_bytes: None,
            dedicated: None,
            observed_at: None,
        });
    }

    let endpoint = format!("http://{}:{}", members[0].endpoint_host, req.s3_port);
    let access_key = format!("forge{}", random_alnum(15)?.to_ascii_lowercase());
    let secret_key = random_alnum(40)?;
    let mut store = ObjectStore {
        id,
        workspace_id: ws,
        environment_id: owner,
        name: name.clone(),
        slug: req.slug.clone(),
        provider: req.provider.clone(),
        mode: MODE_MANAGED.into(),
        region: req.region.clone(),
        path_style: true,
        version: req.version.clone(),
        deployed_version: None,
        runtime: req.runtime.clone(),
        topology: req.topology.clone(),
        s3_port: req.s3_port,
        console_port: req.console_port,
        endpoint: endpoint.clone(),
        credential_id: VaultItemId::new(),
        status: busy_status("install").into(),
        last_error: None,
        last_job_id: None,
        created_by: caller.principal(),
        created_at: now,
        updated_at: now,
    };

    if !forge_catalog::storage::valid_region(&store.region) || store.region.is_empty() {
        return Err(ApiError::bad_request("region: lowercase letters, digits and -, up to 32"));
    }

    let settings = match (req.runtime.as_str(), req.docker.clone()) {
        ("docker", Some(mut d)) => {
            // Adoption is recorded by the adopt flow only.
            d.adopted = false;

            if distributed {
                d.network = forge_core::storage_settings::NetworkMode::Host;
            }

            Some(d)
        }
        ("docker", None) => None,
        (_, Some(_)) => return Err(ApiError::bad_request("container settings apply to the Docker runtime only")),
        (_, None) => None,
    };

    // Another image, volume drivers or driver options reach beyond the store:
    // administrator rights on every member node too.
    if settings.as_ref().is_some_and(forge_core::storage_settings::RustfsSettings::reaches_host) {
        for m in &members {
            caller.authorize(Action::ServerManage, server_ref(m.server_id)).await?;
        }
    }

    // Validates provider, slug, release, ports, data paths, settings and the layout.
    for m in &members {
        spec(&store, m, &members, settings.as_ref()).map_err(ApiError::bad_request)?;
    }

    store.credential_id = vault_credential(
        &state,
        &caller,
        ws,
        format!("{name} (object store root)"),
        &access_key,
        &secret_key,
        &endpoint,
        "Bootstrap (root) credential of a Forge-managed object store. Forge uses it for bucket and object management; keep it for recovery.",
        members.iter().map(|m| VaultLink { kind: "node".into(), id: m.server_id.to_string() }).collect(),
    )
    .await?;
    state.store.put_object_store(&store).await?;

    for m in &members {
        state.store.put_store_member(m).await?;
    }

    if let Some(d) = &settings {
        state.store.put_store_settings(id, d, now).await?;
    }

    // Members install in parallel; distributed members wait for each other.
    let mut jobs = Vec::new();

    for m in &members {
        jobs.push(submit(&state, &caller, &store, m.server_id, "install").await?);
    }

    let member_list: Vec<Value> = members
        .iter()
        .map(|m| json!({ "server_id": m.server_id, "data_paths": m.data_paths, "endpoint_host": m.endpoint_host }))
        .collect();
    caller
        .audit(
            "storage.install",
            &server_ref(members[0].server_id),
            Some(ws),
            json!({
                "store_id": id, "name": name, "provider": store.provider, "version": store.version,
                "runtime": store.runtime, "topology": store.topology, "s3_port": store.s3_port,
                "environment_id": owner,
                "members": member_list, "jobs": jobs.iter().map(|j| j.id).collect::<Vec<_>>(),
            }),
        )
        .await;
    let names = hostnames(&state).await?;
    let first = jobs.first().cloned();

    Ok((StatusCode::ACCEPTED, Json(json!({ "store": view(&store, &members, &names), "job": first, "jobs": jobs }))))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ConnectRequest {
    pub name: String,
    /// `aws`, `s3`, `rustfs`, `minio`, `garage`, `r2`, `wasabi`...
    #[serde(default = "default_external_provider")]
    pub provider: String,
    pub endpoint: String,
    #[serde(default = "default_region")]
    pub region: String,
    #[serde(default)]
    pub path_style: bool,
    pub access_key: String,
    pub secret_key: String,
    /// For credentials limited to one bucket (no ListBuckets): the bucket
    /// used to check them.
    #[serde(default)]
    pub bucket: Option<String>,
    /// The environment the store belongs to (D-107); omit for a shared store.
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub environment_id: Option<EnvironmentId>,
}

fn default_external_provider() -> String {
    "s3".into()
}
fn default_region() -> String {
    "us-east-1".into()
}

const EXTERNAL_PROVIDERS: &[&str] = &["aws", "s3", "rustfs", "minio", "garage", "ceph", "r2", "wasabi", "b2", "other"];

fn parse_endpoint(raw: &str, allow_loopback: bool) -> ApiResult<url::Url> {
    let url = url::Url::parse(raw.trim()).map_err(|_| ApiError::bad_request("endpoint: http(s)://host[:port]"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(ApiError::bad_request("endpoint: http(s)://host[:port], without credentials or a query"));
    }

    // Literal addresses are checked here; host names by the resolver at
    // every connection.
    let literal = match url.host() {
        Some(url::Host::Ipv4(a)) => Some(std::net::IpAddr::V4(a)),
        Some(url::Host::Ipv6(a)) => Some(std::net::IpAddr::V6(a)),
        _ => None,
    };

    let localhost = url.host_str().is_some_and(|h| h.eq_ignore_ascii_case("localhost"));
    if literal.is_some_and(|ip| !endpoint_ip_allowed(ip, allow_loopback)) || (localhost && !allow_loopback) {
        return Err(ApiError::bad_request(
            "endpoint: link-local, metadata and loopback addresses are not allowed for object storage",
        ));
    }

    Ok(url)
}

pub(super) fn s3_error(e: S3Error) -> ApiError {
    match &e {
        S3Error::Invalid(m) => ApiError::bad_request(m.clone()),
        S3Error::Service { code, message, .. } => match code.as_str() {
            "NoSuchBucket" | "NoSuchKey" => ApiError::not_found(message.clone()),
            "BucketNotEmpty" | "BucketAlreadyExists" | "BucketAlreadyOwnedByYou" => {
                ApiError::conflict(format!("{code}: {message}"))
            }
            _ => ApiError::new(StatusCode::BAD_GATEWAY, "store_error", e.to_string()),
        },
        S3Error::Transport(_) => ApiError::new(StatusCode::BAD_GATEWAY, "store_unreachable", e.to_string()),
    }
}

/// Checks a credential: ListBuckets, or a one-key listing of `bucket`.
async fn check_access(client: &Client, bucket: Option<&str>) -> ApiResult<()> {
    match bucket.filter(|b| !b.is_empty()) {
        Some(b) => client.list_objects(b, "", None, 1).await.map(|_| ()).map_err(s3_error),
        None => client.list_buckets().await.map(|_| ()).map_err(s3_error),
    }
}

pub async fn connect(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Json(req): Json<ConnectRequest>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let ws_ref = EntityRef::new(EntityKind::Workspace, ws);
    caller.authorize(Action::StorageManage, ws_ref.clone()).await?;
    let owner = checked_owner(&state, &caller, ws, req.environment_id).await?;
    let name = clean_name(&req.name)?;

    if !EXTERNAL_PROVIDERS.contains(&req.provider.as_str()) {
        return Err(ApiError::bad_request(format!("provider: one of {}", EXTERNAL_PROVIDERS.join(", "))));
    }

    let endpoint = parse_endpoint(&req.endpoint, state.config.storage_allow_loopback)?;
    let region = req.region.trim();
    if region.is_empty() || region.len() > 40 || !region.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        return Err(ApiError::bad_request("region: letters, digits and - (us-east-1 when unsure)"));
    }

    if req.access_key.trim().is_empty()
        || req.access_key.len() > 256
        || req.secret_key.is_empty()
        || req.secret_key.len() > 512
    {
        return Err(ApiError::bad_request("access key and secret key are required"));
    }

    let client = Client::new(
        S3Config {
            endpoint: endpoint.clone(),
            region: region.to_owned(),
            access_key: req.access_key.trim().to_owned(),
            secret_key: req.secret_key.clone(),
            path_style: req.path_style,
        },
        http(state.config.storage_allow_loopback)?,
    )
    .map_err(s3_error)?;
    check_access(&client, req.bucket.as_deref()).await?;
    let ep = endpoint.as_str().trim_end_matches('/').to_owned();
    let now = state.clock.now();
    let mut store = ObjectStore {
        id: ObjectStoreId::new(),
        workspace_id: ws,
        environment_id: owner,
        name: name.clone(),
        slug: String::new(),
        provider: req.provider.clone(),
        mode: MODE_EXTERNAL.into(),
        region: region.to_owned(),
        path_style: req.path_style,
        version: String::new(),
        deployed_version: None,
        runtime: "external".into(),
        topology: "external".into(),
        s3_port: endpoint.port_or_known_default().unwrap_or(443),
        console_port: None,
        endpoint: ep.clone(),
        credential_id: VaultItemId::new(),
        status: "connected".into(),
        last_error: None,
        last_job_id: None,
        created_by: caller.principal(),
        created_at: now,
        updated_at: now,
    };
    store.credential_id = vault_credential(
        &state,
        &caller,
        ws,
        format!("{name} (S3 access key)"),
        req.access_key.trim(),
        &req.secret_key,
        &ep,
        "Credential of a connected S3 service. Forge uses it for bucket and object management.",
        Vec::new(),
    )
    .await?;
    state.store.put_object_store(&store).await?;
    caller
        .audit(
            "storage.connect",
            &ws_ref,
            Some(ws),
            json!({
                "store_id": store.id, "name": name, "provider": store.provider, "endpoint": ep, "region": region,
                "environment_id": owner,
            }),
        )
        .await;

    Ok((StatusCode::CREATED, Json(view(&store, &[], &HashMap::new()))))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ActionRequest {
    pub action: String,
    /// `uninstall` only: also delete the data paths on every node.
    #[serde(default)]
    pub delete_data: bool,
    /// `uninstall` with `delete_data`: the store's name, typed to confirm.
    #[serde(default)]
    pub confirm: Option<String>,
}

pub async fn action(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Path(id): Path<ObjectStoreId>,
    Json(req): Json<ActionRequest>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let (store, members) = load(&state, ws, id).await?;
    let target = store_ref(&store, &members);

    if store.mode == MODE_EXTERNAL {
        if req.action != "disconnect" {
            return Err(ApiError::bad_request("a connected store supports `disconnect` only"));
        }

        caller.authorize_with(Action::StorageManage, target.clone(), denial_details(&store)).await?;
        state.store.delete_object_store(store.id).await?;
        state.store.delete_vault_item(store.credential_id).await?;
        caller
            .audit("storage.disconnect", &target, Some(ws), json!({ "store_id": store.id, "name": store.name }))
            .await;

        return Ok((StatusCode::OK, Json(json!({ "disconnected": store.id }))));
    }

    if !ACTIONS.contains(&req.action.as_str()) {
        return Err(ApiError::bad_request(format!("action: one of {}", ACTIONS.join(", "))));
    }

    let needed = if req.action == "status" { Action::ServerManage } else { Action::StorageManage };
    caller.authorize_with(needed, target.clone(), denial_details(&store)).await?;

    if req.action == "install" && members.iter().any(|m| m.data_paths == [forge_catalog::storage::LEGACY_DATA]) {
        return Err(ApiError::conflict(
            "an adopted App Store container is kept as it is and cannot be repaired by reinstalling",
        ));
    }

    if req.action == "reconfigure" && store.runtime != "docker" {
        return Err(ApiError::bad_request("reconfigure applies to Docker stores"));
    }

    let adopted = state.store.get_store_settings(store.id).await?.is_some_and(|s| s.adopted);
    if adopted && (req.action == "install" || req.delete_data) {
        return Err(ApiError::conflict(
            "an adopted container is not reinstalled and its data is never deleted by Forge; change it under Settings",
        ));
    }

    if crate::jobs::storage::BUSY.contains(&store.status.as_str())
        && req.action != "status"
        && job_active(&state, &store).await
    {
        return Err(ApiError::conflict(format!("the store is {}", store.status)));
    }

    let delete_data = req.action == "uninstall" && req.delete_data;
    if req.delete_data && req.action != "uninstall" {
        return Err(ApiError::bad_request("delete_data applies to `uninstall` only"));
    }

    if delete_data {
        if req.confirm.as_deref().map(str::trim) != Some(store.name.as_str()) {
            return Err(ApiError::bad_request("type the store's name to confirm deleting its data"));
        }

        caller.require_mfa("storage.uninstall", &target).await?;
    }

    let mut jobs = Vec::new();
    let mut skipped = Vec::new();

    for m in &members {
        if req.action == "uninstall" {
            if m.status == "removed" {
                continue;
            }

            // A node that is gone or revoked cannot run anything: its member
            // is dropped so the store can still be removed (best effort).
            let gone = match state.store.get_server(m.server_id).await {
                Ok(s) => s.status == forge_core::model::AgentStatus::Revoked,
                Err(_) => true,
            };

            if gone {
                let mut removed = m.clone();
                removed.status = "removed".into();
                state.store.put_store_member(&removed).await?;
                skipped.push(json!({ "server_id": m.server_id, "result": "node gone or revoked; member dropped" }));

                continue;
            }
        }

        jobs.push(submit_with(&state, &caller, &store, m.server_id, &req.action, delete_data).await?);
    }

    if req.action == "uninstall" && jobs.is_empty() {
        // Nothing left to run on: the store goes now (data on nodes untouched).
        state.store.delete_object_store(store.id).await?;
    }

    if req.action != "status" {
        caller
            .audit(
                &format!("storage.{}", req.action),
                &target,
                Some(ws),
                json!({
                    "store_id": store.id,
                    "name": store.name,
                    "delete_data": delete_data,
                    "jobs": jobs.iter().map(|j| j.id).collect::<Vec<_>>(),
                    "skipped": skipped,
                }),
            )
            .await;
    }

    if !skipped.is_empty() {
        return Ok((StatusCode::ACCEPTED, Json(json!({ "jobs": jobs, "skipped": skipped }))));
    }

    Ok((StatusCode::ACCEPTED, Json(json!({ "jobs": jobs }))))
}

/// Whether the store's busy status is backed by a job that is still queued
/// or running. A cancelled (or otherwise finished) job never reports back, so
/// without this the store would stay `installing` and refuse every action.
async fn job_active(state: &AppState, store: &ObjectStore) -> bool {
    let Some(job_id) = store.last_job_id else {
        // Submitted but not dispatched yet.
        return true;
    };

    match state.store.get_job(job_id).await {
        Ok(job) => matches!(job.status, JobStatus::Queued | JobStatus::Running),
        Err(_) => false,
    }
}

/// The endpoint to use: the store's own, or for a distributed store the
/// first member that accepts a TCP connection (any member serves the whole
/// cluster), so browsing keeps working while a member is down.
async fn reachable_endpoint(state: &AppState, store: &ObjectStore) -> ApiResult<String> {
    if store.topology != TOPOLOGY_DISTRIBUTED {
        return Ok(store.endpoint.clone());
    }

    let members = state.store.list_store_members(store.id).await?;

    for m in members.iter().filter(|m| m.status != "removed") {
        let addr = (m.endpoint_host.as_str(), store.s3_port);
        let probe = tokio::time::timeout(Duration::from_millis(1500), tokio::net::TcpStream::connect(addr));
        if matches!(probe.await, Ok(Ok(_))) {
            return Ok(format!("http://{}:{}", m.endpoint_host, store.s3_port));
        }
    }

    Ok(store.endpoint.clone())
}

/// S3 client for a store, with its credential unsealed for this request.
pub(super) async fn client(state: &AppState, store: &ObjectStore) -> ApiResult<Client> {
    let item = state
        .store
        .get_vault_item(store.credential_id)
        .await?
        .filter(|i| i.workspace_id == store.workspace_id)
        .ok_or_else(|| ApiError::conflict("the store's credential is missing from the vault"))?;
    let secret = state
        .envelope
        .open(&item.sealed, &vault_aad(item.id))
        .map_err(|_| DomainError::internal("cannot unseal the store credential"))?;
    let secret_key = String::from_utf8(secret).map_err(|_| DomainError::internal("store secret is not text"))?;
    let endpoint = parse_endpoint(&reachable_endpoint(state, store).await?, state.config.storage_allow_loopback)?;

    Client::new(
        S3Config {
            endpoint,
            region: store.region.clone(),
            access_key: item.username,
            secret_key,
            path_style: store.path_style,
        },
        http(state.config.storage_allow_loopback)?,
    )
    .map_err(s3_error)
}

pub async fn test(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Path(id): Path<ObjectStoreId>,
) -> ApiResult<Json<Value>> {
    let (store, _) = authorized(&state, &caller, ws, id, Action::StorageData).await?;
    let c = client(&state, &store).await?;
    let started = std::time::Instant::now();
    let result = c.list_buckets().await;
    let ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);

    Ok(Json(match result {
        Ok(b) => json!({ "ok": true, "latency_ms": ms, "buckets": b.len() }),
        Err(e) => json!({ "ok": false, "latency_ms": ms, "error": e.to_string() }),
    }))
}

pub async fn buckets(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Path(id): Path<ObjectStoreId>,
) -> ApiResult<Json<Value>> {
    let (store, _, reach) = reachable(&state, &caller, ws, id, Action::StorageData).await?;
    let shown = shown_error(matches!(reach, Reach::Grants(_)));
    let c = client(&state, &store).await.map_err(&shown)?;
    let mut list = c.list_buckets().await.map_err(s3_error).map_err(&shown)?;

    // Through grants only: the granted buckets, not the store's other ones.
    if let Reach::Grants(grants) = &reach {
        list.retain(|b| grants.iter().any(|g| g.bucket == b.name));
    }

    Ok(Json(json!(list)))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct BucketRequest {
    pub name: String,
}

pub async fn create_bucket(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Path(id): Path<ObjectStoreId>,
    Json(req): Json<BucketRequest>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let (store, members) = authorized(&state, &caller, ws, id, Action::StorageData).await?;

    if !forge_s3::valid_bucket(&req.name) {
        return Err(ApiError::bad_request(
            "bucket name: 3-63 lowercase letters, digits, dots and hyphens, starting and ending with a letter or digit",
        ));
    }

    let c = client(&state, &store).await?;

    // Some stores answer "created" for a bucket that is already there: only
    // a bucket that was missing is a new one.
    let missing = c.list_objects(&req.name, "", None, 1).await.is_err();
    c.create_bucket(&req.name).await.map_err(s3_error)?;
    let target = store_ref(&store, &members);

    let revoked = if missing {
        super::storage_grants::revoke_for_bucket(&state, &caller, &store, &target, &req.name, REASON_BUCKET_RECREATED)
            .await?
    } else {
        0
    };

    caller
        .audit(
            "storage.bucket.create",
            &target,
            Some(ws),
            json!({ "store_id": store.id, "bucket": req.name, "stale_grants_revoked": revoked }),
        )
        .await;

    Ok((StatusCode::CREATED, Json(json!({ "name": req.name }))))
}

pub async fn delete_bucket(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Path((id, bucket)): Path<(ObjectStoreId, String)>,
) -> ApiResult<StatusCode> {
    let (store, members) = authorized(&state, &caller, ws, id, Action::StorageManage).await?;
    client(&state, &store).await?.delete_bucket(&bucket).await.map_err(s3_error)?;
    let target = store_ref(&store, &members);

    // Two steps that cannot be one transaction (the store, then the grants).
    // The deletion is audited first, so it is on record even if revoking
    // fails; grants left live then are revoked when a bucket of this name is
    // created again (`create_bucket`).
    caller.audit("storage.bucket.delete", &target, Some(ws), json!({ "store_id": store.id, "bucket": bucket })).await;
    super::storage_grants::revoke_for_bucket(&state, &caller, &store, &target, &bucket, REASON_BUCKET_DELETED).await?;

    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListQuery {
    #[serde(default)]
    pub prefix: String,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
}

pub async fn objects(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Path((id, bucket)): Path<(ObjectStoreId, String)>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Value>> {
    let BucketScope { store, grant_id, .. } =
        authorized_for_bucket(&state, &caller, ws, id, &bucket, BucketAccess::Read).await?;
    let shown = shown_error(grant_id.is_some());

    if q.prefix.len() > 1024 || q.token.as_ref().is_some_and(|t| t.len() > 2048) {
        return Err(ApiError::bad_request("prefix or token too long"));
    }

    let page = client(&state, &store)
        .await
        .map_err(&shown)?
        .list_objects(&bucket, &q.prefix, q.token.as_deref(), q.limit.unwrap_or(200))
        .await
        .map_err(s3_error)
        .map_err(&shown)?;

    Ok(Json(json!(page)))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct KeyQuery {
    pub key: String,
}

/// `attachment; filename="…"` with a safe ASCII fallback and the UTF-8 name.
fn disposition(key: &str) -> HeaderValue {
    let base = key.rsplit('/').next().unwrap_or("download");
    let ascii: String =
        base.chars().map(|c| if c.is_ascii_graphic() && c != '"' && c != '\\' { c } else { '_' }).take(150).collect();
    let utf8: String = base
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_') {
                char::from(b).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();

    HeaderValue::from_str(&format!("attachment; filename=\"{ascii}\"; filename*=UTF-8''{utf8}"))
        .unwrap_or_else(|_| HeaderValue::from_static("attachment"))
}

pub async fn download(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Path((id, bucket)): Path<(ObjectStoreId, String)>,
    Query(q): Query<KeyQuery>,
) -> ApiResult<Response> {
    let BucketScope { store, members, grant_id } =
        authorized_for_bucket(&state, &caller, ws, id, &bucket, BucketAccess::Read).await?;
    let permit = transfers()
        .try_acquire()
        .map_err(|_| ApiError::new(StatusCode::TOO_MANY_REQUESTS, "busy", "too many transfers; try again shortly"))?;
    let shown = shown_error(grant_id.is_some());
    let c = client(&state, &store).await.map_err(&shown)?;
    let (head, resp) = c.get_object(&bucket, &q.key).await.map_err(s3_error).map_err(&shown)?;
    caller
        .audit(
            "storage.object.download",
            &store_ref(&store, &members),
            Some(ws),
            json!({ "store_id": store.id, "bucket": bucket, "key": q.key, "size": head.size, "grant_id": grant_id }),
        )
        .await;

    // The permit lives as long as the body stream.
    let stream = futures::StreamExt::map(resp.bytes_stream(), move |chunk| {
        let _held = &permit;
        chunk.map_err(std::io::Error::other)
    });
    let mut out = Body::from_stream(stream).into_response();
    let h = out.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        head.content_type
            .as_deref()
            .and_then(|c| HeaderValue::from_str(c).ok())
            .unwrap_or_else(|| HeaderValue::from_static("application/octet-stream")),
    );

    if head.size > 0 {
        h.insert(header::CONTENT_LENGTH, HeaderValue::from(head.size));
    }

    h.insert(header::CONTENT_DISPOSITION, disposition(&q.key));
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));

    // The stored type comes from whoever uploaded the object: if a browser
    // ever renders it, it runs sandboxed with nothing allowed.
    h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("sandbox; default-src 'none'"));

    Ok(out)
}

pub async fn upload(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Path((id, bucket)): Path<(ObjectStoreId, String)>,
    Query(q): Query<KeyQuery>,
    headers: HeaderMap,
    body: Body,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let BucketScope { store, members, grant_id } =
        authorized_for_bucket(&state, &caller, ws, id, &bucket, BucketAccess::ReadWrite).await?;
    let len: u64 =
        headers.get(header::CONTENT_LENGTH).and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok()).ok_or_else(
            || ApiError::new(StatusCode::LENGTH_REQUIRED, "length_required", "Content-Length is required"),
        )?;

    if len > MAX_UPLOAD {
        return Err(ApiError::new(StatusCode::PAYLOAD_TOO_LARGE, "too_large", "uploads are limited to 5 GiB"));
    }

    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .filter(|c| c.len() <= 200)
        .unwrap_or("application/octet-stream")
        .to_owned();
    let _permit = transfers()
        .try_acquire()
        .map_err(|_| ApiError::new(StatusCode::TOO_MANY_REQUESTS, "busy", "too many transfers; try again shortly"))?;
    let shown = shown_error(grant_id.is_some());
    let c = client(&state, &store).await.map_err(&shown)?;
    let stream = reqwest::Body::wrap_stream(body.into_data_stream());
    c.put_object(&bucket, &q.key, stream, len, &content_type).await.map_err(s3_error).map_err(&shown)?;
    caller
        .audit(
            "storage.object.upload",
            &store_ref(&store, &members),
            Some(ws),
            json!({ "store_id": store.id, "bucket": bucket, "key": q.key, "size": len, "grant_id": grant_id }),
        )
        .await;

    Ok((StatusCode::CREATED, Json(json!({ "bucket": bucket, "key": q.key, "size": len }))))
}

pub async fn delete_object(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Path((id, bucket)): Path<(ObjectStoreId, String)>,
    Query(q): Query<KeyQuery>,
) -> ApiResult<StatusCode> {
    let BucketScope { store, members, grant_id } =
        authorized_for_bucket(&state, &caller, ws, id, &bucket, BucketAccess::ReadWrite).await?;
    let shown = shown_error(grant_id.is_some());
    let c = client(&state, &store).await.map_err(&shown)?;
    c.delete_object(&bucket, &q.key).await.map_err(s3_error).map_err(&shown)?;
    caller
        .audit(
            "storage.object.delete",
            &store_ref(&store, &members),
            Some(ws),
            json!({ "store_id": store.id, "bucket": bucket, "key": q.key, "grant_id": grant_id }),
        )
        .await;

    Ok(StatusCode::NO_CONTENT)
}

// ------------------------------------------------------------------ adoption

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AdoptRequest {
    pub name: String,
    #[schemars(with = "String")]
    pub server_id: ServerId,
    /// Host the control plane reaches the node on (default: its hostname).
    #[serde(default)]
    pub endpoint_host: Option<String>,
    /// The environment the store belongs to (D-107); omit for a shared store.
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub environment_id: Option<EnvironmentId>,
    /// Any RustFS container on the node, by name (D-170); omit for the App
    /// Store container.
    #[serde(default)]
    pub container: Option<String>,
    /// Ports, when they cannot be read from the container (host network).
    #[serde(default)]
    pub s3_port: Option<u16>,
    #[serde(default)]
    pub console_port: Option<u16>,
}

/// Adopts the App Store `rustfs` container on a node as a managed store
/// without recreating it: same container, data and credential. The
/// credential moves into the vault; the App Store stops managing it.
pub async fn adopt(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Json(req): Json<AdoptRequest>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    use forge_core::ports::InstallationRepository;
    caller.authorize(Action::StorageManage, server_ref(req.server_id)).await?;
    let owner = checked_owner(&state, &caller, ws, req.environment_id).await?;
    let name = clean_name(&req.name)?;
    let server = state.store.get_server(req.server_id).await.map_err(|_| ApiError::not_found("server not found"))?;
    if server.workspace_id != ws {
        return Err(ApiError::not_found("server not found"));
    }

    if let Some(container) = req.container.clone().filter(|c| !c.is_empty()) {
        // Adoption reads the container's root credential on the node: as
        // much as reading its environment, so node administrators only.
        caller.authorize(Action::ServerManage, server_ref(server.id)).await?;

        if !crate::api::containers::valid_container_id(&container) {
            return Err(ApiError::bad_request("container: a container name"));
        }

        if req.s3_port.is_some_and(|p| p < 1024) || req.console_port.is_some_and(|p| p < 1024) {
            return Err(ApiError::bad_request("ports: 1024-65535"));
        }

        let known = crate::facts::node_address(&state, server.id).await?;
        let host = req
            .endpoint_host
            .as_deref()
            .map(str::trim)
            .filter(|h| !h.is_empty())
            .or(known.as_deref())
            .unwrap_or(&server.hostname)
            .to_owned();

        // Root-signed admin calls go to this address: one of the node's own.
        let own = [known.as_deref(), Some(server.hostname.as_str())];
        if !own.contains(&Some(host.as_str())) {
            return Err(ApiError::bad_request("endpoint host: the node's address or host name"));
        }

        let (store, member, job) = super::objstore::adopt_container(
            &state,
            &caller,
            ws,
            super::objstore::AdoptContainer {
                server_id: server.id,
                name,
                container,
                owner,
                endpoint_host: host,
                s3_port: req.s3_port,
                console_port: req.console_port,
            },
        )
        .await?;
        let names = hostnames(&state).await?;

        return Ok((StatusCode::ACCEPTED, Json(json!({ "store": view(&store, &[member], &names), "job": job }))));
    }

    let inst = state
        .store
        .list_installations(Some(server.id))
        .await?
        .into_iter()
        .find(|i| i.package == "rustfs")
        .ok_or_else(|| ApiError::not_found("no App Store RustFS installation on this node"))?;
    let port = |k: &str, d: u16| inst.params.get(k).and_then(|v| v.parse().ok()).unwrap_or(d);
    let (s3_port, console_port) = (port("port", 9000), port("console_port", 9001));

    for m in state.store.list_node_store_members(server.id).await? {
        if m.data_paths.iter().any(|p| p == forge_catalog::storage::LEGACY_DATA) {
            return Err(ApiError::conflict("the App Store RustFS container is already adopted"));
        }

        if let Some(o) = state.store.get_object_store(m.store_id).await?
            && (o.slug == "rustfs" || o.s3_port == s3_port)
        {
            return Err(ApiError::conflict("another object store on this node uses the name `rustfs` or that port"));
        }
    }

    let known = crate::facts::node_address(&state, server.id).await?;
    let host = req
        .endpoint_host
        .as_deref()
        .map(str::trim)
        .filter(|h| !h.is_empty())
        .or(known.as_deref())
        .unwrap_or(&server.hostname);

    if !forge_catalog::storage::valid_host(host) {
        return Err(ApiError::bad_request("endpoint host: a hostname or IP address"));
    }

    let now = state.clock.now();
    let id = ObjectStoreId::new();
    let store = ObjectStore {
        id,
        workspace_id: ws,
        environment_id: owner,
        name: name.clone(),
        slug: "rustfs".into(),
        provider: "rustfs".into(),
        mode: MODE_MANAGED.into(),
        region: "us-east-1".into(),
        path_style: true,
        version: inst.version.clone(),
        deployed_version: None,
        runtime: "docker".into(),
        topology: TOPOLOGY_STANDALONE.into(),
        s3_port,
        console_port: Some(console_port),
        endpoint: format!("http://{host}:{s3_port}"),
        // Filled by the adopt job with the container's own credential.
        credential_id: VaultItemId::new(),
        status: "adopting".into(),
        last_error: None,
        last_job_id: None,
        created_by: caller.principal(),
        created_at: now,
        updated_at: now,
    };
    let member = StoreMember {
        store_id: id,
        server_id: server.id,
        data_paths: vec![forge_catalog::storage::LEGACY_DATA.into()],
        endpoint_host: host.to_owned(),
        require_mount: false,
        status: "adopting".into(),
        capacity_bytes: None,
        free_bytes: None,
        dedicated: None,
        observed_at: None,
    };
    spec(&store, &member, std::slice::from_ref(&member), None).map_err(ApiError::bad_request)?;
    state.store.put_object_store(&store).await?;
    state.store.put_store_member(&member).await?;
    let job = submit(&state, &caller, &store, server.id, "adopt").await?;
    caller
        .audit(
            "storage.adopt",
            &server_ref(server.id),
            Some(ws),
            json!({
                "store_id": id, "name": name, "installation_id": inst.id, "s3_port": s3_port, "job_id": job.id,
                "environment_id": owner,
            }),
        )
        .await;
    let names = hostnames(&state).await?;

    Ok((StatusCode::ACCEPTED, Json(json!({ "store": view(&store, &[member], &names), "job": job }))))
}

// -------------------------------------------------------------- access keys

/// App keys work on managed RustFS stores (its admin API); connected stores
/// manage keys at their provider.
pub(super) fn keys_supported(store: &ObjectStore) -> ApiResult<()> {
    if store.mode != MODE_MANAGED || store.provider != "rustfs" {
        return Err(ApiError::conflict(
            "access keys are issued for managed RustFS stores; create keys for a connected store at its provider",
        ));
    }

    Ok(())
}

/// Vault items holding this store's app keys, by access key.
async fn key_items(state: &AppState, store: &ObjectStore) -> ApiResult<HashMap<String, VaultItem>> {
    let tag = format!("objstore:{}", store.id);
    Ok(state
        .store
        .list_vault_items(store.workspace_id)
        .await?
        .into_iter()
        .filter(|i| i.tags.contains(&tag) && i.id != store.credential_id)
        .map(|i| (i.username.clone(), i))
        .collect())
}

pub async fn list_keys(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Path(id): Path<ObjectStoreId>,
) -> ApiResult<Json<Vec<Value>>> {
    let (store, _, via) = authorized_via(&state, &caller, ws, id, Action::StorageData).await?;
    keys_supported(&store)?;
    let mut items = key_items(&state, &store).await?;

    // Through the owning environment only: the keys attached to services of
    // that environment. Standalone keys, other environments' keys and keys
    // Forge did not issue are left out.
    let own_only = via == Via::Owner;
    if own_only {
        let own: HashSet<ServiceId> = match store.environment_id {
            Some(ns) => state.store.list_services(Some(ns)).await?.into_iter().map(|a| a.id).collect(),
            None => HashSet::new(),
        };

        items.retain(|_, i| app_storage::attached_service(i).is_some_and(|a| own.contains(&a)));
    }

    let accounts = client(&state, &store).await?.list_service_accounts().await.map_err(s3_error)?;

    Ok(Json(
        accounts
            .iter()
            .filter(|a| !own_only || a["accessKey"].as_str().is_some_and(|ak| items.contains_key(ak)))
            .map(|a| {
                let ak = a["accessKey"].as_str().unwrap_or_default();
                let item = items.get(ak);
                json!({
                    "access_key": ak,
                    "name": a["name"],
                    "description": a["description"],
                    "status": a["accountStatus"],
                    "expiration": a["expiration"],
                    "vault_item_id": item.map(|i| i.id),
                    "bucket": item.and_then(|i| i.tags.iter().find_map(|t| t.strip_prefix("bucket:").map(str::to_owned))),
                    "access": item.and_then(|i| i.tags.iter().find_map(|t| t.strip_prefix("access:").map(str::to_owned))),
                    "managed_by_forge": item.is_some(),
                })
            })
            .collect(),
    ))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct KeyRequest {
    /// What the key is for (a service name).
    pub name: String,
    pub bucket: String,
    /// `read` or `readwrite`.
    #[serde(default = "default_access")]
    pub access: String,
}

fn default_access() -> String {
    "readwrite".into()
}

fn clean_key_name(n: &str) -> ApiResult<String> {
    let n = n.trim();
    if n.is_empty() || n.len() > 32 || !n.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    {
        return Err(ApiError::bad_request("key name: 1 to 32 letters, digits, - _ ."));
    }

    Ok(n.to_owned())
}

/// Issues a key limited to one bucket. The secret goes straight into the
/// vault (reveal there with `secret.read`); it is never in the response.
pub async fn create_key(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Path(id): Path<ObjectStoreId>,
    Json(req): Json<KeyRequest>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let (store, members) = authorized(&state, &caller, ws, id, Action::StorageManage).await?;
    keys_supported(&store)?;
    let name = clean_key_name(&req.name)?;

    if !forge_s3::valid_bucket(&req.bucket) {
        return Err(ApiError::bad_request("bucket name"));
    }

    let write = match req.access.as_str() {
        "read" => false,
        "readwrite" => true,
        _ => return Err(ApiError::bad_request("access: read or readwrite")),
    };

    let c = client(&state, &store).await?;

    // The bucket must exist, so a typo does not create a useless key.
    c.list_objects(&req.bucket, "", None, 1).await.map_err(s3_error)?;
    let (ak, sk) = c
        .add_service_account(
            &name,
            &format!("Forge app key for bucket {}", req.bucket),
            &forge_s3::bucket_policy(&req.bucket, write),
        )
        .await
        .map_err(s3_error)?;
    let item_id = vault_credential(
        &state,
        &caller,
        ws,
        format!("{} / {} ({})", store.name, req.bucket, name),
        &ak,
        &sk,
        &store.endpoint,
        "Service access key limited to one bucket, issued by Forge Object Storage.",
        members.iter().map(|m| VaultLink { kind: "node".into(), id: m.server_id.to_string() }).collect(),
    )
    .await?;
    tag_key_item(&state, item_id, &store, &req.bucket, &req.access).await?;

    // Forge's record of the key (D-170); RustFS holds the key itself.
    let grant = forge_core::storage_iam::KeyGrant {
        bucket: req.bucket.clone(),
        prefix: String::new(),
        access: if write {
            forge_core::storage::BucketAccess::ReadWrite
        } else {
            forge_core::storage::BucketAccess::Read
        },
    };
    let now = state.clock.now();
    state
        .store
        .put_store_key(&forge_core::storage_iam::StoreKey {
            id: forge_core::StoreKeyId::new(),
            store_id: store.id,
            access_key: ak.clone(),
            name: name.clone(),
            grants: Some(vec![grant]),
            vault_item_id: Some(item_id),
            created_by: caller.principal().to_string(),
            created_at: now,
            updated_at: now,
            synced_at: Some(now),
        })
        .await?;
    caller
        .audit(
            "storage.key.create",
            &store_ref(&store, &members),
            Some(ws),
            json!({ "store_id": store.id, "access_key": ak, "name": name, "bucket": req.bucket, "access": req.access }),
        )
        .await;

    Ok((
        StatusCode::CREATED,
        Json(
            json!({ "access_key": ak, "name": name, "bucket": req.bucket, "access": req.access, "vault_item_id": item_id }),
        ),
    ))
}

pub(super) async fn tag_key_item(
    state: &AppState,
    id: VaultItemId,
    store: &ObjectStore,
    bucket: &str,
    access: &str,
) -> ApiResult<()> {
    let mut item = state.store.get_vault_item(id).await?.ok_or_else(|| DomainError::internal("vault item vanished"))?;
    item.tags = vec![
        "object-storage".into(),
        format!("objstore:{}", store.id),
        format!("bucket:{bucket}"),
        format!("access:{access}"),
    ];
    state.store.put_vault_item(&item).await?;

    Ok(())
}

/// Revokes a key at the store and removes it from the vault.
pub async fn delete_key(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Path((id, access_key)): Path<(ObjectStoreId, String)>,
) -> ApiResult<StatusCode> {
    let (store, members) = authorized(&state, &caller, ws, id, Action::StorageManage).await?;
    keys_supported(&store)?;
    let root = state.store.get_vault_item(store.credential_id).await?;
    if root.is_some_and(|r| r.username == access_key) {
        return Err(ApiError::conflict("the store's root key cannot be revoked here"));
    }

    client(&state, &store).await?.delete_service_account(&access_key).await.map_err(s3_error)?;

    if let Some(item) = key_items(&state, &store).await?.remove(&access_key) {
        state.store.delete_vault_item(item.id).await?;
    }

    state.objstore_iam.forget(&store, &access_key).await?;
    caller
        .audit(
            "storage.key.revoke",
            &store_ref(&store, &members),
            Some(ws),
            json!({ "store_id": store.id, "access_key": access_key }),
        )
        .await;

    Ok(StatusCode::NO_CONTENT)
}

/// Replaces a Forge-issued key: creates the new key, proves it works on its
/// bucket, updates the vault item, then retires the old key.
pub async fn rotate_key(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Path((id, access_key)): Path<(ObjectStoreId, String)>,
) -> ApiResult<Json<Value>> {
    let (store, members) = authorized(&state, &caller, ws, id, Action::StorageManage).await?;
    keys_supported(&store)?;
    let mut item = key_items(&state, &store)
        .await?
        .remove(&access_key)
        .ok_or_else(|| ApiError::not_found("no Forge-issued key with that access key"))?;
    let tag = |p: &str| item.tags.iter().find_map(|t| t.strip_prefix(p).map(str::to_owned));
    let (bucket, access) = (tag("bucket:").unwrap_or_default(), tag("access:").unwrap_or_else(|| "readwrite".into()));
    let name = item.name.rsplit_once('(').map_or("app", |(_, n)| n.trim_end_matches(')')).to_owned();
    let c = client(&state, &store).await?;

    // The key keeps its grants (D-170): the policy recorded for it, or the
    // one bucket of keys issued before grants were recorded.
    let recorded = state
        .store
        .list_store_keys(store.id)
        .await?
        .into_iter()
        .find(|r| r.access_key == access_key)
        .and_then(|r| r.grants);

    let policy = match &recorded {
        Some(g) => serde_json::to_value(forge_core::storage_iam::policy_for(g)).map_err(DomainError::internal)?,
        None => forge_s3::bucket_policy(&bucket, access == "readwrite"),
    };

    let (ak, sk) = c
        .add_service_account(
            &clean_key_name(&name).unwrap_or_else(|_| "app".into()),
            &format!("Forge app key for bucket {bucket}"),
            &policy,
        )
        .await
        .map_err(s3_error)?;

    // Prove the new key before retiring the old one.
    let probe = Client::new(
        S3Config {
            endpoint: parse_endpoint(&store.endpoint, state.config.storage_allow_loopback)?,
            region: store.region.clone(),
            access_key: ak.clone(),
            secret_key: sk.clone(),
            path_style: store.path_style,
        },
        http(state.config.storage_allow_loopback)?,
    )
    .map_err(s3_error)?;
    let mut tried = probe.list_objects(&bucket, "", None, 1).await;

    for _ in 0..5 {
        if tried.is_ok() {
            break;
        }

        // New keys can take a moment to replicate on distributed stores.
        tokio::time::sleep(Duration::from_millis(400)).await;
        tried = probe.list_objects(&bucket, "", None, 1).await;
    }

    if let Err(e) = tried {
        let _ = c.delete_service_account(&ak).await;
        return Err(ApiError::new(
            StatusCode::BAD_GATEWAY,
            "rotation_failed",
            format!("the new key did not work, so the old one stays: {e}"),
        ));
    }

    item.username = ak.clone();
    item.sealed = state.envelope.seal(sk.as_bytes(), &vault_aad(item.id)).map_err(DomainError::internal)?;
    item.rotated_at = state.clock.now();
    item.updated_at = item.rotated_at;
    state.store.put_vault_item(&item).await?;
    state.objstore_iam.rotated(&store, &access_key, &ak).await?;

    // The rotation stands once the vault holds the new key; a failure to
    // retire the old key is reported (and audited) instead of hidden.
    let retired = c.delete_service_account(&access_key).await;
    let warning = retired.as_ref().err().map(|e| format!("the old key {access_key} is still valid; revoke it: {e}"));
    caller
        .audit(
            "storage.key.rotate",
            &store_ref(&store, &members),
            Some(ws),
            json!({
                "store_id": store.id, "old_access_key": access_key, "access_key": ak, "bucket": bucket,
                "old_key_retired": retired.is_ok(),
            }),
        )
        .await;

    Ok(Json(json!({
        "access_key": ak, "bucket": bucket, "access": access, "vault_item_id": item.id,
        "old_key_retired": retired.is_ok(), "warning": warning,
    })))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn bucket_grants_never_count_for_a_connector() {
        assert!(grants_count(false, false), "a person's session, an API key, a service-account token");
        assert!(!grants_count(true, false), "a connector's own connection");
        assert!(!grants_count(false, true), "a connector's request replayed with its approver's session");
        assert!(!grants_count(true, true));
    }

    #[test]
    fn a_grant_holder_learns_nothing_about_the_store_from_an_error() {
        let secret = "error sending request for url (http://10.0.0.7:9000/media): connection refused";

        for e in [
            s3_error(S3Error::Transport(secret.into())),
            ApiError::conflict("the store's credential is missing from the vault"),
            ApiError::new(StatusCode::BAD_GATEWAY, "store_error", secret),
        ] {
            let shown = shown_error(true)(e);
            assert_eq!((shown.status, shown.code), (StatusCode::BAD_GATEWAY, "store_unreachable"));
            assert_eq!(shown.message, "the store can't be reached");
        }

        let gone = shown_error(true)(ApiError::not_found("NoSuchKey at http://10.0.0.7:9000/media/a.txt"));
        assert_eq!((gone.status, gone.message.as_str()), (StatusCode::NOT_FOUND, "no such bucket or object"));
        let invalid = shown_error(true)(ApiError::bad_request(secret));
        assert_eq!(invalid.message, "the bucket name or the key is not valid");

        // Someone a role let in keeps the store's own answer.
        assert_eq!(shown_error(false)(ApiError::bad_request(secret)).message, secret);
    }

    #[test]
    fn endpoints_are_plain_urls() {
        assert!(parse_endpoint("https://s3.eu-west-1.amazonaws.com", false).is_ok());
        assert!(parse_endpoint("http://10.0.0.5:9000/", false).is_ok());
        assert!(parse_endpoint("https://user:pw@host", false).is_err());
        assert!(parse_endpoint("https://host/?X-Amz-Credential=x", false).is_err());
        assert!(parse_endpoint("file:///etc/passwd", false).is_err());

        // Never the metadata service or link-local, whatever the setting.
        for bad in
            ["http://169.254.169.254/", "http://[fe80::1]:9000", "http://[fd00:ec2::254]/", "http://0.0.0.0:9000"]
        {
            assert!(parse_endpoint(bad, true).is_err(), "{bad}");
        }

        assert!(parse_endpoint("http://[::ffff:169.254.169.254]/", true).is_err(), "IPv4-mapped");

        // Loopback only when allowed (labs and tests).
        for lo in ["http://127.0.0.1:9000", "http://[::1]:9000", "http://localhost:9000"] {
            assert!(parse_endpoint(lo, false).is_err(), "{lo}");
            assert!(parse_endpoint(lo, true).is_ok(), "{lo}");
        }
    }

    #[tokio::test]
    async fn resolver_drops_forbidden_addresses() {
        use reqwest::dns::Resolve;
        let strict = GuardedResolver { allow_loopback: false };
        let name: reqwest::dns::Name = "localhost".parse().unwrap();
        assert!(strict.resolve(name).await.is_err(), "localhost resolves to loopback only");
        let lab = GuardedResolver { allow_loopback: true };
        let name: reqwest::dns::Name = "localhost".parse().unwrap();
        assert!(lab.resolve(name).await.is_ok());
    }

    #[test]
    fn generated_keys_are_alphanumeric() {
        let k = random_alnum(40).unwrap();
        assert_eq!(k.len(), 40);
        assert!(k.bytes().all(|b| b.is_ascii_alphanumeric()));
        assert_ne!(k, random_alnum(40).unwrap());
    }

    #[test]
    fn download_names_are_safe() {
        let v = disposition("dir/a \"b\".txt");
        let s = v.to_str().unwrap();
        assert!(s.starts_with("attachment; filename=\"a__b_.txt\""));
        assert!(s.contains("filename*=UTF-8''a%20%22b%22.txt"));
    }
}
