use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use forge_core::model::{AgentConnection, OAuthCode, Session, SessionKind, User};
use forge_core::ports::{ConnectorRepository, SessionRepository, UserRepository};
use forge_core::{Clock, DomainError, Principal, Timestamp, UserId};
use forge_secrets::Envelope;
use forge_secrets::crypto::{random_token, sha256_hex};
use serde::Serialize;

use crate::{oidc::OidcClaims, password, pkce, totp};

#[derive(Debug, thiserror::Error)]
pub enum IdentityError {
    #[error("invalid credentials")]
    InvalidCredentials,
    #[error("multi-factor code required")]
    MfaRequired,
    #[error("invalid multi-factor code")]
    InvalidMfa,
    #[error("account disabled")]
    Disabled,
    #[error("invalid grant: {0}")]
    InvalidGrant(String),
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("identity provider: {0}")]
    Provider(String),
    #[error("invalid redirect URI: {0}")]
    InvalidRedirect(String),
    #[error("too many requests; try later")]
    Throttled,
    #[error("invalid target: this client cannot request that resource")]
    InvalidTarget,
    #[error(transparent)]
    Store(#[from] DomainError),
}

/// A registered OAuth public client (desktop/mobile app, CLI).
#[derive(Debug, Clone)]
pub struct OAuthClient {
    pub client_id: String,
    /// Exact-match redirect URIs. Loopback URIs (`http://127.0.0.1/...`)
    /// match any port, as RFC 8252 requires for native apps.
    pub redirect_uris: Vec<String>,
}

impl OAuthClient {
    fn allows_redirect(&self, uri: &str) -> bool {
        self.redirect_uris.iter().any(|allowed| allowed == uri || loopback_match(allowed, uri))
    }
}

fn loopback_match(allowed: &str, uri: &str) -> bool {
    let (Ok(a), Ok(u)) = (url::Url::parse(allowed), url::Url::parse(uri)) else { return false };
    a.scheme() == "http"
        && u.scheme() == "http"
        && matches!(a.host_str(), Some("127.0.0.1" | "[::1]"))
        && a.host_str() == u.host_str()
        && a.path() == u.path()
}

#[derive(Debug, Clone)]
pub struct IdentityConfig {
    pub issuer: String,
    pub web_session_ttl: Duration,
    pub access_token_ttl: Duration,
    pub refresh_token_ttl: Duration,
    pub oauth_code_ttl: Duration,
    pub clients: Vec<OAuthClient>,
    /// Create users on first OIDC login (otherwise only pre-existing users
    /// with a matching verified email may sign in).
    pub oidc_auto_provision: bool,
}

impl Default for IdentityConfig {
    fn default() -> Self {
        Self {
            issuer: "Orbyn Forge".into(),
            web_session_ttl: Duration::from_secs(12 * 3600),
            access_token_ttl: Duration::from_secs(3600),
            refresh_token_ttl: Duration::from_secs(30 * 24 * 3600),
            oauth_code_ttl: Duration::from_secs(60),
            clients: vec![OAuthClient {
                client_id: "forge-app".into(),
                redirect_uris: vec!["http://127.0.0.1/callback".into(), "orbynforge://auth/callback".into()],
            }],
            oidc_auto_provision: false,
        }
    }
}

/// Result of a password login attempt.
#[derive(Debug)]
pub enum LoginOutcome {
    Authenticated { user: User, mfa: bool },
}

/// Secrets for a new browser session. Shown once; only hashes are stored.
#[derive(Debug, Clone)]
pub struct WebSessionTokens {
    pub session_token: String,
    pub csrf_token: String,
    pub expires_at: Timestamp,
}

/// Token response for apps (RFC 6749 §5.1 shape).
#[derive(Debug, Clone, Serialize)]
pub struct TokenPair {
    pub access_token: String,
    pub token_type: &'static str,
    pub expires_in: u64,
    pub refresh_token: String,
}

/// A validated session attached to a request.
#[derive(Debug, Clone)]
pub struct AuthenticatedSession {
    pub user: User,
    pub session: Session,
    /// The MCP connection behind a connector's token (issue #6).
    pub connection: Option<AgentConnection>,
}

impl AuthenticatedSession {
    #[must_use]
    pub fn principal(&self) -> Principal {
        Principal::User(self.user.id)
    }
}

