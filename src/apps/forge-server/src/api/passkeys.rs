//! Passkeys (D-055).
//!
//! * `GET    /api/v1/me/passkeys` — your passkeys.
//! * `POST   /api/v1/me/passkeys/register/begin|finish` — add one (signed in).
//! * `PATCH  /api/v1/me/passkeys/{id}` `{name}`, `DELETE /api/v1/me/passkeys/{id}`.
//! * `POST   /api/v1/auth/passkey/begin|finish` — sign in with a passkey
//!   (no password); the session counts as MFA-verified.
//! * `POST   /api/v1/auth/step-up/passkey/begin|finish` — step up with a
//!   passkey instead of a one-time code.
//! * `DELETE /api/v1/users/{id}/passkeys` — recovery: an administrator
//!   (`user.manage`) removes someone's passkeys.
//!
//! Browsers only offer WebAuthn on HTTPS origins or `localhost`.

use axum::Json;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post};
use forge_core::model::{AuditEvent, AuditOutcome};
use forge_core::{Action, EntityKind, EntityRef, UserId};
use forge_identity::passkeys::{AssertionResponse, PasskeyError, RegistrationResponse};
use forge_identity::webauthn::RelyingParty;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::auth::{Caller, SESSION_COOKIE, Ws, cookie, session_cookies};
use crate::error::{ApiError, ApiResult};
use crate::request_origin::RequestOrigin;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/me/passkeys", get(list))
        .route("/me/passkeys/register/begin", post(register_begin))
        .route("/me/passkeys/register/finish", post(register_finish))
        .route("/me/passkeys/{id}", patch(rename).delete(remove))
        .route("/auth/passkey/begin", post(login_begin))
        .route("/auth/passkey/finish", post(login_finish))
        .route("/auth/step-up/passkey/begin", post(step_up_begin))
        .route("/auth/step-up/passkey/finish", post(step_up_finish))
        .route("/users/{id}/passkeys", delete(remove_all))
}

fn err(e: PasskeyError) -> ApiError {
    match e {
        PasskeyError::Domain(d) => d.into(),
        PasskeyError::Disabled => ApiError::forbidden("account disabled"),
        PasskeyError::Invalid(m) => ApiError::bad_request(m),
        other => ApiError::new(StatusCode::UNAUTHORIZED, "invalid_passkey", other.to_string()),
    }
}

fn with_cookies(mut resp: Response, cookies: [String; 2]) -> Response {
    for c in cookies {
        if let Ok(v) = HeaderValue::from_str(&c) {
            resp.headers_mut().append(header::SET_COOKIE, v);
        }
    }

    resp
}

async fn audit_anon(state: &AppState, actor: &str, action: &str, outcome: AuditOutcome, details: Value) {
    let ev = AuditEvent::new(actor, action, EntityRef::platform(), outcome).with_details(details);
    if let Err(e) = state.audit.append(ev).await {
        tracing::error!(error = %e, "audit write failed");
    }
}

/// The relying party of this request: its host and origin (D-167). A
/// passkey works on the name it was registered under.
fn relying_party(origin: &RequestOrigin) -> ApiResult<RelyingParty> {
    RelyingParty::from_public_url(&origin.origin()).map_err(|e| ApiError::bad_request(e.to_string()))
}

fn view(p: &forge_core::passkey::Passkey) -> Value {
    json!({ "id": p.id, "name": p.name, "created_at": p.created_at, "last_used_at": p.last_used_at })
}

