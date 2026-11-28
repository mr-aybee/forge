//! Service accounts (D-086): non-person identities for CI, schedules and
//! other automation.
//!
//! * `GET  /api/v1/service-accounts` (`view`), `POST` (`access.manage`).
//! * `PATCH /api/v1/service-accounts/{id}`: description, disable/enable.
//! * `GET/POST /api/v1/service-accounts/{id}/tokens`,
//!   `DELETE /api/v1/service-accounts/{id}/tokens/{token}`.
//!
//! A service account is a user without a password, passkey, authenticator
//! or SSO link, so it never signs in interactively; it authenticates only
//! with tokens (API key rows on its user: hashed, 90-day default expiry,
//! last-used). Its access comes only from profile assignments
//! (`/access/assignments` with its user id); the owner is accountable but
//! lends no access. Tokens never hold MFA, except where MFA is turned off
//! for the server (D-064) or the token is elevated (D-097), as for API keys.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use forge_core::model::User;
use forge_core::permissions::{PermissionRepository, ServiceAccount};
use forge_core::ports::{ApiKeyRepository, UserRepository};
use forge_core::{Action, EntityKind, EntityRef, UserId, WorkspaceId};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::auth::{Caller, Ws};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// Most tokens one service account holds at a time.
const MAX_TOKENS: usize = 10;
/// Most tokens issued for one service account per hour.
const MAX_ISSUED_PER_HOUR: usize = 20;

fn ws_ref(ws: WorkspaceId) -> EntityRef {
    EntityRef::new(EntityKind::Workspace, ws)
}

fn user_ref(id: UserId) -> EntityRef {
    EntityRef::new(EntityKind::User, id)
}

/// `[a-z0-9-]`, 3 to 40 characters, not starting or ending with `-`.
fn valid_name(n: &str) -> bool {
    (3..=40).contains(&n.len())
        && n.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !n.starts_with('-')
        && !n.ends_with('-')
}

async fn load(state: &AppState, id: UserId) -> ApiResult<(ServiceAccount, User)> {
    let sa =
        state.store.get_service_account(id).await?.ok_or_else(|| ApiError::not_found("service account not found"))?;
    let user = state.store.get_user(id).await?;

    Ok((sa, user))
}

async fn view(state: &AppState, sa: &ServiceAccount, user: &User) -> Value {
    let owner = state.store.get_user(sa.owner_id).await.ok();
    let tokens = state.store.list_api_keys(user.id).await.unwrap_or_default();

    json!({
        "id": user.id,
        "name": user.username,
        "description": sa.description,
        "disabled": user.disabled,
        "owner_id": sa.owner_id,
        "owner": owner.as_ref().map(|o| o.username.clone().or_else(|| o.email.clone())),
        // The owner is accountable: flag one who is gone or disabled.
        "owner_inactive": owner.as_ref().is_none_or(|o| o.disabled),
        "tokens": tokens.len(),
        "last_used_at": tokens.iter().filter_map(|k| k.last_used_at).max(),
        "created_by": sa.created_by,
        "created_at": sa.created_at,
    })
}

