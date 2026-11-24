//! Port tunnels (#35, D-091, D-100): `forge tunnel open|list|close|grants`.
//!
//! * `POST /tunnels` `{target, bind?, lifetime_minutes? | never_expires?}` —
//!   the target is a service from the [catalogue](crate::service_ports)
//!   (`app`, `stack`, `package`), a node's `ssh` port or, for administrators
//!   with `network.tunnel.any`, a `raw` port. A service is allowed by a role
//!   on its environment (`network.tunnel`), by `terminal.open` for SSH, or by an
//!   explicit [grant](forge_core::tunnels::TunnelGrant). The lifetime is
//!   `min(requested, the grant's expiry, [tunnels] max_lifetime_hours)`;
//!   `never` needs `[tunnels] allow_never_expiry` and `access.manage`.
//!   Returns the tunnel with its dial info; the CLI must hold its control
//!   socket within 30 seconds.
//! * `GET /tunnels/{id}/control` — WebSocket that holds the tunnel open.
//!   Closing it ends the tunnel; revocation, a lost permission, grant or
//!   session, idleness and expiry end it from here (checked every 15 s).
//! * `GET /tunnels/{id}/connect` — WebSocket per TCP connection the CLI
//!   accepted: binary frames both ways, relayed to the node.
//! * `POST /tunnels/{id}/ssh-cert` — for a tunnel opened with `direct`
//!   (#35 phase 2): the node's SSH endpoint and a one-minute certificate
//!   for one SSH connection that may only forward to the tunnel's
//!   destination. Access is re-checked on every certificate.
//! * `GET /tunnels?state=open|closed|all&all=true` (own; everyone's with
//!   `user.manage`), `DELETE /tunnels/{id}` (owner or `user.manage`).
//! * `GET|POST /tunnel-grants`, `DELETE /tunnel-grants/{id}` — explicit
//!   grants (`access.manage`, MFA). Revoking a grant closes the tunnels it
//!   allowed.
//!
//! Every tunnel is also a row (`tunnels`): history, the console list and,
//! after a restart, `closed(server_restart)`. Tunnels are for people
//! (session or API key). AI connectors cannot open them.

use std::sync::atomic::Ordering;
use std::time::Duration;

use axum::Json;
use axum::extract::ws::{Message, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use forge_core::model::{AgentStatus, SessionKind};
use forge_core::ports::{ServerRepository, UserRepository};
use forge_core::tunnels::{TunnelGrant, TunnelGrantRepository, TunnelRecord, TunnelRepository, TunnelTargetKind};
use forge_core::*;
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::auth::{Caller, Ws};
use crate::error::{ApiError, ApiResult};
use crate::request_origin::RequestOrigin;
use crate::service_ports::{LOOPBACK, ServicePort, role_for_target};
use crate::state::AppState;
use crate::tunnels::{Limits, NewTunnel, Refusal, Target, Tunnel};

/// How often a tunnel re-checks its owner's access.
const RECHECK: Duration = Duration::from_secs(15);
/// How long a connection waits for its node to dial back.
const DIAL_WAIT: Duration = Duration::from_secs(20);
/// Largest WebSocket frame accepted from the CLI.
const MAX_FRAME: usize = 256 * 1024;
/// Rows the history list returns at most.
const HISTORY_LIMIT: u32 = 200;
/// The control socket sleeps at most this long before re-reading the expiry
/// (a never-expiring tunnel has no finite deadline).
const MAX_SLEEP: Duration = Duration::from_secs(24 * 3600);

fn server_ref(id: ServerId) -> EntityRef {
    EntityRef::new(EntityKind::Server, id)
}

fn ws_ref(id: WorkspaceId) -> EntityRef {
    EntityRef::new(EntityKind::Workspace, id)
}

fn limits(state: &AppState) -> Limits {
    let t = &state.config.tunnels;
    Limits {
        per_user: t.max_per_user as usize,
        per_node: t.max_connections_per_node as usize,
        global: t.max_connections as usize,
    }
}

fn refusal(r: &Refusal) -> ApiError {
    match r {
        Refusal::Gone => ApiError::not_found("no open tunnel with this id"),
        _ => ApiError::new(StatusCode::CONFLICT, "tunnel_limit", r.to_string()),
    }
}

/// `expires_at` for JSON: `null` when it never expires.
fn expiry_json(t: Timestamp) -> Value {
    if t >= NEVER_EXPIRES { Value::Null } else { json!(t.0) }
}

/// `[tunnels]` as clients need it (`GET /settings`, the lists).
#[must_use]
pub fn settings_view(state: &AppState) -> Value {
    let t = &state.config.tunnels;
    json!({
        "allow_never_expiry": t.allow_never_expiry,
        "default_lifetime_minutes": t.default_lifetime_hours * 60,
        "max_lifetime_minutes": t.max_lifetime_hours * 60,
        "max_per_user": t.max_per_user,
    })
}

/// The checks every tunnel request makes: a person (no AI connector), and
/// the Origin guard for WebSockets.
fn person(caller: &Caller, origin: &RequestOrigin, headers: &HeaderMap, state: &AppState) -> ApiResult<()> {
    if caller.session.session.connection_id.is_some() {
        return Err(ApiError::forbidden("AI connectors cannot open port tunnels"));
    }
    if !crate::request_origin::origin_header_ok(state, origin, headers) {
        return Err(ApiError::forbidden("origin not allowed"));
    }
    Ok(())
}

/// A `{"type": "refresh", "token": "<access token>"}` frame: the session the
/// token names, when it is a live sign-in of the same user with the same
/// second-factor state. Returns its id.
async fn refreshed(state: &AppState, caller: &Caller, text: &str) -> Option<String> {
    use forge_core::ports::SessionRepository;
    let v: Value = serde_json::from_str(text).ok()?;
    if v["type"] != "refresh" {
        return None;
    }
    let token = v["token"].as_str().filter(|t| t.len() <= 512)?;
    let hash = forge_secrets::crypto::sha256_hex(token.as_bytes());
    let s = state.store.get_session(&hash).await.ok()??;
    let same = s.user_id == caller.session.user.id
        && s.expires_at > state.clock.now()
        && s.connection_id.is_none()
        && s.mfa == caller.session.session.mfa;
    same.then_some(hash)
}

/// The owner's session or API key still exists.
async fn session_alive(state: &AppState, caller: &Caller, session: &str) -> bool {
    use forge_core::ports::{ApiKeyRepository, SessionRepository};
    let now = state.clock.now();
    match session.strip_prefix("apikey:") {
        Some(key) => {
            let user = caller.session.user.id;
            let enabled = state.store.get_user(user).await.is_ok_and(|u| !u.disabled);
            enabled
                && state
                    .store
                    .list_api_keys(user)
                    .await
                    .is_ok_and(|keys| keys.iter().any(|k| k.id == key && k.expires_at.is_none_or(|e| e > now)))
        }
        None => matches!(state.store.get_session(session).await, Ok(Some(s)) if s.expires_at > now),
    }
}

// ------------------------------------------------------------ authorization

/// Why a tunnel to one (target, node, port) is allowed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Allowance {
    /// The grant that allows it, when no role does.
    pub grant: Option<TunnelGrant>,
}

