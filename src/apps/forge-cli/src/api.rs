//! HTTP client for the Forge API: bearer auth, token refresh, typed errors.
//!
//! Redirects are never followed (an `Authorization` header must not travel
//! to another origin). Access tokens are refreshed shortly before expiry and
//! once after a 401, under the config lock so parallel `forge` processes do
//! not race on the rotating refresh token.
//!
//! Every mutation carries an `Idempotency-Key` (D-074). The same key is sent
//! again on the refresh, step-up and transport retries, so the server runs
//! the change at most once.

use std::time::Duration;

use reqwest::{Method, StatusCode};
use serde_json::Value;

use crate::creds::{self, Tokens};
use crate::ctx::{Lock, config_dir};
use crate::error::{CliError, Exit, Result};

/// OAuth public client registered in Forge (`forge-identity` defaults).
pub const CLIENT_ID: &str = "forge-app";

#[derive(Debug, Clone)]
enum Payload<'a> {
    None,
    Json(&'a Value),
    Bytes(Vec<u8>),
    /// A local file, streamed with its length (opened again for each attempt).
    File {
        path: &'a std::path::Path,
        len: u64,
        content_type: &'a str,
    },
}

#[derive(Debug)]
enum Auth {
    None,
    /// `FORGE_TOKEN`: used as-is, never refreshed or stored.
    Static(String),
    /// A context login (refreshable, stored).
    Context {
        name: String,
        tokens: Tokens,
    },
}

#[derive(Debug)]
pub struct Api {
    http: reqwest::Client,
    pub server: String,
    auth: Auth,
    /// May prompt for a one-time code when an action needs MFA (a terminal,
    /// and not `--non-interactive`).
    interactive: bool,
    /// `--idempotency-key` for the next mutation, instead of a random one.
    idempotency_key: Option<String>,
    /// Total time allowed per request instead of the client's default
    /// (object transfers take longer than an API call).
    request_timeout: Option<Duration>,
}

/// Retries of a mutation after a transport error, a 502/503/504/429 or
/// `idempotency_in_progress` (same key each time).
const MUTATION_RETRIES: u32 = 3;

fn is_mutation(m: &Method) -> bool {
    matches!(*m, Method::POST | Method::PUT | Method::PATCH | Method::DELETE)
}

/// A random key for one mutation: 128 bits, hex.
pub fn new_idempotency_key() -> Result<String> {
    use ring::rand::{SecureRandom, SystemRandom};
    let mut b = [0u8; 16];
    SystemRandom::new().fill(&mut b).map_err(|_| CliError::new(Exit::General, "no secure random source"))?;

    Ok(hex::encode(b))
}

/// Exponential backoff with jitter, or the server's `Retry-After` (capped).
fn backoff(attempt: u32, retry_after: Option<u64>) -> Duration {
    use ring::rand::{SecureRandom, SystemRandom};

    if let Some(s) = retry_after {
        return Duration::from_secs(s.min(30));
    }

    let base = 250u64 << attempt.min(5);
    let mut j = [0u8; 2];
    let _ = SystemRandom::new().fill(&mut j);

    Duration::from_millis(base + u64::from(u16::from_le_bytes(j)) % base)
}

/// Asks for a one-time code on the terminal (step-up).
pub fn prompt_code(non_interactive: bool) -> Result<String> {
    use std::io::{BufRead, IsTerminal, Write};

    if non_interactive || !std::io::stdin().is_terminal() {
        return Err(CliError::new(Exit::MfaRequired, "pass the code with --code (no terminal to ask on)")
            .with_code("step_up_required"));
    }

    eprint!("One-time code from your authenticator app: ");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    let code: String = line.chars().filter(char::is_ascii_digit).collect();
    if code.len() != 6 {
        return Err(CliError::usage("a one-time code has 6 digits"));
    }

    Ok(code)
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

pub fn http_client(ca_file: Option<&str>) -> Result<reqwest::Client> {
    let mut b = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(300))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("forge-cli/", env!("CARGO_PKG_VERSION")));

    if let Some(path) = ca_file {
        let pem = std::fs::read(path).map_err(|e| CliError::usage(format!("CA file {path}: {e}")))?;
        let certs =
            reqwest::Certificate::from_pem_bundle(&pem).map_err(|e| CliError::usage(format!("CA file {path}: {e}")))?;

        for c in certs {
            b = b.add_root_certificate(c);
        }
    }

    b.build().map_err(|e| CliError::new(Exit::Transport, e.to_string()))
}

