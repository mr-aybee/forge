//! Server-level settings (D-120), shown in the console under **Settings**.
//!
//! * `GET /api/v1/server/settings` — the public address and the effective
//!   `forge.toml` values it falls back to.
//! * `PUT /api/v1/server/public-address` — sets the runtime public address.
//! * `DELETE /api/v1/server/public-address` — clears it (links use `public_url`).
//! * `PUT /api/v1/server/allowed-hosts` — sets the hosts the console answers
//!   on (D-167).
//! * `DELETE /api/v1/server/allowed-hosts` — back to `*` (any host).
//!
//! Reading needs `view` on the platform (a platform-scope role). The writes
//! need `system.update` on the platform and an MFA-verified session (or MFA
//! turned off for the server).

use axum::Json;
use axum::extract::State;
use forge_core::{Action, EntityRef};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// Audit action of a public address change.
pub const AUDIT_PUBLIC_ADDRESS_UPDATE: &str = "server.public_address.update";
/// Audit action of clearing the public address.
pub const AUDIT_PUBLIC_ADDRESS_CLEAR: &str = "server.public_address.clear";
/// Audit action of an allowed-hosts change.
pub const AUDIT_ALLOWED_HOSTS_UPDATE: &str = "server.allowed_hosts.update";
/// Audit action of resetting the allowed hosts to `*`.
pub const AUDIT_ALLOWED_HOSTS_CLEAR: &str = "server.allowed_hosts.clear";

/// Where the address links use comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AddressSource {
    /// The runtime setting.
    Setting,
    /// `public_url` in `forge.toml`.
    Config,
    /// Not pinned: the address an administrator last signed in on.
    LastAdmin,
    /// Not pinned and no administrator has signed in yet.
    None,
}

/// The public address and where it comes from.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct PublicAddressView {
    /// The address background links (alerts, OIDC) use now; empty when
    /// none is known yet. Links in the console use the address you are on.
    pub effective: String,
    pub source: AddressSource,
    /// The runtime setting (`null` when not set).
    pub setting: Option<String>,
    /// `public_url` from `forge.toml` (`null` when not set).
    pub configured: Option<String>,
    /// The address an administrator last signed in on (read-only,
    /// recorded automatically; used when nothing is pinned).
    pub last_admin: Option<String>,
    /// Whether links use HTTPS (from the effective address's scheme).
    pub https: bool,
    /// Browser origins the WebSocket and MCP Origin guards accept besides
    /// the request's own origin.
    pub allowed_origins: Vec<String>,
}

/// The hosts the console answers on.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct AllowedHostsView {
    /// Patterns: `*`, a host name or IP, or `*.example.com`.
    pub hosts: Vec<String>,
    /// Whether any host is allowed (`*` in the list).
    pub any: bool,
}

/// Read-only values from `forge.toml`.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct ConfigView {
    pub public_url: Option<String>,
    pub node_public_url: String,
    pub agent_public_url: String,
    pub cookie_secure: bool,
    /// `[security] trusted_proxies`.
    pub trusted_proxies: Vec<String>,
}

/// `GET /api/v1/server/settings`.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct ServerSettings {
    pub public_address: PublicAddressView,
    pub allowed_hosts: AllowedHostsView,
    pub config: ConfigView,
}

/// Builds the public address view.
#[must_use]
pub fn address_view(state: &AppState) -> PublicAddressView {
    let a = &state.public_address;
    let setting = a.runtime();
    let configured = Some(a.configured().to_owned()).filter(|c| !c.is_empty());
    let last_admin = a.last_admin();
    let effective = a.get();
    let source = match (&setting, &configured, &last_admin) {
        (Some(_), _, _) => AddressSource::Setting,
        (None, Some(_), _) => AddressSource::Config,
        (None, None, Some(_)) => AddressSource::LastAdmin,
        (None, None, None) => AddressSource::None,
    };
    PublicAddressView {
        https: effective.starts_with("https://"),
        source,
        allowed_origins: crate::public_address::allowed_origins(a),
        configured,
        last_admin,
        setting,
        effective,
    }
}

/// Builds the allowed-hosts view.
#[must_use]
pub fn hosts_view(state: &AppState) -> AllowedHostsView {
    AllowedHostsView { hosts: state.allowed_hosts.list(), any: state.allowed_hosts.any() }
}

