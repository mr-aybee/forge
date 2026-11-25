//! Request authentication and the per-request enforcement helper.
//!
//! [`Caller`] accepts either a bearer token (apps, CLI) or the
//! `forge_session` cookie (browser). Cookie-authenticated requests that can
//! change state must carry a matching `X-CSRF-Token` header.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{HeaderMap, Method, header};
use forge_core::model::{AuditEvent, AuditOutcome, SessionKind};
use forge_core::{AccessRequest, Action, AuthzContext, EntityRef, Principal, WorkspaceId};
use forge_identity::{AuthenticatedSession, IdentityService};
use serde_json::Value;

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

pub const SESSION_COOKIE: &str = "forge_session";
pub const CSRF_COOKIE: &str = "forge_csrf";
pub const CSRF_HEADER: &str = "x-csrf-token";

/// Marks a request the MCP endpoint dispatched in process for a connector's
/// connection. Only server code can insert a request extension, so a
/// connector token is refused on every request that did not come from `/mcp`.
#[derive(Debug, Clone)]
pub struct McpDispatch {
    pub connection_id: String,
}

/// The authenticated caller of an API request.
#[derive(Debug, Clone)]
pub struct Caller {
    pub session: AuthenticatedSession,
    state: AppState,
    /// `via` for the audit log when an approved request runs (D-073).
    via: Option<Value>,
    /// The session really verified a second factor (TOTP or passkey at
    /// sign-in or step-up). Unlike `ctx().mfa`, never set by turning MFA
    /// off for the server.
    factor_verified: bool,
    /// Authenticated with an elevated token that counts as MFA (D-097).
    elevated_token: bool,
    /// The token's ceiling when it is a restricted token (#13): every
    /// decision is the owner's decision AND this list.
    ceiling: Option<std::sync::Arc<Vec<forge_core::permissions::TokenGrant>>>,
}

/// Marks a request replayed from an approval: its audit names the approval
/// and the connector that asked. Only server code can insert it.
#[derive(Debug, Clone)]
pub struct ApprovalExec {
    pub approval_id: String,
    pub requester: String,
}

impl Caller {
    /// Whether this request authenticated with a personal API key.
    #[must_use]
    pub fn via_api_key(&self) -> bool {
        self.session.session.id_hash.starts_with("apikey:")
    }

    /// Whether this request is an approved connector request being replayed
    /// with the approver's session (D-073): the approver's roles apply, and
    /// the audit names the connector. What was handed to the approver in
    /// person, such as a bucket grant, does not.
    #[must_use]
    pub fn approved_replay(&self) -> bool {
        self.via.is_some()
    }

    #[must_use]
    pub fn principal(&self) -> Principal {
        self.session.principal()
    }

    /// Whether this request authenticated with an elevated token that the
    /// policy honours (`[security] allow_elevated_tokens`, D-097).
    #[must_use]
    pub fn elevated_token(&self) -> bool {
        self.elevated_token
    }

    /// Whether this session proved a second factor itself. Use it to guard
    /// changes to the account's own factors: with MFA turned off for the
    /// server, `ctx().mfa` is true for every session.
    #[must_use]
    pub fn factor_verified(&self) -> bool {
        self.factor_verified
    }

    #[must_use]
    pub fn ctx(&self) -> AuthzContext {
        AuthzContext { mfa: self.session.session.mfa }
    }

    /// Enforcement point: authorizes `action` on `resource`; denials are audited.
    pub async fn authorize(&self, action: Action, resource: EntityRef) -> ApiResult<()> {
        self.authorize_with(action, resource, Value::Null).await
    }

    /// [`Caller::authorize`], with `denied` as the details of the audit
    /// event when the request is refused: what the resource alone does not
    /// say (for example the store and the environment that owns it).
    pub async fn authorize_with(&self, action: Action, resource: EntityRef, denied: Value) -> ApiResult<()> {
        let req = AccessRequest::new(self.principal(), action, resource.clone(), self.ctx());
        match self.state.authz.authorize(&req).await {
            Ok(()) if self.within_ceiling(action, std::slice::from_ref(&resource)).await[0] => Ok(()),
            Ok(()) => {
                self.record(action.as_str(), &resource, None, AuditOutcome::Denied, denied).await;
                Err(forge_core::DomainError::Denied(format!(
                    "{action} on {resource}: outside what this token was given"
                ))
                .into())
            }
            Err(e) => {
                self.record(action.as_str(), &resource, None, AuditOutcome::Denied, denied).await;
                Err(e.into())
            }
        }
    }