fn transport(e: &reqwest::Error) -> CliError {
    let exit = if e.is_timeout() { Exit::Timeout } else { Exit::Transport };
    let mut msg = e.to_string();
    let mut src = std::error::Error::source(e);

    while let Some(s) = src {
        msg.push_str(": ");
        msg.push_str(&s.to_string());
        src = s.source();
    }

    let mut err = CliError::new(exit, msg);
    err.retryable = true;

    err
}

/// Turns a non-success response into a typed error (RFC 9457 problem body).
pub async fn problem(resp: reqwest::Response) -> CliError {
    let status = resp.status().as_u16();
    let request_id = resp.headers().get("x-request-id").and_then(|v| v.to_str().ok()).map(str::to_owned);
    let body = resp.text().await.unwrap_or_default();
    let v: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    let title = v.get("title").and_then(Value::as_str).or_else(|| v.get("error").and_then(Value::as_str));
    let detail = v
        .get("detail")
        .and_then(Value::as_str)
        .or_else(|| v.get("message").and_then(Value::as_str))
        .or_else(|| v.get("error_description").and_then(Value::as_str));
    let detail = detail.map(str::to_owned).or_else(|| (!body.is_empty() && body.len() < 300).then(|| body.clone()));

    CliError::from_http(status, title, detail.as_deref(), request_id)
}

impl Api {
    pub fn anonymous(server: String, ca_file: Option<&str>) -> Result<Self> {
        Self::new(server, ca_file, Auth::None)
    }

    pub fn with_token(server: String, ca_file: Option<&str>, token: String) -> Result<Self> {
        Self::new(server, ca_file, Auth::Static(token))
    }

    pub fn with_context(server: String, ca_file: Option<&str>, name: String, tokens: Tokens) -> Result<Self> {
        Self::new(server, ca_file, Auth::Context { name, tokens })
    }

    fn new(server: String, ca_file: Option<&str>, auth: Auth) -> Result<Self> {
        Ok(Self {
            http: http_client(ca_file)?,
            server,
            auth,
            interactive: false,
            idempotency_key: None,
            request_timeout: None,
        })
    }

    /// Total time each following request may take (`None`: the client's
    /// five minutes). Object uploads and downloads set a longer one.
    pub fn set_request_timeout(&mut self, timeout: Option<Duration>) {
        self.request_timeout = timeout;
    }

    /// Like [`Api::send`] with a local file as the body, streamed from disk
    /// with its `Content-Length` (never read into memory).
    pub async fn send_file(
        &mut self,
        method: Method,
        path: &str,
        file: &std::path::Path,
        content_type: &str,
    ) -> Result<reqwest::Response> {
        let len =
            tokio::fs::metadata(file).await.map_err(|e| CliError::usage(format!("{}: {e}", file.display())))?.len();
        self.send_payload(method, path, Payload::File { path: file, len, content_type }).await
    }

    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    /// Lets the client ask for a one-time code when an action needs MFA.
    pub fn set_interactive(&mut self, interactive: bool) {
        self.interactive = interactive;
    }

    /// The bearer token for a WebSocket (sent in the `Authorization` header,
    /// never the URL), refreshed first when it expires within two minutes.
    pub async fn access_token(&mut self) -> Result<Option<String>> {
        if let Auth::Context { tokens, .. } = &self.auth
            && tokens.expires_at_ms <= now_ms() + 120_000
        {
            self.refresh().await?;
        }
        Ok(self.bearer().map(str::to_owned))
    }

    /// Uses `key` as the `Idempotency-Key` of the next mutation.
    pub fn set_idempotency_key(&mut self, key: Option<String>) {
        self.idempotency_key = key;
    }

    /// Verifies a one-time code for this session (RFC 9470 step-up). The
    /// server answers a bearer session with a new token pair that carries
    /// MFA; it replaces the stored one.
    pub async fn step_up(&mut self, code: &str) -> Result<Value> {
        if matches!(self.auth, Auth::Static(_)) {
            // The server revokes the stepped-up session and answers with a new
            // pair; a one-shot FORGE_TOKEN would silently stop working.
            return Err(CliError::usage(
                "FORGE_TOKEN cannot be stepped up: sign in with a context (`forge auth login`), then run `forge auth step-up`",
            )
            .with_code("step_up_needs_context"));
        }

        let resp = self
            .send_payload_once(
                Method::POST,
                "/api/v1/auth/step-up",
                Payload::Json(&serde_json::json!({ "code": code })),
                None,
            )
            .await?;
        let v: Value = resp.json().await.map_err(|e| transport(&e))?;
        if v.get("access_token").is_some() {
            let fresh = token_pair(&self.server, &v)?;

            match &mut self.auth {
                Auth::Context { name, tokens } => {
                    let dir = config_dir()?;
                    let _lock = Lock::acquire(&dir)?;
                    creds::save(name, &fresh)?;
                    *tokens = fresh;
                }
                // Refused above: a FORGE_TOKEN is never stepped up.
                Auth::Static(_) | Auth::None => {}
            }
        }

        Ok(serde_json::json!({ "mfa": true }))
    }