/// `GET /service-accounts`.
pub async fn list(State(state): State<AppState>, caller: Caller, Ws(ws): Ws) -> ApiResult<Json<Vec<Value>>> {
    caller.authorize(Action::View, ws_ref(ws)).await?;
    let mut out = Vec::new();

    for sa in state.store.list_service_accounts().await? {
        if let Ok(user) = state.store.get_user(sa.user_id).await {
            out.push(view(&state, &sa, &user).await);
        }
    }

    Ok(Json(out))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NewServiceAccount {
    /// `[a-z0-9-]`, 3–40 characters; the account's username is `svc-<name>`.
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// The accountable person (username or email); default: the caller.
    #[serde(default)]
    pub owner: Option<String>,
}

/// `POST /service-accounts`.
pub async fn create(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Json(req): Json<NewServiceAccount>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    caller.authorize(Action::AccessManage, ws_ref(ws)).await?;

    if caller.via_api_key() {
        return Err(ApiError::forbidden("service accounts are created in the console or the CLI, not with an API key"));
    }

    let name = req.name.trim().to_ascii_lowercase();
    if !valid_name(&name) {
        return Err(ApiError::bad_request("name: 3–40 of a-z, 0-9 and -, not starting or ending with -"));
    }

    let description: String = req.description.trim().chars().filter(|c| !c.is_control()).take(500).collect();

    let owner = match req.owner.as_deref().map(str::trim).filter(|o| !o.is_empty()) {
        None => caller.session.user.clone(),
        Some(login) => {
            let l = login.to_lowercase();
            let found = if l.contains('@') {
                state.store.find_user_by_email(&l).await?
            } else {
                state.store.find_user_by_username(&l).await?
            };

            let o = found.ok_or_else(|| ApiError::not_found("no such owner"))?;
            if state.store.get_service_account(o.id).await?.is_some() {
                return Err(ApiError::bad_request("the owner must be a person"));
            }

            o
        }
    };

    let username = format!("svc-{name}");
    if state.store.find_user_by_username(&username).await?.is_some() {
        return Err(ApiError::conflict(format!("`{username}` exists")));
    }

    let now = state.clock.now();
    let user = User {
        id: UserId::new(),
        username: Some(username.clone()),
        email: None,
        display_name: username.clone(),
        // No password, passkey, authenticator or SSO link: never an
        // interactive sign-in.
        password_hash: None,
        totp_secret_sealed: None,
        oidc_subject: None,
        disabled: false,
        created_at: now,
    };
    let sa = ServiceAccount {
        user_id: user.id,
        owner_id: owner.id,
        description,
        created_by: caller.principal().to_string(),
        created_at: now,
    };

    // The user and its service-account record in one transaction.
    state.store.create_service_account(&user, &sa).await?;
    caller
        .audit("service_account.create", &user_ref(user.id), Some(ws), json!({ "name": username, "owner": owner.id }))
        .await;

    Ok((StatusCode::CREATED, Json(view(&state, &sa, &user).await)))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Change {
    #[serde(default)]
    pub disabled: Option<bool>,
}

/// `PATCH /service-accounts/{id}`: disable or enable. Disabling revokes
/// every token (enabling does not bring them back: issue new ones), ends its
/// sessions, and closes its terminals and event streams at once.
pub async fn update(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Path(id): Path<UserId>,
    Json(req): Json<Change>,
) -> ApiResult<Json<Value>> {
    caller.authorize(Action::AccessManage, ws_ref(ws)).await?;
    let (sa, _) = load(&state, id).await?;

    if let Some(d) = req.disabled {
        if !state.store.set_service_account_disabled(id, d).await? {
            return Err(ApiError::not_found("service account not found"));
        }

        let mut revoked = 0;

        if d {
            state.identity.revoke_all(id).await?;
            revoked = state.store.delete_user_api_keys(id).await?;
            state.remote_sessions.end_matching(|_, u| u == id);
            let _ = state.signed_out.send(id);
        }

        caller
            .audit(
                if d { "service_account.disable" } else { "service_account.enable" },
                &user_ref(id),
                Some(ws),
                json!({ "tokens_revoked": revoked }),
            )
            .await;
    }

    let user = state.store.get_user(id).await?;

    Ok(Json(view(&state, &sa, &user).await))
}

/// `GET /service-accounts/{id}/tokens`: names, prefixes, expiry and last use.
pub async fn tokens(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Path(id): Path<UserId>,
) -> ApiResult<Json<Value>> {
    caller.authorize(Action::View, ws_ref(ws)).await?;
    load(&state, id).await?;

    Ok(Json(json!(state.store.list_api_keys(id).await?)))
}

/// `POST /service-accounts/{id}/tokens`: the secret is in this answer
/// only. The token is a personal access token of the service account, the
/// same kind as `POST /api-keys` issues: the same body, `restrictions`
/// included, and it never acts with more than the account holds.
pub async fn issue_token(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Path(id): Path<UserId>,
    Json(req): Json<super::settings::NewKey>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    caller.authorize(Action::AccessManage, ws_ref(ws)).await?;

    if caller.via_api_key() {
        return Err(ApiError::forbidden("tokens are issued in the console or the CLI, not with an API key"));
    }

    let (_, user) = load(&state, id).await?;
    if user.disabled {
        return Err(ApiError::conflict("the service account is disabled"));
    }

    let existing = state.store.list_api_keys(id).await?;
    let hour_ago = state.clock.now().0 - 3_600_000;

    if existing.len() >= MAX_TOKENS {
        return Err(ApiError::conflict(format!("at most {MAX_TOKENS} tokens per service account; revoke one first")));
    }

    if existing.iter().filter(|k| k.created_at.0 > hour_ago).count() >= MAX_ISSUED_PER_HOUR {
        return Err(ApiError::new(StatusCode::TOO_MANY_REQUESTS, "throttled", "too many tokens issued this hour"));
    }

    if req.elevated {
        super::settings::check_elevated(&state, &caller, "service_account.token.create", &user_ref(id)).await?;
    }

    // Restrictions only narrow the account: every request is checked against
    // the intersection with what the account holds then.
    let (key, secret) = super::settings::mint_key(&state, id, &req).await?;
    caller
        .audit(
            "service_account.token.create",
            &user_ref(id),
            Some(ws),
            json!({
                "token": key.id, "token_id": key.id, "prefix": key.prefix,
                "expires_at": key.expires_at, "elevated": key.elevated,
                "restricted": key.restrictions.as_ref().map(Vec::len),
            }),
        )
        .await;
    let mut v = serde_json::to_value(&key).unwrap_or_default();
    v["secret"] = json!(secret);

    Ok((StatusCode::CREATED, Json(v)))
}

/// `DELETE /service-accounts/{id}/tokens/{token}`: no MFA needed.
pub async fn revoke_token(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Path((id, token)): Path<(UserId, String)>,
) -> ApiResult<StatusCode> {
    caller.authorize(Action::UserManage, ws_ref(ws)).await?;
    load(&state, id).await?;

    if !state.store.delete_api_key(id, &token).await? {
        return Err(ApiError::not_found("token not found"));
    }

    state.remote_sessions.end_api_key(&token);
    caller.audit("service_account.token.revoke", &user_ref(id), Some(ws), json!({ "token": token })).await;

    Ok(StatusCode::NO_CONTENT)
}