impl Allowance {
    /// The expiry a grant imposes on tunnels opened through it.
    fn cap(&self) -> Option<Timestamp> {
        self.grant.as_ref().map(|g| g.expires_at)
    }
}

/// Whether `caller` may tunnel to `port` of the target on `server`: a raw
/// port needs `network.tunnel.any` on the node; SSH `terminal.open` on the
/// node; an app or stack service `network.tunnel` on its environment; a
/// package `network.tunnel` on the node. Anything but a raw port may instead
/// be covered by an active grant to this user. Every check runs with the
/// caller's own context (MFA state), so the guardrails apply.
pub async fn allowance(
    state: &AppState,
    caller: &Caller,
    kind: TunnelTargetKind,
    target_ref: &str,
    environment_id: Option<EnvironmentId>,
    server: ServerId,
    port: u16,
) -> Option<Allowance> {
    let by_role = match kind {
        TunnelTargetKind::Raw => {
            return caller.can(Action::NetworkTunnelAny, server_ref(server)).await.then_some(Allowance { grant: None });
        }
        TunnelTargetKind::Ssh => caller.can(Action::TerminalOpen, server_ref(server)).await,
        TunnelTargetKind::App | TunnelTargetKind::Stack => match environment_id {
            Some(ns) => caller.can(Action::NetworkTunnel, EntityRef::new(EntityKind::Environment, ns)).await,
            None => false,
        },
        TunnelTargetKind::Package => caller.can(Action::NetworkTunnel, server_ref(server)).await,
    };
    if by_role {
        return Some(Allowance { grant: None });
    }
    let now = state.clock.now();
    let user = caller.session.user.id;
    let grants = state.store.list_tunnel_grants(Some(user)).await.ok()?;
    grants
        .into_iter()
        .find(|g| g.covers(user, kind, target_ref, server, port, now))
        .map(|g| Allowance { grant: Some(g) })
}

/// Whether a tunnel's owner may still use it (the 15 s re-check): the
/// session lives and the role or grant that allowed it still holds.
async fn still_allowed(state: &AppState, caller: &Caller, session: &str, t: &Tunnel) -> bool {
    if !session_alive(state, caller, session).await {
        return false;
    }
    let tg = &t.target;
    allowance(state, caller, tg.kind, &tg.target_ref, tg.environment_id, t.server_id, t.port).await.is_some()
}

// ------------------------------------------------------------ targets

/// What the client asks to reach.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TargetSpec {
    /// `app`, `stack`, `package`, `ssh` or `raw`.
    #[schemars(with = "String")]
    pub kind: TunnelTargetKind,
    /// The service: an app by name, id or `<environment>/<name>`; a stack
    /// service as `<stack>/<service>`; a package by package id or
    /// installation id; for `ssh` and `raw` the node (label or id).
    #[serde(rename = "ref", default)]
    pub target: String,
    /// The node, when the service runs on several (or for `ssh`/`raw`).
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub server_id: Option<ServerId>,
    /// Stack service name, when `ref` names the stack alone.
    #[serde(default)]
    pub service: Option<String>,
    /// Keep only these remote ports; one for a tunnel. Required for `raw`.
    #[serde(default, deserialize_with = "ports_field")]
    #[schemars(with = "Option<Vec<u16>>")]
    pub ports: Option<Vec<u16>>,
}

/// Ports as a JSON array, a comma-separated string or `"*"` (every port).
fn ports_field<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Vec<u16>>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        List(Vec<u16>),
        Text(String),
    }
    let Some(raw) = Option::<Raw>::deserialize(d)? else { return Ok(None) };
    let mut ports = match raw {
        Raw::List(l) => l,
        Raw::Text(s) if s.trim() == "*" || s.trim().is_empty() => Vec::new(),
        Raw::Text(s) => s
            .split(',')
            .map(|p| p.trim().parse::<u16>().map_err(|_| serde::de::Error::custom(format!("bad port {p:?}"))))
            .collect::<Result<Vec<u16>, _>>()?,
    };
    if ports.contains(&0) {
        return Err(serde::de::Error::custom("ports are 1 to 65535"));
    }
    ports.sort_unstable();
    ports.dedup();
    Ok(Some(ports))
}

/// The node `spec` names for `ssh` and `raw`: `server_id`, else `ref` as an
/// id, else `ref` as a node label.
async fn node_of(state: &AppState, ws: WorkspaceId, spec: &TargetSpec) -> ApiResult<forge_core::model::Server> {
    if let Some(id) = spec.server_id.or_else(|| spec.target.parse().ok()) {
        return state.store.get_server(id).await.map_err(|_| ApiError::not_found("server not found"));
    }
    let mut servers = state.store.list_servers(Some(ws)).await?;
    servers.retain(|s| s.status != AgentStatus::Revoked);
    let refs: Vec<&forge_core::model::Server> = servers.iter().collect();
    let labels = crate::names::node_labels(&refs, &crate::names::stored_labels(state, ws).await.map_err(internal)?);
    let found =
        servers.iter().find(|s| labels.get(&s.id).is_some_and(|l| l.label == spec.target) || s.hostname == spec.target);
    found.cloned().ok_or_else(|| ApiError::not_found(format!("no node named {:?}", spec.target)))
}

fn internal(e: String) -> ApiError {
    tracing::error!(error = %e, "tunnel target resolution failed");
    ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal error")
}

