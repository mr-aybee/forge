//! Browser SSH terminal.
//!
//! * `GET/PUT /api/v1/servers/{id}/ssh` — the node's SSH endpoint and pinned host key.
//! * `GET /api/v1/ssh/ca` — the user-CA public key nodes must trust.
//! * `GET /api/v1/servers/{id}/terminal?login=&cols=&rows=` — WebSocket.
//! * `GET /api/v1/servers/{id}/containers/{container_id}/shell?login=&cols=&rows=`
//!   — WebSocket: a shell inside a container (`container.exec`, D-100).
//!
//! Security: `terminal.open` is authorized on the server (MFA guardrail in
//! Cedar). Changing the endpoint or pinned host key needs `server.command`
//! (MFA) and is refused when the host or key belongs to another node or the
//! node's own agent reports other host keys. A terminal closes after
//! [`MAX_LIFETIME`], or within [`RECHECK`] of its sign-in session ending or
//! `terminal.open` being withdrawn. The socket must come from the console's origin and its first frame
//! must carry the CSRF token. Forge then signs a certificate valid for five
//! minutes whose only principal is `login`, connects with the pinned host key,
//! and relays bytes. Start and end of every session are audited.
//!
//! Frames: client → server binary = keystrokes, text = JSON control
//! (`auth`, `resize`, `ping`); server → client binary = terminal output, text =
//! JSON status (`ready`, `error`, `pong`, `closed`).
//!
//! The CLI (`forge nodes terminal|exec`, issue #5) connects with its bearer
//! token in the `Authorization` header (never the URL) and sends no `auth`
//! frame: CSRF protects cookies only. A request with both a bearer token and
//! a session cookie is refused. `GET /servers/{id}/exec?login=` runs one
//! command without a terminal: the first frame is
//! `{"type":"exec","argv":[…]}` (each argument quoted for the POSIX shell) or
//! `{"type":"exec","command":"…"}` (`forge nodes exec --shell`); output is
//! binary frames whose first byte is `1` (stdout) or `2` (stderr); stdin is
//! binary frames, `{"type":"eof"}` ends it; `{"type":"exit","code":n}` gives
//!
//! the status.
//!
//! The container shell runs through the agent link by default (D-114): the
//! node's root helper starts a Docker exec with a TTY as the image's default
//! user, and the agent relays it back ([`crate::container_exec`]). No
//! terminal account or credential is needed; the node must run Linux and an
//! agent that reports `container_exec`. With `login=` it uses the old path
//! instead: Forge connects over SSH as that account and runs
//! `sudo -n docker exec -it <container> …` on the PTY. Either way the
//! frames are the terminal's (the closing frame may add `exit_code`), it
//! authorizes `container.exec` (no MFA guardrail; D-100) and audits
//! `container.exec` / `container.exec_close`. Every open session is listed
//! in [`super::remote_sessions`] and can be revoked there.

use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::response::Response;
use forge_core::model::SessionKind;
use forge_core::model::SshEndpoint;
use forge_core::ports::{ServerRepository, SshEndpointRepository};
use forge_core::remote::{ExecEvent, ExecReader, RemoteAuth, RemoteTarget, ShellReader, ShellWriter, TermSize};
use forge_core::{Action, EntityKind, EntityRef, ServerId};
use forge_identity::IdentityService;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// Certificates outlive the handshake only briefly; the session itself is
/// bounded by the idle timeout.
const CERT_TTL: Duration = Duration::from_secs(300);
const IDLE_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const AUTH_TIMEOUT: Duration = Duration::from_secs(10);
/// Hard limit for one terminal session.
pub const MAX_LIFETIME: Duration = Duration::from_secs(12 * 3600);
/// How often an open terminal re-checks the session and `terminal.open`.
const RECHECK: Duration = Duration::from_secs(30);

fn server_ref(id: ServerId) -> EntityRef {
    EntityRef::new(EntityKind::Server, id)
}

/// The certificate principal for `login` on a node: `forge.<server_id>.<login>`
/// when the node accepts only certificates bound to it (D-069; the agent's
/// `forge-agent-helper ssh-principals` prints the same), the bare login for
/// nodes set up before that.
fn cert_principal(server_id: &str, login: &str, bound: bool) -> String {
    if bound { format!("forge.{server_id}.{login}") } else { login.to_owned() }
}

/// Whether the node accepts only certificates bound to it (D-069), from its
/// facts. Older agents do not report it.
async fn node_bound(state: &AppState, server: ServerId) -> bool {
    use forge_core::tags::FactsRepository;
    state
        .store
        .all_facts()
        .await
        .ok()
        .and_then(|f| f.get(&server).cloned())
        .is_some_and(|(v, _)| v.get("ssh_principals").and_then(serde_json::Value::as_bool) == Some(true))
}

/// Logins refused on nodes without node-bound certificates: a certificate
/// for a bare `root` principal would work on every such node that trusts the
/// Forge CA, not only the one the user may open (#13 inventory).
fn privileged_login(login: &str) -> bool {
    login == "root"
}

/// POSIX login names: what sshd accepts as a principal.
fn valid_login(login: &str) -> bool {
    !login.is_empty()
        && login.len() <= 32
        && login.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        && !login.starts_with('-')
}

#[derive(Debug, Serialize)]
pub struct SshInfo {
    pub endpoint: Option<SshEndpoint>,
    /// Add to the node's `TrustedUserCAKeys` (the installer does this).
    pub ca_public_key: String,
}