    fn bearer(&self) -> Option<&str> {
        match &self.auth {
            Auth::None => None,
            Auth::Static(t) => Some(t),
            Auth::Context { tokens, .. } => Some(&tokens.access_token),
        }
    }

    /// Refreshes the access token (rotating the refresh token) under the lock.
    async fn refresh(&mut self) -> Result<()> {
        let Auth::Context { name, tokens } = &mut self.auth else {
            return Err(CliError::auth("the token was rejected; sign in again or supply a new FORGE_TOKEN"));
        };

        let dir = config_dir()?;
        let _lock = Lock::acquire(&dir)?;

        // Another process may have refreshed while we waited for the lock.
        if let Some(stored) = creds::for_server(name, &self.server)?
            && stored.refresh_token != tokens.refresh_token
            && stored.expires_at_ms > now_ms() + 60_000
        {
            *tokens = stored;
            return Ok(());
        }

        let resp = self
            .http
            .post(format!("{}/oauth/token", self.server))
            .form(&[("grant_type", "refresh_token"), ("refresh_token", tokens.refresh_token.as_str())])
            .send()
            .await
            .map_err(|e| transport(&e))?;

        if !resp.status().is_success() {
            let e = problem(resp).await;
            return Err(CliError::auth(format!(
                "your session expired ({}); run `forge auth login --context {name}`",
                e.message
            )));
        }

        let v: Value = resp.json().await.map_err(|e| transport(&e))?;
        let fresh = token_pair(&self.server, &v)?;
        creds::save(name, &fresh)?;
        *tokens = fresh;

        Ok(())
    }

    /// Sends a request with auth and one refresh-and-retry on 401; returns
    /// the successful response (errors become typed `CliError`s).
    pub async fn send(&mut self, method: Method, path: &str, body: Option<&Value>) -> Result<reqwest::Response> {
        self.send_payload(method, path, body.map_or(Payload::None, Payload::Json)).await
    }

    /// Like [`Api::send`] with raw bytes (`application/octet-stream`).
    pub async fn send_bytes(&mut self, method: Method, path: &str, bytes: Vec<u8>) -> Result<reqwest::Response> {
        self.send_payload(method, path, Payload::Bytes(bytes)).await
    }

    /// [`Api::send_with_step_up`] with an `Idempotency-Key` on mutations,
    /// retried with the same key after transient failures.
    async fn send_payload(&mut self, method: Method, path: &str, body: Payload<'_>) -> Result<reqwest::Response> {
        if !is_mutation(&method) {
            return self.send_with_step_up(method, path, body, None).await;
        }

        let key = match self.idempotency_key.take() {
            Some(k) => k,
            None => new_idempotency_key()?,
        };

        let mut attempt = 0;

        loop {
            match self.send_with_step_up(method.clone(), path, body.clone(), Some(&key)).await {
                Err(e) if e.retryable && attempt < MUTATION_RETRIES => {
                    attempt += 1;
                    eprintln!("forge: {method} {path}: {} — retrying ({attempt}/{MUTATION_RETRIES})", e.message);
                    tokio::time::sleep(backoff(attempt, e.retry_after)).await;
                }
                r => return r,
            }
        }
    }