/// A `409 ambiguous_target` with the entries the client may choose from.
fn ambiguous(choices: &[ServicePort]) -> Response {
    let body = json!({
        "type": "https://docs.orbyn.dev/errors/ambiguous_target",
        "title": "ambiguous_target",
        "status": 409,
        "detail": "the target runs on several nodes or ports; choose one with server_id and ports",
        "choices": choices,
    });
    let mut resp = (StatusCode::CONFLICT, Json(body)).into_response();
    resp.headers_mut()
        .insert(axum::http::header::CONTENT_TYPE, axum::http::HeaderValue::from_static("application/problem+json"));
    resp
}

/// Resolves a target to catalogue entries: the ones of its kind that match
/// `ref` (and `service`, `server_id`, `ports`). A raw target becomes one
/// synthetic entry. `online_only` keeps nodes that are online (tunnels);
/// grants cover offline nodes too.
async fn resolve(
    state: &AppState,
    ws: WorkspaceId,
    spec: &TargetSpec,
    online_only: bool,
) -> ApiResult<Vec<ServicePort>> {
    if spec.kind == TunnelTargetKind::Raw {
        let server = node_of(state, ws, spec).await?;
        let labels = crate::names::stored_labels(state, ws).await.map_err(internal)?;
        let label = labels.get(&server.id).map_or_else(|| crate::names::derived_label(&server), |(l, _)| l.clone());
        let ports = spec.ports.clone().unwrap_or_default();
        if ports.is_empty() {
            return Err(ApiError::bad_request("a raw target needs `ports`"));
        }
        return Ok(ports
            .into_iter()
            .map(|port| ServicePort {
                kind: TunnelTargetKind::Raw,
                id: server.id.to_string(),
                name: label.clone(),
                stack: None,
                service: None,
                label: format!("port {port} on {label}"),
                environment_id: None,
                environment: None,
                server_id: server.id,
                server: label.clone(),
                online: server.status == AgentStatus::Online,
                port,
                proto: "tcp".into(),
                role: role_for_target(port).to_owned(),
                dial_ip: LOOPBACK.to_owned(),
            })
            .collect());
    }
    let q = match (&spec.kind, &spec.service) {
        (TunnelTargetKind::Stack, Some(svc)) if !spec.target.contains('/') => format!("{}/{svc}", spec.target),
        _ => spec.target.clone(),
    };
    if q.trim().is_empty() {
        return Err(ApiError::bad_request("`ref` names the service or node"));
    }
    let all = crate::service_ports::catalogue(state, ws).await.map_err(internal)?;
    let mut hits: Vec<ServicePort> = all
        .into_iter()
        .filter(|e| e.kind == spec.kind && e.matches(&q))
        .filter(|e| spec.server_id.is_none_or(|s| e.server_id == s))
        .filter(|e| spec.ports.as_ref().is_none_or(|p| p.is_empty() || p.contains(&e.port)))
        .collect();
    if hits.is_empty() {
        return Err(ApiError::not_found(format!("no {} named {q:?} with those ports or on that node", spec.kind)));
    }
    if online_only {
        hits.retain(|e| e.online);
        if hits.is_empty() {
            return Err(ApiError::conflict(format!("{q} runs on no node that is online")));
        }
    }
    Ok(hits)
}

// ------------------------------------------------------------ lifetime

/// The expiry asked for, within the configured maximum and the grant's
/// own expiry (`cap`). `never` needs `[tunnels] allow_never_expiry` and
/// `access.manage`; denials are audited as `audit_action`.
#[allow(clippy::too_many_arguments)]
async fn expiry(
    state: &AppState,
    caller: &Caller,
    ws: WorkspaceId,
    audit_action: &str,
    resource: &EntityRef,
    minutes: Option<u32>,
    never: bool,
    cap: Option<Timestamp>,
) -> ApiResult<Timestamp> {
    let t = &state.config.tunnels;
    let now = state.clock.now();
    let wanted = if never {
        if !t.allow_never_expiry {
            caller.audit_denied(audit_action, resource, Some(ws), "never-expiring tunnels are disabled").await;
            return Err(ApiError::forbidden(
                "tunnels that never expire are disabled; an administrator can enable `[tunnels] allow_never_expiry`",
            ));
        }
        caller.authorize(Action::AccessManage, ws_ref(ws)).await?;
        NEVER_EXPIRES
    } else {
        let m = i64::from(minutes.unwrap_or(t.default_lifetime_hours * 60));
        if m == 0 || m * 60_000 > t.max_lifetime_ms() {
            return Err(ApiError::bad_request(format!(
                "the lifetime must be between 1 minute and {} hours",
                t.max_lifetime_hours
            )));
        }
        Timestamp(now.0.saturating_add(m * 60_000))
    };
    Ok(cap.map_or(wanted, |c| wanted.min(c)))
}

// ------------------------------------------------------------ views

fn target_json(t: &Target) -> Value {
    let service = (t.kind == TunnelTargetKind::Stack).then(|| t.target_ref.rsplit('/').next().unwrap_or_default());
    json!({
        "kind": t.kind,
        "ref": t.target_ref,
        "name": t.name,
        "label": t.label,
        "environment_id": t.environment_id,
        "service": service,
    })
}

fn view(t: &Tunnel) -> Value {
    let (up, down) = t.bytes();
    json!({
        "id": t.id,
        "status": "open",
        "user_id": t.user_id,
        "user": t.user,
        "server_id": t.server_id,
        "server": t.server,
        "target": target_json(&t.target),
        "port": t.port,
        "role": t.target.role,
        "dial_ip": t.target.dial_ip,
        "bind": t.bind,
        "direct": t.direct,
        "grant_id": t.target.grant_id,
        "started_at": t.started_at,
        "opened_at": t.started_at,
        "expires_at": expiry_json(t.expires_at),
        "never_expires": t.expires_at >= NEVER_EXPIRES,
        "connections": t.connections,
        "bytes_up": up,
        "bytes_down": down,
    })
}