/// Identity use cases. Depends only on ports.
#[derive(Debug, Clone)]
pub struct IdentityService {
    pub(crate) users: Arc<dyn UserRepository>,
    pub(crate) sessions: Arc<dyn SessionRepository>,
    envelope: Envelope,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) config: IdentityConfig,
    /// MCP connectors; `None` until [`IdentityService::with_connectors`].
    pub(crate) connectors: Option<Arc<dyn ConnectorRepository>>,
    /// `[mcp] enabled`: while off, connector tokens and codes do not work
    /// (connections can still be listed and revoked).
    pub(crate) connectors_enabled: bool,
    /// Last accepted authenticator time step per user: each code works once
    /// (in memory; a code's window is only about 90 seconds).
    totp_used: Arc<Mutex<HashMap<UserId, i64>>>,
}

impl IdentityService {
    #[must_use]
    pub fn new(
        users: Arc<dyn UserRepository>,
        sessions: Arc<dyn SessionRepository>,
        envelope: Envelope,
        clock: Arc<dyn Clock>,
        config: IdentityConfig,
    ) -> Self {
        Self {
            users,
            sessions,
            envelope,
            clock,
            config,
            connectors: None,
            connectors_enabled: false,
            totp_used: Arc::default(),
        }
    }

    #[must_use]
    pub fn config(&self) -> &IdentityConfig {
        &self.config
    }

    // ------------------------------------------------------------ users

    /// Creates a local user with a password, signing in with an email.
    pub async fn create_local_user(&self, email: &str, display_name: &str, pw: &str) -> Result<User, IdentityError> {
        self.create_account(None, Some(email), display_name, pw).await
    }

    /// Creates a local user with a password. At least one of `username` and
    /// `email` is required; either can be used to sign in.
    pub async fn create_account(
        &self,
        username: Option<&str>,
        email: Option<&str>,
        display_name: &str,
        pw: &str,
    ) -> Result<User, IdentityError> {
        password::validate(pw).map_err(IdentityError::InvalidRequest)?;
        let (username, email) = sign_in_names(username, email)?;
        let display_name = match display_name.trim() {
            "" => username.clone().or_else(|| email.clone()).unwrap_or_default(),
            d => d.to_owned(),
        };
        let user = User {
            id: UserId::new(),
            username,
            email,
            display_name,
            password_hash: Some(
                password::hash_blocking(pw).await.map_err(|e| IdentityError::Store(DomainError::Internal(e)))?,
            ),
            totp_secret_sealed: None,
            oidc_subject: None,
            disabled: false,
            created_at: self.clock.now(),
        };
        self.users.create_user(&user).await.map_err(in_use)?;
        Ok(user)
    }

    /// Changes a user's username, email and display name.
    pub async fn update_profile(
        &self,
        user: &User,
        username: Option<&str>,
        email: Option<&str>,
        display_name: &str,
    ) -> Result<User, IdentityError> {
        let (username, email) = sign_in_names(username, email)?;
        let display_name = display_name.trim();
        if display_name.is_empty() || display_name.chars().count() > 100 {
            return Err(IdentityError::InvalidRequest("the display name must be 1-100 characters".into()));
        }
        self.users
            .update_user_profile(user.id, username.as_deref(), email.as_deref(), display_name)
            .await
            .map_err(in_use)?;
        Ok(User { username, email, display_name: display_name.to_owned(), ..user.clone() })
    }

    /// Changes a user's own password after checking the current one.
    pub async fn change_password(&self, user: &User, current: &str, new: &str) -> Result<(), IdentityError> {
        let Some(hash) = user.password_hash.as_deref() else {
            return Err(IdentityError::InvalidRequest("this account signs in through single sign-on".into()));
        };
        if !password::verify_blocking(current, hash).await {
            return Err(IdentityError::InvalidCredentials);
        }
        password::validate(new).map_err(IdentityError::InvalidRequest)?;
        if new == current {
            return Err(IdentityError::InvalidRequest("choose a password different from the current one".into()));
        }
        let hash = password::hash_blocking(new).await.map_err(|e| IdentityError::Store(DomainError::Internal(e)))?;
        self.users.set_password_hash(user.id, &hash).await?;
        Ok(())
    }

