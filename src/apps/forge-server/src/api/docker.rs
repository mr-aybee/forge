//! A node's Docker Engine API, over mutual TLS (D-125).
//!
//! Endpoint management:
//! * `GET /servers/{id}/docker/endpoint`: state, address, versions,
//!   certificate expiry (`server.manage`);
//! * `POST /servers/{id}/docker/endpoint`: enable (`server.command`, MFA),
//!   a `docker.endpoint.enable` job;
//! * `DELETE /servers/{id}/docker/endpoint`: disable (`server.command`), a
//!   `docker.endpoint.disable` job;
//! * `POST /servers/{id}/docker/endpoint/check`: ask the daemon now.
//! * `GET /docker/catalog`: every Engine API endpoint Forge forwards.
//!
//! The Engine API itself: `ANY /servers/{id}/docker/engine/{path}` forwards
//! one call, exactly as the Engine API reference describes it, to the
//! node's daemon. Before anything is sent:
//! 1. the method and path must be in [`forge_docker::catalog`]; anything
//!    else is refused (deny by default), as are dot segments and encoded
//!    separators;
//! 2. a JSON body the catalog inspects (container create, exec create,
//!    volume create, …) is read, at most [`MAX_INSPECTED`], and settings
//!    that reach the host raise the call to the administrator class;
//! 3. the caller is authorized for the class: reads `server.manage`,
//!    changes `container.manage`, exec/attach/archive `container.exec`,
//!    root on the node `server.command` (MFA). Denials are audited.
//!
//! Responses stream (logs, events, pulls, tar archives); attach, exec start
//! and BuildKit sessions are upgraded to a raw duplex stream. A container
//! inspect returns `Config.Env` values only to callers who hold
//! `container.inspect_env`. Only allow-listed headers pass either way.
//! Every call except reads is audited as `docker.api.<op>` with the
//! method, path, status and the host-reaching settings; never a body.

use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::extract::{Path, Query, Request, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use bytes::Bytes;
use forge_core::model::{DockerEndpoint, NewJob};
use forge_core::{Action, EntityKind, EntityRef, ServerId};
use forge_docker::catalog::{self, Class, Transfer};
use forge_docker::guard;
use http_body_util::BodyExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::jobs::docker_endpoint::{DISABLE_KIND, ENABLE_KIND};
use crate::state::AppState;

/// Largest JSON body read for inspection (container create and friends).
pub const MAX_INSPECTED: usize = 1024 * 1024;
/// Largest streamed request body (image loads, builds, archive uploads).
pub const MAX_UPLOAD: usize = 8 * 1024 * 1024 * 1024;
/// Largest container inspect answer redacted in memory.
const MAX_INSPECT_ANSWER: usize = 8 * 1024 * 1024;
/// Longest query string forwarded.
const MAX_QUERY: usize = 8 * 1024;

/// Audit action of an Engine API call (`docker.api.<op>` is in `details.op`).
pub const AUDIT_API: &str = "docker.api";
pub const AUDIT_ENABLE: &str = "docker.endpoint.enable";
pub const AUDIT_DISABLE: &str = "docker.endpoint.disable";

/// Request headers passed to the daemon.
const REQUEST_HEADERS: &[&str] = &[
    "content-type",
    "x-registry-auth",
    "x-registry-config",
    "accept",
    // WebSocket attach (`/containers/{id}/attach/ws`).
    "sec-websocket-key",
    "sec-websocket-version",
    "sec-websocket-protocol",
    "sec-websocket-extensions",
];
/// Response headers passed back.
const RESPONSE_HEADERS: &[&str] = &[
    "content-type",
    "api-version",
    "docker-experimental",
    "ostype",
    "server",
    "docker-content-digest",
    "x-docker-container-path-stat",
    "sec-websocket-accept",
    "sec-websocket-protocol",
    "sec-websocket-extensions",
];

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/docker/catalog", get(catalog_list))
        .route("/servers/{id}/docker/endpoint", get(status).post(enable).delete(disable))
        .route("/servers/{id}/docker/endpoint/check", post(check))
        .route(
            "/servers/{id}/docker/engine/{*path}",
            // HEAD is served by the GET handler.
            get(engine).post(engine).put(engine).delete(engine).layer(axum::extract::DefaultBodyLimit::disable()),
        )
}