fn row_view(r: &TunnelRecord, user: &str, server: &str) -> Value {
    let service =
        (r.target_kind == TunnelTargetKind::Stack).then(|| r.target_ref.rsplit('/').next().unwrap_or_default());
    json!({
        "id": r.id,
        "status": if r.closed_at.is_some() { "closed" } else { "open" },
        "user_id": r.user_id,
        "user": user,
        "server_id": r.server_id,
        "server": server,
        "target": { "kind": r.target_kind, "ref": r.target_ref, "name": r.label, "label": r.label, "service": service },
        "port": r.port,
        "dial_ip": r.dial_ip,
        "bind": r.bind,
        "grant_id": r.grant_id,
        "started_at": r.opened_at,
        "opened_at": r.opened_at,
        "expires_at": expiry_json(r.expires_at),
        "never_expires": r.never_expires(),
        "closed_at": r.closed_at,
        "close_reason": r.close_reason,
        "connections": 0,
        "bytes_up": r.bytes_up,
        "bytes_down": r.bytes_down,
    })
}

fn record(t: &Tunnel) -> TunnelRecord {
    let (up, down) = t.bytes();
    TunnelRecord {
        id: t.id.clone(),
        workspace_id: t.workspace_id,
        user_id: t.user_id,
        server_id: t.server_id,
        target_kind: t.target.kind,
        target_ref: t.target.target_ref.clone(),
        label: t.target.label.clone(),
        port: t.port,
        dial_ip: t.target.dial_ip.clone(),
        bind: t.bind.clone(),
        grant_id: t.target.grant_id,
        opened_at: t.started_at,
        expires_at: t.expires_at,
        closed_at: None,
        close_reason: None,
        bytes_up: up,
        bytes_down: down,
    }
}

/// Ends a tunnel everywhere: the registry (every leg closes) and its row.
pub async fn end_tunnel(state: &AppState, id: &str, reason: &'static str) -> Option<Tunnel> {
    let t = state.tunnels.close_with(id, reason)?;
    let (up, down) = t.bytes();
    if let Err(e) = state.store.close_tunnel(id, state.clock.now(), reason, up, down).await {
        tracing::warn!(error = %e, tunnel = id, "tunnel row not closed");
    }
    Some(t)
}

// ------------------------------------------------------------ open

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OpenRequest {
    pub target: TargetSpec,
    /// The address the CLI listens on, for the audit log.
    #[serde(default)]
    pub bind: String,
    /// Lifetime in minutes; the default is `[tunnels] default_lifetime_hours`.
    #[serde(default)]
    pub lifetime_minutes: Option<u32>,
    /// No expiry: `[tunnels] allow_never_expiry` and `access.manage`.
    #[serde(default)]
    pub never_expires: bool,
    /// Connect straight to the node's SSH instead of relaying through this
    /// server (#35 phase 2). The node must report `ssh_tunnel` and have an
    /// SSH endpoint with a pinned host key.
    #[serde(default)]
    pub direct: bool,
}

/// `POST /api/v1/tunnels`.
pub async fn open(
    State(state): State<AppState>,
    caller: Caller,
    origin: RequestOrigin,
    Ws(ws): Ws,
    headers: HeaderMap,
    Json(req): Json<OpenRequest>,
) -> ApiResult<Response> {
    person(&caller, &origin, &headers, &state)?;
    let hits = resolve(&state, ws, &req.target, true).await?;
    let [entry] = hits.as_slice() else { return Ok(ambiguous(&hits)) };
    let server = state.store.get_server(entry.server_id).await.map_err(|_| ApiError::not_found("server not found"))?;
    let resource = server_ref(server.id);
    let raw = entry.kind == TunnelTargetKind::Raw;
    let Some(allowed) =
        allowance(&state, &caller, entry.kind, &entry.id, entry.environment_id, entry.server_id, entry.port).await
    else {
        let why = match entry.kind {
            TunnelTargetKind::Raw => "raw ports need `network.tunnel.any` on the node (MFA)".to_owned(),
            TunnelTargetKind::Ssh => "SSH tunnels need `terminal.open` on the node (MFA) or a tunnel grant".to_owned(),
            _ => format!(
                "no role on the service's environment allows `network.tunnel` to {} (MFA), and no grant covers it",
                entry.label
            ),
        };
        caller
            .audit_denied(
                "network.tunnel.open",
                &resource,
                Some(server.workspace_id),
                &format!("{} port {}: {why}", entry.label, entry.port),
            )
            .await;
        return Err(ApiError::forbidden(why));
    };
    let expires_at = expiry(
        &state,
        &caller,
        ws,
        "network.tunnel.open",
        &resource,
        req.lifetime_minutes,
        req.never_expires,
        allowed.cap(),
    )
    .await?;
    if req.direct {
        direct_endpoint(&state, server.id).await?;
    }
    let bind: String = req.bind.chars().filter(|c| !c.is_control()).take(64).collect();
    let u = &caller.session.user;
    let name = u.username.clone().or_else(|| u.email.clone()).unwrap_or_else(|| u.id.to_string());
    let target = Target {
        kind: entry.kind,
        target_ref: entry.id.clone(),
        name: entry.name.clone(),
        label: entry.label.clone(),
        environment_id: entry.environment_id,
        role: entry.role.clone(),
        dial_ip: entry.dial_ip.clone(),
        grant_id: allowed.grant.as_ref().map(|g| g.id),
    };
    let now = state.clock.now();
    let t = state
        .tunnels
        .open(
            NewTunnel {
                workspace_id: server.workspace_id,
                user_id: u.id,
                user: name,
                server_id: server.id,
                server: entry.server.clone(),
                target,
                port: entry.port,
                bind: bind.clone(),
                direct: req.direct,
                now,
                expires_at,
            },
            limits(&state),
        )
        .map_err(|r| refusal(&r))?;
    if let Err(e) = state.store.put_tunnel(&record(&t)).await {
        state.tunnels.close_with(&t.id, "closed");
        return Err(e.into());
    }
    caller
        .audit(
            "network.tunnel.open",
            &resource,
            Some(server.workspace_id),
            json!({
                "tunnel": t.id, "target": target_json(&t.target), "port": t.port, "dial_ip": t.target.dial_ip,
                "bind": bind, "raw": raw, "direct": t.direct, "grant": t.target.grant_id, "expires_at": expiry_json(t.expires_at),
                "via_api_key": caller.via_api_key(),
            }),
        )
        .await;
    Ok((StatusCode::CREATED, Json(view(&t))).into_response())
}

// ------------------------------------------------------------ direct (#35 phase 2)

