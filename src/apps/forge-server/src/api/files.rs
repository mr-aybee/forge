//! File explorer API: roots, browsing, viewing, downloads, editing, uploads,
//! copy/move/delete, trash, revisions and search on a node.
//!
//! Handlers are thin: every decision (root resolution, Cedar actions,
//! read-only roots, protected paths, upload ownership, audit) is made by
//! [`forge_files::FileService`], which the SFTP gateway uses too. The node
//! enforces confinement and the protected-path policy again.

use axum::Json;
use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use base64::Engine as _;
use forge_core::ServerId;
use forge_core::files::FileGrant;
use forge_core::files::{
    ArchiveFormat, BULK_MAX_ITEMS, DEFAULT_LIST_LIMIT, FileOp, FilePath, MAX_READ_BYTES, NODE_ROOT, Overwrite,
    RootKind, RootSpec, SearchSpec,
};
use forge_files::{Actor, Audit, FileError};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// Bytes fetched per download chunk.
/// One piece of a download: several node reads in one round trip (#15).
const DOWNLOAD_CHUNK: u32 = 2 * 1024 * 1024;
/// Largest file the browser download relays.
const MAX_DOWNLOAD: u64 = 512 * 1024 * 1024;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/servers/{id}/files/roots", get(roots).post(add_root))
        .route("/servers/{id}/files/roots/{root}", delete(delete_root))
        .route("/servers/{id}/files/list", get(list))
        .route("/servers/{id}/files/stat", get(stat))
        .route("/servers/{id}/files/read", get(read))
        .route("/servers/{id}/files/hash", get(hash))
        .route("/servers/{id}/files/download", get(download))
        .route("/servers/{id}/files/mkdir", post(mkdir))
        .route("/servers/{id}/files/move", post(move_entry))
        .route("/servers/{id}/files/copy", post(copy))
        .route("/servers/{id}/files/delete", post(delete_entry))
        .route("/servers/{id}/files/save", post(save))
        .route("/servers/{id}/files/uploads", post(begin_upload))
        .route("/servers/{id}/files/uploads/{upload}", get(upload_status).put(upload_chunk).delete(abort_upload))
        .route("/servers/{id}/files/uploads/{upload}/commit", post(commit_upload))
        .route("/servers/{id}/files/trash", get(trash))
        .route("/servers/{id}/files/trash/restore", post(trash_restore))
        .route("/servers/{id}/files/trash/purge", post(trash_purge))
        .route("/servers/{id}/files/revisions", get(revisions))
        .route("/servers/{id}/files/revision", get(revision))
        .route("/servers/{id}/files/search", post(search))
        .route("/servers/{id}/files/chmod", post(chmod))
        .route("/servers/{id}/files/chown", post(chown))
        .route("/servers/{id}/files/acl", get(acl))
        .route("/servers/{id}/files/compress", post(compress))
        .route("/servers/{id}/files/extract", post(extract))
        .route("/servers/{id}/files/bulk", post(bulk))
        .route("/servers/{id}/files/archive", post(make_archive))
        .route("/servers/{id}/files/archive/{archive}", get(download_archive))
        .route("/servers/{id}/files/sftp", get(sftp_info))
        .route("/servers/{id}/files/grants", get(list_grants).post(create_grant))
        .route("/servers/{id}/files/grants/{grant}", get(get_grant).patch(update_grant))
        .route("/servers/{id}/files/grants/{grant}/revoke", post(revoke_grant))
        .route("/servers/{id}/files/grants/{grant}/reset-password", post(reset_grant_password))
        .route("/files/grants", get(list_all_grants))
}

fn actor(caller: &Caller) -> Actor {
    Actor::user(caller.principal(), caller.ctx())
}

/// Maps a service refusal to an HTTP problem.
fn api_error(e: FileError) -> ApiError {
    let msg = e.to_string();
    match e {
        FileError::Invalid(_) => ApiError::new(StatusCode::BAD_REQUEST, "bad_request", msg),
        FileError::Denied(_) => ApiError::forbidden(msg),
        FileError::StepUp(_) => ApiError::new(StatusCode::FORBIDDEN, "step_up_required", msg),
        FileError::NotFound(_) => ApiError::not_found(msg),
        FileError::Conflict(_) => ApiError::conflict(msg),
        FileError::TooLarge(_) => ApiError::new(StatusCode::PAYLOAD_TOO_LARGE, "too_large", msg),
        FileError::Busy(_) => ApiError::new(StatusCode::TOO_MANY_REQUESTS, "busy", msg),
        FileError::Unavailable(_) => ApiError::new(StatusCode::GATEWAY_TIMEOUT, "node_timeout", msg),
        FileError::Outdated => ApiError::new(StatusCode::NOT_IMPLEMENTED, "agent_outdated", msg),
        FileError::Node(_) => ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, "file_error", msg),
        FileError::Internal(_) => ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal error"),
    }
}

fn parse_path(path: Option<&str>) -> ApiResult<FilePath> {
    FilePath::parse(path.unwrap_or("/")).map_err(ApiError::bad_request)
}

fn root_id(root: Option<String>) -> String {
    root.unwrap_or_else(|| NODE_ROOT.to_owned())
}

