//! Login, logout, current user, TOTP enrollment, OAuth 2.1 + PKCE for apps,
//! and OIDC SSO.

use axum::extract::{ConnectInfo, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::{Form, Json};
use forge_core::model::{AuditEvent, AuditOutcome, SessionKind};
use forge_core::ports::RoleBindingRepository;
use forge_core::{EntityKind, EntityRef};
use forge_identity::LoginOutcome;
use serde::Deserialize;
use serde_json::json;

use crate::auth::{Caller, SESSION_COOKIE, clear_cookies, cookie, session_cookies};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct LoginRequest {
    /// Username or email (`email` is accepted for older clients).
    #[serde(alias = "email")]
    pub login: String,
    pub password: String,
    #[serde(default)]
    pub totp: Option<String>,
    /// Login challenge (`GET /auth/challenge`) and its solution.
    #[serde(default)]
    pub challenge_id: Option<String>,
    #[serde(default)]
    pub nonce: Option<String>,
    /// Image CAPTCHA (`GET /auth/captcha`) and the characters it shows,
    /// needed after failed sign-ins.
    #[serde(default)]
    pub captcha_id: Option<String>,
    #[serde(default)]
    pub captcha_answer: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ChallengeQuery {
    /// The sign-in name: a name with recent failures gets a harder challenge.
    #[serde(default)]
    pub login: Option<String>,
}

/// `GET /auth/challenge`: a proof-of-work challenge for the next sign-in
/// (D-064). `{required: false}` when the server does not ask for one.
pub async fn challenge(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>,
    headers: HeaderMap,
    Query(q): Query<ChallengeQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    if !state.config.security.login_challenge {
        return Ok(Json(json!({ "required": false })));
    }
    let ip = crate::throttle::client_ip(&headers, peer, &state.config.trusted_proxies());
    let c = state.login_challenges.issue(q.login.as_deref(), Some(ip)).ok_or_else(|| {
        ApiError::new(StatusCode::TOO_MANY_REQUESTS, "throttled", "too many sign-in attempts; try later")
    })?;
    let expires_at = state.clock.now().0 + i64::try_from(c.expires_in.as_millis()).unwrap_or(120_000);
    Ok(Json(json!({
        "required": true,
        "id": c.id,
        "algorithm": "sha256-leading-zero-bits",
        "salt": c.salt_hex,
        "difficulty": c.bits,
        "expires_at": expires_at,
    })))
}

/// `GET /auth/captcha`: whether the next sign-in as `login` from this
/// address needs the image CAPTCHA and, if so, a new picture (a PNG data
/// URL; the characters are never in the response as text).
pub async fn captcha(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>,
    headers: HeaderMap,
    Query(q): Query<ChallengeQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let ip = crate::throttle::client_ip(&headers, peer, &state.config.trusted_proxies());
    let threshold = state.config.security.captcha_after_failures;
    if !state.captchas.required(q.login.as_deref(), ip, threshold) {
        return Ok(Json(json!({ "required": false })));
    }
    let c = state.captchas.issue(ip).ok_or_else(|| {
        ApiError::new(StatusCode::TOO_MANY_REQUESTS, "throttled", "too many sign-in attempts; try later")
    })?;
    use base64::Engine as _;
    let image = format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(&c.png));
    let expires_at = state.clock.now().0 + i64::try_from(c.expires_in.as_millis()).unwrap_or(120_000);
    Ok(Json(json!({
        "required": true,
        "id": c.id,
        "image": image,
        "length": crate::captcha::LEN,
        "expires_at": expires_at,
    })))
}

/// Checks the image CAPTCHA when this sign-in needs one. Every answer uses
/// the picture up; the client fetches a new one.
fn check_captcha(state: &AppState, req: &LoginRequest, key: &str, ip: std::net::IpAddr) -> ApiResult<()> {
    if !state.captchas.required(Some(key), ip, state.config.security.captcha_after_failures) {
        return Ok(());
    }
    let refused = |code: &'static str, m: &str| ApiError::new(StatusCode::BAD_REQUEST, code, m);
    let (Some(id), Some(answer)) = (req.captcha_id.as_deref(), req.captcha_answer.as_deref()) else {
        return Err(refused(
            "captcha_required",
            "after failed attempts, type the characters in the picture (GET /api/v1/auth/captcha)",
        ));
    };
    state.captchas.verify(id, answer, ip).map_err(|r| match r {
        crate::captcha::Refusal::Wrong => {
            refused("captcha_failed", "the characters did not match; try the new picture")
        }
        _ => refused("captcha_failed", "the picture expired or was used; try the new one"),
    })
}

/// Checks the login challenge of a password sign-in.
fn check_challenge(state: &AppState, req: &LoginRequest, key: &str, ip: std::net::IpAddr) -> ApiResult<()> {
    if !state.config.security.login_challenge {
        return Ok(());
    }
    let (Some(id), Some(nonce)) = (req.challenge_id.as_deref(), req.nonce.as_deref()) else {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "challenge_required",
            "solve a login challenge first (GET /api/v1/auth/challenge)",
        ));
    };
    state.login_challenges.verify(id, nonce, key, Some(ip)).map_err(|r| {
        let m = match r {
            crate::challenge::Refusal::Invalid => "the login challenge expired or was used; get a new one",
            crate::challenge::Refusal::TooEasy => "this sign-in needs a harder challenge; get a new one with ?login=",
            crate::challenge::Refusal::Wrong => "the login challenge was not solved",
        };
        ApiError::new(StatusCode::BAD_REQUEST, "challenge_failed", m)
    })
}