/// Certificates for direct tunnels only need to outlive the SSH handshake.
const DIRECT_CERT_TTL: Duration = Duration::from_secs(60);
/// The node account direct tunnels log in as (the agent's
/// `ssh_principals::TUNNEL_USER`).
const TUNNEL_USER: &str = "forge-tunnel";

/// The SSH endpoint direct tunnels to `server` use: the node must report
/// `ssh_tunnel` (its sshd limits tunnel logins to one destination) and have
/// an endpoint with a pinned host key.
async fn direct_endpoint(state: &AppState, server: ServerId) -> ApiResult<forge_core::model::SshEndpoint> {
    use forge_core::ports::SshEndpointRepository;
    use forge_core::tags::FactsRepository;
    let ready = || async {
        let facts = state.store.all_facts().await?;
        DomainResult::Ok(
            facts.get(&server).is_some_and(|(v, _)| v.get("ssh_tunnel").and_then(Value::as_bool) == Some(true)),
        )
    };
    // Stored facts can predate the node's setup (they are refreshed daily
    // and on agent updates, which race the helper): ask the node once more.
    let mut is_ready = ready().await?;
    if !is_ready && state.tunnels.may_refresh_facts(server, state.clock.now()) {
        if let Err(e) = crate::facts::collect(state, server).await {
            tracing::debug!(error = %e, "facts refresh for a direct tunnel failed");
        }
        is_ready = ready().await?;
    }
    if !is_ready {
        return Err(ApiError::conflict(
            "this node is not set up for direct tunnels (it needs the browser terminal and a current agent); open the tunnel without --direct",
        ));
    }
    state.store.get_ssh_endpoint(server).await?.ok_or_else(|| {
        ApiError::conflict("this node has no SSH endpoint with a pinned host key; set one on the node's Terminal page")
    })
}

/// What the CLI needs for one direct connection. The private key is for
/// this connection only and is never stored.
#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct DirectCert {
    /// The node's SSH endpoint and its pinned host key.
    pub host: String,
    pub port: u16,
    pub host_key_fingerprint: String,
    /// The account to log in as.
    pub user: String,
    /// The destination to forward to, as the node sees it.
    pub dial_ip: String,
    pub dial_port: u16,
    pub key_openssh: String,
    pub cert_openssh: String,
}

