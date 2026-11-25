//! MCP connectors on Forge's OAuth server (GitHub issue #6): discovery
//! metadata (RFC 8414, RFC 9728), dynamic client registration (RFC 7591),
//! the consent step for registered connectors, and Connected apps.
//!
//! A connector's tokens are audience-bound to `/mcp` (RFC 8707): the rest of
//! the API refuses them (see `auth.rs`), and `/mcp` refuses every other
//! token. Consent happens in the console with the person's web session, so a
//! connector can never approve itself.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use forge_core::model::{AgentConnection, AgentMode, AuditEvent, AuditOutcome, SessionKind};
use forge_core::{Action, EntityRef, UserId};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::auth::{Caller, SESSION_COOKIE, cookie};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// Path of the MCP endpoint, the protected resource.
pub const MCP_PATH: &str = "/mcp";
/// How long a consent request waits for the person.
const CONSENT_TTL: Duration = Duration::from_secs(10 * 60);
const MAX_PENDING: usize = 1000;
/// Open sign-in requests per person, so nobody can fill the shared map.
const MAX_PENDING_PER_USER: usize = 10;

/// An authorization request from a connector, waiting for consent. Bound to
/// the person whose browser started it.
#[derive(Debug, Clone)]
pub struct PendingConsent {
    pub user_id: UserId,
    pub client_id: String,
    pub client_name: String,
    pub redirect_uri: String,
    pub state: Option<String>,
    pub code_challenge: String,
    pub resource: String,
    at: Instant,
}

/// Consent requests by id (in memory; a restart only means "start again").
#[derive(Debug, Default)]
pub struct ConsentRequests(Mutex<HashMap<String, PendingConsent>>);

impl ConsentRequests {
    fn insert(&self, p: PendingConsent) -> ApiResult<String> {
        let Ok(mut m) = self.0.lock() else {
            return Err(ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", "consent store unavailable"));
        };

        m.retain(|_, v| v.at.elapsed() < CONSENT_TTL);

        if m.len() >= MAX_PENDING || m.values().filter(|v| v.user_id == p.user_id).count() >= MAX_PENDING_PER_USER {
            return Err(ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "throttled",
                "too many sign-in requests; try later",
            ));
        }

        let id = forge_secrets::crypto::random_token::<18>();
        m.insert(id.clone(), p);

        Ok(id)
    }

    fn get(&self, id: &str) -> Option<PendingConsent> {
        let m = self.0.lock().ok()?;
        m.get(id).filter(|p| p.at.elapsed() < CONSENT_TTL).cloned()
    }

    fn take(&self, id: &str) -> Option<PendingConsent> {
        let mut m = self.0.lock().ok()?;
        m.remove(id).filter(|p| p.at.elapsed() < CONSENT_TTL)
    }
}

/// The OAuth issuer: the pinned public address only (runtime setting or
/// `public_url`). Never a request's host or the last administrator
/// address: tokens are bound to it, so it must not follow a `Host` header.
fn issuer(state: &AppState) -> ApiResult<String> {
    state.public_address.pinned().ok_or_else(|| {
        ApiError::new(
            StatusCode::CONFLICT,
            crate::error::ERR_NO_PUBLIC_ADDRESS,
            "MCP connectors need a pinned public address: set it under Settings > Server, or public_url in forge.toml",
        )
    })
}

/// The canonical resource identifier of `/mcp` (RFC 8707, RFC 9728).
pub fn resource_url(state: &AppState) -> ApiResult<String> {
    Ok(format!("{}{MCP_PATH}", issuer(state)?))
}

fn enabled(state: &AppState) -> ApiResult<()> {
    if state.config.mcp.enabled {
        Ok(())
    } else {
        Err(ApiError::not_found("MCP connectors are not enabled on this Forge"))
    }
}