fn with_cookies(mut resp: Response, cookies: [String; 2]) -> Response {
    for c in cookies {
        if let Ok(v) = HeaderValue::from_str(&c) {
            resp.headers_mut().append(header::SET_COOKIE, v);
        }
    }
    resp
}

async fn audit_login(state: &AppState, actor: &str, outcome: AuditOutcome, method: &str) {
    let ev =
        AuditEvent::new(actor, "auth.login", EntityRef::platform(), outcome).with_details(json!({ "method": method }));
    if let Err(e) = state.audit.append(ev).await {
        tracing::error!(error = %e, "audit write failed");
    }
}

pub async fn login(
    State(state): State<AppState>,
    crate::auth::SecureCookies(secure): crate::auth::SecureCookies,
    ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>,
    origin: crate::request_origin::RequestOrigin,
    headers: HeaderMap,
    Json(req): Json<LoginRequest>,
) -> ApiResult<Response> {
    let key = req.login.trim().to_lowercase();
    // Limits per account-and-address, per address and per account (see
    // `throttle::login_keys`): spraying and lockouts of the real owner.
    let ip = crate::throttle::client_ip(&headers, peer, &state.config.trusted_proxies());
    let limits = crate::throttle::login_keys(&key, ip);
    if !limits.iter().all(|(k, max)| state.login_throttle.allowed_up_to(k, *max)) {
        return Err(ApiError::new(StatusCode::TOO_MANY_REQUESTS, "throttled", "too many failed attempts; try later"));
    }
    check_challenge(&state, &req, &key, ip)?;
    check_captcha(&state, &req, &key, ip)?;
    let outcome = state.identity.login_password(&key, &req.password, req.totp.as_deref()).await;
    let LoginOutcome::Authenticated { user, mfa } = match outcome {
        Ok(o) => o,
        Err(e) => {
            if !matches!(e, forge_identity::IdentityError::MfaRequired) {
                for (k, _) in &limits {
                    state.login_throttle.record_failure(k);
                }
                state.login_challenges.failed(&key, Some(ip));
                state.captchas.failed(&key, ip);
                audit_login(&state, &format!("login:{key}"), AuditOutcome::Denied, "password").await;
            }
            return Err(e.into());
        }
    };
    crate::access::refuse_service_account(&state, user.id).await?;
    // The address-wide count stays: one success does not clear spraying.
    state.login_throttle.reset(&limits[0].0);
    state.login_throttle.reset(&limits[1].0);
    state.login_challenges.succeeded(&key);
    state.captchas.succeeded(&key);
    let tokens = state.identity.create_web_session(user.id, mfa).await?;
    audit_login(&state, &format!("user:{}", user.id), AuditOutcome::Success, "password").await;
    record_admin_address(&state, user.id, &origin).await;
    let ttl = state.identity.config().web_session_ttl.as_secs();
    let body = Json(json!({ "user": user, "mfa": mfa, "csrf_token": tokens.csrf_token })).into_response();
    Ok(with_cookies(body, session_cookies(&tokens.session_token, &tokens.csrf_token, ttl, secure)))
}

