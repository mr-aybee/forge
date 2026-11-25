//! Test harness: an in-process control plane on ephemeral ports and an API
//! client that handles cookies and CSRF like the web UI does.
#![allow(dead_code, clippy::unwrap_used)]

use std::time::Duration;

use forge_server::{App, ServerConfig, Services, bootstrap};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use tokio::net::TcpListener;

pub const ADMIN_EMAIL: &str = "admin@example.com";
pub const PASSWORD: &str = "correct horse battery staple";

/// Trash and revision store for in-process agents, instead of
/// `C:\ProgramData`. `forge_agent::files::store_override` is process-wide and
/// the first call wins, and all integration tests share one process, so every
/// test passes this directory: it lives as long as the process, and no test's
/// `TempDir` can delete it while another test's agent still writes to it.
/// It sits under Cargo's `target/tmp`, one per process.
pub fn agent_store() -> &'static std::path::Path {
    static DIR: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    DIR.get_or_init(|| tempfile::Builder::new().prefix("agent-store-").tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap())
        .path()
}

pub struct Harness {
    pub app: App,
    pub base: String,
    pub agent_url: String,
    pub default_org: String,
    pub dir: tempfile::TempDir,
}

pub fn test_config(dir: &std::path::Path) -> ServerConfig {
    ServerConfig {
        data_dir: dir.to_path_buf(),
        cookie_secure: false,
        // Tests that fail sign-ins on purpose would otherwise need the image
        // CAPTCHA; `captcha.rs` tests turn it on.
        security: forge_server::config::SecuritySettings {
            captcha_after_failures: 0,
            // Off by default (D-064, revised); tests cover the opt-in
            // hardened mode, where guarded actions need MFA.
            require_mfa: true,
            ..forge_server::config::SecuritySettings::default()
        },
        heartbeat_hold_secs: 1,
        offline_after_secs: 5,
        agent_hostnames: vec!["localhost".into(), "127.0.0.1".into()],
        jobs: forge_server::config::JobSettings { lease_secs: 5, concurrency: 4, poll_ms: 50 },
        // The object-storage tests run an S3 stand-in on 127.0.0.1.
        storage_allow_loopback: true,
        // Never write this machine's hosts file from tests (D-084); the names
        // tests turn it on with agents that refuse the command.
        names: forge_server::config::NameSettings { enabled: false, ..Default::default() },
        // No analytics worker unless a test asks for one (`analytics_worker.rs`).
        analytics: forge_server::config::AnalyticsSettings {
            engine: forge_server::config::AnalyticsEngine::Off,
            // A free port: never the default 55132, which a worker of a
            // running Forge on this machine may hold.
            worker_agent_listen: std::net::SocketAddr::from(([127, 0, 0, 1], 0)),
            ..Default::default()
        },
        // Alert evaluation and notification passes run often, so tests
        // wait for an alarm or a message in well under a second per pass.
        alerts: forge_server::config::AlertSettings {
            evaluate_interval_ms: 500,
            deliver_interval_ms: 250,
            ..Default::default()
        },
        ..ServerConfig::default()
    }
}

pub async fn start() -> Harness {
    start_configured(|_| {}).await
}

/// Like [`start_configured`]; the name the local-names tests use.
#[allow(dead_code)]
pub async fn start_with(adjust: impl FnOnce(&mut ServerConfig)) -> Harness {
    start_configured(adjust).await
}

/// [`start`] with a changed configuration.
pub async fn start_configured(change: impl FnOnce(&mut ServerConfig)) -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let mut config = test_config(dir.path());
    change(&mut config);
    let api = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let agent = TcpListener::bind("127.0.0.1:0").await.unwrap();
    config.public_url = Some(format!("http://{}", api.local_addr().unwrap()));
    let state = Services::build(config).await.unwrap();
    let init = bootstrap::init_admin(&state, None, Some(ADMIN_EMAIL), "Admin", PASSWORD, false).await.unwrap();
    let app = App::start_with(state, api, agent).unwrap();

    Harness {
        base: format!("http://{}", app.api_addr),
        agent_url: format!("https://127.0.0.1:{}", app.agent_addr.port()),
        default_org: init.workspace_id.to_string(),
        app,
        dir,
    }
}

/// A logged-in API client (cookie session + CSRF header).
pub struct Client {
    pub http: reqwest::Client,
    pub base: String,
    pub csrf: Option<String>,
    pub bearer: Option<String>,
}

impl Client {
    pub fn anonymous(base: &str) -> Self {
        let http = reqwest::Client::builder()
            .cookie_store(true)
            .redirect(reqwest::redirect::Policy::none())
            // No idle pooling: tests restart servers on the same port.
            .pool_max_idle_per_host(0)
            .build()
            .unwrap();
        Self { http, base: base.to_owned(), csrf: None, bearer: None }
    }

