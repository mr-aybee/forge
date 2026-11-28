//! Node onboarding: SSH probe, assisted onboarding, manual installer,
//! agent downloads, and metadata for vaulted credentials.

use std::time::Duration;

use axum::Json;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use forge_core::model::{NewJob, SecretRecord};
use forge_core::ports::{ClusterRepository, SecretRepository, TenancyRepository};
use forge_core::remote::{RemoteAuth, RemoteTarget};
use forge_core::*;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::auth::Caller;
use crate::auth::Ws;
use crate::error::{ApiError, ApiResult};
use crate::install::{InstallParams, render_script};
use crate::jobs::node_bootstrap::{BootstrapCredential, BootstrapPayload, secret_aad};
use crate::request_origin::{NodeUrls, RequestOrigin};
use crate::state::AppState;

/// How long an unkept bootstrap credential may live in the vault.
const CREDENTIAL_TTL: Duration = Duration::from_secs(3600);

fn valid_host(h: &str) -> ApiResult<String> {
    let h = h.trim();
    let ok = !h.is_empty()
        && h.len() <= 253
        && h.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '[' | ']'));

    if ok { Ok(h.to_owned()) } else { Err(ApiError::bad_request("invalid host")) }
}

fn valid_user(u: &str) -> ApiResult<String> {
    let ok = !u.is_empty()
        && u.len() <= 32
        && u.chars().next().is_some_and(|c| c.is_ascii_lowercase() || c == '_')
        && u.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '-' | '.'));
    if ok { Ok(u.to_owned()) } else { Err(ApiError::bad_request("invalid username")) }
}