/// Remembers the address a platform administrator signed in on, for links
/// built without a request when no address is pinned (D-167). Never fails
/// the sign-in: a store error is logged.
pub(crate) async fn record_admin_address(
    state: &AppState,
    user: forge_core::UserId,
    origin: &crate::request_origin::RequestOrigin,
) {
    let req = forge_core::AccessRequest::new(
        forge_core::Principal::User(user),
        forge_core::Action::SystemUpdate,
        EntityRef::platform(),
        forge_core::AuthzContext { mfa: true },
    );
    if !state.authz.is_allowed(&req).await {
        return;
    }
    // Only a name this machine or its configuration vouches for: the Host
    // header is the client's, and this address feeds emails and webhooks.
    let local = match tokio::task::spawn_blocking(crate::public_address::LocalNames::read).await {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!(error = %e, "could not read this machine's addresses");
            return;
        }
    };
    let runtime = state.public_address.runtime();
    let configured: Vec<&str> = state
        .config
        .agent_hostnames
        .iter()
        .map(String::as_str)
        .chain(state.config.public_url.as_deref())
        .chain(state.config.node_public_url.as_deref())
        .chain(runtime.as_deref())
        .collect();
    if !crate::public_address::host_trusted(origin.host(), &local, &configured, &state.allowed_hosts) {
        tracing::debug!(host = origin.host(), "not recording an administrator address on an unconfigured name");
        return;
    }
    match crate::public_address::record_last_admin(
        &state.store,
        &state.public_address,
        &origin.origin(),
        state.clock.now(),
    )
    .await
    {
        Ok(Some(change)) => {
            let ev = AuditEvent::new(
                format!("user:{user}"),
                crate::public_address::AUDIT_LAST_ADMIN_UPDATE,
                EntityRef::platform(),
                AuditOutcome::Success,
            )
            .with_details(json!({ "from": change.from, "to": change.to }));
            if let Err(e) = state.audit.append(ev).await {
                tracing::error!(error = %e, "audit write failed");
            }
        }
        Ok(None) => {}
        Err(e) => tracing::warn!(error = %e, "could not record the administrator's console address"),
    }
}

pub async fn logout(
    State(state): State<AppState>,
    crate::auth::SecureCookies(secure): crate::auth::SecureCookies,
    caller: Caller,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let mut kind = "web";
    if let Some(token) = cookie(&headers, SESSION_COOKIE) {
        state.identity.logout(token).await?;
    } else if let Some(token) = crate::auth::bearer(&headers)
        && !caller.via_api_key()
    {
        // Native clients (CLI, apps): revoke the bearer session; its refresh
        // token lives in the same record, so the whole login ends.
        state.identity.logout(token).await?;
        kind = "bearer";
    }
    caller.audit("auth.logout", &EntityRef::platform(), None, json!({ "session": kind })).await;
    Ok(with_cookies(StatusCode::NO_CONTENT.into_response(), clear_cookies(secure)))
}