    /// Whether this request's token, if restricted, covers `action` on each
    /// of `resources` (#13). Unrestricted callers: all true. A resource
    /// that cannot be resolved is outside (fails closed).
    async fn within_ceiling(&self, action: Action, resources: &[EntityRef]) -> Vec<bool> {
        let Some(ceiling) = &self.ceiling else { return vec![true; resources.len()] };
        use forge_core::ports::ResourceResolver;

        self.state
            .store
            .resolve_many(resources)
            .await
            .into_iter()
            .map(|r| r.is_ok_and(|r| forge_core::permissions::ceiling_allows(ceiling, action, &r)))
            .collect()
    }

    /// The token's ceiling, when this is a restricted token.
    #[must_use]
    pub fn ceiling(&self) -> Option<&[forge_core::permissions::TokenGrant]> {
        self.ceiling.as_deref().map(Vec::as_slice)
    }

    /// Non-failing check used to filter lists.
    pub async fn can(&self, action: Action, resource: EntityRef) -> bool {
        self.state.authz.is_allowed(&AccessRequest::new(self.principal(), action, resource.clone(), self.ctx())).await
            && self.within_ceiling(action, std::slice::from_ref(&resource)).await[0]
    }

    /// [`Caller::can`] for many resources at once (list endpoints).
    /// Which of `actions` the caller holds over `scope` (and everything
    /// beneath it) right now: the escalation guard's test (#13).
    pub async fn holds_over(&self, actions: &[Action], scope: &EntityRef) -> Vec<bool> {
        let reqs: Vec<AccessRequest> =
            actions.iter().map(|a| AccessRequest::new(self.principal(), *a, scope.clone(), self.ctx())).collect();
        let mut held = self.state.authz.holds_over(&reqs).await;

        for (i, a) in actions.iter().enumerate() {
            if held[i] {
                held[i] = self.within_ceiling(*a, std::slice::from_ref(scope)).await[0];
            }
        }

        held
    }

    pub async fn can_many(&self, action: Action, resources: Vec<EntityRef>) -> Vec<bool> {
        let within = self.within_ceiling(action, &resources).await;
        let reqs: Vec<AccessRequest> =
            resources.into_iter().map(|r| AccessRequest::new(self.principal(), action, r, self.ctx())).collect();
        let allowed = self.state.authz.is_allowed_many(&reqs).await;

        allowed.into_iter().zip(within).map(|(a, w)| a && w).collect()
    }

    /// Records a successful mutating action.
    pub async fn audit(&self, action: &str, resource: &EntityRef, org: Option<WorkspaceId>, details: Value) {
        self.record(action, resource, org, AuditOutcome::Success, details).await;
    }

    /// Requires an MFA-verified session (or MFA turned off for the server)
    /// for an action that hands out access, such as granting a role: a
    /// leaked API key or a session without MFA must not create a new
    /// identity that then passes the MFA guardrail with its own factor.
    /// Denials are audited as `action`.
    pub async fn require_mfa(&self, action: &str, resource: &EntityRef) -> ApiResult<()> {
        if self.ctx().mfa {
            return Ok(());
        }

        self.audit_denied(action, resource, None, "an MFA-verified session is required").await;

        Err(ApiError::new(
            axum::http::StatusCode::FORBIDDEN,
            "step_up_required",
            "verify it's you with your code or passkey (step up); set up MFA in Profile → Password & security (/profile/security)",
        ))
    }

    /// Records a refused mutating action with the reason.
    pub async fn audit_denied(&self, action: &str, resource: &EntityRef, org: Option<WorkspaceId>, reason: &str) {
        self.record(action, resource, org, AuditOutcome::Denied, serde_json::json!({ "reason": reason })).await;
    }