/// `GET /.well-known/oauth-authorization-server` (RFC 8414).
pub async fn authorization_server_metadata(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let iss = issuer(&state)?;
    let mut m = json!({
        "issuer": iss,
        "authorization_endpoint": format!("{iss}/oauth/authorize"),
        "token_endpoint": format!("{iss}/oauth/token"),
        "response_types_supported": ["code"],
        "response_modes_supported": ["query"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["none"],
        "authorization_response_iss_parameter_supported": true,
    });

    if state.config.mcp.enabled {
        m["registration_endpoint"] = json!(format!("{iss}/oauth/register"));
    }

    Ok(Json(m))
}

/// `GET /.well-known/oauth-protected-resource[/mcp]` (RFC 9728).
pub async fn protected_resource_metadata(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    enabled(&state)?;
    Ok(Json(json!({
        "resource": resource_url(&state)?,
        "authorization_servers": [issuer(&state)?],
        "bearer_methods_supported": ["header"],
        "resource_name": "Orbyn Forge",
        "resource_documentation": "https://docs.orbyn.dev/concepts/mcp-connectors/",
    })))
}

/// The `WWW-Authenticate` value for a 401 at `/mcp` (RFC 9728 §5.1).
pub fn www_authenticate(state: &AppState, error: Option<&str>) -> ApiResult<String> {
    let meta = format!("{}/.well-known/oauth-protected-resource{MCP_PATH}", issuer(state)?);

    match error {
        Some(e) => Ok(format!("Bearer resource_metadata=\"{meta}\", error=\"{e}\"")),
        None => Ok(format!("Bearer resource_metadata=\"{meta}\"")),
    }
}

// ------------------------------------------------------------ registration

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RegisterRequest {
    #[serde(default)]
    pub client_name: Option<String>,
    #[serde(default)]
    pub redirect_uris: Vec<String>,
    #[serde(default)]
    pub grant_types: Option<Vec<String>>,
    #[serde(default)]
    pub response_types: Option<Vec<String>>,
    #[serde(default)]
    pub token_endpoint_auth_method: Option<String>,
}

/// RFC 7591 error body.
fn register_error(code: &str, description: &str) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": code, "error_description": description }))).into_response()
}

/// `POST /oauth/register` (RFC 7591): registers a public connector client.
/// Anonymous, so it is capped per hour; it grants nothing until a person
/// approves a connection on the consent page.
pub async fn register(State(state): State<AppState>, Json(req): Json<RegisterRequest>) -> ApiResult<Response> {
    enabled(&state)?;
    issuer(&state)?;

    if req.token_endpoint_auth_method.as_deref().is_some_and(|m| m != "none") {
        return Ok(register_error("invalid_client_metadata", "only public clients (token_endpoint_auth_method none)"));
    }

    if req.grant_types.as_ref().is_some_and(|g| g.iter().any(|t| t != "authorization_code" && t != "refresh_token")) {
        return Ok(register_error("invalid_client_metadata", "grant_types: authorization_code and refresh_token only"));
    }

    if req.response_types.as_ref().is_some_and(|r| r.iter().any(|t| t != "code")) {
        return Ok(register_error("invalid_client_metadata", "response_types: code only"));
    }

    let name = req.client_name.as_deref().unwrap_or("Unnamed connector");

    let client = match state
        .identity
        .register_client(name, &req.redirect_uris, None, state.config.mcp.registrations_per_hour)
        .await
    {
        Ok(c) => c,
        Err(forge_identity::IdentityError::InvalidRedirect(m)) => {
            return Ok(register_error("invalid_redirect_uri", &m));
        }
        Err(forge_identity::IdentityError::InvalidRequest(m)) => {
            return Ok(register_error("invalid_client_metadata", &m));
        }
        Err(e) => return Err(e.into()),
    };

    let hosts: Vec<String> = client.redirect_uris.iter().map(|u| forge_identity::redirect_host(u)).collect();
    audit(
        &state,
        "connector:anonymous",
        "oauth.client.register",
        AuditOutcome::Success,
        json!({
            "client_id": client.client_id, "client_name": client.name, "redirect_hosts": hosts,
        }),
    )
    .await;

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "client_id": client.client_id,
            "client_id_issued_at": client.created_at.0 / 1000,
            "client_name": client.name,
            "redirect_uris": client.redirect_uris,
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none",
        })),
    )
        .into_response())
}

// ------------------------------------------------------------ authorize + consent

/// `GET /oauth/authorize` for a registered connector: checks the request,
/// then sends the signed-in person to the consent page. Nothing is issued
/// without their approval. Errors are shown, never redirected, until the
/// redirect URI is known to be registered.
pub async fn connector_authorize(
    state: &AppState,
    headers: &HeaderMap,
    q: &super::session::AuthorizeQuery,
    login_next: String,
) -> ApiResult<Response> {
    enabled(state)?;
    let resource = resource_url(state)?;
    if q.resource.as_deref().is_some_and(|r| r.trim_end_matches('/') != resource) {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_target",
            format!("the only resource is {resource}"),
        ));
    }

    let client = state
        .identity
        .check_connector_request(&q.client_id, &q.redirect_uri, &q.code_challenge, &q.code_challenge_method)
        .await?;

    let session = match cookie(headers, SESSION_COOKIE) {
        Some(t) => state.identity.authenticate(t, SessionKind::Web).await?,
        None => None,
    };

    let Some(session) = session else {
        let login =
            format!("/login?next={}", url::form_urlencoded::byte_serialize(login_next.as_bytes()).collect::<String>());
        return Ok(Redirect::to(&login).into_response());
    };

    let id = state.consents.insert(PendingConsent {
        user_id: session.user.id,
        client_id: client.client_id,
        client_name: client.name,
        redirect_uri: q.redirect_uri.clone(),
        state: q.state.clone(),
        code_challenge: q.code_challenge.clone(),
        resource,
        at: Instant::now(),
    })?;

    Ok(Redirect::to(&format!("/connect?request={id}")).into_response())
}