pub async fn me(State(state): State<AppState>, caller: Caller) -> ApiResult<Json<serde_json::Value>> {
    let bindings = state.store.list_bindings_for_user(caller.session.user.id).await?;
    Ok(Json(json!({
        "user": caller.session.user,
        "mfa": caller.session.session.mfa,
        "mfa_enrolled": caller.session.user.mfa_enabled(),
        "session_kind": caller.session.session.kind,
        "bindings": bindings,
    })))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UpdateProfile {
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    pub display_name: String,
    /// Your current password, when the account has no MFA factor and the
    /// username or email changes (see `reauth`).
    #[serde(default)]
    pub current_password: Option<String>,
}

/// `PATCH /me`: the signed-in user changes their own username, email and
/// display name. Blank values clear a field; one of username/email must stay.
pub async fn update_me(
    State(state): State<AppState>,
    caller: Caller,
    Json(req): Json<UpdateProfile>,
) -> ApiResult<Json<serde_json::Value>> {
    let before = &caller.session.user;
    let target = EntityRef::new(EntityKind::User, before.id);
    // The username and email are sign-in names: changing them needs the same
    // proof of identity as changing a factor, so a stolen session or an API
    // key cannot take the account over. The display name alone does not.
    let norm = |v: Option<&str>| v.map(str::trim).filter(|v| !v.is_empty()).map(str::to_lowercase);
    let names_change = norm(req.username.as_deref()) != norm(before.username.as_deref())
        || norm(req.email.as_deref()) != norm(before.email.as_deref());
    if names_change {
        super::reauth::for_factor_change(&state, &caller, req.current_password.as_deref(), "user.profile.update")
            .await?;
    }
    let user = match state
        .identity
        .update_profile(before, req.username.as_deref(), req.email.as_deref(), &req.display_name)
        .await
    {
        Ok(u) => u,
        Err(e) => {
            caller.audit_denied("user.profile.update", &target, None, &e.to_string()).await;
            return Err(e.into());
        }
    };
    caller
        .audit(
            "user.profile.update",
            &target,
            None,
            json!({
                "username": { "from": before.username, "to": user.username },
                "email": { "from": before.email, "to": user.email },
                "display_name_changed": before.display_name != user.display_name,
            }),
        )
        .await;
    Ok(Json(json!({ "user": user })))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ChangePassword {
    pub current_password: String,
    pub new_password: String,
}

/// `POST /me/password`: the signed-in user changes their own password.
pub async fn change_password(
    State(state): State<AppState>,
    crate::auth::SecureCookies(secure): crate::auth::SecureCookies,
    caller: Caller,
    Json(req): Json<ChangePassword>,
) -> ApiResult<Response> {
    let user = &caller.session.user;
    let target = EntityRef::new(EntityKind::User, user.id);
    if caller.via_api_key() {
        return Err(ApiError::forbidden("API keys cannot change a password"));
    }
    let key = format!("password:{}", user.id);
    if !state.login_throttle.allowed(&key) {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "throttled",
            "too many attempts; try again in 15 minutes",
        ));
    }
    if let Err(e) = state.identity.change_password(user, &req.current_password, &req.new_password).await {
        if matches!(e, forge_identity::IdentityError::InvalidCredentials) {
            state.login_throttle.record_failure(&key);
            caller.audit_denied("user.password.change", &target, None, "wrong current password").await;
            return Err(ApiError::new(
                StatusCode::UNAUTHORIZED,
                "invalid_password",
                "the current password is not correct",
            ));
        }
        caller.audit_denied("user.password.change", &target, None, &e.to_string()).await;
        return Err(e.into());
    }
    state.login_throttle.reset(&key);
    crate::bootstrap::forget_initial_password(&state, user.id).await;
    // Every other sign-in ends: web sessions, app and connector tokens with
    // their refresh tokens. A browser session that made the change gets a
    // fresh session of the same kind (same MFA state); API keys stay.
    state.identity.revoke_all(user.id).await?;
    caller.audit("user.password.change", &target, None, json!({ "other_sessions_ended": true })).await;
    if caller.session.session.kind != SessionKind::Web {
        return Ok(StatusCode::NO_CONTENT.into_response());
    }
    // With MFA not required every session counts as verified (auth.rs); store
    // only real verification, so turning MFA on later still asks for it.
    let mfa = caller.factor_verified();
    let tokens = state.identity.create_web_session(user.id, mfa).await?;
    let ttl = state.identity.config().web_session_ttl.as_secs();
    Ok(with_cookies(
        StatusCode::NO_CONTENT.into_response(),
        session_cookies(&tokens.session_token, &tokens.csrf_token, ttl, secure),
    ))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct StepUp {
    pub code: String,
}

/// `POST /auth/step-up`: upgrades the current sign-in to an MFA-verified one
/// with a fresh authenticator code. The session is replaced (new cookies), the
/// old one is ended.
pub async fn step_up(
    State(state): State<AppState>,
    crate::auth::SecureCookies(secure): crate::auth::SecureCookies,
    caller: Caller,
    headers: HeaderMap,
    Json(req): Json<StepUp>,
) -> ApiResult<Response> {
    let user = &caller.session.user;
    if caller.via_api_key() {
        return Err(ApiError::forbidden("API keys cannot be stepped up"));
    }
    if !user.mfa_enabled() {
        return Err(ApiError::bad_request(
            "set up an authenticator app in Profile → Password & security (/profile/security) first",
        ));
    }
    let key = format!("step-up:{}", user.id);
    if !state.login_throttle.allowed(&key) {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "throttled",
            "too many attempts; try again in 15 minutes",
        ));
    }
    if state.identity.verify_step_up(user, req.code.trim()).is_err() {
        state.login_throttle.record_failure(&key);
        caller.audit_denied("auth.step_up", &EntityRef::new(EntityKind::User, user.id), None, "invalid code").await;
        return Err(ApiError::new(StatusCode::UNAUTHORIZED, "invalid_mfa", "that code is not valid"));
    }
    state.login_throttle.reset(&key);
    // Apps and the CLI: a new token pair with MFA replaces the bearer session.
    if caller.session.session.kind == SessionKind::Bearer {
        let tokens = state.identity.step_up_bearer(&caller.session.session).await?;
        caller
            .audit("auth.step_up", &EntityRef::new(EntityKind::User, user.id), None, json!({ "session": "bearer" }))
            .await;
        let mut resp = Json(tokens).into_response();
        resp.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        return Ok(resp);
    }
    let tokens = state.identity.create_web_session(user.id, true).await?;
    if let Some(old) = cookie(&headers, SESSION_COOKIE) {
        state.identity.logout(old).await?;
    }
    caller.audit("auth.step_up", &EntityRef::new(EntityKind::User, user.id), None, json!({})).await;
    let ttl = state.identity.config().web_session_ttl.as_secs();
    let body = Json(json!({ "mfa": true, "csrf_token": tokens.csrf_token })).into_response();
    Ok(with_cookies(body, session_cookies(&tokens.session_token, &tokens.csrf_token, ttl, secure)))
}