    /// Records an allowed action that failed where it took effect (e.g. a node
    /// refused it), with the error.
    pub async fn audit_failed(
        &self,
        action: &str,
        resource: &EntityRef,
        org: Option<WorkspaceId>,
        details: Value,
        error: &str,
    ) {
        let mut details = if details.is_object() { details } else { serde_json::json!({}) };
        details["error"] = Value::from(error);
        self.record(action, resource, org, AuditOutcome::Failure, details).await;
    }

    async fn record(
        &self,
        action: &str,
        resource: &EntityRef,
        org: Option<WorkspaceId>,
        outcome: AuditOutcome,
        details: Value,
    ) {
        let mut details = if details.is_null() { Value::Object(serde_json::Map::new()) } else { details };

        // Calls made by an AI connector name it, and its mode, in the audit log.
        if let (Some(c), Value::Object(m)) = (&self.session.connection, &mut details) {
            m.insert(
                "via".into(),
                serde_json::json!({ "connector": c.client_name, "client_id": c.client_id, "connection": c.id, "mode": c.mode }),
            );
        }

        if let (Some(v), Value::Object(m)) = (&self.via, &mut details) {
            m.insert("via".into(), v.clone());
        }

        // Calls made with an API key or service-account token name it.
        if let (Some(id), Value::Object(m)) = (self.session.session.id_hash.strip_prefix("apikey:"), &mut details) {
            m.entry("token_id").or_insert_with(|| Value::from(id));
            m.insert("elevated".into(), Value::from(self.elevated_token));
        }

        let ev = AuditEvent::new(self.principal(), action, resource, outcome).in_org(org).with_details(details);
        if let Err(e) = self.state.audit.append(ev).await {
            tracing::error!(error = %e, "audit write failed");
        }
    }
}

/// Reads one cookie value from the request headers.
#[must_use]
pub fn cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find_map(|(k, v)| (k == name).then_some(v))
}

pub(crate) fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers.get(header::AUTHORIZATION)?.to_str().ok()?.strip_prefix("Bearer ").map(str::trim)
}

fn is_safe(method: &Method) -> bool {
    matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

impl FromRequestParts<AppState> for Caller {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        if let Some(token) = bearer(&parts.headers)
            && token.starts_with("fk_")
        {
            return api_key_caller(state, token).await;
        }

        if let Some(token) = bearer(&parts.headers) {
            let mut session =
                state.identity.authenticate(token, SessionKind::Bearer).await?.ok_or_else(ApiError::unauthorized)?;
            let factor_verified = session.session.mfa && session.connection.is_none();

            if let Some(c) = &session.connection {
                // RFC 8707: a connector's token is for /mcp only. Its calls
                // reach handlers solely through the in-process MCP dispatch,
                // which marks them with an extension HTTP cannot set.
                let dispatched = parts.extensions.get::<McpDispatch>().is_some_and(|d| d.connection_id == c.id);
                if !dispatched {
                    return Err(ApiError::new(
                        axum::http::StatusCode::UNAUTHORIZED,
                        "invalid_token",
                        "this token is for the MCP endpoint only",
                    ));
                }

                // Agents never hold MFA, not even with the development toggle.
                session.session.mfa = false;
            } else {
                session.session.mfa |= !state.config.mfa_required();
            }

            return Ok(Self {
                session,
                state: state.clone(),
                via: None,
                factor_verified,
                elevated_token: false,
                ceiling: None,
            });
        }

        let token = cookie(&parts.headers, SESSION_COOKIE).ok_or_else(ApiError::unauthorized)?;
        let mut session =
            state.identity.authenticate(token, SessionKind::Web).await?.ok_or_else(ApiError::unauthorized)?;
        let factor_verified = session.session.mfa;

        // Local development only (refused at start-up for non-loopback URLs).
        session.session.mfa |= !state.config.mfa_required();

        if !is_safe(&parts.method) {
            let header = parts.headers.get(CSRF_HEADER).and_then(|v| v.to_str().ok());
            if !IdentityService::verify_csrf(&session, header) {
                return Err(ApiError::forbidden("missing or invalid CSRF token"));
            }
        }

        let via = parts
            .extensions
            .get::<ApprovalExec>()
            .map(|e| serde_json::json!({ "approval": e.approval_id, "connector": e.requester }));

        Ok(Self { session, state: state.clone(), via, factor_verified, elevated_token: false, ceiling: None })
    }
}