/// Consent is given only from the console: a browser session (cookie with
/// CSRF), never a bearer or API key, and only by the person who started it.
fn consent_caller(caller: &Caller, p: &PendingConsent) -> ApiResult<()> {
    if caller.session.session.kind != SessionKind::Web || caller.via_api_key() {
        return Err(ApiError::forbidden("approve connectors in the Forge console"));
    }

    if caller.session.user.id != p.user_id {
        return Err(ApiError::not_found("sign-in request not found or expired"));
    }

    Ok(())
}

fn allowed_modes(state: &AppState) -> Vec<AgentMode> {
    [AgentMode::ReadOnly, AgentMode::Ask, AgentMode::Auto]
        .into_iter()
        .filter(|m| *m <= state.config.mcp.max_mode)
        .collect()
}

/// `GET /api/v1/oauth/consent/{id}`: what the consent page shows.
pub async fn consent_get(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    issuer(&state)?;
    let p = state.consents.get(&id).ok_or_else(|| ApiError::not_found("sign-in request not found or expired"))?;
    consent_caller(&caller, &p)?;
    let client = state.identity.connector_client(&p.client_id).await?;

    Ok(Json(json!({
        "request_id": id,
        "client": { "client_id": client.client_id, "name": client.name, "registered_at": client.created_at },
        "redirect_uri": p.redirect_uri,
        "redirect_host": forge_identity::redirect_host(&p.redirect_uri),
        "resource": p.resource,
        "modes": allowed_modes(&state),
        "default_mode": AgentMode::Ask.min(state.config.mcp.max_mode),
        "user": caller.session.user.login(),
    })))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConsentDecision {
    pub approve: bool,
    #[serde(default)]
    #[schemars(with = "Option<crate::api::schema_types::AgentMode>")]
    pub mode: Option<AgentMode>,
}

/// `POST /api/v1/oauth/consent/{id}`: approve (with a mode) or deny.
/// Returns where the browser goes next: the connector's redirect URI.
pub async fn consent_decide(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
    Json(d): Json<ConsentDecision>,
) -> ApiResult<Json<Value>> {
    let iss = issuer(&state)?;
    let p = state.consents.get(&id).ok_or_else(|| ApiError::not_found("sign-in request not found or expired"))?;
    consent_caller(&caller, &p)?;
    let p = state.consents.take(&id).ok_or_else(|| ApiError::not_found("sign-in request not found or expired"))?;
    let mut target = url::Url::parse(&p.redirect_uri).map_err(|_| ApiError::bad_request("bad redirect_uri"))?;
    let resource = EntityRef::platform();

    if !d.approve {
        target.query_pairs_mut().append_pair("error", "access_denied");

        if let Some(s) = &p.state {
            target.query_pairs_mut().append_pair("state", s);
        }

        target.query_pairs_mut().append_pair("iss", &iss);
        caller.audit_denied("connector.approve", &resource, None, &format!("{} was not approved", p.client_name)).await;

        return Ok(Json(json!({ "redirect": target.as_str() })));
    }

    let mode = d.mode.unwrap_or_else(|| AgentMode::Ask.min(state.config.mcp.max_mode));
    if !allowed_modes(&state).contains(&mode) {
        return Err(ApiError::forbidden(format!(
            "the highest mode on this Forge is {}",
            state.config.mcp.max_mode.as_str()
        )));
    }

    let (connection, code) = state
        .identity
        .approve_connection(caller.session.user.id, &p.client_id, &p.redirect_uri, &p.code_challenge, &p.resource, mode)
        .await?;
    target.query_pairs_mut().append_pair("code", &code);

    if let Some(s) = &p.state {
        target.query_pairs_mut().append_pair("state", s);
    }

    target.query_pairs_mut().append_pair("iss", &iss);
    caller
        .audit(
            "connector.approve",
            &resource,
            None,
            json!({
                "connection": connection.id, "client_id": connection.client_id, "client_name": connection.client_name,
                "redirect_host": connection.redirect_host, "mode": connection.mode,
            }),
        )
        .await;

    Ok(Json(json!({ "redirect": target.as_str() })))
}

// ------------------------------------------------------------ Connected apps

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ConnectionsQuery {
    /// Everyone's connections (administrators).
    #[serde(default)]
    pub all: bool,
}

fn connection_view(c: &AgentConnection) -> Value {
    json!({
        "id": c.id, "user_id": c.user_id, "client_id": c.client_id, "client_name": c.client_name,
        "redirect_host": c.redirect_host, "mode": c.mode, "created_at": c.created_at,
        "last_used_at": c.last_used_at, "revoked_at": c.revoked_at,
    })
}

/// `GET /api/v1/connected-apps`: your connectors; `?all=true` lists
/// everyone's (`user.manage`).
pub async fn connections(
    State(state): State<AppState>,
    caller: Caller,
    Query(q): Query<ConnectionsQuery>,
) -> ApiResult<Json<Value>> {
    // Listing and revoking work even while connectors are turned off.
    let user = if q.all {
        caller.authorize(Action::UserManage, EntityRef::platform()).await?;
        None
    } else {
        Some(caller.session.user.id)
    };

    let list = state.identity.list_connections(user).await?;

    Ok(Json(json!({ "connections": list.iter().map(connection_view).collect::<Vec<_>>() })))
}

/// `POST /api/v1/connected-apps/{id}/revoke`: yours, or anyone's with
/// `user.manage`. Its tokens stop working at once.
pub async fn revoke_connection(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let c = state.identity.get_connection(&id).await?.ok_or_else(|| ApiError::not_found("connection not found"))?;
    if c.user_id != caller.session.user.id {
        caller.authorize(Action::UserManage, EntityRef::platform()).await?;
    }

    state.identity.revoke_connection(&id).await?;
    caller
        .audit(
            "connector.revoke",
            &EntityRef::platform(),
            None,
            json!({ "connection": c.id, "client_name": c.client_name, "owner": c.user_id }),
        )
        .await;
    let cancelled = cancel_approvals(&state, &caller, &c).await?;

    Ok(Json(json!({ "revoked": c.id, "cancelled_approvals": cancelled })))
}

/// A revoked connection's waiting requests can never be approved: cancels
/// them and records each one.
async fn cancel_approvals(state: &AppState, caller: &Caller, c: &AgentConnection) -> ApiResult<usize> {
    use forge_core::ports::ApprovalRepository;
    let ids = state.store.cancel_pending_approvals(&c.id, caller.session.user.id, state.clock.now()).await?;

    for id in &ids {
        caller
            .audit(
                "mcp.approval.cancel",
                &EntityRef::platform(),
                None,
                json!({ "approval": id, "connection": c.id, "client_name": c.client_name, "reason": "connection revoked" }),
            )
            .await;
    }

    Ok(ids.len())
}

/// `GET /api/v1/oauth/clients`: registered connectors (`user.manage`).
pub async fn clients(State(state): State<AppState>, caller: Caller) -> ApiResult<Json<Value>> {
    caller.authorize(Action::UserManage, EntityRef::platform()).await?;
    Ok(Json(json!({ "clients": state.identity.list_clients().await? })))
}

/// `POST /api/v1/oauth/clients/{id}/revoke`: blocks a connector and ends
/// every connection made with it (`user.manage`).
pub async fn revoke_client(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    caller.authorize(Action::UserManage, EntityRef::platform()).await?;
    let client = state.identity.connector_client(&id).await.map_err(|_| ApiError::not_found("client not found"))?;
    state.identity.revoke_client(&id).await?;
    caller
        .audit(
            "oauth.client.revoke",
            &EntityRef::platform(),
            None,
            json!({ "client_id": id, "client_name": client.name }),
        )
        .await;
    let mut cancelled = 0;

    for c in state.identity.list_connections(None).await?.iter().filter(|c| c.client_id == id) {
        cancelled += cancel_approvals(&state, &caller, c).await?;
    }

    Ok(Json(json!({ "revoked": id, "cancelled_approvals": cancelled })))
}

async fn audit(state: &AppState, actor: &str, action: &str, outcome: AuditOutcome, details: Value) {
    let ev = AuditEvent::new(actor, action, EntityRef::platform(), outcome).with_details(details);
    if let Err(e) = state.audit.append(ev).await {
        tracing::error!(error = %e, "audit write failed");
    }
}
