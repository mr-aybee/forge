//! Instance settings and personal API keys.
//!
//! * `GET /api/v1/settings` — instance facts, sign-in providers, MFA adoption
//!   and the key/value settings (`view` on the workspace).
//! * `PUT/DELETE /api/v1/settings/{key}` — `workspace.manage`. Services can
//!   reference a setting as `${forge.settings.<key>}` in an environment value.
//! * `GET/POST /api/v1/api-keys`, `DELETE /api/v1/api-keys/{id}` — the
//!   caller's own keys. The full key is returned once; only its hash is kept.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use forge_core::model::ApiKey;
use forge_core::ports::{ApiKeyRepository, SettingsRepository, UserRepository};
use forge_core::{Action, EntityKind, EntityRef};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::auth::{Caller, Ws};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

fn ws_ref(ws: forge_core::WorkspaceId) -> EntityRef {
    EntityRef::new(EntityKind::Workspace, ws)
}

/// Setting keys: `[a-z][a-z0-9_.-]{0,63}`.
#[must_use]
pub fn valid_setting_key(k: &str) -> bool {
    !k.is_empty()
        && k.len() <= 64
        && k.as_bytes()[0].is_ascii_lowercase()
        && k.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'.' | b'-'))
}

/// Keys under `agents.`, `packages.`, `server.`, `analytics.` and `names.` belong to their own endpoints (they
/// need MFA and validation to change), so the generic editor refuses them.
fn reserved(key: &str) -> ApiResult<()> {
    if key.starts_with("agents.") {
        return Err(ApiError::bad_request("this setting is managed on its own page (Updates → Agents)"));
    }
    if key.starts_with("packages.") {
        return Err(ApiError::bad_request("this setting is managed with the package's update policy"));
    }
    if key.starts_with("server.") {
        return Err(ApiError::bad_request(
            "server settings are changed with /api/v1/server/... (console: Settings); the public address with PUT /api/v1/server/public-address",
        ));
    }
    if key.starts_with("analytics.") {
        return Err(ApiError::bad_request(
            "analytics settings are changed with PUT /api/v1/analytics/settings (Settings → Analytics)",
        ));
    }
    if key.starts_with(crate::agent_keys::SETTING_PREFIX) {
        return Err(ApiError::bad_request("the agent key is managed with forge-server agent-key"));
    }
    if key.starts_with("names.") {
        return Err(ApiError::bad_request(
            "the local name suffix is set with PUT /api/v1/names/suffix (DNS → Local names)",
        ));
    }
    Ok(())
}

pub async fn get(State(state): State<AppState>, caller: Caller, Ws(ws): Ws) -> ApiResult<Json<Value>> {
    caller.authorize(Action::View, ws_ref(ws)).await?;
    let users = state.store.list_users().await?;
    let mfa = users.iter().filter(|u| u.totp_secret_sealed.is_some()).count();
    let values: Vec<Value> = state
        .store
        .all_settings()
        .await?
        .into_iter()
        // Internal records (who enabled automatic package patches).
        .filter(|(k, _)| !k.starts_with("packages.auto_patch_by."))
        // The control-plane key state (sealed) and agents' pins (D-167).
        .filter(|(k, _)| !k.starts_with(crate::agent_keys::SETTING_PREFIX))
        .map(|(k, v)| json!({ "key": k, "value": v }))
        .collect();
    Ok(Json(json!({
        "instance": {
            "version": crate::VERSION,
            "public_url": state.config.public_url,
            "node_public_url": state.config.node_forge_url(),
            "agent_public_url": state.config.agent_public_url,
            "cookie_secure": state.config.cookie_secure,
            "data_dir": state.config.data_dir,
        },
        "auth": {
            "local_passwords": true,
            "oidc": state.config.oidc.as_ref().map(|o| json!({ "issuer_url": o.issuer_url, "client_id": o.client_id, "auto_provision": o.auto_provision })),
            "users": users.len(),
            "users_with_mfa": mfa,
            // Read from the guardrail policy itself, so it never drifts.
            "mfa_required_for": mfa_actions_sorted(),
            "mfa_required": state.config.mfa_required(),
            "allow_elevated_tokens": state.config.elevated_tokens_allowed(),
        },
        "tunnels": crate::api::tunnels::settings_view(&state),
        "values": values,
    })))
}

fn mfa_actions_sorted() -> Vec<&'static String> {
    let mut v: Vec<&'static String> = super::openapi::mfa_actions().iter().collect();
    v.sort();
    v
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SettingValue {
    pub value: String,
}