/// Runs one operation (root placeholder replaced by the service).
async fn run(state: &AppState, caller: &Caller, id: ServerId, root: &str, op: FileOp) -> ApiResult<Json<Value>> {
    state.files.run(&actor(caller), id, root, op, Audit::Record).await.map(Json).map_err(api_error)
}

// ---- roots -------------------------------------------------------------------

/// The file roots the caller may open on a server, with permissions.
pub async fn roots(State(state): State<AppState>, caller: Caller, Path(id): Path<ServerId>) -> ApiResult<Json<Value>> {
    let (os, roots) = state.files.roots(&actor(&caller), id).await.map_err(api_error)?;
    Ok(Json(json!({ "server_id": id, "os": os, "mfa": caller.ctx().mfa, "roots": roots })))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct NewRoot {
    #[schemars(with = "crate::api::schema_types::RootKind")]
    pub kind: RootKind,
    pub label: String,
    /// Node path (service) or path inside the container.
    pub path: String,
    /// Full container id (container roots).
    #[serde(default)]
    pub container: Option<String>,
    #[serde(default = "yes")]
    pub read_only: bool,
    #[serde(default)]
    pub service_id: Option<String>,
}

fn yes() -> bool {
    true
}

/// Approves a service or container directory as a root (`file.admin`).
pub async fn add_root(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServerId>,
    Json(req): Json<NewRoot>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let base = parse_path(Some(&req.path))?;
    let spec = match req.kind {
        RootKind::Service => RootSpec::Dir { base },
        RootKind::Container => RootSpec::Container {
            container: req.container.ok_or_else(|| ApiError::bad_request("container roots need `container`"))?,
            base,
        },
        RootKind::Node => return Err(ApiError::bad_request("the whole-node root is built in")),
    };
    let root = state
        .files
        .add_root(&actor(&caller), id, req.kind, req.label, spec, req.read_only, req.service_id)
        .await
        .map_err(api_error)?;
    Ok((StatusCode::CREATED, Json(json!(root))))
}

/// Removes an approved root; files are not touched.
pub async fn delete_root(
    State(state): State<AppState>,
    caller: Caller,
    Path((id, root)): Path<(ServerId, String)>,
) -> ApiResult<StatusCode> {
    state.files.delete_root(&actor(&caller), id, &root).await.map_err(api_error)?;
    Ok(StatusCode::NO_CONTENT)
}

// ---- reading -----------------------------------------------------------------

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListQuery {
    pub root: Option<String>,
    pub path: Option<String>,
    #[serde(default)]
    pub offset: u32,
    pub limit: Option<u32>,
    #[serde(default)]
    pub hidden: bool,
}

/// One page of a directory.
pub async fn list(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServerId>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Value>> {
    let op = FileOp::List {
        root: RootSpec::Node,
        path: parse_path(q.path.as_deref())?,
        offset: q.offset,
        limit: q.limit.unwrap_or(DEFAULT_LIST_LIMIT),
        hidden: q.hidden,
    };
    run(&state, &caller, id, &root_id(q.root), op).await
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PathQuery {
    pub root: Option<String>,
    pub path: Option<String>,
}

/// Metadata of one path (links are not followed).
pub async fn stat(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServerId>,
    Query(q): Query<PathQuery>,
) -> ApiResult<Json<Value>> {
    let op = FileOp::Stat { root: RootSpec::Node, path: parse_path(q.path.as_deref())? };
    run(&state, &caller, id, &root_id(q.root), op).await
}

/// SHA-256 of a file.
pub async fn hash(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServerId>,
    Query(q): Query<PathQuery>,
) -> ApiResult<Json<Value>> {
    let op = FileOp::Hash { root: RootSpec::Node, path: parse_path(q.path.as_deref())? };
    run(&state, &caller, id, &root_id(q.root), op).await
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ReadQuery {
    pub root: Option<String>,
    pub path: Option<String>,
    #[serde(default)]
    pub offset: u64,
    pub length: Option<u32>,
    pub version: Option<String>,
}

/// One range of a file for the viewer (base64 content, version token).
pub async fn read(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServerId>,
    Query(q): Query<ReadQuery>,
) -> ApiResult<Json<Value>> {
    // Up to 1 MiB per request (default); the node is read in 256 KiB pieces
    // in one round trip, so older agents work unchanged (#15).
    let length = q.length.unwrap_or(DOWNLOAD_CHUNK);
    state
        .files
        .read_range(
            &actor(&caller),
            id,
            &root_id(q.root),
            parse_path(q.path.as_deref())?,
            q.offset,
            length,
            q.version,
            Audit::Record,
        )
        .await
        .map(Json)
        .map_err(api_error)
}

/// `attachment; filename="…"` with a safe ASCII fallback and the UTF-8 name.
fn disposition(name: &str) -> HeaderValue {
    let name: String = name.chars().filter(|c| !c.is_control()).collect();
    let ascii: String =
        name.chars().map(|c| if c.is_ascii_graphic() && c != '"' && c != '\\' { c } else { '_' }).take(150).collect();
    let utf8: String = name
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

fn decode(v: &Value) -> Result<Vec<u8>, String> {
    base64::engine::general_purpose::STANDARD
        .decode(v["content_b64"].as_str().unwrap_or_default())
        .map_err(|_| "the node sent invalid content".to_owned())
}

/// Streams a whole file. Every chunk is pinned to the version read first:
/// if the file changes mid-download the stream fails instead of mixing
/// versions. One `file.download` audit event covers the transfer.
pub async fn download(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServerId>,
    Query(q): Query<PathQuery>,
) -> ApiResult<Response> {
    let root = root_id(q.root);
    let path = parse_path(q.path.as_deref())?;
    let name = path.name().ok_or_else(|| ApiError::bad_request("cannot download a directory"))?.to_owned();
    let who = actor(&caller);
    let first = state
        .files
        .read_range(&who, id, &root, path.clone(), 0, DOWNLOAD_CHUNK, None, Audit::Skip)
        .await
        .map_err(api_error)?;
    let size = first["size"].as_u64().unwrap_or(0);
    if size > MAX_DOWNLOAD {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "too_large",
            "files over 512 MiB cannot be downloaded through the console; use SFTP access",
        ));
    }
    let version = first["version"].as_str().unwrap_or_default().to_owned();
    let head = decode(&first).map_err(|e| ApiError::new(StatusCode::BAD_GATEWAY, "bad_node_response", e))?;
    state.files.audit(&who, id, "file.download", json!({ "root": root, "path": path.to_string(), "size": size })).await;

    let start = head.len() as u64;
    let files = state.files.clone();
    let chunk_path = path.clone();
    let stream = futures::stream::unfold((Some(head), start), move |(pending, offset): (Option<Vec<u8>>, u64)| {
        let (files, who, root, version, path) =
            (files.clone(), who.clone(), root.clone(), version.clone(), chunk_path.clone());
        async move {
            if let Some(chunk) = pending {
                return Some((Ok(Bytes::from(chunk)), (None, offset)));
            }
            if offset >= size {
                return None;
            }
            let chunk =
                match files.read_range(&who, id, &root, path, offset, DOWNLOAD_CHUNK, Some(version), Audit::Skip).await
                {
                    Ok(v) => decode(&v),
                    Err(e) => Err(format!("download stopped: {e}")),
                };
            match chunk {
                Ok(c) if !c.is_empty() => {
                    let next = offset + c.len() as u64;
                    Some((Ok(Bytes::from(c)), (None, next)))
                }
                Ok(_) => None,
                Err(e) => Some((Err(std::io::Error::other(e)), (None, size))),
            }
        }
    });
    // Fused: a consumer that polls again after the end gets None, not a panic.
    let mut out = Body::from_stream(futures::StreamExt::fuse(stream)).into_response();
    let h = out.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/octet-stream"));
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(size));
    h.insert(header::CONTENT_DISPOSITION, disposition(&name));
    // File bytes come from the node, not from Forge: never let the browser
    // render them on the console's origin.
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("sandbox; default-src 'none'"));
    Ok(out)
}

// ---- changing ----------------------------------------------------------------

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct MkdirBody {
    pub root: Option<String>,
    pub path: String,
    /// Create missing parent folders too (like `mkdir -p`); an existing
    /// folder is then not an error.
    #[serde(default)]
    pub parents: bool,
}

/// Creates a directory (with `parents`, every missing folder on the way).
pub async fn mkdir(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServerId>,
    Json(b): Json<MkdirBody>,
) -> ApiResult<Json<Value>> {
    let op = FileOp::Mkdir { root: RootSpec::Node, path: parse_path(Some(&b.path))?, parents: b.parents };
    run(&state, &caller, id, &root_id(b.root), op).await
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct FromTo {
    pub root: Option<String>,
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub overwrite: bool,
}

/// Renames or moves a file or directory within a root.
pub async fn move_entry(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServerId>,
    Json(b): Json<FromTo>,
) -> ApiResult<Json<Value>> {
    let op = FileOp::Rename {
        root: RootSpec::Node,
        from: parse_path(Some(&b.from))?,
        to: parse_path(Some(&b.to))?,
        overwrite: b.overwrite,
    };
    run(&state, &caller, id, &root_id(b.root), op).await
}

/// Copies a file or directory tree within a root.
pub async fn copy(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServerId>,
    Json(b): Json<FromTo>,
) -> ApiResult<Json<Value>> {
    let op = FileOp::Copy {
        root: RootSpec::Node,
        from: parse_path(Some(&b.from))?,
        to: parse_path(Some(&b.to))?,
        overwrite: b.overwrite,
    };
    run(&state, &caller, id, &root_id(b.root), op).await
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DeleteBody {
    pub root: Option<String>,
    pub path: String,
    #[serde(default)]
    pub permanent: bool,
}

/// Moves an entry to the trash, or deletes it permanently.
pub async fn delete_entry(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServerId>,
    Json(b): Json<DeleteBody>,
) -> ApiResult<Json<Value>> {
    let op = FileOp::Delete { root: RootSpec::Node, path: parse_path(Some(&b.path))?, permanent: b.permanent };
    run(&state, &caller, id, &root_id(b.root), op).await
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SaveBody {
    pub root: Option<String>,
    pub path: String,
    pub content_b64: String,
    /// The version the editor opened; absent to create a new file.
    #[serde(default)]
    pub version: Option<String>,
}

/// Saves editor content atomically; `409` if the file changed since `version`.
pub async fn save(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServerId>,
    Json(b): Json<SaveBody>,
) -> ApiResult<Json<Value>> {
    let content = base64::engine::general_purpose::STANDARD
        .decode(&b.content_b64)
        .map_err(|_| ApiError::bad_request("content_b64 is not valid base64"))?;
    let path = parse_path(Some(&b.path))?;
    state
        .files
        .save(&actor(&caller), id, &root_id(b.root), path, &content, b.version)
        .await
        .map(Json)
        .map_err(api_error)
}

// ---- uploads -----------------------------------------------------------------

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum OverwriteBody {
    Mode(String),
    Version { version: String },
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct BeginUpload {
    pub root: Option<String>,
    pub path: String,
    pub size: u64,
    /// `"never"` (default), `"always"` or `{ "version": "…" }`.
    #[serde(default)]
    pub overwrite: Option<OverwriteBody>,
}

/// Starts a resumable upload.
pub async fn begin_upload(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServerId>,
    Json(b): Json<BeginUpload>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let overwrite = match b.overwrite {
        None => Overwrite::Never,
        Some(OverwriteBody::Mode(m)) if m == "never" => Overwrite::Never,
        Some(OverwriteBody::Mode(m)) if m == "always" => Overwrite::Always,
        Some(OverwriteBody::Version { version }) => Overwrite::IfVersion { version },
        Some(OverwriteBody::Mode(m)) => return Err(ApiError::bad_request(format!("unknown overwrite mode `{m}`"))),
    };
    let up = state
        .files
        .begin_upload(&actor(&caller), id, &root_id(b.root), parse_path(Some(&b.path))?, b.size, overwrite)
        .await
        .map_err(api_error)?;
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "id": up.id,
            "path": up.path.to_string(),
            "size": up.size,
            "chunk_size": forge_core::files::API_TRANSFER_BYTES,
            // Chunks a client may send at once; the server keeps them in order.
            "parallel": forge_files::UPLOAD_PARALLEL,
            "expires_at": up.expires_at.0,
        })),
    ))
}