    /// Finds a user by username, or by email when `login` contains `@`.
    async fn find_by_login(&self, login: &str) -> Result<Option<User>, IdentityError> {
        let login = login.trim();
        Ok(if login.contains('@') {
            self.users.find_user_by_email(login).await?
        } else {
            self.users.find_user_by_username(login).await?
        })
    }

    // ------------------------------------------------------------ login

    /// Password login with a username or an email. If the user has TOTP
    /// enabled, `totp_code` is required.
    pub async fn login_password(
        &self,
        login: &str,
        pw: &str,
        totp_code: Option<&str>,
    ) -> Result<LoginOutcome, IdentityError> {
        let user = self.find_by_login(login).await?;
        let Some(user) = user else {
            // Spend the same time as a real verification.
            let _ = password::verify_blocking(pw, password::dummy_hash()).await;
            return Err(IdentityError::InvalidCredentials);
        };
        // An account without a password (single sign-on) spends the same
        // time, so timing does not tell it apart.
        let hash = user.password_hash.as_deref().unwrap_or_else(|| password::dummy_hash());
        let ok = password::verify_blocking(pw, hash).await && user.password_hash.is_some();
        if !ok {
            return Err(IdentityError::InvalidCredentials);
        }
        if user.disabled {
            return Err(IdentityError::Disabled);
        }
        let mfa = if user.mfa_enabled() {
            let code = totp_code.ok_or(IdentityError::MfaRequired)?;
            self.check_totp(&user, code)?;
            true
        } else {
            false
        };
        Ok(LoginOutcome::Authenticated { user, mfa })
    }

    /// Step-up check for destructive actions: verifies a fresh TOTP code of
    /// a user who has MFA enrolled.
    pub fn verify_step_up(&self, user: &User, code: &str) -> Result<(), IdentityError> {
        self.check_totp(user, code)
    }

    fn check_totp(&self, user: &User, code: &str) -> Result<(), IdentityError> {
        let sealed = user.totp_secret_sealed.as_deref().ok_or(IdentityError::InvalidMfa)?;
        let secret = self
            .envelope
            .open(sealed, &totp_aad(user.id))
            .map_err(|e| IdentityError::Store(DomainError::internal(e)))?;
        let step = totp::matched_step(&secret, code, self.clock.now().0 / 1000).ok_or(IdentityError::InvalidMfa)?;
        // Single use: a code (or an older one) that already signed in or
        // stepped up is refused, so a seen code cannot be replayed.
        let mut used = self.totp_used.lock().unwrap_or_else(PoisonError::into_inner);
        if used.get(&user.id).is_some_and(|last| step <= *last) {
            return Err(IdentityError::InvalidMfa);
        }
        used.insert(user.id, step);
        Ok(())
    }

    // ------------------------------------------------------------ TOTP enrollment

    /// Generates a TOTP secret. The caller shows it (QR) and must confirm
    /// with [`Self::confirm_totp`] before it takes effect.
    #[must_use]
    pub fn new_totp_secret(&self, user: &User) -> (String, String) {
        let secret = totp::generate_secret();
        (totp::base32_encode(&secret), totp::provisioning_uri(&secret, &self.config.issuer, user.login()))
    }

    /// Verifies a code for `secret_b32` and stores the secret sealed.
    pub async fn confirm_totp(&self, user: UserId, secret_b32: &str, code: &str) -> Result<(), IdentityError> {
        let secret =
            totp::base32_decode(secret_b32).ok_or_else(|| IdentityError::InvalidRequest("bad secret".into()))?;
        if secret.len() < 16 {
            return Err(IdentityError::InvalidRequest("secret too short".into()));
        }
        if !totp::verify(&secret, code, self.clock.now().0 / 1000) {
            return Err(IdentityError::InvalidMfa);
        }
        let sealed =
            self.envelope.seal(&secret, &totp_aad(user)).map_err(|e| IdentityError::Store(DomainError::internal(e)))?;
        self.users.set_totp_secret(user, Some(&sealed)).await?;
        Ok(())
    }

    // ------------------------------------------------------------ web sessions