pub async fn get_ssh(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServerId>,
) -> ApiResult<Json<SshInfo>> {
    caller.authorize(Action::View, server_ref(id)).await?;
    Ok(Json(SshInfo {
        endpoint: state.store.get_ssh_endpoint(id).await?,
        ca_public_key: state.ssh_ca.public_openssh(),
    }))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PutSsh {
    pub host: String,
    pub port: u16,
    /// Fingerprint the user confirmed after `POST /nodes/probe`.
    pub host_key_fingerprint: String,
}

pub async fn put_ssh(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServerId>,
    Json(req): Json<PutSsh>,
) -> ApiResult<Json<SshEndpoint>> {
    caller.authorize(Action::ServerManage, server_ref(id)).await?;
    // Where root terminals for this node connect to: as strong as running a
    // command on it (MFA).
    caller.authorize(Action::ServerCommand, server_ref(id)).await?;
    let server = state.store.get_server(id).await.map_err(|_| ApiError::not_found("server not found"))?;
    let host = req.host.trim();
    if host.is_empty() || host.len() > 253 || host.chars().any(|c| c.is_whitespace() || c == '/') {
        return Err(ApiError::bad_request("invalid host"));
    }
    if req.port == 0 || !req.host_key_fingerprint.starts_with("SHA256:") {
        return Err(ApiError::bad_request("port and a SHA256 host-key fingerprint are required"));
    }
    if let Err(reason) = endpoint_belongs_to(&state, &caller, &server, host, req.port, &req.host_key_fingerprint).await
    {
        caller.audit_denied("server.ssh_endpoint", &server_ref(id), Some(server.workspace_id), &reason).await;
        return Err(ApiError::bad_request(reason));
    }
    let endpoint = SshEndpoint {
        server_id: id,
        host: host.to_owned(),
        port: req.port,
        host_key_fingerprint: req.host_key_fingerprint,
        updated_at: state.clock.now(),
    };
    state.store.put_ssh_endpoint(&endpoint).await?;
    caller
        .audit(
            "server.ssh_endpoint",
            &server_ref(id),
            Some(server.workspace_id),
            json!({ "host": endpoint.host, "port": endpoint.port, "host_key": endpoint.host_key_fingerprint }),
        )
        .await;
    Ok(Json(endpoint))
}

fn same_host(a: &str, b: &str) -> bool {
    let norm =
        |s: &str| s.trim().trim_start_matches('[').trim_end_matches(']').trim_end_matches('.').to_ascii_lowercase();
    norm(a) == norm(b)
}

/// The agent-reported address of a node (live, else the last saved facts).
fn agent_address(
    s: ServerId,
    facts: &std::collections::HashMap<ServerId, (serde_json::Value, forge_core::Timestamp)>,
) -> Option<String> {
    crate::facts::live_address(s).or_else(|| facts.get(&s).and_then(|(f, _)| f["address"].as_str()).map(str::to_owned))
}

/// Refuses an endpoint that points at another node: a host key pinned for
/// another node, another node's `host:port`, or another node's agent address
/// (unless this node shares it, behind the same NAT). When this node's agent
/// is online and reports its host keys, the fingerprint must be one of them.
async fn endpoint_belongs_to(
    state: &AppState,
    caller: &Caller,
    server: &forge_core::model::Server,
    host: &str,
    port: u16,
    fingerprint: &str,
) -> Result<(), String> {
    use forge_core::tags::FactsRepository;
    let others = state.store.list_servers(None).await.map_err(|e| e.to_string())?;
    let facts = state.store.all_facts().await.unwrap_or_default();
    let own_address = agent_address(server.id, &facts);
    for o in others.iter().filter(|o| o.id != server.id) {
        let name = o.display_name.clone().unwrap_or_else(|| o.hostname.clone());
        if let Ok(Some(e)) = state.store.get_ssh_endpoint(o.id).await {
            if e.host_key_fingerprint == fingerprint {
                return Err(format!("this host key is pinned for node {name}"));
            }
            if same_host(&e.host, host) && e.port == port {
                return Err(format!("{host}:{port} is the SSH endpoint of node {name}"));
            }
        }
        if let Some(addr) = agent_address(o.id, &facts)
            && same_host(&addr, host)
            && !own_address.as_deref().is_some_and(|own| same_host(own, &addr))
        {
            return Err(format!("{host} is the address of node {name}"));
        }
    }
    if server.status == forge_core::model::AgentStatus::Online
        && let Ok(r) = super::sites::run_as(
            state,
            caller.principal(),
            caller.ctx(),
            server.id,
            json!({ "type": "collect_facts" }),
            Duration::from_secs(20),
        )
        .await
    {
        let reported: Vec<String> = r
            .pointer("/result/ssh_host_keys")
            .and_then(serde_json::Value::as_array)
            .map(|k| k.iter().filter_map(serde_json::Value::as_str).filter_map(fingerprint_of).collect())
            .unwrap_or_default();
        if !reported.is_empty() && !reported.iter().any(|f| f == fingerprint) {
            return Err("this host key is not one of the keys the node's agent reports".into());
        }
    }
    Ok(())
}

fn fingerprint_of(line: &str) -> Option<String> {
    fingerprint(line)
}

pub async fn ca(State(state): State<AppState>, _caller: Caller) -> Json<serde_json::Value> {
    Json(json!({ "public_key": state.ssh_ca.public_openssh() }))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TerminalQuery {
    pub login: String,
    #[serde(default = "default_cols")]
    pub cols: u32,
    #[serde(default = "default_rows")]
    pub rows: u32,
}
const fn default_cols() -> u32 {
    120
}
const fn default_rows() -> u32 {
    32
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Control {
    Auth { csrf: String },
    Resize { cols: u32, rows: u32 },
    Ping { t: Option<f64> },
    Exec { argv: Option<Vec<String>>, command: Option<String> },
    Eof,
}

/// Longest command (after quoting) an exec session accepts.
const MAX_COMMAND: usize = 64 * 1024;

/// Quotes one argument for the POSIX shell.
fn sh_quote(arg: &str) -> String {
    let plain = !arg.is_empty() && arg.bytes().all(|b| b.is_ascii_alphanumeric() || b"_@%+=:,./-".contains(&b));
    if plain { arg.to_owned() } else { format!("'{}'", arg.replace('\'', "'\\''")) }
}

/// The command line an exec frame asks for.
fn exec_command(argv: Option<Vec<String>>, command: Option<String>) -> Result<String, &'static str> {
    let cmd = match (argv, command) {
        (Some(a), None) if !a.is_empty() => a.iter().map(|x| sh_quote(x)).collect::<Vec<_>>().join(" "),
        (None, Some(c)) if !c.trim().is_empty() => c,
        _ => return Err("send either argv (a non-empty list) or command"),
    };
    if cmd.len() > MAX_COMMAND || cmd.contains('\0') {
        return Err("command too long or contains a NUL byte");
    }
    Ok(cmd)
}

/// Which client opens the socket. The CLI sends its bearer token in the
/// `Authorization` header; the console uses its cookie. Both at once is refused.
fn client_kind(caller: &Caller, headers: &HeaderMap) -> ApiResult<&'static str> {
    let bearer = crate::auth::bearer(headers).is_some();
    let cookie = crate::auth::cookie(headers, crate::auth::SESSION_COOKIE).is_some();
    if bearer && cookie {
        return Err(ApiError::bad_request("send either a bearer token or a session cookie, not both"));
    }
    Ok(if caller.session.session.kind == SessionKind::Bearer { "cli" } else { "console" })
}

pub async fn open(
    State(state): State<AppState>,
    caller: Caller,
    origin: crate::request_origin::RequestOrigin,
    Path(id): Path<ServerId>,
    Query(q): Query<TerminalQuery>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> ApiResult<Response> {
    // Cross-site WebSocket hijacking guard.
    if !crate::request_origin::origin_header_ok(&state, &origin, &headers) {
        return Err(ApiError::forbidden("origin not allowed; open the console on the address you use"));
    }
    let client = client_kind(&caller, &headers)?;
    let (workspace, endpoint) = prepare(&state, &caller, id, &q, Action::TerminalOpen).await?;
    let size = TermSize { cols: q.cols, rows: q.rows }.clamped();
    let session =
        Session { state, caller, server_id: id, workspace, endpoint, login: q.login, size, client, container: None };
    Ok(ws.max_message_size(256 * 1024).on_upgrade(move |socket| session.run(socket)))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ContainerShellQuery {
    /// Opt in to the SSH path: connect as this account (it needs
    /// passwordless sudo for `docker`). Without it the shell runs through
    /// the agent (D-114).
    #[serde(default)]
    pub login: Option<String>,
    #[serde(default = "default_cols")]
    pub cols: u32,
    #[serde(default = "default_rows")]
    pub rows: u32,
}

/// The fixed command a container shell runs on the node: `docker exec` into
/// `container` with bash when the image has it, else sh. `container` was
/// validated (`[A-Za-z0-9][A-Za-z0-9_.-]*`), so no quoting is needed.
#[must_use]
pub fn container_shell_command(container: &str) -> String {
    format!(
        "sudo -n docker exec -it {container} sh -c 'command -v bash >/dev/null 2>&1 && exec bash || exec sh' \
         || echo 'forge: docker exec failed (the login needs passwordless sudo and Docker on this node)' >&2"
    )
}

/// Which way a container shell goes: SSH only when `login=` asks for it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ShellPath {
    Agent,
    Ssh(String),
}

fn shell_path(login: Option<&str>) -> ShellPath {
    login.map_or(ShellPath::Agent, |l| ShellPath::Ssh(l.to_owned()))
}

/// A checked agent-path target: the node's workspace and the place held for
/// the shell until it registers.
struct AgentTarget {
    workspace: forge_core::WorkspaceId,
    reservation: super::remote_sessions::Reservation,
}

/// The audit action of a container shell opening (also its refusals).
const EXEC_OPEN: &str = "container.exec";

/// Refuses an agent-path shell, auditing why (`container.exec`, denied).
async fn refuse(caller: &Caller, id: ServerId, workspace: Option<forge_core::WorkspaceId>, e: ApiError) -> ApiError {
    caller.audit_denied(EXEC_OPEN, &server_ref(id), workspace, &e.message).await;
    e
}

/// The agent path's checks: `container.exec`, a Linux node that is online
/// and whose agent reports `container_exec`, and the per-user and per-node
/// limits, whose place is held from here on. Refusals are audited.
async fn agent_shell_target(state: &AppState, caller: &Caller, id: ServerId) -> ApiResult<AgentTarget> {
    use super::remote_sessions::Limits;
    use crate::container_exec::{MAX_PER_NODE, MAX_PER_USER};
    use forge_core::tags::FactsRepository;
    caller.authorize(Action::ContainerExec, server_ref(id)).await?;
    let server = state.store.get_server(id).await.map_err(|_| ApiError::not_found("server not found"))?;
    let ws = Some(server.workspace_id);
    if server.os.eq_ignore_ascii_case("windows") {
        return Err(
            refuse(caller, id, ws, ApiError::conflict("container shell is not supported on Windows nodes")).await
        );
    }
    if server.status != forge_core::model::AgentStatus::Online {
        return Err(refuse(caller, id, ws, ApiError::conflict("the node is offline")).await);
    }
    let facts = state.store.all_facts().await?;
    let ready = match facts.get(&id) {
        None => Err(ApiError::conflict("the node has not reported its facts yet; try again in a minute")),
        Some((f, _)) if f.get("container_exec").and_then(serde_json::Value::as_bool) != Some(true) => {
            Err(ApiError::new(
                axum::http::StatusCode::CONFLICT,
                "agent_update_required",
                "container shells need a current agent on a Linux node whose helper is pinned: update the agent \
                 (Software → Updates → Agents), or pass `login` to use SSH",
            ))
        }
        Some(_) => Ok(()),
    };
    if let Err(e) = ready {
        return Err(refuse(caller, id, ws, e).await);
    }
    let limits = Limits { per_user: MAX_PER_USER, per_node: MAX_PER_NODE };
    match state.remote_sessions.reserve(SHELL_KIND, caller.session.user.id, id, limits) {
        Ok(reservation) => Ok(AgentTarget { workspace: server.workspace_id, reservation }),
        Err(r) => Err(refuse(caller, id, ws, ApiError::conflict(limit_message(r))).await),
    }
}

/// The remote-session kind of a container shell.
const SHELL_KIND: &str = "container";

fn limit_message(r: super::remote_sessions::Refusal) -> String {
    use super::remote_sessions::Refusal;
    match r {
        Refusal::PerUser(n) => format!("you already have {n} open container shells"),
        Refusal::PerNode(n) => format!("this node already has {n} open container shells"),
    }
}

/// `GET /servers/{id}/containers/{container_id}/shell?login=&cols=&rows=`:
/// a shell inside a container (WebSocket; `container.exec`).
pub async fn container_shell(
    State(state): State<AppState>,
    caller: Caller,
    origin: crate::request_origin::RequestOrigin,
    Path((id, container_id)): Path<(ServerId, String)>,
    Query(q): Query<ContainerShellQuery>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> ApiResult<Response> {
    if !crate::request_origin::origin_header_ok(&state, &origin, &headers) {
        return Err(ApiError::forbidden("origin not allowed"));
    }
    if !super::containers::valid_container_id(&container_id) {
        return Err(ApiError::bad_request("invalid container id"));
    }
    let client = client_kind(&caller, &headers)?;
    let size = TermSize { cols: q.cols, rows: q.rows }.clamped();
    let login = match shell_path(q.login.as_deref()) {
        ShellPath::Ssh(login) => login,
        ShellPath::Agent => {
            let target = agent_shell_target(&state, &caller, id).await?;
            let session = AgentShell { state, caller, server_id: id, target, container: container_id, size, client };
            return Ok(ws.max_message_size(256 * 1024).on_upgrade(move |socket| session.run(socket)));
        }
    };
    let tq = TerminalQuery { login, cols: q.cols, rows: q.rows };
    let (workspace, endpoint) = prepare(&state, &caller, id, &tq, Action::ContainerExec).await?;
    let session = Session {
        state,
        caller,
        server_id: id,
        workspace,
        endpoint,
        login: tq.login,
        size,
        client,
        container: Some(container_id),
    };
    Ok(ws.max_message_size(256 * 1024).on_upgrade(move |socket| session.run(socket)))
}

/// The answer of `…/shell/check`.
#[derive(Debug, Serialize)]
pub struct ShellCheck {
    pub ok: bool,
    /// `agent` or `ssh`.
    pub path: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub login: Option<String>,
}

/// `GET /servers/{id}/containers/{container_id}/shell/check?login=`: the
/// container shell's checks as plain JSON (see [`check`]).
pub async fn container_shell_check(
    State(state): State<AppState>,
    caller: Caller,
    Path((id, container_id)): Path<(ServerId, String)>,
    Query(q): Query<ContainerShellQuery>,
) -> ApiResult<Json<ShellCheck>> {
    if !super::containers::valid_container_id(&container_id) {
        return Err(ApiError::bad_request("invalid container id"));
    }
    let login = match shell_path(q.login.as_deref()) {
        ShellPath::Ssh(login) => login,
        ShellPath::Agent => {
            // The place is given back at once: this only checks.
            drop(agent_shell_target(&state, &caller, id).await?);
            return Ok(Json(ShellCheck { ok: true, path: "agent", host: None, port: None, login: None }));
        }
    };
    let tq = TerminalQuery { login, cols: q.cols, rows: q.rows };
    let (_, endpoint) = prepare(&state, &caller, id, &tq, Action::ContainerExec).await?;
    Ok(Json(ShellCheck {
        ok: true,
        path: "ssh",
        host: Some(endpoint.host),
        port: Some(endpoint.port),
        login: Some(tq.login),
    }))
}

/// `GET /servers/{id}/exec?login=`: runs one command without a terminal
/// (WebSocket; same checks as the terminal, `terminal.open`).
pub async fn exec(
    State(state): State<AppState>,
    caller: Caller,
    origin: crate::request_origin::RequestOrigin,
    Path(id): Path<ServerId>,
    Query(q): Query<TerminalQuery>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> ApiResult<Response> {
    if !crate::request_origin::origin_header_ok(&state, &origin, &headers) {
        return Err(ApiError::forbidden("origin not allowed"));
    }
    let client = client_kind(&caller, &headers)?;
    let (workspace, endpoint) = prepare(&state, &caller, id, &q, Action::TerminalOpen).await?;
    let size = TermSize { cols: q.cols, rows: q.rows }.clamped();
    let session =
        Session { state, caller, server_id: id, workspace, endpoint, login: q.login, size, client, container: None };
    Ok(ws.max_message_size(1024 * 1024).on_upgrade(move |socket| session.run_exec(socket)))
}

/// `GET /servers/{id}/terminal/check?login=`: the same checks as opening a
/// terminal, as plain JSON. Browsers hide why a WebSocket upgrade failed, so
/// the console calls this first to show the real reason (and to prompt for
/// step-up MFA).
pub async fn check(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServerId>,
    Query(q): Query<TerminalQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let (_, endpoint) = prepare(&state, &caller, id, &q, Action::TerminalOpen).await?;
    Ok(Json(json!({ "ok": true, "host": endpoint.host, "port": endpoint.port })))
}

/// Checks and resolves what a session needs: a valid login, `action` on the
/// node (`terminal.open`, or `container.exec` for a container shell), root
/// only on node-bound nodes, and the node's SSH endpoint.
async fn prepare(
    state: &AppState,
    caller: &Caller,
    id: ServerId,
    q: &TerminalQuery,
    action: Action,
) -> ApiResult<(forge_core::WorkspaceId, SshEndpoint)> {
    if !valid_login(&q.login) {
        return Err(ApiError::bad_request("invalid login name"));
    }
    caller.authorize(action, server_ref(id)).await?;
    if privileged_login(&q.login) && !node_bound(state, id).await {
        caller
            .audit_denied(action.as_str(), &server_ref(id), None, "root on a node without node-bound certificates")
            .await;
        return Err(ApiError::new(
            axum::http::StatusCode::CONFLICT,
            "agent_update_required",
            "this node's agent does not bind terminal certificates to the node, so a root certificate would also              work on other such nodes. Update the agent (Software → Updates → Agents), or sign in with another account",
        ));
    }
    let server = state.store.get_server(id).await.map_err(|_| ApiError::not_found("server not found"))?;
    let endpoint = match state.store.get_ssh_endpoint(id).await? {
        Some(e) => e,
        None => auto_endpoint(state, caller, &server).await?,
    };
    Ok((server.workspace_id, endpoint))
}

/// `SHA256:<base64>` of an OpenSSH public key line (`type base64 …`).
#[must_use]
pub fn fingerprint(pubkey_line: &str) -> Option<String> {
    use base64::Engine;
    let blob = pubkey_line.split_whitespace().nth(1)?;
    let raw = base64::engine::general_purpose::STANDARD.decode(blob).ok()?;
    let digest = ring::digest::digest(&ring::digest::SHA256, &raw);
    Some(format!("SHA256:{}", base64::engine::general_purpose::STANDARD_NO_PAD.encode(digest.as_ref())))
}

/// Nodes enrolled with the installer (not onboarded over SSH) have no SSH
/// endpoint yet: ask the node's own agent (mutually authenticated) for its
/// host key, and pin it with the node's address on port 22. Audited.
async fn auto_endpoint(
    state: &AppState,
    caller: &Caller,
    server: &forge_core::model::Server,
) -> ApiResult<SshEndpoint> {
    let none = || ApiError::conflict("no SSH endpoint configured for this node (node → SSH / Terminal → set endpoint)");
    if server.status != forge_core::model::AgentStatus::Online {
        return Err(none());
    }
    let r = super::sites::run_as(
        state,
        caller.principal(),
        caller.ctx(),
        server.id,
        json!({ "type": "collect_facts" }),
        Duration::from_secs(20),
    )
    .await
    .map_err(|_| none())?;
    let keys = r.pointer("/result/ssh_host_keys").and_then(serde_json::Value::as_array).cloned().unwrap_or_default();
    let Some(fp) = keys.iter().filter_map(serde_json::Value::as_str).find_map(fingerprint) else {
        return Err(ApiError::conflict(
            "this node did not report an SSH host key (is OpenSSH installed?); set the endpoint by hand under node → SSH",
        ));
    };
    let endpoint = SshEndpoint {
        server_id: server.id,
        host: super::ingress::node_address(state, server).await,
        port: 22,
        host_key_fingerprint: fp,
        updated_at: state.clock.now(),
    };
    state.store.put_ssh_endpoint(&endpoint).await?;
    caller
        .audit(
            "server.ssh_endpoint",
            &server_ref(server.id),
            Some(server.workspace_id),
            json!({ "host": endpoint.host, "port": endpoint.port, "host_key": endpoint.host_key_fingerprint, "source": "agent" }),
        )
        .await;
    Ok(endpoint)
}

struct Session {
    state: AppState,
    caller: Caller,
    server_id: ServerId,
    workspace: forge_core::WorkspaceId,
    endpoint: SshEndpoint,
    login: String,
    size: TermSize,
    /// `console` or `cli`.
    client: &'static str,
    /// A container shell (`docker exec` into this container) instead of a
    /// login shell on the node.
    container: Option<String>,
}

async fn send_status(socket: &mut WebSocket, v: serde_json::Value) -> bool {
    socket.send(Message::Text(v.to_string().into())).await.is_ok()
}

/// Cookie sessions prove the console with a CSRF token in the first frame
/// (cookies alone must not open a shell). Bearer sessions (the CLI) sent
/// their token in the `Authorization` header and skip it.
async fn authenticate(caller: &Caller, socket: &mut WebSocket) -> bool {
    if caller.session.session.kind == SessionKind::Bearer {
        return true;
    }
    let authed = match tokio::time::timeout(AUTH_TIMEOUT, socket.recv()).await {
        Ok(Some(Ok(Message::Text(t)))) => match serde_json::from_str::<Control>(&t) {
            Ok(Control::Auth { csrf }) => IdentityService::verify_csrf(&caller.session, Some(&csrf)),
            _ => false,
        },
        _ => false,
    };
    if !authed {
        let _ = send_status(socket, json!({"type": "error", "message": "missing or invalid CSRF token"})).await;
    }
    authed
}

/// What a new session registers as.
struct NewRemote<'a> {
    server_id: ServerId,
    login: &'a str,
    kind: &'static str,
    client: &'static str,
}

fn register(state: &AppState, caller: &Caller, n: &NewRemote<'_>) -> super::remote_sessions::Registration {
    let u = &caller.session.user;
    state.remote_sessions.register(super::remote_sessions::NewSession {
        user_id: u.id,
        user: u.username.clone().or_else(|| u.email.clone()).unwrap_or_else(|| u.id.to_string()),
        server_id: n.server_id,
        login: n.login.to_owned(),
        kind: n.kind,
        client: n.client,
        started_at: state.clock.now(),
        credential: caller.session.session.id_hash.clone(),
    })
}

/// What an open session re-checks every [`RECHECK`].
struct Recheck<'a> {
    state: &'a AppState,
    caller: &'a Caller,
    action: Action,
    server_id: ServerId,
}

impl Recheck<'_> {
    /// Whether the sign-in session still exists and still allows the
    /// session's action (`terminal.open` or `container.exec`).
    async fn still_allowed(&self) -> bool {
        crate::auth::still_signed_in(self.state, self.caller).await
            && self.caller.can(self.action, server_ref(self.server_id)).await
    }
}

impl Session {
    async fn authenticate(&self, socket: &mut WebSocket) -> bool {
        authenticate(&self.caller, socket).await
    }

    fn recheck(&self) -> Recheck<'_> {
        Recheck { state: &self.state, caller: &self.caller, action: self.action(), server_id: self.server_id }
    }

    /// The action this session keeps needing: `container.exec` for a
    /// container shell, `terminal.open` otherwise.
    fn action(&self) -> Action {
        if self.container.is_some() { Action::ContainerExec } else { Action::TerminalOpen }
    }

    fn register(&self, kind: &'static str) -> super::remote_sessions::Registration {
        let n = NewRemote { server_id: self.server_id, login: &self.login, kind, client: self.client };
        register(&self.state, &self.caller, &n)
    }

    async fn run(self, mut socket: WebSocket) {
        if !self.authenticate(&mut socket).await {
            return;
        }
        let (kind, opened, closed) = match self.container {
            Some(_) => ("container", "container.exec", "container.exec_close"),
            None => ("terminal", "terminal.open", "terminal.close"),
        };
        let mut reg = self.register(kind);

        let started = Instant::now();
        let resource = server_ref(self.server_id);
        let mut details = json!({
            "login": self.login, "host": self.endpoint.host, "port": self.endpoint.port,
            "client": self.client, "session": reg.id,
        });
        if let Some(c) = &self.container {
            details["container"] = json!(c);
        }
        self.caller.audit(opened, &resource, Some(self.workspace), details.clone()).await;

        let (bytes_in, bytes_out, reason) = match self.connect().await {
            Ok((reader, writer)) => {
                let ready =
                    json!({"type": "ready", "login": self.login, "host": self.endpoint.host, "session": reg.id});
                if !send_status(&mut socket, ready).await {
                    writer.close().await;
                    return;
                }
                relay(&mut socket, reader, writer, &self.recheck(), &mut reg.revoked).await
            }
            Err(e) => {
                let _ = send_status(&mut socket, json!({"type": "error", "message": e.to_string()})).await;
                (0, 0, "connect_failed")
            }
        };
        let _ = send_status(&mut socket, json!({"type": "closed", "reason": reason})).await;
        let mut end = details;
        end["duration_secs"] = json!(started.elapsed().as_secs());
        end["bytes_in"] = json!(bytes_in);
        end["bytes_out"] = json!(bytes_out);
        end["reason"] = json!(reason);
        self.caller.audit(closed, &resource, Some(self.workspace), end).await;
    }

    async fn connect(&self) -> forge_core::DomainResult<(Box<dyn ShellReader>, Box<dyn ShellWriter>)> {
        let remote = self.remote().await?;
        // The shell keeps its channel; the session handle is moved into a
        // background holder so the connection lives as long as the shell.
        let (reader, writer) = match &self.container {
            Some(c) => remote.shell_command("xterm-256color", self.size, &container_shell_command(c)).await?,
            None => remote.shell("xterm-256color", self.size).await?,
        };
        Ok((Box::new(Holder { inner: reader, _session: remote }), writer))
    }

    /// An SSH connection to the node as `login`, with a fresh short-lived certificate.
    async fn remote(&self) -> forge_core::DomainResult<Box<dyn forge_core::remote::RemoteSession>> {
        let key_id = format!("forge:{}:{}", self.caller.principal(), self.server_id);
        // A node that reports node-bound certificates gets one only it
        // accepts (D-069); older nodes still take the bare login, never root.
        let bound = node_bound(&self.state, self.server_id).await;
        if privileged_login(&self.login) && !bound {
            return Err(forge_core::DomainError::Denied("root needs node-bound certificates: update the agent".into()));
        }
        let principal = cert_principal(&self.server_id.to_string(), &self.login, bound);
        let cert = self.state.ssh_ca.issue(&principal, &key_id, CERT_TTL)?;
        let target = RemoteTarget {
            host: self.endpoint.host.clone(),
            port: self.endpoint.port,
            username: self.login.clone(),
            host_key_fingerprint: self.endpoint.host_key_fingerprint.clone(),
        };
        let auth = RemoteAuth::Certificate { key_openssh: cert.key_openssh, cert_openssh: cert.cert_openssh };
        self.state.ssh.connect(&target, &auth).await
    }

    async fn run_exec(self, mut socket: WebSocket) {
        if !self.authenticate(&mut socket).await {
            return;
        }
        // The command comes next.
        let command = match tokio::time::timeout(AUTH_TIMEOUT, socket.recv()).await {
            Ok(Some(Ok(Message::Text(t)))) => match serde_json::from_str::<Control>(&t) {
                Ok(Control::Exec { argv, command }) => exec_command(argv, command),
                _ => Err("the first frame must be {\"type\":\"exec\",…}"),
            },
            _ => Err("no command received"),
        };
        let command = match command {
            Ok(c) => c,
            Err(m) => {
                let _ = send_status(&mut socket, json!({"type": "error", "message": m})).await;
                return;
            }
        };
        let mut reg = self.register("exec");
        let started = Instant::now();
        let resource = server_ref(self.server_id);
        // The command is audited (truncated): pass secrets on stdin, not in arguments.
        let shown: String = command.chars().take(1000).collect();
        let details = json!({
            "login": self.login, "host": self.endpoint.host, "port": self.endpoint.port,
            "client": self.client, "session": reg.id, "command": shown,
        });
        self.caller.audit("terminal.exec", &resource, Some(self.workspace), details.clone()).await;
        let opened = match self.remote().await {
            Ok(remote) => match remote.exec_stream(&command).await {
                Ok((reader, writer)) => Ok((remote, reader, writer)),
                Err(e) => Err(e),
            },
            Err(e) => Err(e),
        };
        let (status, bytes_in, bytes_out, reason) = match opened {
            Ok((_remote, reader, writer)) => {
                if !send_status(&mut socket, json!({"type": "ready", "session": reg.id})).await {
                    writer.close().await;
                    return;
                }
                relay_exec(&mut socket, reader, writer, &self.recheck(), &mut reg.revoked).await
            }
            Err(e) => {
                let _ = send_status(&mut socket, json!({"type": "error", "message": e.to_string()})).await;
                (None, 0, 0, "connect_failed")
            }
        };
        let _ = send_status(&mut socket, json!({"type": "closed", "reason": reason})).await;
        let mut end = details;
        end["duration_secs"] = json!(started.elapsed().as_secs());
        end["bytes_in"] = json!(bytes_in);
        end["bytes_out"] = json!(bytes_out);
        end["exit_status"] = json!(status);
        end["reason"] = json!(reason);
        self.caller.audit("terminal.exec_close", &resource, Some(self.workspace), end).await;
    }
}