/// Bytes an upload has received, for resuming.
pub async fn upload_status(
    State(state): State<AppState>,
    caller: Caller,
    Path((_, upload)): Path<(ServerId, String)>,
) -> ApiResult<Json<Value>> {
    let (up, received) = state.files.upload_status(&actor(&caller), &upload).await.map_err(api_error)?;
    Ok(Json(json!({
        "id": up.id,
        "path": up.path.to_string(),
        "size": up.size,
        "received": received,
        "expires_at": up.expires_at.0,
    })))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct OffsetQuery {
    pub offset: u64,
}

/// One chunk as the raw request body (at most `chunk_size` bytes).
pub async fn upload_chunk(
    State(state): State<AppState>,
    caller: Caller,
    Path((_, upload)): Path<(ServerId, String)>,
    Query(q): Query<OffsetQuery>,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let received = state.files.upload_chunk(&actor(&caller), &upload, q.offset, &body).await.map_err(api_error)?;
    Ok(Json(json!({ "received": received })))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CommitBody {
    pub sha256: String,
}

/// Verifies an upload (size, SHA-256) and moves it into place.
pub async fn commit_upload(
    State(state): State<AppState>,
    caller: Caller,
    Path((_, upload)): Path<(ServerId, String)>,
    Json(b): Json<CommitBody>,
) -> ApiResult<Json<Value>> {
    state.files.commit_upload(&actor(&caller), &upload, &b.sha256).await.map(Json).map_err(api_error)
}

/// Cancels an upload and removes its partial file.
pub async fn abort_upload(
    State(state): State<AppState>,
    caller: Caller,
    Path((_, upload)): Path<(ServerId, String)>,
) -> ApiResult<StatusCode> {
    state.files.abort_upload(&actor(&caller), &upload).await.map_err(api_error)?;
    Ok(StatusCode::NO_CONTENT)
}

// ---- trash and revisions -------------------------------------------------------

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RootQuery {
    pub root: Option<String>,
}

/// Trash entries of a root.
pub async fn trash(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServerId>,
    Query(q): Query<RootQuery>,
) -> ApiResult<Json<Value>> {
    run(&state, &caller, id, &root_id(q.root), FileOp::TrashList { root: RootSpec::Node }).await
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TrashBody {
    pub root: Option<String>,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub overwrite: bool,
}

/// Restores a trash entry to where it was.
pub async fn trash_restore(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServerId>,
    Json(b): Json<TrashBody>,
) -> ApiResult<Json<Value>> {
    let entry = b.id.ok_or_else(|| ApiError::bad_request("`id` is required"))?;
    let op = FileOp::TrashRestore { root: RootSpec::Node, id: entry, overwrite: b.overwrite };
    run(&state, &caller, id, &root_id(b.root), op).await
}

/// Deletes one trash entry, or all of a root's entries, for good.
pub async fn trash_purge(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServerId>,
    Json(b): Json<TrashBody>,
) -> ApiResult<Json<Value>> {
    run(&state, &caller, id, &root_id(b.root), FileOp::TrashPurge { root: RootSpec::Node, id: b.id }).await
}

/// Saved revisions of a file.
pub async fn revisions(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServerId>,
    Query(q): Query<PathQuery>,
) -> ApiResult<Json<Value>> {
    let op = FileOp::Revisions { root: RootSpec::Node, path: parse_path(q.path.as_deref())? };
    run(&state, &caller, id, &root_id(q.root), op).await
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RevisionQuery {
    pub root: Option<String>,
    pub path: Option<String>,
    pub revision: String,
    #[serde(default)]
    pub offset: u64,
    pub length: Option<u32>,
}

/// Content of a saved revision.
pub async fn revision(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServerId>,
    Query(q): Query<RevisionQuery>,
) -> ApiResult<Json<Value>> {
    let op = FileOp::RevisionRead {
        root: RootSpec::Node,
        path: parse_path(q.path.as_deref())?,
        revision: q.revision,
        offset: q.offset,
        length: q.length.unwrap_or(MAX_READ_BYTES),
    };
    run(&state, &caller, id, &root_id(q.root), op).await
}

// ---- search --------------------------------------------------------------------

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SearchBody {
    pub root: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[schemars(with = "serde_json::Value")]
    pub spec: SearchSpec,
}

/// Bounded filename and content search on the node.
pub async fn search(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServerId>,
    Json(b): Json<SearchBody>,
) -> ApiResult<Json<Value>> {
    let op = FileOp::Search { root: RootSpec::Node, path: parse_path(b.path.as_deref())?, spec: b.spec };
    run(&state, &caller, id, &root_id(b.root), op).await
}

// ---- permissions, archives and bulk actions (step 6) -------------------------

/// A mode as an octal string (`"0755"`) or a number.
fn parse_mode(v: &Value) -> ApiResult<u32> {
    match v {
        Value::String(s) => u32::from_str_radix(s.trim_start_matches("0o"), 8)
            .map_err(|_| ApiError::bad_request("mode must be octal, such as 0644")),
        Value::Number(n) => {
            n.as_u64().and_then(|n| u32::try_from(n).ok()).ok_or_else(|| ApiError::bad_request("invalid mode"))
        }
        _ => Err(ApiError::bad_request("mode is required")),
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ChmodBody {
    pub root: Option<String>,
    pub path: String,
    pub mode: Value,
    #[serde(default)]
    pub recursive: bool,
    #[serde(default)]
    pub dry_run: bool,
}

/// Sets permission bits (Linux; `file.admin`, MFA). `dry_run` previews.
pub async fn chmod(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServerId>,
    Json(b): Json<ChmodBody>,
) -> ApiResult<Json<Value>> {
    let op = FileOp::SetPermissions {
        root: RootSpec::Node,
        path: parse_path(Some(&b.path))?,
        mode: parse_mode(&b.mode)?,
        recursive: b.recursive,
        dry_run: b.dry_run,
    };
    run(&state, &caller, id, &root_id(b.root), op).await
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ChownBody {
    pub root: Option<String>,
    pub path: String,
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub group: Option<String>,
    #[serde(default)]
    pub recursive: bool,
    #[serde(default)]
    pub dry_run: bool,
}

/// Sets owner and/or group by name (Linux; `file.admin`, MFA).
pub async fn chown(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServerId>,
    Json(b): Json<ChownBody>,
) -> ApiResult<Json<Value>> {
    let op = FileOp::SetOwner {
        root: RootSpec::Node,
        path: parse_path(Some(&b.path))?,
        owner: b.owner.filter(|s| !s.is_empty()),
        group: b.group.filter(|s| !s.is_empty()),
        recursive: b.recursive,
        dry_run: b.dry_run,
    };
    run(&state, &caller, id, &root_id(b.root), op).await
}

/// The access control list of a path (Windows `icacls`, read-only).
pub async fn acl(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServerId>,
    Query(q): Query<PathQuery>,
) -> ApiResult<Json<Value>> {
    let op = FileOp::Acl { root: RootSpec::Node, path: parse_path(q.path.as_deref())? };
    run(&state, &caller, id, &root_id(q.root), op).await
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CompressBody {
    pub root: Option<String>,
    pub sources: Vec<String>,
    pub dest: String,
    #[serde(default)]
    #[schemars(with = "Option<crate::api::schema_types::ArchiveFormat>")]
    pub format: Option<ArchiveFormat>,
}

fn paths(list: &[String]) -> ApiResult<Vec<FilePath>> {
    if list.is_empty() || list.len() > BULK_MAX_ITEMS {
        return Err(ApiError::bad_request(format!("choose between 1 and {BULK_MAX_ITEMS} items")));
    }
    list.iter().map(|p| parse_path(Some(p))).collect()
}

/// Packs files and folders into a new `.zip` or `.tar.gz` (no overwrite).
pub async fn compress(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServerId>,
    Json(b): Json<CompressBody>,
) -> ApiResult<Json<Value>> {
    let dest = parse_path(Some(&b.dest))?;
    let format = b
        .format
        .or_else(|| ArchiveFormat::from_name(dest.name().unwrap_or_default()))
        .ok_or_else(|| ApiError::bad_request("the archive name must end in .zip, .tar.gz or .tgz"))?;
    let op = FileOp::Compress { root: RootSpec::Node, sources: paths(&b.sources)?, dest: Some(dest), format };
    run(&state, &caller, id, &root_id(b.root), op).await
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ExtractBody {
    pub root: Option<String>,
    pub archive: String,
    pub dest: String,
    /// Replace an existing folder `dest`: unpack next to it, then swap it in
    /// (needs `file.delete` too). Without it an existing `dest` is a `409`.
    #[serde(default)]
    pub replace: bool,
}

/// Unpacks an archive into a new folder (or, with `replace`, swaps it in for
/// an existing one), refusing unsafe entries.
pub async fn extract(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServerId>,
    Json(b): Json<ExtractBody>,
) -> ApiResult<Json<Value>> {
    let op = FileOp::Extract {
        root: RootSpec::Node,
        archive: parse_path(Some(&b.archive))?,
        dest: parse_path(Some(&b.dest))?,
        replace: b.replace,
    };
    run(&state, &caller, id, &root_id(b.root), op).await
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BulkAction {
    Copy,
    Move,
    Trash,
    Delete,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct BulkBody {
    pub root: Option<String>,
    pub action: BulkAction,
    pub paths: Vec<String>,
    /// Destination folder for copy and move.
    #[serde(default)]
    pub to: Option<String>,
    #[serde(default)]
    pub overwrite: bool,
}

/// Copies, moves or deletes many items; one result per item.
pub async fn bulk(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServerId>,
    Json(b): Json<BulkBody>,
) -> ApiResult<Json<Value>> {
    let items = paths(&b.paths)?;
    let root = root_id(b.root);
    let to = match (&b.action, &b.to) {
        (BulkAction::Copy | BulkAction::Move, Some(t)) => Some(parse_path(Some(t))?),
        (BulkAction::Copy | BulkAction::Move, None) => return Err(ApiError::bad_request("copy and move need `to`")),
        _ => None,
    };
    let who = actor(&caller);
    let mut results = Vec::with_capacity(items.len());
    let (mut ok, mut failed) = (0usize, 0usize);
    for p in items {
        let target = match (&to, p.name()) {
            (Some(dir), Some(name)) => Some(dir.join(name).map_err(ApiError::bad_request)?),
            _ => None,
        };
        let op = match (&b.action, target) {
            (BulkAction::Copy, Some(t)) => {
                FileOp::Copy { root: RootSpec::Node, from: p.clone(), to: t, overwrite: b.overwrite }
            }
            (BulkAction::Move, Some(t)) => {
                FileOp::Rename { root: RootSpec::Node, from: p.clone(), to: t, overwrite: b.overwrite }
            }
            (BulkAction::Trash, _) => FileOp::Delete { root: RootSpec::Node, path: p.clone(), permanent: false },
            (BulkAction::Delete, _) => FileOp::Delete { root: RootSpec::Node, path: p.clone(), permanent: true },
            _ => {
                failed += 1;
                results.push(json!({ "path": p.to_string(), "ok": false, "error": "the root itself cannot be moved" }));
                continue;
            }
        };
        match state.files.run(&who, id, &root, op, Audit::Record).await {
            Ok(_) => {
                ok += 1;
                results.push(json!({ "path": p.to_string(), "ok": true }));
            }
            Err(e) => {
                failed += 1;
                // Authorization failures stop the batch: the rest would fail too.
                let stop = matches!(e, FileError::StepUp(_));
                results.push(json!({ "path": p.to_string(), "ok": false, "error": e.to_string() }));
                if stop {
                    return Err(api_error(e));
                }
            }
        }
    }
    Ok(Json(json!({ "ok": ok, "failed": failed, "results": results })))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ArchiveBody {
    pub root: Option<String>,
    pub paths: Vec<String>,
    #[serde(default)]
    #[schemars(with = "Option<crate::api::schema_types::ArchiveFormat>")]
    pub format: Option<ArchiveFormat>,
}

/// Packs a selection into a temporary archive for download (read access).
pub async fn make_archive(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServerId>,
    Json(b): Json<ArchiveBody>,
) -> ApiResult<Json<Value>> {
    let format = b.format.unwrap_or(ArchiveFormat::Zip);
    let op = FileOp::Compress { root: RootSpec::Node, sources: paths(&b.paths)?, dest: None, format };
    run(&state, &caller, id, &root_id(b.root), op).await
}

/// Streams a temporary archive as an attachment, then deletes it.
pub async fn download_archive(
    State(state): State<AppState>,
    caller: Caller,
    Path((id, archive)): Path<(ServerId, String)>,
    Query(q): Query<RootQuery>,
) -> ApiResult<Response> {
    let root = root_id(q.root);
    let who = actor(&caller);
    let read =
        |offset: u64| FileOp::TempRead { root: RootSpec::Node, id: archive.clone(), offset, length: MAX_READ_BYTES };
    let first = state.files.run(&who, id, &root, read(0), Audit::Skip).await.map_err(api_error)?;
    let size = first["size"].as_u64().unwrap_or(0);
    let head = decode(&first).map_err(|e| ApiError::new(StatusCode::BAD_GATEWAY, "bad_node_response", e))?;
    state.files.audit(&who, id, "file.download", json!({ "root": root, "archive": archive, "size": size })).await;
    let start = head.len() as u64;
    let (files, archive2, root2) = (state.files.clone(), archive.clone(), root.clone());
    let stream = futures::stream::unfold(
        (Some(head), start, false),
        move |(pending, offset, done): (Option<Vec<u8>>, u64, bool)| {
            let (files, who, root, archive) = (files.clone(), who.clone(), root2.clone(), archive2.clone());
            async move {
                if done {
                    return None;
                }
                if let Some(chunk) = pending {
                    return Some((Ok(Bytes::from(chunk)), (None, offset, false)));
                }
                if offset >= size {
                    // Finished: remove the temporary archive.
                    let del = FileOp::TempDelete { root: RootSpec::Node, id: archive };
                    let _ = files.run(&who, id, &root, del, Audit::Skip).await;
                    return None;
                }
                let op = FileOp::TempRead { root: RootSpec::Node, id: archive, offset, length: MAX_READ_BYTES };
                match files
                    .run(&who, id, &root, op, Audit::Skip)
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(|v| decode(&v))
                {
                    Ok(c) if !c.is_empty() => {
                        let next = offset + c.len() as u64;
                        Some((Ok(Bytes::from(c)), (None, next, false)))
                    }
                    Ok(_) => None,
                    Err(e) => Some((Err(std::io::Error::other(e)), (None, size, true))),
                }
            }
        },
    );
    let ext = first["ext"].as_str().unwrap_or("zip");
    // Fused: a consumer that polls again after the end gets None, not a panic.
    let mut out = Body::from_stream(futures::StreamExt::fuse(stream)).into_response();
    let h = out.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/octet-stream"));
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(size));
    h.insert(header::CONTENT_DISPOSITION, disposition(&format!("files.{ext}")));
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("sandbox; default-src 'none'"));
    Ok(out)
}

// ---- temporary SFTP access -------------------------------------------------

/// Where clients connect: configured host and port, or the API's host and
/// the listener's port.
fn sftp_endpoint(state: &AppState) -> (String, u16) {
    use std::net::SocketAddr;
    let cfg = &state.config.sftp;
    let host = cfg.public_host.clone().unwrap_or_else(|| {
        let url = state.public_address.get();
        let url = url.as_str();
        let rest = url.split_once("://").map_or(url, |(_, r)| r);
        let host = rest.split(['/', '?']).next().unwrap_or("localhost");
        // Strip a port, keeping IPv6 brackets.
        match host.rsplit_once(':') {
            Some((h, p)) if p.bytes().all(|b| b.is_ascii_digit()) && !h.ends_with(':') => h.to_owned(),
            _ => host.to_owned(),
        }
    });
    let bound = state.sftp_bound.get().map_or(cfg.listen.port(), SocketAddr::port);
    (host, cfg.public_port.unwrap_or(bound))
}

/// Connection details clients need (no secrets).
pub async fn sftp_info(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServerId>,
) -> ApiResult<Json<Value>> {
    // Same visibility as the explorer itself.
    state.files.roots(&actor(&caller), id).await.map_err(api_error)?;
    let Some(key) = state.sftp_host_key.as_ref() else {
        return Ok(Json(json!({ "enabled": false })));
    };
    let (host, port) = sftp_endpoint(&state);
    Ok(Json(json!({
        "enabled": true,
        "protocol": "SFTP",
        "host": host,
        "port": port,
        "fingerprint": key.fingerprint(),
        "max_hours": state.config.sftp.max_hours,
        "allow_never_expiry": state.files.allow_never_expiry(),
        "can_never_expire": state.files.can_issue_never_expiring(&actor(&caller), id).await,
    })))
}

/// A grant as the API shows it: plus `status`, `active` and `never_expires`.
fn grant_json(g: &FileGrant, now: forge_core::Timestamp) -> Value {
    let mut v = json!(g);
    v["active"] = json!(g.is_active(now));
    v["status"] = json!(g.status(now));
    v["never_expires"] = json!(g.never_expires());
    v
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GrantFilter {
    /// Only grants on this server.
    pub server: Option<String>,
    /// `active`, `expired` or `revoked`.
    pub status: Option<String>,
    /// Case-insensitive match on the name, id, creator or server name.
    pub q: Option<String>,
}

/// Temporary SFTP access grants across the fleet: on every server where
/// you hold `file.read`, all grants with `file.admin`, else your own.
pub async fn list_all_grants(
    State(state): State<AppState>,
    caller: Caller,
    Query(f): Query<GrantFilter>,
) -> ApiResult<Json<Value>> {
    if let Some(s) = f.status.as_deref()
        && !matches!(s, "active" | "expired" | "revoked")
    {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "invalid", "status is active, expired or revoked"));
    }
    let now = state.clock.now();
    let q = f.q.as_deref().map(str::to_lowercase).filter(|q| !q.is_empty());
    let grants: Vec<Value> = state
        .files
        .list_all_grants(&actor(&caller))
        .await
        .map_err(api_error)?
        .into_iter()
        .filter(|(g, _)| f.server.as_deref().is_none_or(|s| s == g.server_id.to_string()))
        .filter(|(g, _)| f.status.as_deref().is_none_or(|s| s == g.status(now)))
        .filter(|(g, s)| {
            q.as_deref().is_none_or(|q| {
                let name = s.display_name.as_deref().unwrap_or(&s.hostname);
                [g.name.as_str(), g.id.as_str(), g.created_by.as_str(), name, s.hostname.as_str()]
                    .iter()
                    .any(|f| f.to_lowercase().contains(q))
            })
        })
        .map(|(g, s)| {
            let mut v = grant_json(&g, now);
            v["server_name"] = json!(s.display_name.as_deref().unwrap_or(&s.hostname));
            v
        })
        .collect();
    Ok(Json(json!({ "grants": grants })))
}

/// One temporary SFTP access grant (no secrets).
pub async fn get_grant(
    State(state): State<AppState>,
    caller: Caller,
    Path((id, grant)): Path<(ServerId, String)>,
) -> ApiResult<Json<Value>> {
    let g = state.files.get_grant(&actor(&caller), id, &grant).await.map_err(api_error)?;
    Ok(Json(grant_json(&g, state.clock.now())))
}

/// Changes a grant (folders, permissions, restrictions, name, expiry)
/// without changing its password. Needs a recent MFA check.
pub async fn update_grant(
    State(state): State<AppState>,
    caller: Caller,
    Path((id, grant)): Path<(ServerId, String)>,
    Json(req): Json<forge_files::GrantChanges>,
) -> ApiResult<Json<Value>> {
    let g = state.files.update_grant(&actor(&caller), id, &grant, req).await.map_err(api_error)?;
    Ok(Json(grant_json(&g, state.clock.now())))
}

/// Replaces a grant's password; the new one is in this answer only.
pub async fn reset_grant_password(
    State(state): State<AppState>,
    caller: Caller,
    Path((id, grant)): Path<(ServerId, String)>,
) -> ApiResult<Json<Value>> {
    let Some(key) = state.sftp_host_key.as_ref() else {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "sftp_disabled",
            "the SFTP gateway is not enabled on this Forge",
        ));
    };
    let issued = state.files.reset_grant_password(&actor(&caller), id, &grant).await.map_err(api_error)?;
    let (host, port) = sftp_endpoint(&state);
    Ok(Json(json!({
        "grant": grant_json(&issued.grant, state.clock.now()),
        "connection": {
            "protocol": "SFTP",
            "host": host,
            "port": port,
            "username": issued.grant.id,
            "password": issued.token,
            "expires_at": issued.grant.expires_at.0,
            "fingerprint": key.fingerprint(),
        },
    })))
}

/// Temporary SFTP access grants on a server (all with `file.admin`, else your own).
pub async fn list_grants(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServerId>,
) -> ApiResult<Json<Value>> {
    let grants = state.files.list_grants(&actor(&caller), id).await.map_err(api_error)?;
    let now = state.clock.now();
    let grants: Vec<Value> = grants.into_iter().map(|g| grant_json(&g, now)).collect();
    Ok(Json(json!({ "grants": grants })))
}

/// Issues temporary SFTP access; the token is in this answer only.
pub async fn create_grant(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServerId>,
    Json(req): Json<forge_files::NewGrant>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let Some(key) = state.sftp_host_key.as_ref() else {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "sftp_disabled",
            "the SFTP gateway is not enabled on this Forge",
        ));
    };
    let issued = state.files.create_grant(&actor(&caller), id, req).await.map_err(api_error)?;
    let (host, port) = sftp_endpoint(&state);
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "grant": grant_json(&issued.grant, state.clock.now()),
            "connection": {
                "protocol": "SFTP",
                "host": host,
                "port": port,
                "username": issued.grant.id,
                "password": issued.token,
                "expires_at": issued.grant.expires_at.0,
                "fingerprint": key.fingerprint(),
            },
        })),
    ))
}