/// Your passkeys (names and dates; never key material).
pub async fn list(State(state): State<AppState>, caller: Caller) -> ApiResult<Json<Vec<Value>>> {
    let keys = state.passkeys.list(caller.session.user.id).await.map_err(err)?;
    Ok(Json(keys.iter().map(view).collect()))
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct RegisterBegin {
    /// Required when the account has no MFA yet (no authenticator app and no
    /// passkey): proves the person, not just a stolen session, adds the key.
    #[serde(default)]
    pub password: Option<String>,
}

/// Options for adding a passkey to your account. With MFA already set up,
/// the session must be MFA-verified; otherwise the current password is
/// required. A passkey grants MFA sign-ins, so a stolen session alone must
/// never be able to add one.
pub async fn register_begin(
    State(state): State<AppState>,
    caller: Caller,
    origin: RequestOrigin,
    body: Option<Json<RegisterBegin>>,
) -> ApiResult<Json<Value>> {
    let password = body.and_then(|Json(b)| b.password);
    super::reauth::for_factor_change(&state, &caller, password.as_deref(), "auth.passkey.add").await?;
    let user = &caller.session.user;
    let me = EntityRef::new(EntityKind::User, user.id);
    let options = state.passkeys.register_begin(user, &relying_party(&origin)?).await.map_err(err)?;
    caller.audit("auth.passkey.add_begin", &me, None, json!({})).await;

    Ok(Json(options))
}

/// Verifies and stores a new passkey.
pub async fn register_finish(
    State(state): State<AppState>,
    caller: Caller,
    origin: RequestOrigin,
    Json(req): Json<RegistrationResponse>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let user = &caller.session.user;

    if caller.via_api_key() {
        return Err(ApiError::forbidden("passkeys are added in the console"));
    }

    match state.passkeys.register_finish(user, &req, &relying_party(&origin)?).await {
        Ok(p) => {
            caller
                .audit(
                    "auth.passkey.add",
                    &EntityRef::new(EntityKind::User, user.id),
                    None,
                    json!({ "passkey": p.id, "name": p.name }),
                )
                .await;
            Ok((StatusCode::CREATED, Json(view(&p))))
        }
        Err(e) => {
            caller
                .audit_denied("auth.passkey.add", &EntityRef::new(EntityKind::User, user.id), None, &e.to_string())
                .await;
            Err(err(e))
        }
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct Rename {
    pub name: String,
}

/// Renames one of your passkeys.
pub async fn rename(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
    Json(req): Json<Rename>,
) -> ApiResult<StatusCode> {
    let user = caller.session.user.id;
    if state.passkeys.rename(user, &id, &req.name).await.map_err(err)? {
        caller
            .audit(
                "auth.passkey.rename",
                &EntityRef::new(EntityKind::User, user),
                None,
                json!({ "passkey": id, "name": req.name.trim() }),
            )
            .await;
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found("passkey not found"))
    }
}

/// Removes one of your passkeys.
/// Needs a session that verified a factor itself (see `reauth`): a stolen
/// session must not strip the owner's second factor.
pub async fn remove(State(state): State<AppState>, caller: Caller, Path(id): Path<String>) -> ApiResult<StatusCode> {
    super::reauth::for_factor_change(&state, &caller, None, "auth.passkey.remove").await?;
    let user = caller.session.user.id;
    if !state.passkeys.remove(user, &id).await.map_err(err)? {
        caller.audit_denied("auth.passkey.remove", &EntityRef::new(EntityKind::User, user), None, "not found").await;
        return Err(ApiError::not_found("passkey not found"));
    }

    caller.audit("auth.passkey.remove", &EntityRef::new(EntityKind::User, user), None, json!({ "passkey": id })).await;

    Ok(StatusCode::NO_CONTENT)
}

/// Sign-in ceremonies started per 10 seconds, across all clients: each one
/// stores a challenge, so an unauthenticated flood must stay cheap.
const BEGINS_PER_WINDOW: u32 = 200;

fn begin_allowed() -> bool {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    static W: Mutex<Option<(Instant, u32)>> = Mutex::new(None);
    let Ok(mut w) = W.lock() else { return false };
    let now = Instant::now();

    match w.as_mut() {
        Some((start, n)) if now.duration_since(*start) < Duration::from_secs(10) => {
            *n += 1;
            *n <= BEGINS_PER_WINDOW
        }
        _ => {
            *w = Some((now, 1));
            true
        }
    }
}

/// Options for signing in with a passkey.
pub async fn login_begin(State(state): State<AppState>, origin: RequestOrigin) -> ApiResult<Json<Value>> {
    if !begin_allowed() {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "throttled",
            "too many sign-ins right now; try again",
        ));
    }

    Ok(Json(state.passkeys.login_begin(&relying_party(&origin)?).await.map_err(err)?))
}