pub async fn get(State(state): State<AppState>, caller: Caller) -> ApiResult<Json<ServerSettings>> {
    // Platform-scope readers only (not workspace roles); reading needs no MFA.
    caller.authorize(Action::View, EntityRef::platform()).await?;
    let c = &state.config;
    Ok(Json(ServerSettings {
        public_address: address_view(&state),
        allowed_hosts: hosts_view(&state),
        config: ConfigView {
            public_url: c.public_url.clone(),
            node_public_url: c.node_forge_url().to_owned(),
            agent_public_url: c.agent_public_url.clone(),
            cookie_secure: c.cookie_secure,
            trusted_proxies: c.security.trusted_proxies.clone(),
        },
    }))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PublicAddressRequest {
    /// `https://forge.example.com`: scheme, host and optional port; no path.
    pub value: String,
}

async fn guard(caller: &Caller, action: &str) -> ApiResult<EntityRef> {
    let platform = EntityRef::platform();
    caller.authorize(Action::SystemUpdate, platform.clone()).await?;
    caller.require_mfa(action, &platform).await?;
    Ok(platform)
}

pub async fn put_public_address(
    State(state): State<AppState>,
    caller: Caller,
    Json(req): Json<PublicAddressRequest>,
) -> ApiResult<Json<PublicAddressView>> {
    let platform = guard(&caller, AUDIT_PUBLIC_ADDRESS_UPDATE).await?;
    if let Err(e) = crate::public_address::validate(&req.value) {
        caller.audit_denied(AUDIT_PUBLIC_ADDRESS_UPDATE, &platform, None, &e.to_string()).await;
        return Err(ApiError::bad_request(format!("value: {e}")));
    }
    let from = state.public_address.get();
    crate::public_address::save(&state.store, &state.public_address, Some(&req.value), state.clock.now()).await?;
    let view = address_view(&state);
    caller.audit(AUDIT_PUBLIC_ADDRESS_UPDATE, &platform, None, json!({ "from": from, "to": view.effective })).await;
    Ok(Json(view))
}

pub async fn delete_public_address(
    State(state): State<AppState>,
    caller: Caller,
) -> ApiResult<Json<PublicAddressView>> {
    let platform = guard(&caller, AUDIT_PUBLIC_ADDRESS_CLEAR).await?;
    let from = state.public_address.get();
    crate::public_address::save(&state.store, &state.public_address, None, state.clock.now()).await?;
    let view = address_view(&state);
    caller.audit(AUDIT_PUBLIC_ADDRESS_CLEAR, &platform, None, json!({ "from": from, "to": view.effective })).await;
    Ok(Json(view))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AllowedHostsRequest {
    /// Patterns: `*`, `forge.example.com`, `192.0.2.1`, `*.example.com`.
    pub hosts: Vec<String>,
}

pub async fn put_allowed_hosts(
    State(state): State<AppState>,
    caller: Caller,
    Json(req): Json<AllowedHostsRequest>,
) -> ApiResult<Json<AllowedHostsView>> {
    let platform = guard(&caller, AUDIT_ALLOWED_HOSTS_UPDATE).await?;
    if let Err(e) = forge_core::allowed_hosts::parse_list(&req.hosts) {
        caller.audit_denied(AUDIT_ALLOWED_HOSTS_UPDATE, &platform, None, &e.to_string()).await;
        return Err(ApiError::bad_request(format!("hosts: {e}")));
    }
    let from = state.allowed_hosts.list();
    crate::allowed_hosts::save(&state.store, &state.allowed_hosts, Some(&req.hosts), state.clock.now()).await?;
    let view = hosts_view(&state);
    caller.audit(AUDIT_ALLOWED_HOSTS_UPDATE, &platform, None, json!({ "from": from, "to": view.hosts })).await;
    Ok(Json(view))
}

pub async fn delete_allowed_hosts(State(state): State<AppState>, caller: Caller) -> ApiResult<Json<AllowedHostsView>> {
    let platform = guard(&caller, AUDIT_ALLOWED_HOSTS_CLEAR).await?;
    let from = state.allowed_hosts.list();
    crate::allowed_hosts::save(&state.store, &state.allowed_hosts, None, state.clock.now()).await?;
    let view = hosts_view(&state);
    caller.audit(AUDIT_ALLOWED_HOSTS_CLEAR, &platform, None, json!({ "from": from, "to": view.hosts })).await;
    Ok(Json(view))
}