pub async fn totp_begin(State(state): State<AppState>, caller: Caller) -> ApiResult<Json<serde_json::Value>> {
    let (secret, uri) = state.identity.new_totp_secret(&caller.session.user);
    Ok(Json(json!({ "secret": secret, "otpauth_uri": uri })))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TotpConfirm {
    pub secret: String,
    pub code: String,
    /// Your current password. Required when the account has no MFA factor
    /// yet (no authenticator app and no passkey).
    #[serde(default)]
    pub password: Option<String>,
}

/// Enrolls (or replaces) the caller's authenticator app. A factor grants
/// MFA sign-ins and step-up, so a stolen session or token alone must never
/// add one: with a factor already set up the session must be MFA-verified,
/// otherwise the current password is required. API keys are refused.
pub async fn totp_confirm(
    State(state): State<AppState>,
    caller: Caller,
    Json(req): Json<TotpConfirm>,
) -> ApiResult<StatusCode> {
    super::reauth::for_factor_change(&state, &caller, req.password.as_deref(), "auth.totp.enroll").await?;
    let user = &caller.session.user;
    let me = EntityRef::new(EntityKind::User, user.id);
    state.identity.confirm_totp(user.id, &req.secret, &req.code).await?;
    caller.audit("auth.totp.enroll", &me, None, json!({ "replaced": user.mfa_enabled() })).await;
    Ok(StatusCode::NO_CONTENT)
}

// ------------------------------------------------------------ OAuth for apps

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AuthorizeQuery {
    pub response_type: String,
    pub client_id: String,
    pub redirect_uri: String,
    pub code_challenge: String,
    pub code_challenge_method: String,
    #[serde(default)]
    pub state: Option<String>,
    /// RFC 8707 resource indicator (connectors: the `/mcp` URL).
    #[serde(default)]
    pub resource: Option<String>,
}

/// `GET /oauth/authorize`. Uses the browser's web session; first-party
/// clients get implicit consent. Without a session, sends the user to log in.
pub async fn oauth_authorize(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<AuthorizeQuery>,
) -> ApiResult<Response> {
    if q.response_type != "code" {
        return Err(ApiError::bad_request("response_type must be `code`"));
    }
    if !state.identity.is_first_party(&q.client_id) {
        // Registered connectors (MCP): always through the consent page.
        let next = format!("/oauth/authorize?{}", serde_urlencoded_query(&q));
        return super::connectors::connector_authorize(&state, &headers, &q, next).await;
    }
    if q.resource.is_some() {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "invalid_target", "this client cannot request a resource"));
    }
    let session = match cookie(&headers, SESSION_COOKIE) {
        Some(t) => state.identity.authenticate(t, SessionKind::Web).await?,
        None => None,
    };
    let Some(session) = session else {
        let next = format!("/oauth/authorize?{}", serde_urlencoded_query(&q));
        let login =
            format!("/login?next={}", url::form_urlencoded::byte_serialize(next.as_bytes()).collect::<String>());
        return Ok(Redirect::to(&login).into_response());
    };
    let code = state
        .identity
        .authorize_code(&session, &q.client_id, &q.redirect_uri, &q.code_challenge, &q.code_challenge_method)
        .await?;
    let mut target = url::Url::parse(&q.redirect_uri).map_err(|_| ApiError::bad_request("bad redirect_uri"))?;
    target.query_pairs_mut().append_pair("code", &code);
    if let Some(s) = &q.state {
        target.query_pairs_mut().append_pair("state", s);
    }
    Ok(Redirect::to(target.as_str()).into_response())
}