/// Scope for onboarding: the target cluster if given, else the workspace.
async fn onboarding_scope(state: &AppState, ws: WorkspaceId, cluster: Option<ClusterId>) -> ApiResult<EntityRef> {
    state.store.get_workspace(ws).await?;

    match cluster {
        Some(c) => {
            if state.store.get_cluster(c).await?.workspace_id != ws {
                return Err(ApiError::bad_request("cluster belongs to another workspace"));
            }

            Ok(EntityRef::new(EntityKind::Cluster, c))
        }
        None => Ok(EntityRef::new(EntityKind::Workspace, ws)),
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ProbeRequest {
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
}

fn default_port() -> u16 {
    22
}

/// Step 1: fetch the host key so the user can confirm it. No credential is sent.
pub async fn probe(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Json(req): Json<ProbeRequest>,
) -> ApiResult<Json<Value>> {
    caller.authorize(Action::NodeOnboard, onboarding_scope(&state, ws, None).await?).await?;
    let host = valid_host(&req.host)?;
    let info = state.ssh.probe(&host, req.port).await?;

    Ok(Json(json!({ "host": host, "port": req.port, "host_key": info })))
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct OnboardRequest {
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    pub username: String,
    /// The fingerprint the user confirmed after the probe.
    pub host_key_fingerprint: String,
    #[schemars(with = "serde_json::Value")]
    pub auth: RemoteAuth,
    #[serde(default)]
    pub sudo_password: Option<String>,
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub cluster_id: Option<ClusterId>,
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub environment_id: Option<EnvironmentId>,
    #[serde(default)]
    pub keep_credential: bool,
    #[serde(default)]
    pub hostname: Option<String>,
    /// Terminal account the installer creates (D-078); none when empty.
    #[serde(default)]
    pub terminal_account: Option<String>,
    /// Passwordless sudo for that account.
    #[serde(default)]
    pub terminal_account_sudo: bool,
}

/// A requested terminal account, checked; `None` when not asked for.
fn terminal_account(name: Option<&str>) -> ApiResult<Option<String>> {
    match name.map(str::trim).filter(|n| !n.is_empty()) {
        None => Ok(None),
        Some(n) if forge_core::tags::valid_account_name(n) => Ok(Some(n.to_owned())),
        Some(n) => Err(ApiError::bad_request(format!(
            "terminal account `{n}` is not allowed: use 1-32 of a-z, 0-9, `_` and `-`, starting with a letter or `_`, and not a system name"
        ))),
    }
}

/// Step 2: seal the credential in the vault and start `node.bootstrap`.
pub async fn onboard(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    origin: RequestOrigin,
    Json(req): Json<OnboardRequest>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let scope = onboarding_scope(&state, ws, req.cluster_id).await?;
    caller.authorize(Action::NodeOnboard, scope.clone()).await?;

    if let Some(e) = req.environment_id {
        if state.store.get_environment(e).await?.workspace_id != ws {
            return Err(ApiError::bad_request("environment belongs to another workspace"));
        }

        // As for an enrollment token into that environment.
        caller.authorize(Action::ServerEnroll, EntityRef::new(EntityKind::Environment, e)).await?;
    }

    if !req.host_key_fingerprint.starts_with("SHA256:") {
        return Err(ApiError::bad_request("confirm the host key fingerprint from the probe first"));
    }

    let account = terminal_account(req.terminal_account.as_deref())?;
    let target = RemoteTarget {
        host: valid_host(&req.host)?,
        port: req.port,
        username: valid_user(&req.username)?,
        host_key_fingerprint: req.host_key_fingerprint.clone(),
    };

    let secret_id = SecretId::new();
    let cred = BootstrapCredential { auth: req.auth, sudo_password: req.sudo_password };
    let plain = serde_json::to_vec(&cred).map_err(DomainError::internal)?;
    let sealed = state.envelope.seal(&plain, &secret_aad(secret_id)).map_err(DomainError::internal)?;
    let now = state.clock.now();
    state
        .store
        .put_secret(&SecretRecord {
            id: secret_id,
            workspace_id: ws,
            kind: "ssh.bootstrap".into(),
            label: format!("{}@{}:{}", target.username, target.host, target.port),
            sealed,
            // Kept credentials still need deliberate deletion; others expire.
            expires_at: if req.keep_credential { None } else { Some(now.plus(CREDENTIAL_TTL)) },
            created_by: caller.principal(),
            created_at: now,
        })
        .await?;

    let payload = BootstrapPayload {
        workspace_id: ws,
        target: target.clone(),
        secret_id,
        cluster_id: req.cluster_id,
        environment_id: req.environment_id,
        keep_credential: req.keep_credential,
        hostname: req.hostname.map(|h| h.chars().take(63).collect()),
        terminal_account_sudo: account.is_some() && req.terminal_account_sudo,
        terminal_account: account,
        // The addresses the node is told to use: the one the administrator
        // reached Forge on (D-167), never a localhost default.
        urls: Some(NodeUrls::from_state(&state, &origin).into()),
    };
    let job = state
        .jobs
        .submit(NewJob {
            kind: "node.bootstrap".into(),
            workspace_id: Some(ws),
            resource: scope,
            principal: caller.principal(),
            context: caller.ctx(),
            idempotency_key: None,
            payload: serde_json::to_value(&payload).map_err(DomainError::internal)?,
            max_attempts: 3,
        })
        .await;

    let job = match job {
        Ok(j) => j,
        Err(e) => {
            let _ = state.store.delete_secret(secret_id).await;
            return Err(e.into());
        }
    };

    caller
        .audit(
            "node.onboard",
            &EntityRef::new(EntityKind::Workspace, ws),
            Some(ws),
            json!({ "target": format!("{}@{}:{}", target.username, target.host, target.port), "job_id": job.id, "host_key": target.host_key_fingerprint, "keep_credential": req.keep_credential }),
        )
        .await;

    Ok((StatusCode::ACCEPTED, Json(json!({ "job": job }))))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct InstallQuery {
    #[serde(default)]
    pub hostname: Option<String>,
    /// Terminal account to create (D-078).
    #[serde(default)]
    pub terminal_account: Option<String>,
    /// `1` or `true`: passwordless sudo for that account.
    #[serde(default)]
    pub terminal_account_sudo: Option<String>,
}

/// `GET /install.sh` — the manual installer (public; contains no secrets).
/// Its URLs come from the address the node fetched it from (D-167), so
/// each network's nodes use an address they can reach.
pub async fn install_script(
    State(state): State<AppState>,
    origin: RequestOrigin,
    Query(q): Query<InstallQuery>,
) -> ApiResult<Response> {
    let hostname =
        // An empty value is no override (the enroll call would refuse `--hostname ''`).
        q.hostname.filter(|h| {
            !h.is_empty() && h.len() <= 63 && h.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
        });
    let account = terminal_account(q.terminal_account.as_deref())?;
    let sudo = account.is_some() && matches!(q.terminal_account_sudo.as_deref(), Some("1" | "true"));
    let urls = NodeUrls::from_state(&state, &origin);
    let body = render_script(
        &state.dist,
        &InstallParams {
            forge_url: &urls.forge_url,
            agent_url: &urls.agent_url,
            token: None,
            hostname: hostname.as_deref(),
            ssh_user_ca: Some(&state.ssh_ca.public_openssh()),
            terminal_account: account.as_deref(),
            terminal_account_sudo: sudo,
        },
    );

    Ok(([(header::CONTENT_TYPE, "text/x-shellscript; charset=utf-8")], SCRIPT_CACHE_HEADERS, body).into_response())
}

/// The installers embed the address they were fetched from: never cache
/// them, and key any cache that ignores `no-store` on those headers.
const SCRIPT_CACHE_HEADERS: [(header::HeaderName, &str); 2] =
    [(header::CACHE_CONTROL, "no-store"), (header::VARY, "Host, X-Forwarded-Host, X-Forwarded-Proto")];

/// Bytes read per chunk of an agent download.
const DOWNLOAD_CHUNK: usize = 64 * 1024;

/// `GET /agent/download/{os}/{arch}/{binary}` — agent binaries (public).
pub async fn download(
    State(state): State<AppState>,
    Path((os, arch, bin)): Path<(String, String, String)>,
) -> ApiResult<Response> {
    let (path, sum) = state.dist.binary(&os, &arch, &bin).ok_or_else(|| ApiError::not_found("no such agent build"))?;

    // Public and large: streamed in chunks, and only so many at once.
    let permit =
        state.downloads.clone().try_acquire_owned().map_err(|_| {
            ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "busy", "too many downloads; try again shortly")
        })?;
    let file = tokio::fs::File::open(&path).await.map_err(DomainError::internal)?;
    let len = file.metadata().await.map_err(DomainError::internal)?.len();
    let stream = futures::stream::unfold((file, permit), |(mut file, permit)| async move {
        use tokio::io::AsyncReadExt;
        let mut buf = vec![0u8; DOWNLOAD_CHUNK];
        match file.read(&mut buf).await {
            Ok(0) => None,
            Ok(n) => {
                buf.truncate(n);
                Some((Ok::<_, std::io::Error>(axum::body::Bytes::from(buf)), (file, permit)))
            }
            Err(e) => Some((Err(e), (file, permit))),
        }
    });
    let mut resp = Response::new(Body::from_stream(stream));
    let h = resp.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/octet-stream"));
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(len));

    if let Ok(v) = HeaderValue::from_str(&format!("sha256={sum}")) {
        h.insert("x-checksum", v);
    }

    Ok(resp)
}

/// `GET /install.ps1` — the Windows node installer (public, like
/// `install.sh`; D-079). It carries no token: run it with
/// `$env:FORGE_ENROLL_TOKEN` set.
pub async fn install_ps1(State(state): State<AppState>, origin: RequestOrigin) -> Response {
    let urls = NodeUrls::from_state(&state, &origin);
    let body = crate::install::render_ps1(&state.dist, &urls.forge_url, &urls.agent_url);

    ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], SCRIPT_CACHE_HEADERS, body).into_response()
}