/// A container shell through the agent link (D-114).
struct AgentShell {
    state: AppState,
    caller: Caller,
    server_id: ServerId,
    /// The node's workspace and the place held under the limits.
    target: AgentTarget,
    container: String,
    size: TermSize,
    /// `console` or `cli`.
    client: &'static str,
}

/// What `container.exec` records for an agent-path shell.
#[derive(Debug, Clone, Serialize)]
struct ExecOpened<'a> {
    container: &'a str,
    client: &'static str,
    session: &'a str,
    path: &'static str,
}

/// What `container.exec_close` records.
#[derive(Debug, Serialize)]
struct ExecClosed<'a> {
    #[serde(flatten)]
    opened: ExecOpened<'a>,
    duration_secs: u64,
    bytes_in: u64,
    bytes_out: u64,
    reason: &'static str,
    exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'a str>,
}

/// Audit details as JSON. Both types hold only strings and numbers, which
/// always serialise; `Null` is never reached.
fn audit_value(v: &impl Serialize) -> serde_json::Value {
    serde_json::to_value(v).unwrap_or(serde_json::Value::Null)
}

impl AgentShell {
    async fn run(self, mut socket: WebSocket) {
        use crate::container_exec::{OpenRequest, ShellEnd};
        let resource = server_ref(self.server_id);
        let workspace = Some(self.target.workspace);
        if !authenticate(&self.caller, &mut socket).await {
            self.caller.audit_denied(EXEC_OPEN, &resource, workspace, "missing or invalid CSRF token").await;
            return;
        }
        let n = NewRemote { server_id: self.server_id, login: "agent", kind: SHELL_KIND, client: self.client };
        let mut reg = register(&self.state, &self.caller, &n);
        // The registration holds the place now; check again, so that no
        // path around the reservation can pass the limit.
        drop(self.target.reservation);
        let (mine, on_node) =
            self.state.remote_sessions.count_open(SHELL_KIND, self.caller.session.user.id, self.server_id);
        let over = if mine > crate::container_exec::MAX_PER_USER {
            Some(super::remote_sessions::Refusal::PerUser(crate::container_exec::MAX_PER_USER))
        } else if on_node > crate::container_exec::MAX_PER_NODE {
            Some(super::remote_sessions::Refusal::PerNode(crate::container_exec::MAX_PER_NODE))
        } else {
            None
        };
        if let Some(r) = over {
            let m = limit_message(r);
            self.caller.audit_denied(EXEC_OPEN, &resource, workspace, &m).await;
            let _ = send_status(&mut socket, json!({"type": "error", "message": m})).await;
            return;
        }
        let started = Instant::now();
        let opened = ExecOpened { container: &self.container, client: self.client, session: &reg.id, path: "agent" };
        self.caller.audit(EXEC_OPEN, &resource, workspace, audit_value(&opened)).await;
        let request = OpenRequest {
            server_id: self.server_id,
            container: self.container.clone(),
            size: self.size,
            principal: self.caller.principal().to_string(),
            lifetime: MAX_LIFETIME,
        };
        let recheck = Recheck {
            state: &self.state,
            caller: &self.caller,
            action: Action::ContainerExec,
            server_id: self.server_id,
        };
        let (bytes_in, bytes_out, reason, end) = match crate::container_exec::open(&self.state, &request).await {
            Ok(shell) => {
                let ready = json!({"type": "ready", "container": self.container, "session": reg.id, "path": "agent"});
                if !send_status(&mut socket, ready).await {
                    shell.writer.close().await;
                    return;
                }
                let end = shell.end.clone();
                let (i, o, r) =
                    relay(&mut socket, Box::new(shell.reader), Box::new(shell.writer), &recheck, &mut reg.revoked)
                        .await;
                let end = end.lock().ok().and_then(|e| e.clone());
                (i, o, r, end)
            }
            Err(e) => {
                let _ = send_status(&mut socket, json!({"type": "error", "message": e.to_string()})).await;
                (0, 0, "connect_failed", Some(ShellEnd::Error(e.to_string())))
            }
        };
        let (exit_code, error) = match &end {
            Some(ShellEnd::Exit(c)) => (Some(*c), None),
            Some(ShellEnd::Error(m)) => (None, Some(m.as_str())),
            None => (None, None),
        };
        if reason != "connect_failed"
            && let Some(m) = error
        {
            let _ = send_status(&mut socket, json!({"type": "error", "message": m})).await;
        }
        let _ = send_status(&mut socket, json!({"type": "closed", "reason": reason, "exit_code": exit_code})).await;
        let closed = ExecClosed {
            opened,
            duration_secs: started.elapsed().as_secs(),
            bytes_in,
            bytes_out,
            reason,
            exit_code,
            error,
        };
        self.caller.audit("container.exec_close", &resource, workspace, audit_value(&closed)).await;
    }
}