pub async fn put(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Path(key): Path<String>,
    Json(req): Json<SettingValue>,
) -> ApiResult<StatusCode> {
    caller.authorize(Action::WorkspaceManage, ws_ref(ws)).await?;
    if !valid_setting_key(&key) {
        return Err(ApiError::bad_request("key: lowercase letters, digits, `_`, `.`, `-` (max 64)"));
    }
    reserved(&key)?;
    if req.value.len() > 4096 || req.value.contains(['\n', '\r', '\0']) {
        return Err(ApiError::bad_request("value: one line, at most 4096 characters"));
    }
    state.store.put_setting(&key, &req.value, state.clock.now()).await?;
    caller.audit("settings.update", &ws_ref(ws), Some(ws), json!({ "key": key })).await;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn delete(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Path(key): Path<String>,
) -> ApiResult<StatusCode> {
    caller.authorize(Action::WorkspaceManage, ws_ref(ws)).await?;
    reserved(&key)?;
    state.store.delete_setting(&key).await?;
    caller.audit("settings.delete", &ws_ref(ws), Some(ws), json!({ "key": key })).await;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn list_keys(State(state): State<AppState>, caller: Caller) -> ApiResult<Json<Vec<ApiKey>>> {
    Ok(Json(state.store.list_api_keys(caller.session.user.id).await?))
}

/// Days a new key lives unless the request says otherwise.
pub const DEFAULT_KEY_DAYS: u32 = 90;

/// A personal access token, for oneself or for a service account. Unknown
/// fields are refused: a misspelt `restrictions` must not make a key with
/// all of its owner's access.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NewKey {
    pub name: String,
    /// Days until expiry (1-3650); default 90.
    #[serde(default)]
    pub expires_days: Option<u32>,
    /// A key that never expires. Only when asked for explicitly; such keys
    /// are flagged in the lists.
    #[serde(default)]
    pub never_expires: bool,
    /// May run MFA-guarded actions without a prompt (D-097). Needs an
    /// MFA-verified session to create (unless MFA is off for the server)
    /// and `[security] allow_elevated_tokens`.
    #[serde(default)]
    pub elevated: bool,
    /// Restrict the token (#13): it may do only these actions at these
    /// scopes, and never more than its owner holds at the time. Omitted:
    /// the token acts with all of its owner's access.
    #[serde(default)]
    pub restrictions: Option<Vec<NewTokenGrant>>,
}

/// One entry of a restricted token.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NewTokenGrant {
    /// `<kind>:<id>`: workspace, cluster, project, environment, server or service.
    pub scope: String,
    /// Action names from `GET /access/catalog`.
    pub actions: Vec<String>,
}

/// Parses a restricted token's entries: known actions, assignable scopes
/// that exist, at most [`forge_core::permissions::MAX_TOKEN_GRANTS`], and
/// at least one (an empty ceiling would be a token that can do nothing).
async fn parse_restrictions(
    state: &AppState,
    req: &[NewTokenGrant],
) -> ApiResult<Vec<forge_core::permissions::TokenGrant>> {
    use forge_core::permissions::{MAX_TOKEN_GRANTS, TokenGrant, parse_actions, parse_scope};
    if req.is_empty() || req.len() > MAX_TOKEN_GRANTS {
        return Err(ApiError::bad_request(format!("restrictions: 1 to {MAX_TOKEN_GRANTS} entries")));
    }
    let mut out = Vec::with_capacity(req.len());
    for g in req {
        let scope = parse_scope(&g.scope)?;
        // Fails closed for forged ids.
        state.authz_resolve(&scope).await.map_err(|_| ApiError::not_found(format!("no such scope {scope}")))?;
        out.push(TokenGrant { scope, actions: parse_actions(&g.actions)? });
    }
    Ok(out)
}

/// Refuses an elevated token unless the policy allows them and the caller's
/// session is MFA-verified (or MFA is off for the server). Denials are
/// audited as `action`.
pub(crate) async fn check_elevated(
    state: &AppState,
    caller: &Caller,
    action: &str,
    resource: &EntityRef,
) -> ApiResult<()> {
    if !state.config.elevated_tokens_allowed() {
        caller.audit_denied(action, resource, None, "elevated tokens are turned off for this server").await;
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "elevated_tokens_disabled",
            "elevated tokens are turned off for this server ([security] allow_elevated_tokens = false)",
        ));
    }
    caller.require_mfa(action, resource).await
}

#[derive(Debug, Serialize)]
pub struct CreatedKey {
    #[serde(flatten)]
    pub key: ApiKey,
    /// Shown once.
    pub secret: String,
}

/// Creates a key for `user` from `req` (name, expiry): random secret, only
/// its SHA-256 stored, 90 days unless `never_expires`. Shared by personal
/// keys and service account tokens (D-086).
pub(crate) async fn mint_key(state: &AppState, user: forge_core::UserId, req: &NewKey) -> ApiResult<(ApiKey, String)> {
    let restrictions = match &req.restrictions {
        Some(r) => Some(parse_restrictions(state, r).await?),
        None => None,
    };
    let name = req.name.trim();
    if name.is_empty() || name.len() > 80 {
        return Err(ApiError::bad_request("name: 1–80 characters"));
    }
    let prefix = format!("fk_{}", forge_secrets::crypto::random_token::<6>().to_lowercase().replace(['-', '_'], "x"));
    let secret = format!("{prefix}_{}", forge_secrets::crypto::random_token::<32>());
    let now = state.clock.now();
    let key = ApiKey {
        id: forge_core::JobId::new().to_string(),
        user_id: user,
        name: name.to_owned(),
        prefix,
        secret_hash: forge_secrets::crypto::sha256_hex(secret.as_bytes()),
        expires_at: (!req.never_expires).then(|| {
            let days = req.expires_days.unwrap_or(DEFAULT_KEY_DAYS).clamp(1, 3650);
            now.plus(std::time::Duration::from_secs(u64::from(days) * 86_400))
        }),
        last_used_at: None,
        created_at: now,
        elevated: req.elevated,
        restrictions,
    };
    state.store.create_api_key(&key).await?;
    Ok((key, secret))
}