    pub async fn create_web_session(&self, user: UserId, mfa: bool) -> Result<WebSessionTokens, IdentityError> {
        let session_token = random_token::<32>();
        let csrf_token = random_token::<32>();
        let now = self.clock.now();
        let expires_at = now.plus(self.config.web_session_ttl);
        self.sessions
            .create_session(&Session {
                id_hash: sha256_hex(session_token.as_bytes()),
                user_id: user,
                kind: SessionKind::Web,
                csrf_hash: Some(sha256_hex(csrf_token.as_bytes())),
                refresh_hash: None,
                mfa,
                created_at: now,
                expires_at,
                connection_id: None,
            })
            .await?;
        Ok(WebSessionTokens { session_token, csrf_token, expires_at })
    }

    /// Resolves a session cookie or bearer token. Expired sessions are deleted.
    pub async fn authenticate(
        &self,
        token: &str,
        kind: SessionKind,
    ) -> Result<Option<AuthenticatedSession>, IdentityError> {
        let hash = sha256_hex(token.as_bytes());
        let Some(session) = self.sessions.get_session(&hash).await? else { return Ok(None) };
        if session.kind != kind {
            return Ok(None);
        }
        if session.expires_at <= self.clock.now() {
            self.sessions.delete_session(&hash).await?;
            return Ok(None);
        }
        let user = self.users.get_user(session.user_id).await?;
        if user.disabled {
            return Ok(None);
        }
        // A connector's token lives only as long as its connection, and
        // only while connectors are turned on.
        if session.connection_id.is_some() && !self.connectors_enabled {
            return Ok(None);
        }
        let connection = match &session.connection_id {
            Some(id) => match self.active_connection(id).await? {
                Some(c) => Some(c),
                None => {
                    self.sessions.delete_session(&hash).await?;
                    return Ok(None);
                }
            },
            None => None,
        };
        Ok(Some(AuthenticatedSession { user, session, connection }))
    }

    /// Double-submit check: the header value must hash to the session's CSRF hash.
    #[must_use]
    pub fn verify_csrf(session: &AuthenticatedSession, header_value: Option<&str>) -> bool {
        match (&session.session.csrf_hash, header_value) {
            (Some(expected), Some(given)) => {
                forge_secrets::crypto::constant_time_eq(expected.as_bytes(), sha256_hex(given.as_bytes()).as_bytes())
            }
            _ => false,
        }
    }

    pub async fn logout(&self, token: &str) -> Result<(), IdentityError> {
        self.sessions.delete_session(&sha256_hex(token.as_bytes())).await?;
        Ok(())
    }

    /// Revokes every session of a user (e.g. on disable or password change):
    /// web sessions, app tokens and their refresh tokens, connector tokens.
    pub async fn revoke_all(&self, user: UserId) -> Result<(), IdentityError> {
        self.sessions.delete_user_sessions(user).await?;
        Ok(())
    }

    // ------------------------------------------------------------ OAuth 2.1 + PKCE

    /// Authorization endpoint logic: the user is already authenticated (web
    /// session). Returns the one-time code to send to `redirect_uri`.
    pub async fn authorize_code(
        &self,
        session: &AuthenticatedSession,
        client_id: &str,
        redirect_uri: &str,
        code_challenge: &str,
        code_challenge_method: &str,
    ) -> Result<String, IdentityError> {
        let client = self.client(client_id)?;
        if !client.allows_redirect(redirect_uri) {
            return Err(IdentityError::InvalidRequest("redirect_uri not registered".into()));
        }
        if code_challenge_method != "S256" || code_challenge.len() != 43 {
            return Err(IdentityError::InvalidRequest("PKCE S256 code_challenge required".into()));
        }
        let code = random_token::<32>();
        self.sessions
            .create_oauth_code(&OAuthCode {
                code_hash: sha256_hex(code.as_bytes()),
                user_id: session.user.id,
                client_id: client_id.to_owned(),
                redirect_uri: redirect_uri.to_owned(),
                code_challenge: code_challenge.to_owned(),
                mfa: session.session.mfa,
                expires_at: self.clock.now().plus(self.config.oauth_code_ttl),
                connection_id: None,
            })
            .await?;
        Ok(code)
    }

    /// Token endpoint, `grant_type=authorization_code`.
    pub async fn exchange_code(
        &self,
        code: &str,
        code_verifier: &str,
        client_id: &str,
        redirect_uri: &str,
    ) -> Result<TokenPair, IdentityError> {
        self.exchange_code_for(code, code_verifier, client_id, redirect_uri, None).await
    }