/// Revokes a temporary SFTP access grant; open sessions close.
pub async fn revoke_grant(
    State(state): State<AppState>,
    caller: Caller,
    Path((id, grant)): Path<(ServerId, String)>,
) -> ApiResult<Json<Value>> {
    let g = state.files.revoke_grant(&actor(&caller), id, &grant).await.map_err(api_error)?;
    Ok(Json(grant_json(&g, state.clock.now())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disposition_is_safe() {
        let h = disposition("r\u{e9}sum\u{e9} \"x\"\r\n.txt");
        let s = h.to_str().unwrap_or_default();
        assert!(s.starts_with("attachment; filename=\"r_sum__"));
        assert!(s.contains("filename*=UTF-8''r%C3%A9sum%C3%A9%20%22x%22.txt"));
        assert!(!s.contains('\r') && !s.contains('\n'));
    }

    #[test]
    fn service_errors_map_to_statuses() {
        assert_eq!(api_error(FileError::NotFound("x".into())).status, StatusCode::NOT_FOUND);
        assert_eq!(api_error(FileError::Conflict("x".into())).status, StatusCode::CONFLICT);
        assert_eq!(api_error(FileError::StepUp("x".into())).code, "step_up_required");
        assert_eq!(api_error(FileError::Outdated).status, StatusCode::NOT_IMPLEMENTED);
        assert_eq!(api_error(FileError::Busy("x".into())).status, StatusCode::TOO_MANY_REQUESTS);
    }
}