pub async fn create_key(
    State(state): State<AppState>,
    caller: Caller,
    Json(req): Json<NewKey>,
) -> ApiResult<(StatusCode, Json<CreatedKey>)> {
    // A leaked key must not be able to mint more keys.
    if caller.via_api_key() {
        return Err(ApiError::forbidden("API keys cannot create API keys; sign in to the console"));
    }
    let me = EntityRef::new(EntityKind::User, caller.session.user.id);
    if req.elevated {
        check_elevated(&state, &caller, "apikey.create", &me).await?;
    }
    // A token can only be given what its creator holds now (#13); it is
    // checked again, as the intersection, at every request.
    if let Some(r) = &req.restrictions {
        for g in parse_restrictions(&state, r).await? {
            let missing = crate::access::missing(&caller, &g.actions, &g.scope).await;
            if !missing.is_empty() {
                caller
                    .audit_denied("apikey.create", &me, None, "the token lists actions its owner does not hold")
                    .await;
                return Err(ApiError::new(
                    StatusCode::FORBIDDEN,
                    "escalation",
                    format!(
                        "you can only give a token what you hold; at {} you lack: {}",
                        g.scope,
                        missing.iter().map(|a| a.as_str()).collect::<Vec<_>>().join(", ")
                    ),
                ));
            }
        }
    }
    let (key, secret) = mint_key(&state, caller.session.user.id, &req).await?;
    caller
        .audit(
            "apikey.create",
            &me,
            None,
            json!({
                "name": key.name, "prefix": key.prefix, "expires_at": key.expires_at,
                "token_id": key.id, "elevated": key.elevated,
                "restricted": key.restrictions.as_ref().map(Vec::len),
            }),
        )
        .await;
    Ok((StatusCode::CREATED, Json(CreatedKey { key, secret })))
}

pub async fn delete_key(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    if !state.store.delete_api_key(caller.session.user.id, &id).await? {
        return Err(ApiError::not_found("API key not found"));
    }
    // Terminals and exec sessions opened with the key end now.
    state.remote_sessions.end_api_key(&id);
    caller
        .audit("apikey.revoke", &EntityRef::new(EntityKind::User, caller.session.user.id), None, json!({ "id": id }))
        .await;
    Ok(StatusCode::NO_CONTENT)
}

// ------------------------------------------------- other users' keys (admins)

/// `GET /users/{id}/api-keys`: a user's API keys (no secrets). Platform
/// `user.manage`.
pub async fn list_user_keys(
    State(state): State<AppState>,
    caller: Caller,
    Path(user): Path<forge_core::UserId>,
) -> ApiResult<Json<Vec<ApiKey>>> {
    caller.authorize(forge_core::Action::UserManage, EntityRef::platform()).await?;
    Ok(Json(state.store.list_api_keys(user).await?))
}

/// `DELETE /users/{id}/api-keys/{key}`: revokes one of a user's keys.
/// Platform `user.manage`; no MFA, so removing access is never held up.
pub async fn delete_user_key(
    State(state): State<AppState>,
    caller: Caller,
    Path((user, id)): Path<(forge_core::UserId, String)>,
) -> ApiResult<StatusCode> {
    let target = EntityRef::new(EntityKind::User, user);
    caller.authorize(forge_core::Action::UserManage, EntityRef::platform()).await?;
    if !state.store.delete_api_key(user, &id).await? {
        caller.audit_denied("apikey.revoke", &target, None, "not found").await;
        return Err(ApiError::not_found("API key not found"));
    }
    state.remote_sessions.end_api_key(&id);
    caller.audit("apikey.revoke", &target, None, json!({ "id": id, "by_admin": true })).await;
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /users/{id}/api-keys`: revokes all of a user's keys.
pub async fn delete_user_keys(
    State(state): State<AppState>,
    caller: Caller,
    Path(user): Path<forge_core::UserId>,
) -> ApiResult<Json<serde_json::Value>> {
    let target = EntityRef::new(EntityKind::User, user);
    caller.authorize(forge_core::Action::UserManage, EntityRef::platform()).await?;
    let n = state.store.delete_user_api_keys(user).await?;
    state.remote_sessions.end_user_api_keys(user);
    caller.audit("apikey.revoke_all", &target, None, json!({ "revoked": n })).await;
    Ok(Json(json!({ "revoked": n })))
}

#[cfg(test)]
mod tests {
    use super::valid_setting_key;

    #[test]
    fn setting_keys() {
        assert!(valid_setting_key("company.name"));
        assert!(!valid_setting_key("Company"));
        assert!(!valid_setting_key("a b"));
    }
}