/// Base64url of the longest credential id the spec allows (1023 bytes).
const MAX_CREDENTIAL_ID_B64: usize = 1366;

/// Signs in with a passkey: an MFA-verified session. Every failure gives the
/// same answer, so nothing is learnt about credentials or accounts.
pub async fn login_finish(
    State(state): State<AppState>,
    crate::auth::SecureCookies(secure): crate::auth::SecureCookies,
    origin: RequestOrigin,
    Json(req): Json<AssertionResponse>,
) -> ApiResult<Response> {
    let generic = || ApiError::new(StatusCode::UNAUTHORIZED, "invalid_passkey", "passkey sign-in failed; try again");

    if req.credential_id.is_empty() || req.credential_id.len() > MAX_CREDENTIAL_ID_B64 {
        return Err(generic());
    }

    let key = format!("passkey:{}", req.credential_id);
    if !state.login_throttle.allowed(&key) {
        return Err(ApiError::new(StatusCode::TOO_MANY_REQUESTS, "throttled", "too many failed attempts; try later"));
    }

    let (user, passkey) = match state.passkeys.login_finish(&req, &relying_party(&origin)?).await {
        Ok(u) => u,
        Err(e) => {
            // Only a failure after a valid challenge counts against the
            // credential, so junk requests cannot lock someone out.
            if matches!(
                e,
                PasskeyError::WebAuthn(
                    forge_identity::webauthn::WebAuthnError::BadSignature
                        | forge_identity::webauthn::WebAuthnError::CounterRegression
                )
            ) {
                state.login_throttle.record_failure(&key);
            }

            let cred: String = req.credential_id.chars().take(24).collect();
            audit_anon(
                &state,
                &format!("passkey:{cred}"),
                "auth.login",
                AuditOutcome::Denied,
                json!({ "method": "passkey", "credential": cred, "reason": e.to_string() }),
            )
            .await;

            return Err(match e {
                PasskeyError::Domain(d) if !matches!(d, forge_core::DomainError::NotFound(_)) => d.into(),
                _ => generic(),
            });
        }
    };

    state.login_throttle.reset(&key);
    crate::access::refuse_service_account(&state, user.id).await?;
    let tokens = state.identity.create_web_session(user.id, true).await?;
    audit_anon(
        &state,
        &format!("user:{}", user.id),
        "auth.login",
        AuditOutcome::Success,
        json!({ "method": "passkey", "passkey": passkey }),
    )
    .await;
    super::session::record_admin_address(&state, user.id, &origin).await;
    let ttl = state.identity.config().web_session_ttl.as_secs();
    let body = Json(json!({ "user": user, "mfa": true, "csrf_token": tokens.csrf_token })).into_response();

    Ok(with_cookies(body, session_cookies(&tokens.session_token, &tokens.csrf_token, ttl, secure)))
}

/// Options for stepping up with one of your passkeys.
pub async fn step_up_begin(
    State(state): State<AppState>,
    caller: Caller,
    origin: RequestOrigin,
) -> ApiResult<Json<Value>> {
    if caller.via_api_key() {
        return Err(ApiError::forbidden("API keys cannot be stepped up"));
    }

    Ok(Json(state.passkeys.step_up_begin(&caller.session.user, &relying_party(&origin)?).await.map_err(err)?))
}