/// Keeps the SSH connection alive for as long as its shell is read.
struct Holder {
    inner: Box<dyn ShellReader>,
    _session: Box<dyn forge_core::remote::RemoteSession>,
}

#[async_trait::async_trait]
impl ShellReader for Holder {
    async fn recv(&mut self) -> Option<Vec<u8>> {
        self.inner.recv().await
    }
}

async fn relay(
    socket: &mut WebSocket,
    mut reader: Box<dyn ShellReader>,
    writer: Box<dyn ShellWriter>,
    session: &Recheck<'_>,
    revoked: &mut tokio::sync::watch::Receiver<bool>,
) -> (u64, u64, &'static str) {
    let (mut bytes_in, mut bytes_out) = (0u64, 0u64);
    let deadline = tokio::time::Instant::now() + MAX_LIFETIME;
    let mut recheck = tokio::time::interval(RECHECK);
    recheck.tick().await;
    let reason = loop {
        tokio::select! {
            () = tokio::time::sleep_until(deadline) => break "max_lifetime",
            _ = revoked.changed() => break "revoked",
            _ = recheck.tick() => {
                if !session.still_allowed().await {
                    break "session_ended";
                }
            }
            out = reader.recv() => match out {
                Some(data) => {
                    bytes_out += data.len() as u64;
                    if socket.send(Message::Binary(data.into())).await.is_err() {
                        break "client_gone";
                    }
                }
                None => break "shell_exited",
            },
            msg = tokio::time::timeout(IDLE_TIMEOUT, socket.recv()) => match msg {
                Err(_) => break "idle_timeout",
                Ok(Some(Ok(Message::Binary(data)))) => {
                    bytes_in += data.len() as u64;
                    if writer.write(&data).await.is_err() {
                        break "write_failed";
                    }
                }
                Ok(Some(Ok(Message::Text(t)))) => match serde_json::from_str::<Control>(&t) {
                    Ok(Control::Resize { cols, rows }) => {
                        let _ = writer.resize(TermSize { cols, rows }).await;
                    }
                    Ok(Control::Ping { t }) => {
                        let _ = send_status(socket, json!({"type": "pong", "t": t})).await;
                    }
                    _ => {}
                },
                Ok(Some(Ok(Message::Close(_)) | Err(_)) | None) => break "client_closed",
                Ok(Some(Ok(_))) => {}
            },
        }
    };
    writer.close().await;
    (bytes_in, bytes_out, reason)
}