fn server_ref(id: ServerId) -> EntityRef {
    EntityRef::new(EntityKind::Server, id)
}

const fn action_for(class: Class) -> Action {
    match class {
        Class::Read => Action::ServerManage,
        Class::Manage => Action::ContainerManage,
        Class::Exec => Action::ContainerExec,
        Class::Admin => Action::ServerCommand,
    }
}

// ---------------------------------------------------------------- catalog

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CatalogEntry {
    pub method: &'static str,
    pub path: &'static str,
    /// `read`, `manage`, `exec` or `admin`.
    pub class: &'static str,
    /// The Forge action it needs (before body checks).
    pub action: &'static str,
    /// `inspect`, `stream` or `hijack`.
    pub transfer: &'static str,
    pub op: &'static str,
}

const fn class_name(c: Class) -> &'static str {
    match c {
        Class::Read => "read",
        Class::Manage => "manage",
        Class::Exec => "exec",
        Class::Admin => "admin",
    }
}

const fn transfer_name(t: Transfer) -> &'static str {
    match t {
        Transfer::Inspect => "inspect",
        Transfer::Stream => "stream",
        Transfer::Hijack => "hijack",
    }
}

async fn catalog_list(caller: Caller) -> ApiResult<Json<Vec<CatalogEntry>>> {
    caller.authorize(Action::View, EntityRef::platform()).await?;
    Ok(Json(
        catalog::ENDPOINTS
            .iter()
            .map(|e| CatalogEntry {
                method: e.method,
                path: e.path,
                class: class_name(e.class),
                action: action_for(e.class).as_str(),
                transfer: transfer_name(e.transfer),
                op: e.op,
            })
            .collect(),
    ))
}

// ---------------------------------------------------------------- endpoint

/// A node's Docker API endpoint as the console and CLI see it.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct EndpointView {
    /// `disabled`, `pending`, `enabled` or `failed`.
    pub state: String,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub engine_version: Option<String>,
    pub api_version: Option<String>,
    /// SHA-256 of the node's Docker CA certificate (hex).
    pub ca_fingerprint: Option<String>,
    /// When dockerd's certificate expires (epoch ms).
    pub daemon_cert_not_after: Option<i64>,
    pub last_error: Option<String>,
    pub updated_at: Option<i64>,
}

fn view(ep: Option<&DockerEndpoint>) -> EndpointView {
    match ep {
        None => EndpointView {
            state: "disabled".into(),
            host: None,
            port: None,
            engine_version: None,
            api_version: None,
            ca_fingerprint: None,
            daemon_cert_not_after: None,
            last_error: None,
            updated_at: None,
        },
        Some(e) => EndpointView {
            state: e.state.to_string(),
            host: Some(e.host.clone()),
            port: Some(e.port),
            engine_version: e.engine_version.clone(),
            api_version: e.api_version.clone(),
            ca_fingerprint: forge_pki::cert_fingerprint(&e.ca_cert_pem),
            daemon_cert_not_after: Some(e.daemon_cert_not_after.0),
            last_error: e.last_error.clone(),
            updated_at: Some(e.updated_at.0),
        },
    }
}

async fn status(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServerId>,
) -> ApiResult<Json<EndpointView>> {
    caller.authorize(Action::ServerManage, server_ref(id)).await?;
    Ok(Json(view(state.docker.endpoint(id).await?.as_ref())))
}