/// Steps up with a passkey: same result as a one-time code.
pub async fn step_up_finish(
    State(state): State<AppState>,
    crate::auth::SecureCookies(secure): crate::auth::SecureCookies,
    caller: Caller,
    origin: RequestOrigin,
    headers: HeaderMap,
    Json(req): Json<AssertionResponse>,
) -> ApiResult<Response> {
    let user = &caller.session.user;

    if caller.via_api_key() {
        return Err(ApiError::forbidden("API keys cannot be stepped up"));
    }

    let key = format!("step-up:{}", user.id);
    if !state.login_throttle.allowed(&key) {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "throttled",
            "too many attempts; try again in 15 minutes",
        ));
    }

    let passkey = match state.passkeys.step_up_finish(user, &req, &relying_party(&origin)?).await {
        Ok(p) => p,
        Err(e) => {
            state.login_throttle.record_failure(&key);
            caller.audit_denied("auth.step_up", &EntityRef::new(EntityKind::User, user.id), None, &e.to_string()).await;

            return Err(err(e));
        }
    };

    state.login_throttle.reset(&key);

    // Apps and the CLI: a new token pair with MFA replaces the bearer session
    // (as a code step-up does), instead of a second, browser session.
    if caller.session.session.kind == forge_core::model::SessionKind::Bearer {
        let tokens = state.identity.step_up_bearer(&caller.session.session).await?;
        caller
            .audit(
                "auth.step_up",
                &EntityRef::new(EntityKind::User, user.id),
                None,
                json!({ "method": "passkey", "passkey": passkey, "session": "bearer" }),
            )
            .await;
        let mut resp = Json(tokens).into_response();
        resp.headers_mut().insert(axum::http::header::CACHE_CONTROL, axum::http::HeaderValue::from_static("no-store"));

        return Ok(resp);
    }

    let tokens = state.identity.create_web_session(user.id, true).await?;

    if let Some(old) = cookie(&headers, SESSION_COOKIE) {
        state.identity.logout(old).await?;
    }

    caller
        .audit(
            "auth.step_up",
            &EntityRef::new(EntityKind::User, user.id),
            None,
            json!({ "method": "passkey", "passkey": passkey }),
        )
        .await;
    let ttl = state.identity.config().web_session_ttl.as_secs();
    let body = Json(json!({ "mfa": true, "csrf_token": tokens.csrf_token })).into_response();

    Ok(with_cookies(body, session_cookies(&tokens.session_token, &tokens.csrf_token, ttl, secure)))
}

/// Recovery: removes every passkey of a user and signs them out everywhere.
/// Platform-wide `user.manage` (accounts are platform-wide: a workspace
/// administrator must not strip a platform administrator's keys) and an
/// MFA session.
pub async fn remove_all(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Path(id): Path<UserId>,
) -> ApiResult<Json<Value>> {
    use forge_core::ports::UserRepository;
    let target = EntityRef::new(EntityKind::User, id);
    caller.authorize(Action::UserManage, EntityRef::platform()).await?;

    if !caller.ctx().mfa {
        caller.audit_denied("auth.passkey.remove_all", &target, Some(ws), "an MFA session is required").await;
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "step_up_required",
            "removing someone's passkeys needs an MFA session",
        ));
    }

    state.store.get_user(id).await.map_err(|_| ApiError::not_found("user not found"))?;
    let n = state.passkeys.remove_all(id).await.map_err(err)?;

    // Sessions created with a removed (perhaps stolen) passkey end too, and
    // the API keys such a session may have made.
    state.identity.revoke_all(id).await?;
    let keys = forge_core::ports::ApiKeyRepository::delete_user_api_keys(&state.store, id).await?;
    state.remote_sessions.end_user_api_keys(id);
    caller
        .audit(
            "auth.passkey.remove_all",
            &target,
            Some(ws),
            json!({ "removed": n, "sessions_revoked": true, "api_keys_revoked": keys }),
        )
        .await;

    Ok(Json(json!({ "removed": n, "api_keys_revoked": keys })))
}