    /// [`IdentityService::exchange_code`] with an RFC 8707 `resource`. A
    /// connector's code yields a token bound to its connection's resource;
    /// first-party clients may not name a resource.
    pub async fn exchange_code_for(
        &self,
        code: &str,
        code_verifier: &str,
        client_id: &str,
        redirect_uri: &str,
        resource: Option<&str>,
    ) -> Result<TokenPair, IdentityError> {
        let grant = |m: &str| IdentityError::InvalidGrant(m.to_owned());
        // Single use: the code is deleted whatever the outcome.
        let stored =
            self.sessions.take_oauth_code(&sha256_hex(code.as_bytes())).await?.ok_or_else(|| grant("unknown code"))?;
        if stored.expires_at <= self.clock.now() {
            return Err(grant("code expired"));
        }
        if stored.client_id != client_id || stored.redirect_uri != redirect_uri {
            return Err(grant("client or redirect mismatch"));
        }
        if !pkce::verify(code_verifier, &stored.code_challenge) {
            return Err(grant("PKCE verification failed"));
        }
        match stored.connection_id {
            Some(_) if !self.connectors_enabled => Err(grant("AI connectors are turned off on this Forge")),
            Some(id) => {
                let c = self.active_connection(&id).await?.ok_or_else(|| grant("the connection was revoked"))?;
                check_resource(resource, &c)?;
                self.issue_bearer(stored.user_id, false, Some(id), self.clock.now()).await
            }
            None if resource.is_some() => Err(IdentityError::InvalidTarget),
            None => self.issue_bearer(stored.user_id, stored.mfa, None, self.clock.now()).await,
        }
    }

    /// Token endpoint, `grant_type=refresh_token`. Rotates the refresh token.
    pub async fn refresh(&self, refresh_token: &str) -> Result<TokenPair, IdentityError> {
        self.refresh_for(refresh_token, None, None).await
    }

    /// [`IdentityService::refresh`] with an RFC 8707 `resource`. A connector
    /// keeps its connection and must name its `client_id` (RFC 6749 §6); a
    /// revoked connection cannot refresh.
    pub async fn refresh_for(
        &self,
        refresh_token: &str,
        resource: Option<&str>,
        client_id: Option<&str>,
    ) -> Result<TokenPair, IdentityError> {
        let hash = sha256_hex(refresh_token.as_bytes());
        let old = self
            .sessions
            .find_by_refresh(&hash)
            .await?
            .ok_or_else(|| IdentityError::InvalidGrant("unknown refresh token".into()))?;
        if old.connection_id.is_some() && !self.connectors_enabled {
            return Err(IdentityError::InvalidGrant("AI connectors are turned off on this Forge".into()));
        }
        self.sessions.delete_session(&old.id_hash).await?;
        let refresh_expiry = old.created_at.plus(self.config.refresh_token_ttl);
        if refresh_expiry <= self.clock.now() {
            return Err(IdentityError::InvalidGrant("refresh token expired".into()));
        }
        let user = self.users.get_user(old.user_id).await?;
        if user.disabled {
            return Err(IdentityError::Disabled);
        }
        match old.connection_id {
            Some(id) => {
                let c = self
                    .active_connection(&id)
                    .await?
                    .ok_or_else(|| IdentityError::InvalidGrant("the connection was revoked".into()))?;
                if client_id != Some(c.client_id.as_str()) {
                    return Err(IdentityError::InvalidGrant("client_id does not match the refresh token".into()));
                }
                check_resource(resource, &c)?;
                self.issue_bearer(user.id, false, Some(id), old.created_at).await
            }
            None if resource.is_some() => Err(IdentityError::InvalidTarget),
            None => self.issue_bearer(user.id, old.mfa, None, old.created_at).await,
        }
    }

    /// Issues a bearer session. `signed_in_at` is when the person signed in
    /// (not when this token was rotated): it is stored as `created_at`, so
    /// `refresh_token_ttl` is an absolute lifetime that refreshing never
    /// extends.
    pub(crate) async fn issue_bearer(
        &self,
        user: UserId,
        mfa: bool,
        connection_id: Option<String>,
        signed_in_at: forge_core::Timestamp,
    ) -> Result<TokenPair, IdentityError> {
        let access = random_token::<32>();
        let refresh = random_token::<32>();
        let now = self.clock.now();
        self.sessions
            .create_session(&Session {
                id_hash: sha256_hex(access.as_bytes()),
                user_id: user,
                kind: SessionKind::Bearer,
                csrf_hash: None,
                refresh_hash: Some(sha256_hex(refresh.as_bytes())),
                mfa,
                created_at: signed_in_at,
                expires_at: now.plus(self.config.access_token_ttl),
                connection_id,
            })
            .await?;
        Ok(TokenPair {
            access_token: access,
            token_type: "Bearer",
            expires_in: self.config.access_token_ttl.as_secs(),
            refresh_token: refresh,
        })
    }