    /// [`Api::send_payload_once`], and once more after a step-up when the
    /// action needs MFA and a terminal can ask for the code.
    async fn send_with_step_up(
        &mut self,
        method: Method,
        path: &str,
        body: Payload<'_>,
        key: Option<&str>,
    ) -> Result<reqwest::Response> {
        match self.send_payload_once(method.clone(), path, body.clone(), key).await {
            // A FORGE_TOKEN cannot be stepped up here: the server would
            // revoke it and hand back a pair this process cannot keep.
            Err(mut e)
                if e.exit == Exit::MfaRequired
                    && e.code == "step_up_required"
                    && matches!(self.auth, Auth::Static(_)) =>
            {
                e.message = format!(
                    "{} — this needs an MFA-verified session, and a token is never stepped up. Use an elevated token \
                     (`forge api-keys create --elevated` or `forge service-accounts token-create --elevated`), or sign in \
                     with a context and run `forge auth step-up`. Set up MFA in Profile → Password & security (/profile/security)",
                    e.message
                );
                Err(e)
            }
            Err(e)
                if e.exit == Exit::MfaRequired
                    && e.code == "step_up_required"
                    && self.interactive
                    && path != "/api/v1/auth/step-up" =>
            {
                eprintln!("This action needs multi-factor authentication ({}).", e.message);
                let code = prompt_code(false).map_err(|_| e)?;
                self.step_up(&code).await?;
                self.send_payload_once(method, path, body, key).await
            }
            r => r,
        }
    }

    async fn send_payload_once(
        &mut self,
        method: Method,
        path: &str,
        body: Payload<'_>,
        key: Option<&str>,
    ) -> Result<reqwest::Response> {
        if let Auth::Context { tokens, .. } = &self.auth
            && tokens.expires_at_ms <= now_ms() + 60_000
        {
            self.refresh().await?;
        }
        let mut retried = false;

        loop {
            let mut req = self.http.request(method.clone(), format!("{}{path}", self.server));

            if let Some(t) = self.bearer() {
                req = req.bearer_auth(t);
            }

            if let Some(k) = key {
                req = req.header("Idempotency-Key", k);
            }

            if let Some(t) = self.request_timeout {
                req = req.timeout(t);
            }

            match &body {
                Payload::None => {}
                Payload::Json(b) => req = req.json(b),
                Payload::Bytes(b) => {
                    req = req.header(reqwest::header::CONTENT_TYPE, "application/octet-stream").body(b.clone());
                }
                Payload::File { path, len, content_type } => {
                    let f = tokio::fs::File::open(path)
                        .await
                        .map_err(|e| CliError::usage(format!("{}: {e}", path.display())))?;
                    req = req
                        .header(reqwest::header::CONTENT_TYPE, *content_type)
                        .header(reqwest::header::CONTENT_LENGTH, *len)
                        .body(reqwest::Body::from(f));
                }
            }

            let resp = req.send().await.map_err(|e| transport(&e))?;
            let status = resp.status();
            if status == StatusCode::UNAUTHORIZED && !retried && matches!(self.auth, Auth::Context { .. }) {
                retried = true;
                self.refresh().await?;

                continue;
            }

            if status.is_redirection() {
                return Err(CliError::new(
                    Exit::Transport,
                    format!("unexpected redirect from {path} (redirects are not followed)"),
                ));
            }

            if !status.is_success() {
                let retry_after = resp
                    .headers()
                    .get(reqwest::header::RETRY_AFTER)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.trim().parse::<u64>().ok());
                let mut e = problem(resp).await;
                e.retry_after = retry_after;

                return Err(e);
            }

            return Ok(resp);
        }
    }

    /// Sends a request; JSON in, JSON out. 204 → `Value::Null`.
    pub async fn call(&mut self, method: Method, path: &str, body: Option<&Value>) -> Result<Value> {
        let resp = self.send(method, path, body).await?;
        if resp.status() == StatusCode::NO_CONTENT {
            return Ok(Value::Null);
        }

        let text = resp.text().await.map_err(|e| transport(&e))?;

        Ok(serde_json::from_str(&text).unwrap_or(Value::String(text)))
    }

    pub async fn get(&mut self, path: &str) -> Result<Value> {
        self.call(Method::GET, path, None).await
    }

    pub async fn post(&mut self, path: &str, body: &Value) -> Result<Value> {
        self.call(Method::POST, path, Some(body)).await
    }

    /// Revokes the session server-side (best effort; reports the outcome).
    pub async fn logout(&mut self) -> Result<()> {
        self.call(Method::POST, "/api/v1/auth/logout", None).await.map(|_| ())
    }
}

/// Parses an OAuth token response into stored tokens.
pub fn token_pair(server: &str, v: &Value) -> Result<Tokens> {
    let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
    let access_token = s("access_token").ok_or_else(|| CliError::auth("token response without access_token"))?;
    let refresh_token = s("refresh_token").ok_or_else(|| CliError::auth("token response without refresh_token"))?;
    let expires_in = v.get("expires_in").and_then(Value::as_i64).unwrap_or(3600);

    Ok(Tokens { server: server.to_owned(), access_token, refresh_token, expires_at_ms: now_ms() + expires_in * 1000 })
}