/// `Set-Cookie` values for a new web session.
#[must_use]
pub fn session_cookies(session_token: &str, csrf_token: &str, max_age_secs: u64, secure: bool) -> [String; 2] {
    let sec = if secure { "; Secure" } else { "" };
    [
        format!("{SESSION_COOKIE}={session_token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={max_age_secs}{sec}"),
        // Readable by the UI so it can echo it in X-CSRF-Token.
        format!("{CSRF_COOKIE}={csrf_token}; Path=/; SameSite=Strict; Max-Age={max_age_secs}{sec}"),
    ]
}

#[must_use]
pub fn clear_cookies(secure: bool) -> [String; 2] {
    let sec = if secure { "; Secure" } else { "" };
    [
        format!("{SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0{sec}"),
        format!("{CSRF_COOKIE}=; Path=/; SameSite=Strict; Max-Age=0{sec}"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cookies() {
        let mut h = HeaderMap::new();
        h.insert(header::COOKIE, "a=1; forge_session=abc; b=2".parse().unwrap_or_else(|_| unreachable!()));
        assert_eq!(cookie(&h, SESSION_COOKIE), Some("abc"));
        assert_eq!(cookie(&h, "missing"), None);
    }

    #[test]
    fn only_allowed_elevated_tokens_count_as_mfa() {
        assert!(token_mfa(false, false, false), "MFA off: every key");
        assert!(token_mfa(true, true, true), "elevated and allowed");
        assert!(!token_mfa(true, false, true), "a normal key needs step-up");
        assert!(!token_mfa(true, true, false), "the policy downgrades elevated keys");
    }

    #[test]
    fn session_cookie_is_httponly_strict() {
        let [s, c] = session_cookies("t", "c", 60, true);
        assert!(s.contains("HttpOnly") && s.contains("SameSite=Strict") && s.contains("Secure"));
        assert!(!c.contains("HttpOnly"));
    }
}

/// The deployment's workspace. One Forge deployment is one workspace
/// (DECISIONS.md D-016), so it is resolved implicitly instead of being part
/// of every URL. It is created by `forge-server init`.
#[derive(Debug, Clone, Copy)]
pub struct Ws(pub WorkspaceId);

impl FromRequestParts<AppState> for Ws {
    type Rejection = ApiError;

    async fn from_request_parts(_parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        let all = forge_core::ports::TenancyRepository::list_workspaces(&state.store).await?;
        match all.as_slice() {
            [only] => Ok(Self(only.id)),
            [] => Err(ApiError::not_found("no workspace: run `forge-server init` first")),
            // Fail closed: never guess which workspace a request means
            // (#13 inventory). Nothing in Forge creates a second one.
            more => {
                tracing::error!(count = more.len(), "more than one workspace in the store (D-016 allows one)");
                Err(ApiError::new(
                    axum::http::StatusCode::SERVICE_UNAVAILABLE,
                    "multiple_workspaces",
                    "this deployment has more than one workspace, which Forge does not support (D-016); ask the operator to check the database",
                ))
            }
        }
    }
}

/// Authenticates `Authorization: Bearer fk_…` (personal API key or
/// service-account token). The key acts as its owner without a second
/// factor: where MFA is required, MFA-guarded actions (terminal, package
/// installs, node onboarding…) are possible only with a key marked elevated,
/// and only while `[security] allow_elevated_tokens` is on (D-097); where the
/// operator turned MFA off, a key counts as verified like a browser session
/// (D-064). Connector tokens never do (D-063).
async fn api_key_caller(state: &AppState, token: &str) -> Result<Caller, ApiError> {
    use forge_core::ports::{ApiKeyRepository, UserRepository};
    let hash = forge_secrets::crypto::sha256_hex(token.as_bytes());
    let key = state.store.api_key_by_hash(&hash).await?.ok_or_else(ApiError::unauthorized)?;
    let now = state.clock.now();
    if key.expires_at.is_some_and(|e| e <= now) {
        return Err(ApiError::unauthorized());
    }

    let user = state.store.get_user(key.user_id).await.map_err(|_| ApiError::unauthorized())?;
    if user.disabled {
        return Err(ApiError::unauthorized());
    }

    let _ = state.store.touch_api_key(&key.id, now).await;
    let elevated = key.elevated && state.config.elevated_tokens_allowed();
    let session = forge_core::model::Session {
        // Marks the caller as an API key (see `Caller::via_api_key`).
        id_hash: format!("apikey:{}", key.id),
        user_id: user.id,
        kind: SessionKind::Bearer,
        csrf_hash: None,
        refresh_hash: None,
        mfa: token_mfa(state.config.mfa_required(), key.elevated, state.config.elevated_tokens_allowed()),
        created_at: key.created_at,
        expires_at: key.expires_at.unwrap_or(forge_core::Timestamp(i64::MAX)),
        connection_id: None,
    };

    Ok(Caller {
        session: AuthenticatedSession { user, session, connection: None },
        state: state.clone(),
        via: None,
        // Never a real second factor: the audit shows "token (elevated)".
        factor_verified: false,
        elevated_token: elevated,
        ceiling: key.restrictions.map(std::sync::Arc::new),
    })
}

/// A caller for a webhook delivery (D-159): the webhook's creator, never
/// MFA-verified (MFA-guarded actions are refused), audited as the webhook.
/// `None` when the creator is gone or disabled.
pub(crate) async fn webhook_caller(
    state: &AppState,
    creator: &Principal,
    webhook: forge_core::WebhookId,
) -> Result<Option<Caller>, ApiError> {
    use forge_core::ports::UserRepository;
    let Principal::User(user_id) = creator;
    let Ok(user) = state.store.get_user(*user_id).await else { return Ok(None) };
    if user.disabled {
        return Ok(None);
    }

    let now = state.clock.now();
    let session = forge_core::model::Session {
        id_hash: format!("webhook:{webhook}"),
        user_id: user.id,
        kind: SessionKind::Bearer,
        csrf_hash: None,
        refresh_hash: None,
        mfa: false,
        created_at: now,
        expires_at: now,
        connection_id: None,
    };

    Ok(Some(Caller {
        session: AuthenticatedSession { user, session, connection: None },
        state: state.clone(),
        via: Some(serde_json::json!({ "webhook": webhook })),
        factor_verified: false,
        elevated_token: false,
        ceiling: None,
    }))
}

/// Whether an API key counts as MFA-verified: always where MFA is off for
/// the server, otherwise only an elevated key while the policy allows them.
#[must_use]
pub fn token_mfa(mfa_required: bool, elevated: bool, allow_elevated: bool) -> bool {
    !mfa_required || (elevated && allow_elevated)
}

/// Whether the sign-in behind `caller` still exists and has not expired:
/// the session row (or the API key) is there, and the user is not
/// disabled. Long-lived connections (events, terminals) re-check it.
pub async fn still_signed_in(state: &AppState, caller: &Caller) -> bool {
    use forge_core::ports::{ApiKeyRepository, SessionRepository, UserRepository};
    let now = state.clock.now();
    let s = &caller.session.session;
    if s.expires_at <= now {
        return false;
    }

    if !matches!(state.store.get_user(s.user_id).await, Ok(u) if !u.disabled) {
        return false;
    }

    if let Some(key_id) = s.id_hash.strip_prefix("apikey:") {
        return matches!(state.store.list_api_keys(s.user_id).await, Ok(keys) if keys.iter().any(|k| k.id == key_id));
    }

    matches!(state.store.get_session(&s.id_hash).await, Ok(Some(row)) if row.expires_at > now)
}

/// Whether this response's session cookies get `Secure`:
/// `cookie_secure`, or HTTPS reported by a trusted proxy
/// (see [`crate::public_address::cookies_secure`]).
#[derive(Debug, Clone, Copy)]
pub struct SecureCookies(pub bool);

impl FromRequestParts<AppState> for SecureCookies {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        // Without the TCP peer (in-process dispatch) only the config counts.
        let secure = parts.extensions.get::<axum::extract::ConnectInfo<std::net::SocketAddr>>().map_or(
            state.config.cookie_secure,
            |ci| {
                crate::public_address::cookies_secure(
                    state.config.cookie_secure,
                    &parts.headers,
                    ci.0,
                    &state.config.trusted_proxies(),
                )
            },
        );
        Ok(Self(secure))
    }
}