    fn client(&self, client_id: &str) -> Result<&OAuthClient, IdentityError> {
        self.config
            .clients
            .iter()
            .find(|c| c.client_id == client_id)
            .ok_or_else(|| IdentityError::InvalidRequest("unknown client_id".into()))
    }

    // ------------------------------------------------------------ OIDC

    /// Maps verified OIDC claims to a local user: by linked subject first,
    /// then by verified email (linking it), then optional auto-provisioning.
    pub async fn login_oidc(&self, claims: &OidcClaims) -> Result<User, IdentityError> {
        let subject = format!("{}|{}", claims.issuer, claims.subject);
        if let Some(u) = self.users.find_user_by_oidc_subject(&subject).await? {
            return if u.disabled { Err(IdentityError::Disabled) } else { Ok(u) };
        }
        let email = claims
            .email
            .as_deref()
            .filter(|_| claims.email_verified)
            .ok_or_else(|| IdentityError::Provider("provider did not return a verified email".into()))?;
        if let Some(u) = self.users.find_user_by_email(email).await? {
            if u.disabled {
                return Err(IdentityError::Disabled);
            }
            self.users.set_oidc_subject(u.id, &subject).await?;
            return Ok(u);
        }
        if !self.config.oidc_auto_provision {
            return Err(IdentityError::InvalidCredentials);
        }
        let user = User {
            id: UserId::new(),
            username: None,
            email: Some(email.to_lowercase()),
            display_name: claims.name.clone().unwrap_or_else(|| email.to_owned()),
            password_hash: None,
            totp_secret_sealed: None,
            oidc_subject: Some(subject),
            disabled: false,
            created_at: self.clock.now(),
        };
        self.users.create_user(&user).await?;
        Ok(user)
    }
}

/// Validates and lowercases the sign-in names; at least one is required.
/// Blank values count as absent.
pub fn sign_in_names(
    username: Option<&str>,
    email: Option<&str>,
) -> Result<(Option<String>, Option<String>), IdentityError> {
    let username = username.map(str::trim).filter(|u| !u.is_empty()).map(str::to_lowercase);
    let email = email.map(str::trim).filter(|e| !e.is_empty()).map(str::to_lowercase);
    if let Some(u) = &username {
        let ok = (3..=64).contains(&u.len())
            && u.as_bytes()[0].is_ascii_alphanumeric()
            && u.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'.' | b'_' | b'-'));
        if !ok {
            return Err(IdentityError::InvalidRequest(
                "a username is 3-64 characters: letters, digits, '.', '_' or '-', starting with a letter or digit"
                    .into(),
            ));
        }
    }
    if let Some(e) = &email {
        let valid = e.len() <= 254
            && !e.chars().any(char::is_whitespace)
            && e.split_once('@').is_some_and(|(local, domain)| !local.is_empty() && !domain.is_empty());
        if !valid {
            return Err(IdentityError::InvalidRequest("invalid email".into()));
        }
    }
    if username.is_none() && email.is_none() {
        return Err(IdentityError::InvalidRequest("a username or an email is required".into()));
    }
    Ok((username, email))
}

/// RFC 8707: a requested resource must be the connection's.
fn check_resource(resource: Option<&str>, c: &AgentConnection) -> Result<(), IdentityError> {
    match resource {
        Some(r) if r.trim_end_matches('/') != c.resource => Err(IdentityError::InvalidTarget),
        _ => Ok(()),
    }
}

fn in_use(e: DomainError) -> IdentityError {
    match e {
        DomainError::Conflict(_) => IdentityError::InvalidRequest("that username or email is already in use".into()),
        e => IdentityError::Store(e),
    }
}

fn totp_aad(user: UserId) -> String {
    format!("user:{user}:totp")
}