    pub async fn login(base: &str, email: &str, password: &str) -> Self {
        let mut c = Self::anonymous(base);
        let body = login_body(base, email, password).await;
        let (status, body) = c.call(Method::POST, "/api/v1/auth/login", Some(body)).await;
        assert_eq!(status, StatusCode::OK, "login failed: {body}");
        c.csrf = body["csrf_token"].as_str().map(str::to_owned);

        c
    }

    pub async fn call(&self, method: Method, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        let mut req = self.http.request(method, format!("{}{}", self.base, path));

        if let Some(csrf) = &self.csrf {
            req = req.header("x-csrf-token", csrf);
        }

        if let Some(b) = &self.bearer {
            req = req.bearer_auth(b);
        }

        if let Some(b) = body {
            req = req.json(&b);
        }

        let resp = req.send().await.unwrap();
        let status = resp.status();
        let text = resp.text().await.unwrap();

        (status, serde_json::from_str(&text).unwrap_or(Value::String(text)))
    }

    pub async fn get(&self, path: &str) -> (StatusCode, Value) {
        self.call(Method::GET, path, None).await
    }

    pub async fn post(&self, path: &str, body: Value) -> (StatusCode, Value) {
        self.call(Method::POST, path, Some(body)).await
    }

    pub async fn put(&self, path: &str, body: Value) -> (StatusCode, Value) {
        self.call(Method::PUT, path, Some(body)).await
    }
}

/// A sign-in body with a solved login challenge (D-064), as the console
/// sends it: `{login, password, challenge_id, nonce}`.
pub async fn login_body(base: &str, login: &str, password: &str) -> Value {
    let mut body = json!({ "login": login, "password": password });
    let (_, c) = Client::anonymous(base).get(&format!("/api/v1/auth/challenge?login={login}")).await;
    if c["required"] == true {
        let salt = forge_secrets::pow::salt_from_hex(c["salt"].as_str().unwrap()).unwrap();
        let bits = u8::try_from(c["difficulty"].as_u64().unwrap()).unwrap();
        body["challenge_id"] = c["id"].clone();
        body["nonce"] = json!(forge_secrets::pow::solve(&salt, bits));
    }

    body
}

/// Creates a user and binds `role` at `scope`, returning a logged-in client.
/// A new user with `role` at `scope`, signed in (without MFA). Created
/// directly in the server: through the API, creating users and granting
/// roles needs an MFA-verified session (`require_mfa`), which tests of other
/// features need not set up. Tests of those endpoints call the API.
pub async fn user_with_role(h: &Harness, _admin: &Client, email: &str, role: &str, scope: &str) -> Client {
    use forge_core::Role;
    use forge_core::model::RoleBinding;
    let state = &h.app.state;
    let user = state.identity.create_account(None, Some(email), email, PASSWORD).await.unwrap();
    let role: Role = serde_json::from_value(json!(role)).unwrap();
    let binding = RoleBinding {
        id: forge_core::RoleBindingId::new(),
        user_id: user.id,
        role,
        scope: scope.parse().unwrap(),
        created_at: state.clock.now(),
    };
    state.grant(&binding).await.unwrap();

    Client::login(&h.base, email, PASSWORD).await
}

/// Enrolls an authenticator for the signed-in user of `c` and steps up, so
/// `c` is MFA-verified. Returns the TOTP secret.
pub async fn step_up(c: &mut Client) -> Vec<u8> {
    use forge_identity::totp;
    let (_, t) = c.post("/api/v1/auth/totp", json!({})).await;
    let secret = totp::base32_decode(t["secret"].as_str().unwrap()).unwrap();
    let now = || {
        i64::try_from(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()).unwrap()
    };
    let (s, b) = c
        .post(
            "/api/v1/auth/totp/confirm",
            json!({"password": PASSWORD, "secret": t["secret"], "code": format!("{:06}", totp::code_at(&secret, now()))}),
        )
        .await;
    assert_eq!(s, StatusCode::NO_CONTENT, "{b}");
    let (s, r) =
        c.post("/api/v1/auth/step-up", json!({"code": format!("{:06}", totp::code_at(&secret, now() + 30))})).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    c.csrf = r["csrf_token"].as_str().map(str::to_owned);

    secret
}

/// Polls a job until it reaches a terminal state.
pub async fn wait_job(c: &Client, id: &str, timeout: Duration) -> Value {
    let deadline = tokio::time::Instant::now() + timeout;

    loop {
        let (_, job) = c.get(&format!("/api/v1/jobs/{id}")).await;
        if matches!(job["status"].as_str(), Some("succeeded" | "failed" | "cancelled")) {
            return job;
        }

        assert!(tokio::time::Instant::now() < deadline, "job {id} did not finish: {job}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// How long a negative check ("nothing more is sent") waits: four alert
/// evaluation passes, and so at least as many notification passes, under
/// the harness' configuration. Nothing can be polled for here, since the
/// check is that no state appears.
pub fn quiet_period(h: &Harness) -> Duration {
    let a = &h.app.state.config.alerts;
    Duration::from_millis(4 * a.evaluate_interval_ms.max(a.deliver_interval_ms))
}