async fn check(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServerId>,
) -> ApiResult<Json<EndpointView>> {
    caller.authorize(Action::ServerManage, server_ref(id)).await?;
    let ep = state.docker.check(id).await?;
    Ok(Json(view(Some(&ep))))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnableRequest {
    /// Address the control plane dials (an IP address or DNS name of the
    /// node). Default: the address its agent connects from.
    #[serde(default)]
    pub host: Option<String>,
    /// TCP port dockerd listens on. Default 2376.
    #[serde(default)]
    pub port: Option<u16>,
    /// Address dockerd binds; default `host` when it is one of the node's
    /// addresses. `0.0.0.0` listens on all of them.
    #[serde(default)]
    pub listen: Option<std::net::IpAddr>,
    /// Enabling restarts dockerd. Without `live-restore`, running containers
    /// stop (those with a restart policy start again): set this to accept.
    #[serde(default)]
    pub acknowledge_restart: bool,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct JobStarted {
    pub job_id: String,
}

/// A host the control plane can dial: an IP address or a DNS name.
fn valid_host(h: &str) -> bool {
    h.parse::<std::net::IpAddr>().is_ok()
        || (!h.is_empty()
            && h.len() <= 253
            && h.split('.').all(|l| {
                !l.is_empty()
                    && l.len() <= 63
                    && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                    && !l.starts_with('-')
                    && !l.ends_with('-')
            }))
}

async fn enable(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServerId>,
    Json(req): Json<EnableRequest>,
) -> ApiResult<(StatusCode, Json<JobStarted>)> {
    let resource = server_ref(id);
    caller.authorize(Action::ServerCommand, resource.clone()).await?;
    let server = state.docker.server(id).await.map_err(|_| ApiError::not_found("server not found"))?;
    let host = match req.host {
        Some(h) => h,
        None => default_host(&state, id).await?,
    };
    if !valid_host(&host) {
        return Err(ApiError::bad_request("`host` must be an IP address or a DNS name"));
    }
    let port = req.port.unwrap_or(forge_catalog::docker_tls::DEFAULT_PORT);
    if port < 1024 {
        return Err(ApiError::bad_request("`port` must be 1024 or higher"));
    }
    let payload = json!({
        "server_id": id, "host": host, "port": port, "listen": req.listen,
        "acknowledge_restart": req.acknowledge_restart,
    });
    let job = state
        .jobs
        .submit(NewJob {
            kind: ENABLE_KIND.into(),
            workspace_id: Some(server.workspace_id),
            resource: resource.clone(),
            principal: caller.principal(),
            context: caller.ctx(),
            idempotency_key: None,
            payload: payload.clone(),
            max_attempts: 1,
        })
        .await?;
    caller
        .audit(AUDIT_ENABLE, &resource, Some(server.workspace_id), json!({ "job_id": job.id, "request": payload }))
        .await;
    Ok((StatusCode::ACCEPTED, Json(JobStarted { job_id: job.id.to_string() })))
}

/// Where the node is reached by default (`crate::facts::node_address`):
/// the address its agent connects from, else the one in its facts.
async fn default_host(state: &AppState, id: ServerId) -> ApiResult<String> {
    crate::facts::node_address(state, id).await?.ok_or_else(|| {
        ApiError::bad_request("no address known for this node: pass `host` (the address the control plane dials)")
    })
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DisableQuery {
    /// Forget the endpoint without asking the node (when the node is gone).
    #[serde(default)]
    pub force: bool,
}

async fn disable(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServerId>,
    Query(q): Query<DisableQuery>,
) -> ApiResult<(StatusCode, Json<JobStarted>)> {
    let resource = server_ref(id);
    caller.authorize(Action::ServerCommand, resource.clone()).await?;
    let server = state.docker.server(id).await.map_err(|_| ApiError::not_found("server not found"))?;
    if state.docker.endpoint(id).await?.is_none() {
        return Err(ApiError::conflict("this node's Docker API endpoint is not enabled"));
    }
    let payload = json!({ "server_id": id, "force": q.force });
    let job = state
        .jobs
        .submit(NewJob {
            kind: DISABLE_KIND.into(),
            workspace_id: Some(server.workspace_id),
            resource: resource.clone(),
            principal: caller.principal(),
            context: caller.ctx(),
            idempotency_key: None,
            payload: payload.clone(),
            max_attempts: 1,
        })
        .await?;
    caller
        .audit(AUDIT_DISABLE, &resource, Some(server.workspace_id), json!({ "job_id": job.id, "force": q.force }))
        .await;
    Ok((StatusCode::ACCEPTED, Json(JobStarted { job_id: job.id.to_string() })))
}

// ---------------------------------------------------------------- engine

fn docker_error(status: StatusCode, code: &'static str, message: impl Into<String>) -> ApiError {
    ApiError::new(status, code, message)
}

/// Copies allow-listed headers.
fn copy_headers(from: &HeaderMap, to: &mut HeaderMap, allowed: &[&str]) {
    for name in allowed {
        for v in from.get_all(*name) {
            if let Ok(n) = HeaderName::from_bytes(name.as_bytes()) {
                to.append(n, v.clone());
            }
        }
    }
}

/// Query keys only, for the audit entry (values may name registries or
/// carry filters; never needed to understand the change).
fn query_keys(q: &str) -> Vec<String> {
    q.split('&').filter_map(|kv| kv.split('=').next()).filter(|k| !k.is_empty()).map(str::to_owned).collect()
}

async fn engine(
    State(state): State<AppState>,
    caller: Caller,
    Path((id, path)): Path<(ServerId, String)>,
    mut req: Request,
) -> Response {
    match forward(&state, &caller, id, &format!("/{path}"), &mut req).await {
        Ok(r) => r,
        Err(e) => e.into_response(),
    }
}

async fn forward(
    state: &AppState,
    caller: &Caller,
    id: ServerId,
    path: &str,
    req: &mut Request,
) -> ApiResult<Response> {
    let method = req.method().as_str().to_owned();
    let matched = catalog::lookup(&method, path).map_err(|e| match e {
        catalog::MatchError::Unknown { .. } => {
            docker_error(StatusCode::NOT_FOUND, "docker_unknown_endpoint", e.to_string())
        }
        catalog::MatchError::BadPath(_) => ApiError::bad_request(e.to_string()),
    })?;
    let query = req.uri().query().unwrap_or_default().to_owned();
    if query.len() > MAX_QUERY {
        return Err(ApiError::bad_request("query string too long"));
    }
    let endpoint = matched.endpoint;
    let resource = server_ref(id);

    // 1. Read an inspected body and look for host access.
    let mut inspected: Option<Bytes> = None;
    let mut reasons: Vec<&'static str> = Vec::new();
    if endpoint.transfer == Transfer::Inspect {
        let body = std::mem::take(req.body_mut());
        let bytes = http_body_util::Limited::new(body, MAX_INSPECTED)
            .collect()
            .await
            .map_err(|_| docker_error(StatusCode::PAYLOAD_TOO_LARGE, "body_too_large", "request body over 1 MiB"))?
            .to_bytes();
        if !bytes.is_empty() {
            let v: Value = serde_json::from_slice(&bytes)
                .map_err(|e| ApiError::bad_request(format!("the request body is not JSON: {e}")))?;
            reasons = guard::host_access(endpoint.op, &v);
        }
        inspected = Some(bytes);
    }
    let class = guard::effective_class(endpoint.class, &reasons);

    // 2. Authorize (denials are audited by `authorize_with`).
    let denied = json!({ "op": endpoint.op, "method": method, "path": matched.path, "host_access": reasons });
    caller.authorize_with(action_for(class), resource.clone(), denied).await?;

    // 3. The node's endpoint.
    let (_, target) = state.docker.target(id, false).await.map_err(|e| match e {
        forge_core::DomainError::Conflict(m) => docker_error(StatusCode::CONFLICT, "docker_endpoint_unavailable", m),
        other => other.into(),
    })?;
    let server = state.docker.server(id).await.map_err(|_| ApiError::not_found("server not found"))?;

    // 4. Build the upstream request.
    let version = matched.version.as_deref().map(|v| format!("/{v}")).unwrap_or_default();
    let uri = if query.is_empty() {
        format!("http://docker{version}{}", matched.path)
    } else {
        format!("http://docker{version}{}?{query}", matched.path)
    };
    let mut builder = axum::http::Request::builder().method(method.as_str()).uri(uri).header(header::HOST, "docker");
    if let Some(h) = builder.headers_mut() {
        copy_headers(req.headers(), h, REQUEST_HEADERS);
    }
    let hijack = endpoint.transfer == Transfer::Hijack;
    let client_upgrade = if hijack {
        let upgrade = req.headers().get(header::UPGRADE).cloned();
        let Some(upgrade) = upgrade else {
            return Err(ApiError::bad_request(
                "this endpoint upgrades the connection: send `Connection: Upgrade` and `Upgrade: tcp` (or a WebSocket)",
            ));
        };
        builder = builder.header(header::CONNECTION, "Upgrade").header(header::UPGRADE, upgrade);
        Some(req.extensions_mut().remove::<hyper::upgrade::OnUpgrade>().ok_or_else(|| {
            ApiError::bad_request("this connection cannot be upgraded (HTTP/2 or a proxy that drops Upgrade)")
        })?)
    } else {
        None
    };
    let body = match inspected {
        Some(b) => Body::from(b),
        None => Body::new(http_body_util::Limited::new(std::mem::take(req.body_mut()), MAX_UPLOAD)),
    };
    let upstream = builder.body(body).map_err(|e| ApiError::bad_request(e.to_string()))?;

    // 5. Send it.
    let audit = class != Class::Read;
    let details = |status: u16| {
        json!({
            "op": endpoint.op, "method": method, "path": matched.path, "query": query_keys(&query),
            "status": status, "class": class_name(class), "host_access": reasons,
        })
    };
    let resp = match target.send(upstream).await {
        Ok(r) => r,
        Err(e) => {
            if audit {
                caller.audit_failed(AUDIT_API, &resource, Some(server.workspace_id), details(0), &e.to_string()).await;
            }
            return Err(docker_error(StatusCode::BAD_GATEWAY, "docker_unreachable", e.to_string()));
        }
    };
    let status = resp.status();
    if audit {
        caller.audit(AUDIT_API, &resource, Some(server.workspace_id), details(status.as_u16())).await;
    }

    let (parts, body) = resp.into_parts();

    // 6. Upgraded streams: join the two connections.
    if let Some(client_upgrade) = client_upgrade {
        if status != StatusCode::SWITCHING_PROTOCOLS {
            // The daemon refused (no such container, …): pass its answer on.
            return Ok(passthrough(&parts, Body::new(body)));
        }
        let mut out = passthrough(&parts, Body::empty());
        for n in [header::CONNECTION, header::UPGRADE] {
            if let Some(v) = parts.headers.get(&n) {
                out.headers_mut().insert(n, v.clone());
            }
        }
        let upstream_upgrade = hyper::upgrade::on(axum::http::Response::from_parts(parts, body));
        tokio::spawn(async move {
            let (Ok(up), Ok(down)) = (upstream_upgrade.await, client_upgrade.await) else {
                tracing::debug!("docker upgrade did not complete");
                return;
            };
            let mut up = hyper_util::rt::TokioIo::new(up);
            let mut down = hyper_util::rt::TokioIo::new(down);
            if let Err(e) = tokio::io::copy_bidirectional(&mut up, &mut down).await {
                tracing::debug!(error = %e, "docker stream ended");
            }
        });
        return Ok(out);
    }

    // 7. A container inspect without `container.inspect_env`: redact values.
    if endpoint.op == "container.inspect"
        && status.is_success()
        && !caller.can(Action::ContainerInspectEnv, resource.clone()).await
    {
        let bytes = http_body_util::Limited::new(body, MAX_INSPECT_ANSWER)
            .collect()
            .await
            .map_err(|e| docker_error(StatusCode::BAD_GATEWAY, "docker_unreachable", e.to_string()))?
            .to_bytes();
        let mut v: Value = serde_json::from_slice(&bytes)
            .map_err(|e| docker_error(StatusCode::BAD_GATEWAY, "docker_bad_answer", e.to_string()))?;
        catalog::redact_env(&mut v);
        let mut out =
            passthrough(&parts, Body::from(serde_json::to_vec(&v).map_err(forge_core::DomainError::internal)?));
        out.headers_mut().insert("x-forge-redacted", HeaderValue::from_static("Config.Env"));
        return Ok(out);
    }
    Ok(passthrough(&parts, Body::new(body)))
}

/// The daemon's status and allow-listed headers, with `body`.
fn passthrough(parts: &axum::http::response::Parts, body: Body) -> Response {
    let mut out = Response::new(body);
    *out.status_mut() = parts.status;
    copy_headers(&parts.headers, out.headers_mut(), RESPONSE_HEADERS);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_and_query_keys() {
        assert!(valid_host("10.0.0.5") && valid_host("fe80::1") && valid_host("node-1.example.com"));
        assert!(!valid_host("") && !valid_host("a b") && !valid_host("-x.example") && !valid_host("x;y"));
        assert_eq!(query_keys("all=1&filters=%7B%7D&"), vec!["all", "filters"]);
    }

    #[test]
    fn classes_map_to_existing_actions() {
        assert_eq!(action_for(Class::Read), Action::ServerManage);
        assert_eq!(action_for(Class::Manage), Action::ContainerManage);
        assert_eq!(action_for(Class::Exec), Action::ContainerExec);
        assert_eq!(action_for(Class::Admin), Action::ServerCommand);
    }
}