/// `GET /workspaces/{ws}/onboarding` — what the UI needs for the manual path.
pub async fn onboarding_info(
    State(state): State<AppState>,
    caller: Caller,
    origin: RequestOrigin,
    Ws(ws): Ws,
) -> ApiResult<Json<Value>> {
    caller.authorize(Action::ServerEnroll, EntityRef::new(EntityKind::Workspace, ws)).await?;
    let urls = NodeUrls::from_state(&state, &origin);
    let url = urls.forge_url.as_str();

    Ok(Json(json!({
        "agents_available": state.dist.available(),
        "install_url": format!("{url}/install.sh"),
        "agent_url": urls.agent_url,
        "linux": format!("curl -fsSL {url}/install.sh | sudo FORGE_ENROLL_TOKEN='<token>' sh"),
        "windows_available": state.dist.windows_available(),
        "windows": format!("$env:FORGE_ENROLL_TOKEN = '<token>'; irm {url}/install.ps1 | iex"),
    })))
}

/// Vaulted credential metadata (never the secret itself).
pub async fn list_secrets(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
) -> ApiResult<Json<Vec<SecretRecord>>> {
    caller.authorize(Action::WorkspaceManage, EntityRef::new(EntityKind::Workspace, ws)).await?;
    Ok(Json(state.store.list_secrets(ws).await?))
}

pub async fn delete_secret(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<SecretId>,
) -> ApiResult<StatusCode> {
    let rec = state.store.get_secret(id).await?.ok_or_else(|| ApiError::not_found("secret not found"))?;
    caller.authorize(Action::WorkspaceManage, EntityRef::new(EntityKind::Workspace, rec.workspace_id)).await?;
    state.store.delete_secret(id).await?;
    caller
        .audit(
            "secret.delete",
            &EntityRef::new(EntityKind::Workspace, rec.workspace_id),
            Some(rec.workspace_id),
            json!({ "secret_id": id, "label": rec.label }),
        )
        .await;

    Ok(StatusCode::NO_CONTENT)
}
