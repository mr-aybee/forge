//! Proof of identity before a change to the caller's own sign-in factors
//! (authenticator app, passkeys) or sign-in names (username, email). A factor grants MFA sign-ins and step-up,
//! so a stolen session or token alone must never add, replace or remove one.
//!
//! * API keys: refused.
//! * The account has a factor: the session must have verified one itself
//!   (sign-in with a code or passkey, or step-up). `ctx().mfa` is not
//!   enough: with MFA turned off for the server it is true for every session.
//! * No factor yet: the current password, throttled. An account without a
//!   password (single sign-on only) instead needs a browser sign-in made
//!   within the last [`FRESH_SSO`].

use axum::http::StatusCode;
use forge_core::model::SessionKind;
use forge_core::{EntityKind, EntityRef};

use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// How recent a single sign-on sign-in must be to add a first factor.
pub const FRESH_SSO: std::time::Duration = std::time::Duration::from_secs(5 * 60);

/// Checks that the caller may change their own factors. `action` names the
/// audit action of denials (`auth.totp.enroll`, `auth.passkey.add`, ...).
pub async fn for_factor_change(
    state: &AppState,
    caller: &Caller,
    password: Option<&str>,
    action: &str,
) -> ApiResult<()> {
    let user = &caller.session.user;
    let me = EntityRef::new(EntityKind::User, user.id);
    let deny = |reason: &'static str| caller.audit_denied(action, &me, None, reason);
    if caller.via_api_key() {
        deny("API keys cannot change sign-in factors").await;
        return Err(ApiError::forbidden("sign-in factors are changed in the console, not with an API key"));
    }
    let has_passkeys = state
        .passkeys
        .has_passkeys(user.id)
        .await
        .map_err(|e| ApiError::from(forge_core::DomainError::internal(e.to_string())))?;
    if user.mfa_enabled() || has_passkeys {
        if caller.factor_verified() {
            return Ok(());
        }
        deny("a session that verified a factor is required").await;
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "step_up_required",
            "verify it's you with your code or passkey (step up) first",
        ));
    }
    if user.password_hash.is_none() {
        let fresh = caller.session.session.kind == SessionKind::Web
            && state.clock.now().0 - caller.session.session.created_at.0
                <= i64::try_from(FRESH_SSO.as_millis()).unwrap_or(i64::MAX);
        if fresh {
            return Ok(());
        }
        deny("single sign-on account: a fresh sign-in is required").await;
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "reauthentication_required",
            "sign out and sign in again with single sign-on, then add it within 5 minutes",
        ));
    }
    let throttle = format!("factor-change:{}", user.id);
    if !state.login_throttle.allowed(&throttle) {
        deny("throttled").await;
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "throttled",
            "too many attempts; try again in 15 minutes",
        ));
    }
    let login = user.username.clone().or_else(|| user.email.clone()).unwrap_or_default();
    let pw = password.unwrap_or_default();
    if pw.is_empty() || state.identity.login_password(&login, pw, None).await.is_err() {
        state.login_throttle.record_failure(&throttle);
        deny("current password required").await;
        return Err(ApiError::new(StatusCode::UNAUTHORIZED, "password_required", "enter your current password first"));
    }
    state.login_throttle.reset(&throttle);
    Ok(())
}