/// Relays one exec session: stdin in, tagged stdout/stderr out, then the
/// exit status. Returns `(exit status, bytes in, bytes out, reason)`.
async fn relay_exec(
    socket: &mut WebSocket,
    mut reader: Box<dyn ExecReader>,
    writer: Box<dyn ShellWriter>,
    session: &Recheck<'_>,
    revoked: &mut tokio::sync::watch::Receiver<bool>,
) -> (Option<u32>, u64, u64, &'static str) {
    let (mut bytes_in, mut bytes_out, mut status) = (0u64, 0u64, None);
    let deadline = tokio::time::Instant::now() + MAX_LIFETIME;
    let mut recheck = tokio::time::interval(RECHECK);
    recheck.tick().await;
    let mut input_open = true;
    let reason = loop {
        tokio::select! {
            () = tokio::time::sleep_until(deadline) => break "max_lifetime",
            _ = revoked.changed() => break "revoked",
            _ = recheck.tick() => {
                if !session.still_allowed().await {
                    break "session_ended";
                }
            }
            ev = reader.recv() => match ev {
                Some(ExecEvent::Stdout(d) | ExecEvent::Stderr(d)) if d.is_empty() => {}
                Some(ExecEvent::Stdout(d)) => {
                    bytes_out += d.len() as u64;
                    let mut f = Vec::with_capacity(d.len() + 1);
                    f.push(1u8);
                    f.extend_from_slice(&d);
                    if socket.send(Message::Binary(f.into())).await.is_err() {
                        break "client_gone";
                    }
                }
                Some(ExecEvent::Stderr(d)) => {
                    bytes_out += d.len() as u64;
                    let mut f = Vec::with_capacity(d.len() + 1);
                    f.push(2u8);
                    f.extend_from_slice(&d);
                    if socket.send(Message::Binary(f.into())).await.is_err() {
                        break "client_gone";
                    }
                }
                Some(ExecEvent::Exit(code)) => {
                    status = Some(code);
                    let _ = send_status(socket, json!({"type": "exit", "code": code})).await;
                }
                None => break "exited",
            },
            msg = socket.recv(), if input_open => match msg {
                Some(Ok(Message::Binary(data))) => {
                    bytes_in += data.len() as u64;
                    if writer.write(&data).await.is_err() {
                        break "write_failed";
                    }
                }
                Some(Ok(Message::Text(t))) => match serde_json::from_str::<Control>(&t) {
                    Ok(Control::Eof) => {
                        input_open = false;
                        let _ = writer.eof().await;
                    }
                    Ok(Control::Ping { t }) => {
                        let _ = send_status(socket, json!({"type": "pong", "t": t})).await;
                    }
                    _ => {}
                },
                Some(Ok(Message::Close(_)) | Err(_)) | None => break "client_closed",
                Some(Ok(_)) => {}
            },
        }
    };
    writer.close().await;
    (status, bytes_in, bytes_out, reason)
}