/// `POST /api/v1/tunnels/{id}/ssh-cert`: a certificate for one direct
/// connection, after the same access re-check the control socket makes.
pub async fn ssh_cert(
    State(state): State<AppState>,
    caller: Caller,
    origin: RequestOrigin,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<DirectCert>> {
    person(&caller, &origin, &headers, &state)?;
    let t = owned(&state, &caller, &id)?;
    if !t.direct {
        return Err(ApiError::conflict("this tunnel relays through the server; it was not opened with --direct"));
    }
    if !t.claimed {
        return Err(ApiError::conflict("hold the tunnel's control socket first"));
    }
    let resource = server_ref(t.server_id);
    let session = caller.session.session.id_hash.clone();
    if state.clock.now() >= t.expires_at || !still_allowed(&state, &caller, &session, &t).await {
        caller
            .audit_denied(
                "network.tunnel.direct",
                &resource,
                Some(t.workspace_id),
                "access withdrawn or tunnel expired",
            )
            .await;
        return Err(ApiError::forbidden("this tunnel is no longer allowed"));
    }
    let endpoint = direct_endpoint(&state, t.server_id).await?;
    let ip: std::net::IpAddr =
        t.target.dial_ip.parse().map_err(|_| ApiError::conflict("this target has no address to forward to"))?;
    let principal = format!("forge.{}.{TUNNEL_USER}", t.server_id);
    let key_id = format!("forge-tunnel/{}/{ip}/{}", t.id, t.port);
    let cert = state.ssh_ca.issue_tunnel(&principal, &key_id, DIRECT_CERT_TTL)?;
    t.last_activity.store(state.clock.now().0, Ordering::Relaxed);
    caller
        .audit(
            "network.tunnel.direct",
            &resource,
            Some(t.workspace_id),
            json!({
                "tunnel": t.id, "port": t.port, "dial_ip": t.target.dial_ip,
                "ssh": format!("{}:{}", endpoint.host, endpoint.port),
            }),
        )
        .await;
    Ok(Json(DirectCert {
        host: endpoint.host,
        port: endpoint.port,
        host_key_fingerprint: endpoint.host_key_fingerprint,
        user: TUNNEL_USER.to_owned(),
        dial_ip: t.target.dial_ip.clone(),
        dial_port: t.port,
        key_openssh: cert.key_openssh,
        cert_openssh: cert.cert_openssh,
    }))
}

fn owned(state: &AppState, caller: &Caller, id: &str) -> ApiResult<Tunnel> {
    state
        .tunnels
        .get(id)
        .filter(|t| t.user_id == caller.session.user.id)
        .ok_or_else(|| ApiError::not_found("no open tunnel with this id"))
}

/// `GET /api/v1/tunnels/{id}/control` (WebSocket): holds the tunnel open.
pub async fn control(
    State(state): State<AppState>,
    caller: Caller,
    origin: RequestOrigin,
    Path(id): Path<String>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> ApiResult<Response> {
    person(&caller, &origin, &headers, &state)?;
    if caller.session.session.kind != SessionKind::Bearer {
        return Err(ApiError::forbidden("tunnels are opened with the forge CLI"));
    }
    let t = owned(&state, &caller, &id)?;
    let mut revoked = state.tunnels.claim(&id).ok_or_else(|| ApiError::conflict("this tunnel is already held"))?;
    Ok(ws.max_message_size(MAX_FRAME).on_upgrade(move |mut socket| async move {
        let idle_ms = i64::from(state.config.tunnels.idle_timeout_secs) * 1000;
        let mut session = caller.session.session.id_hash.clone();
        let mut recheck = tokio::time::interval(RECHECK);
        recheck.tick().await;
        let _ = socket.send(Message::Text(json!({ "type": "open", "tunnel": view(&t) }).to_string().into())).await;
        let reason: &'static str = loop {
            let remaining = Duration::from_millis(u64::try_from(t.expires_at.0 - state.clock.now().0).unwrap_or(0));
            let deadline = tokio::time::Instant::now() + remaining.min(MAX_SLEEP);
            tokio::select! {
                _ = revoked.changed() => break revoked.borrow().unwrap_or("closed"),
                () = tokio::time::sleep_until(deadline) => {
                    if state.clock.now() >= t.expires_at {
                        break "expired";
                    }
                }
                _ = recheck.tick() => {
                    if state.clock.now() >= t.expires_at {
                        break "expired";
                    }
                    if !still_allowed(&state, &caller, &session, &t).await {
                        break "access_withdrawn";
                    }
                    let now = state.clock.now().0;
                    let current = state.tunnels.get(&id);
                    let quiet = current.as_ref().is_some_and(|c| c.connections == 0 && now - c.last_activity.load(Ordering::Relaxed) >= idle_ms);
                    if quiet {
                        break "idle";
                    }
                }
                msg = socket.recv() => match msg {
                    None | Some(Err(_) | Ok(Message::Close(_))) => break "client_closed",
                    // The CLI refreshed its sign-in: follow the new session, so a
                    // token rotation does not end the tunnel, and revoking the
                    // sign-in still does.
                    Some(Ok(Message::Text(text))) => {
                        if let Some(next) = refreshed(&state, &caller, text.as_str()).await {
                            session = next;
                        } else if t.direct
                            && serde_json::from_str::<Value>(text.as_str()).is_ok_and(|v| v["type"] == "active")
                        {
                            // A direct tunnel's connections run outside this
                            // server; the CLI reports them so idleness holds.
                            if let Some(cur) = state.tunnels.get(&id) {
                                cur.last_activity.store(state.clock.now().0, Ordering::Relaxed);
                            }
                        }
                    }
                    Some(Ok(_)) => {}
                },
            }
        };
        let closed = end_tunnel(&state, &id, reason).await.unwrap_or(t);
        let (up, down) = closed.bytes();
        let _ = socket.send(Message::Text(json!({ "type": "closed", "reason": reason }).to_string().into())).await;
        let _ = socket.close().await;
        caller
            .audit(
                "network.tunnel.close",
                &server_ref(closed.server_id),
                Some(closed.workspace_id),
                json!({
                    "tunnel": closed.id,
                    "target": target_json(&closed.target),
                    "port": closed.port,
                    "reason": reason,
                    "bytes_up": up,
                    "bytes_down": down,
                    "duration_ms": state.clock.now().0 - closed.started_at.0,
                }),
            )
            .await;
    }))
}

/// `GET /api/v1/tunnels/{id}/connect` (WebSocket): one TCP connection.
pub async fn connect(
    State(state): State<AppState>,
    caller: Caller,
    origin: RequestOrigin,
    Path(id): Path<String>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> ApiResult<Response> {
    person(&caller, &origin, &headers, &state)?;
    let t = owned(&state, &caller, &id)?;
    let slot = state.tunnels.connect(&id, limits(&state)).map_err(|r| refusal(&r))?;
    let mut revoked = state.tunnels.watch(&id).ok_or_else(|| ApiError::not_found("no open tunnel with this id"))?;
    Ok(ws.max_message_size(MAX_FRAME).on_upgrade(move |socket| async move {
        let _slot = slot;
        let (mut tx, mut rx) = socket.split();
        let now = state.clock.now();
        let (conn, token, node_rx) = state.tunnels.expect(&id, t.server_id, now);
        let cmd = forge_proto::Command::TunnelOpen(forge_proto::messages::TunnelOpen {
            connection_id: conn,
            port: u32::from(t.port),
            // The signed copy of what this tunnel may reach: its one port.
            allowed_ports: vec![u32::from(t.port)],
            stream_token: token,
            idle_secs: state.config.tunnels.idle_timeout_secs,
            expires_at_ms: t.expires_at.0,
            dial_ip: if t.target.dial_ip == LOOPBACK { String::new() } else { t.target.dial_ip.clone() },
        });
        let (body, signed) = state.signer.issue(
            &t.server_id.to_string(),
            &caller.principal().to_string(),
            cmd,
            now,
            Duration::from_secs(60),
        );
        if let Err(e) = state.hub.enqueue(t.server_id, &body.operation_id, signed) {
            let _ = tx.send(Message::Close(None)).await;
            tracing::debug!(error = %e, "tunnel command not queued");
            return;
        }
        // The node dials back, or reports why it could not.
        let mut node_rx = node_rx;
        let node = tokio::select! {
            r = tokio::time::timeout(DIAL_WAIT, &mut node_rx) => r.ok().and_then(Result::ok),
            res = state.hub.wait(&body.operation_id, DIAL_WAIT) => match res {
                Some(r) if !r.ok => {
                    let text = json!({ "type": "error", "message": r.error }).to_string();
                    let _ = tx.send(Message::Text(text.into())).await;
                    None
                }
                _ => tokio::time::timeout(DIAL_WAIT, node_rx).await.ok().and_then(Result::ok),
            },
        };
        let Some(node) = node else {
            let _ = tx.send(Message::Close(None)).await;
            return;
        };
        // Each direction ends on its own (half-close): `{"type": "eof"}` from
        // the CLI ends the stream to the node; the end of the node's stream is
        // passed on as `eof`. The connection closes when both have ended.
        let mut to_node = Some(node.to_node);
        let mut from_node = node.from_node.into_data_stream();
        let (up, down, last) = (t.bytes_up.clone(), t.bytes_down.clone(), t.last_activity.clone());
        let (mut client_done, mut node_done) = (false, false);
        while !(client_done && node_done) {
            tokio::select! {
                _ = revoked.changed() => break,
                m = rx.next(), if !client_done => match m {
                    Some(Ok(Message::Binary(b))) => {
                        up.fetch_add(b.len() as u64, Ordering::Relaxed);
                        last.store(state.clock.now().0, Ordering::Relaxed);
                        let sent = match &to_node {
                            Some(n) => n.send(b).await.is_ok(),
                            None => false,
                        };
                        if !sent {
                            break;
                        }
                    }
                    Some(Ok(Message::Text(t))) if t.as_str().contains("\"eof\"") => {
                        client_done = true;
                        to_node = None;
                    }
                    None | Some(Err(_) | Ok(Message::Close(_))) => break,
                    Some(Ok(_)) => {}
                },
                d = from_node.next(), if !node_done => match d {
                    // An HTTP/2 stream can end with an empty data frame.
                    Some(Ok(b)) if b.is_empty() => {}
                    Some(Ok(b)) => {
                        down.fetch_add(b.len() as u64, Ordering::Relaxed);
                        last.store(state.clock.now().0, Ordering::Relaxed);
                        if tx.send(Message::Binary(b)).await.is_err() {
                            break;
                        }
                    }
                    _ => {
                        node_done = true;
                        let eof = json!({ "type": "eof" }).to_string();
                        if tx.send(Message::Text(eof.into())).await.is_err() {
                            break;
                        }
                    }
                },
            }
        }
        drop(to_node);
        let _ = tx.send(Message::Close(None)).await;
    }))
}

// ------------------------------------------------------------ list, close

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct ListQuery {
    /// Everyone's tunnels (`user.manage`).
    #[serde(default)]
    pub all: bool,
    /// `open` (default), `closed` (history, newest first) or `all`.
    #[serde(default)]
    pub state: Option<String>,
}

/// Display names of users, for lists.
async fn user_names(state: &AppState) -> std::collections::HashMap<UserId, String> {
    state
        .store
        .list_users()
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|u| (u.id, u.username.clone().or_else(|| u.email.clone()).unwrap_or_else(|| u.id.to_string())))
        .collect()
}

async fn node_names(state: &AppState) -> std::collections::HashMap<ServerId, String> {
    let servers = state.store.list_servers(None).await.unwrap_or_default();
    let refs: Vec<&forge_core::model::Server> = servers.iter().collect();
    let ws = servers.first().map(|s| s.workspace_id);
    let stored = match ws {
        Some(ws) => crate::names::stored_labels(state, ws).await.unwrap_or_default(),
        None => Default::default(),
    };
    crate::names::node_labels(&refs, &stored).into_iter().map(|(id, l)| (id, l.label)).collect()
}

/// `GET /api/v1/tunnels`.
pub async fn list(State(state): State<AppState>, caller: Caller, Query(q): Query<ListQuery>) -> ApiResult<Json<Value>> {
    if q.all {
        caller.authorize(Action::UserManage, EntityRef::platform()).await?;
    }
    let me = caller.session.user.id;
    let which = q.state.as_deref().unwrap_or("open");
    if !matches!(which, "open" | "closed" | "all") {
        return Err(ApiError::bad_request("state is open, closed or all"));
    }
    let mut items: Vec<Value> = Vec::new();
    if which != "closed" {
        items.extend(state.tunnels.list().iter().filter(|t| t.claimed && (q.all || t.user_id == me)).map(view));
    }
    if which != "open" {
        let rows = state.store.list_tunnels((!q.all).then_some(me), HISTORY_LIMIT).await?;
        let (users, nodes) = (user_names(&state).await, node_names(&state).await);
        for r in rows {
            if r.closed_at.is_none() {
                // Open in the store but gone from the registry: never claimed
                // (dropped after 30 s). Record that now.
                if state.tunnels.get(&r.id).is_none() {
                    let _ =
                        state.store.close_tunnel(&r.id, state.clock.now(), "unclaimed", r.bytes_up, r.bytes_down).await;
                }
                continue;
            }
            let user = users.get(&r.user_id).cloned().unwrap_or_else(|| r.user_id.to_string());
            let node = nodes.get(&r.server_id).cloned().unwrap_or_default();
            items.push(row_view(&r, &user, &node));
        }
    }
    Ok(Json(json!({ "tunnels": items, "settings": settings_view(&state) })))
}

/// `DELETE /api/v1/tunnels/{id}`: closes a tunnel and its connections.
pub async fn close(State(state): State<AppState>, caller: Caller, Path(id): Path<String>) -> ApiResult<StatusCode> {
    let t = state.tunnels.get(&id).ok_or_else(|| ApiError::not_found("no open tunnel with this id"))?;
    if t.user_id != caller.session.user.id && !caller.can(Action::UserManage, EntityRef::platform()).await {
        return Err(ApiError::not_found("no open tunnel with this id"));
    }
    end_tunnel(&state, &id, "closed").await;
    caller
        .audit(
            "network.tunnel.revoke",
            &server_ref(t.server_id),
            Some(t.workspace_id),
            json!({ "tunnel": id, "owner": t.user, "target": target_json(&t.target), "port": t.port }),
        )
        .await;
    Ok(StatusCode::NO_CONTENT)
}

// ------------------------------------------------------------ grants

fn grant_view(g: &TunnelGrant, user: &str, name: Option<&str>, now: Timestamp) -> Value {
    json!({
        "id": g.id,
        "workspace_id": g.workspace_id,
        "user_id": g.user_id,
        "user": user,
        "target": {
            "kind": g.target_kind,
            "ref": g.target_ref,
            "name": name.unwrap_or(&g.target_ref),
            "server_id": g.server_id,
            "ports": g.ports,
        },
        "server_id": g.server_id,
        "ports": g.ports,
        "created_by": g.created_by,
        "created_at": g.created_at,
        "expires_at": expiry_json(g.expires_at),
        "never_expires": g.never_expires(),
        "revoked_at": g.revoked_at,
        "note": g.note,
        "status": g.status(now),
    })
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct GrantsQuery {
    /// Include expired and revoked grants.
    #[serde(default)]
    pub all: bool,
    /// One user's grants.
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub user: Option<UserId>,
}

/// `GET /api/v1/tunnel-grants`: the caller's grants; everyone's with
/// `access.manage`. Active ones unless `all=true`.
pub async fn list_grants(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Query(q): Query<GrantsQuery>,
) -> ApiResult<Json<Value>> {
    let admin = caller.can(Action::AccessManage, ws_ref(ws)).await;
    let me = caller.session.user.id;
    let user = if admin { q.user } else { Some(me) };
    if !admin && q.user.is_some_and(|u| u != me) {
        return Err(ApiError::forbidden("listing other people's grants needs `access.manage`"));
    }
    let now = state.clock.now();
    let grants = state.store.list_tunnel_grants(user).await?;
    let (users, names) = (user_names(&state).await, target_names(&state, ws).await);
    let items: Vec<Value> = grants
        .iter()
        .filter(|g| q.all || g.is_active(now))
        .map(|g| {
            let user = users.get(&g.user_id).cloned().unwrap_or_else(|| g.user_id.to_string());
            grant_view(g, &user, names.get(&g.target_ref).map(String::as_str), now)
        })
        .collect();
    Ok(Json(json!({
        "grants": items,
        "allow_never_expiry": state.config.tunnels.allow_never_expiry,
        "can_never_expire": state.config.tunnels.allow_never_expiry && admin,
        "settings": settings_view(&state),
    })))
}

/// Target reference → the name people know it by, from the catalogue.
async fn target_names(state: &AppState, ws: WorkspaceId) -> std::collections::HashMap<String, String> {
    crate::service_ports::catalogue(state, ws).await.unwrap_or_default().into_iter().map(|e| (e.id, e.name)).collect()
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NewGrantRequest {
    #[schemars(with = "String")]
    pub user_id: UserId,
    /// The service or node (`raw` cannot be granted). `target.ports` limits
    /// the grant to those remote ports; `target.server_id` to one node.
    pub target: TargetSpec,
    /// Lifetime in minutes; the default is `[tunnels] default_lifetime_hours`.
    #[serde(default)]
    pub lifetime_minutes: Option<u32>,
    /// No expiry: `[tunnels] allow_never_expiry` (and `access.manage`).
    #[serde(default)]
    pub never_expires: bool,
    #[serde(default)]
    pub note: String,
}

/// `POST /api/v1/tunnel-grants` (`access.manage`, MFA).
pub async fn create_grant(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Json(req): Json<NewGrantRequest>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    caller.authorize(Action::AccessManage, ws_ref(ws)).await?;
    if !req.target.kind.grantable() {
        return Err(ApiError::bad_request("raw ports cannot be granted; they need `network.tunnel.any`"));
    }
    let grantee = state.store.get_user(req.user_id).await.map_err(|_| ApiError::not_found("user not found"))?;
    if grantee.disabled {
        return Err(ApiError::conflict("the user is disabled"));
    }
    // Resolve the target by name; every node it runs on unless one is named.
    let hits = resolve(&state, ws, &req.target, false).await?;
    let first = &hits[0];
    if hits.iter().any(|h| h.id != first.id) {
        return Ok((
            StatusCode::CONFLICT,
            Json(json!({
                "type": "https://docs.orbyn.dev/errors/ambiguous_target",
                "title": "ambiguous_target",
                "status": 409,
                "detail": "several services match; name one by `<environment>/<name>` or id",
                "choices": hits,
            })),
        ));
    }
    let note: String = req.note.chars().filter(|c| !c.is_control()).take(200).collect();
    let expires_at = expiry(
        &state,
        &caller,
        ws,
        "network.tunnel.grant.create",
        &ws_ref(ws),
        req.lifetime_minutes,
        req.never_expires,
        None,
    )
    .await?;
    let now = state.clock.now();
    let g = TunnelGrant {
        id: TunnelGrantId::new(),
        workspace_id: ws,
        user_id: grantee.id,
        target_kind: first.kind,
        target_ref: first.id.clone(),
        server_id: req.target.server_id,
        ports: req.target.ports.clone().unwrap_or_default(),
        created_by: caller.principal().to_string(),
        created_at: now,
        expires_at,
        revoked_at: None,
        note,
    };
    state.store.put_tunnel_grant(&g).await?;
    let user = grantee.username.clone().or_else(|| grantee.email.clone()).unwrap_or_else(|| grantee.id.to_string());
    caller
        .audit(
            "network.tunnel.grant.create",
            &ws_ref(ws),
            Some(ws),
            json!({
                "grant": g.id, "user_id": g.user_id, "user": user, "target": { "kind": g.target_kind, "ref": g.target_ref, "name": first.name },
                "server_id": g.server_id, "ports": g.ports, "expires_at": expiry_json(g.expires_at), "never_expires": g.never_expires(),
            }),
        )
        .await;
    Ok((StatusCode::CREATED, Json(grant_view(&g, &user, Some(&first.name), now))))
}

/// `DELETE /api/v1/tunnel-grants/{id}` (`access.manage`): revokes the grant
/// and closes every open tunnel it allowed.
pub async fn revoke_grant(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Path(id): Path<TunnelGrantId>,
) -> ApiResult<StatusCode> {
    caller.authorize(Action::AccessManage, ws_ref(ws)).await?;
    let mut g = state
        .store
        .get_tunnel_grant(id)
        .await?
        .filter(|g| g.workspace_id == ws)
        .ok_or_else(|| ApiError::not_found("grant not found"))?;
    if g.revoked_at.is_none() {
        g.revoked_at = Some(state.clock.now());
        state.store.put_tunnel_grant(&g).await?;
    }
    let live: Vec<String> =
        state.tunnels.list().into_iter().filter(|t| t.target.grant_id == Some(id)).map(|t| t.id).collect();
    for t in &live {
        end_tunnel(&state, t, "access_withdrawn").await;
    }
    caller
        .audit(
            "network.tunnel.grant.revoke",
            &ws_ref(ws),
            Some(ws),
            json!({ "grant": id, "user_id": g.user_id, "target": { "kind": g.target_kind, "ref": g.target_ref }, "tunnels_closed": live }),
        )
        .await;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn target_specs_accept_ports_as_list_csv_or_star() {
        let p = |v: Value| serde_json::from_value::<TargetSpec>(v).map(|t| t.ports);
        assert_eq!(p(json!({"kind": "app", "ref": "site"})).unwrap(), None);
        assert_eq!(p(json!({"kind": "app", "ref": "site", "ports": [5341, 80, 80]})).unwrap(), Some(vec![80, 5341]));
        assert_eq!(p(json!({"kind": "stack", "ref": "seq-b/seq", "ports": "80, 5341"})).unwrap(), Some(vec![80, 5341]));
        assert_eq!(p(json!({"kind": "ssh", "ref": "lab-b", "ports": "*"})).unwrap(), Some(vec![]));
        assert!(p(json!({"kind": "raw", "ref": "lab-b", "ports": [0]})).is_err());
        assert!(p(json!({"kind": "raw", "ref": "lab-b", "ports": "x"})).is_err());
        assert!(p(json!({"kind": "vpn", "ref": "lab-b"})).is_err(), "unknown kind");
    }

    #[test]
    fn never_expiring_times_are_null_in_json() {
        assert_eq!(expiry_json(NEVER_EXPIRES), Value::Null);
        assert_eq!(expiry_json(Timestamp(5)), json!(5));
    }
}
