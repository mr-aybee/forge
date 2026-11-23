//! Version and capability discovery, so one app can manage panels of
//! different versions. Both endpoints are public and contain no secrets.

use axum::Json;
use axum::extract::State;
use serde::Serialize;

use crate::state::AppState;

#[derive(Debug, Serialize)]
pub struct VersionInfo {
    pub product: &'static str,
    pub version: &'static str,
    pub api_versions: &'static [&'static str],
    pub plugin_api: &'static str,
    pub agent_protocol: &'static str,
    pub build: Option<&'static str>,
}

pub async fn version() -> Json<VersionInfo> {
    Json(VersionInfo {
        product: "orbyn-forge",
        version: crate::VERSION,
        api_versions: &["v1"],
        plugin_api: forge_sdk::PLUGIN_API_VERSION,
        agent_protocol: "forge.agent.v1",
        build: option_env!("FORGE_BUILD_SHA"),
    })
}

#[derive(Debug, Serialize)]
pub struct Capability {
    pub id: &'static str,
    pub version: u32,
    pub enabled: bool,
}

#[derive(Debug, Serialize)]
pub struct Capabilities {
    pub capabilities: Vec<Capability>,
}

pub async fn capabilities(State(state): State<AppState>) -> Json<Capabilities> {
    let c = |id, enabled| Capability { id, version: 1, enabled };
    Json(Capabilities {
        capabilities: vec![
            c("auth.local", true),
            c("auth.totp", true),
            c("auth.login_challenge", state.config.security.login_challenge),
            c("security.mfa_required", state.config.mfa_required()),
            // Deprecated: the opposite of security.mfa_required, for older consoles.
            c("dev.skip_mfa", !state.config.mfa_required()),
            c(
                match state.config.environment {
                    crate::config::DeploymentMode::Production => "env.production",
                    crate::config::DeploymentMode::Staging => "env.staging",
                    crate::config::DeploymentMode::Development => "env.development",
                    crate::config::DeploymentMode::Test => "env.test",
                },
                true,
            ),
            c("auth.oidc", state.oidc.is_some()),
            c("auth.passkeys", true),
            c("auth.oauth_pkce", true),
            c("tenancy", true),
            c("rbac.cedar", true),
            c("agents.enrollment", true),
            c("agents.commands", true),
            c("jobs.durable", true),
            c("audit.hash_chain", true),
            c("plugins.wasm", true),
            c("events.websocket", true),
            c("access.ssh", true),
            c("access.sftp", true),
            c("access.wireguard", true),
            c("files", true),
            c("proxy", true),
            c("dns", true),
            c("containers", true),
            c("stacks", true),
            c("object_storage", true),
            c("mcp", state.config.mcp.enabled),
            // Declared extension points, not implemented yet:
            c("access.rdp", false),
            c("deployments", false),
        ],
    })
}

pub async fn health(State(state): State<AppState>) -> Result<&'static str, crate::error::ApiError> {
    state.store.ping().await?;
    Ok("ok")
}