#[cfg(test)]
mod exec_tests {
    use super::{exec_command, sh_quote};

    #[test]
    fn arguments_are_quoted_for_the_shell() {
        assert_eq!(sh_quote("ls"), "ls");
        assert_eq!(sh_quote("/var/log"), "/var/log");
        assert_eq!(sh_quote("a b"), "'a b'");
        assert_eq!(sh_quote(""), "''");
        assert_eq!(sh_quote("it's"), "'it'\\''s'");
        assert_eq!(sh_quote("$(id)"), "'$(id)'");
        assert_eq!(sh_quote("x;rm -rf /"), "'x;rm -rf /'");
        let argv = Some(vec!["echo".into(), "a b".into(), "$HOME".into()]);
        assert_eq!(exec_command(argv, None).as_deref(), Ok("echo 'a b' '$HOME'"));
        assert_eq!(exec_command(None, Some("echo $HOME | wc -c".into())).as_deref(), Ok("echo $HOME | wc -c"));
        assert!(exec_command(Some(vec![]), None).is_err());
        assert!(exec_command(Some(vec!["a".into()]), Some("b".into())).is_err());
        assert!(exec_command(None, Some("a\0b".into())).is_err());
    }
}

#[cfg(test)]
mod tests {
    use super::{ShellPath, cert_principal, container_shell_command, shell_path, valid_login};