fn serde_urlencoded_query(q: &AuthorizeQuery) -> String {
    let mut s = url::form_urlencoded::Serializer::new(String::new());
    s.append_pair("response_type", &q.response_type)
        .append_pair("client_id", &q.client_id)
        .append_pair("redirect_uri", &q.redirect_uri)
        .append_pair("code_challenge", &q.code_challenge)
        .append_pair("code_challenge_method", &q.code_challenge_method);
    if let Some(st) = &q.state {
        s.append_pair("state", st);
    }
    if let Some(r) = &q.resource {
        s.append_pair("resource", r);
    }
    s.finish()
}

#[derive(Debug, Deserialize)]
pub struct TokenRequest {
    pub grant_type: String,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub code_verifier: Option<String>,
    #[serde(default)]
    pub client_id: Option<String>,
    #[serde(default)]
    pub redirect_uri: Option<String>,
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// RFC 8707 resource indicator.
    #[serde(default)]
    pub resource: Option<String>,
}

/// `POST /oauth/token` (form-encoded, RFC 6749).
pub async fn oauth_token(State(state): State<AppState>, Form(req): Form<TokenRequest>) -> ApiResult<Response> {
    let missing = |f: &str| ApiError::bad_request(format!("missing `{f}`"));
    let tokens = match req.grant_type.as_str() {
        "authorization_code" => {
            state
                .identity
                .exchange_code_for(
                    req.code.as_deref().ok_or_else(|| missing("code"))?,
                    req.code_verifier.as_deref().ok_or_else(|| missing("code_verifier"))?,
                    req.client_id.as_deref().ok_or_else(|| missing("client_id"))?,
                    req.redirect_uri.as_deref().ok_or_else(|| missing("redirect_uri"))?,
                    req.resource.as_deref(),
                )
                .await?
        }
        "refresh_token" => {
            state
                .identity
                .refresh_for(
                    req.refresh_token.as_deref().ok_or_else(|| missing("refresh_token"))?,
                    req.resource.as_deref(),
                    req.client_id.as_deref(),
                )
                .await?
        }
        _ => return Err(ApiError::new(StatusCode::BAD_REQUEST, "unsupported_grant_type", "unsupported grant_type")),
    };
    let mut resp = Json(tokens).into_response();
    resp.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

// ------------------------------------------------------------ OIDC SSO

/// Cookie that ties an SSO sign-in to the browser that started it.
const OIDC_COOKIE: &str = "forge_oidc";
/// How long an SSO sign-in may take (seconds).
const OIDC_COOKIE_SECS: u64 = 600;

/// `Set-Cookie` for the SSO state. `SameSite=Lax`, because the identity
/// provider's redirect back is a cross-site top-level navigation.
fn oidc_cookie(value: &str, max_age: u64, secure: bool) -> String {
    let secure = if secure { "; Secure" } else { "" };
    format!("{OIDC_COOKIE}={value}; Path=/api/v1/auth/oidc; HttpOnly; SameSite=Lax; Max-Age={max_age}{secure}")
}

/// Whether the browser presents the SSO state it was given at the start.
fn oidc_state_bound(headers: &HeaderMap, state: &str) -> bool {
    !state.is_empty()
        && crate::auth::cookie(headers, OIDC_COOKIE)
            .is_some_and(|c| forge_secrets::crypto::constant_time_eq(c.as_bytes(), state.as_bytes()))
}

pub async fn oidc_login(
    State(state): State<AppState>,
    crate::auth::SecureCookies(secure): crate::auth::SecureCookies,
) -> ApiResult<Response> {
    let provider = state.oidc.as_ref().ok_or_else(|| ApiError::not_found("SSO is not configured"))?;
    let (url, csrf_state, pending) = provider.begin();
    let cookie = oidc_cookie(&csrf_state, OIDC_COOKIE_SECS, secure);
    state.oidc_pending.insert(csrf_state, pending)?;
    let mut resp = Redirect::to(&url).into_response();
    if let Ok(v) = HeaderValue::from_str(&cookie) {
        resp.headers_mut().append(header::SET_COOKIE, v);
    }
    Ok(resp)
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct OidcCallback {
    pub code: String,
    pub state: String,
}

pub async fn oidc_callback(
    State(state): State<AppState>,
    crate::auth::SecureCookies(secure): crate::auth::SecureCookies,
    headers: HeaderMap,
    Query(q): Query<OidcCallback>,
) -> ApiResult<Response> {
    let provider = state.oidc.as_ref().ok_or_else(|| ApiError::not_found("SSO is not configured"))?;
    // The callback must come back to the browser that started the sign-in:
    // a callback link sent by someone else (login CSRF) has no matching cookie.
    if !oidc_state_bound(&headers, &q.state) {
        audit_login(&state, "oidc:unbound", AuditOutcome::Denied, "oidc").await;
        return Err(ApiError::bad_request("this sign-in was started in another browser; start it again here"));
    }
    let pending =
        state.oidc_pending.take(&q.state).ok_or_else(|| ApiError::bad_request("unknown or expired login state"))?;
    let claims = provider.complete(&q.code, pending).await?;
    let user = match state.identity.login_oidc(&claims).await {
        Ok(u) => u,
        Err(e) => {
            audit_login(&state, &format!("oidc:{}", claims.subject), AuditOutcome::Denied, "oidc").await;
            return Err(e.into());
        }
    };
    crate::access::refuse_service_account(&state, user.id).await?;
    // Do not assume the IdP performed MFA: without verified `amr` evidence the
    // session is not MFA-verified, so sensitive actions still need step-up.
    let tokens = state.identity.create_web_session(user.id, false).await?;
    audit_login(&state, &format!("user:{}", user.id), AuditOutcome::Success, "oidc").await;
    let ttl = state.identity.config().web_session_ttl.as_secs();
    let mut resp = with_cookies(
        Redirect::to("/").into_response(),
        session_cookies(&tokens.session_token, &tokens.csrf_token, ttl, secure),
    );
    if let Ok(v) = HeaderValue::from_str(&oidc_cookie("", 0, secure)) {
        resp.headers_mut().append(header::SET_COOKIE, v);
    }
    Ok(resp)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// Security review: an SSO callback link started by someone else (login
    /// CSRF) is refused unless this browser holds the matching state cookie.
    #[test]
    fn sso_callbacks_need_the_state_cookie_of_this_browser() {
        let with = |cookie: &str| {
            let mut h = HeaderMap::new();
            h.insert(header::COOKIE, cookie.parse().unwrap());
            h
        };
        assert!(oidc_state_bound(&with("forge_oidc=abc123"), "abc123"));
        assert!(!oidc_state_bound(&HeaderMap::new(), "abc123"), "no cookie: started elsewhere");
        assert!(!oidc_state_bound(&with("forge_oidc=mine"), "attackers"), "another sign-in's state");
        assert!(!oidc_state_bound(&with("forge_oidc="), ""), "empty state");
        let c = oidc_cookie("abc123", OIDC_COOKIE_SECS, true);
        assert!(
            c.contains("HttpOnly") && c.contains("SameSite=Lax") && c.contains("Secure") && c.contains("Max-Age=600")
        );
    }
}