    /// D-114: SSH only when `login=` opts in; otherwise the agent path.
    #[test]
    fn the_ssh_path_needs_an_explicit_login() {
        assert_eq!(shell_path(None), ShellPath::Agent);
        assert_eq!(shell_path(Some("forgeops")), ShellPath::Ssh("forgeops".into()));
    }

    /// The container shell runs one fixed command built from a validated id.
    #[test]
    fn container_shell_command_is_fixed() {
        let c = container_shell_command("web-1");
        assert!(c.starts_with("sudo -n docker exec -it web-1 sh -c 'command -v bash"), "{c}");
        assert!(c.contains("exec bash || exec sh"));
        assert!(crate::api::containers::valid_container_id("web-1"));
        assert!(!crate::api::containers::valid_container_id("web;id"));
        assert!(!crate::api::containers::valid_container_id("-web"));
    }

    /// Security review (D-069): certificates name the node they are for.
    #[test]
    fn certificates_for_bound_nodes_name_the_node() {
        let a = "01a1d3c4-8057-76f1-a2dd-df3f6ebd2625";
        assert_eq!(cert_principal(a, "root", true), format!("forge.{a}.root"));
        assert_eq!(cert_principal(a, "root", false), "root", "older nodes keep the bare login");
    }

    #[test]
    fn login_names_are_restricted() {
        for ok in ["forgeops", "deploy_user", "a.b-c"] {
            assert!(valid_login(ok), "{ok}");
        }
        for bad in ["", "-oProxyCommand", "root;id", "a b", "x".repeat(33).as_str()] {
            assert!(!valid_login(bad), "{bad}");
        }
    }
}

#[cfg(test)]
mod fingerprint_tests {
    #[test]
    fn matches_ssh_keygen() {
        // `ssh-keygen -lf` on this key prints the expected fingerprint.
        let line = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIG6IMXb/sO2komyDPBGDGg0Y5zKVsyA4V1ev54McMNbO";
        assert_eq!(super::fingerprint(line).as_deref(), Some("SHA256:p7J4bvgA12FZ6xfsdezNTeLW4+teVuH1Esb8DL5yQYs"));
        assert!(super::fingerprint("ssh-ed25519 !!!").is_none());
    }
}
