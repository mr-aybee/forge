//! Control-plane configuration (`forge.toml`, overridable by environment).

use std::net::SocketAddr;
use std::path::PathBuf;

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ServerConfig {
    /// Directory for the SQLite database, CA, keys and plugin artifacts.
    pub data_dir: PathBuf,
    /// `sqlite:...` or `postgres://...`; defaults to `<data_dir>/forge.db`.
    /// Older single-line form of `[database]`; when set it wins.
    pub database_url: Option<String>,
    /// The operational (transactional) database.
    pub database: DatabaseSettings,
    /// The analytics database: node history and logs.
    pub analytics: AnalyticsSettings,
    /// REST API + UI listener. Bind to localhost and put Caddy in front for
    /// public TLS, or bind privately.
    pub api_listen: SocketAddr,
    /// Agent mTLS listener.
    pub agent_listen: SocketAddr,
    /// Names/IPs in the agent listener's certificate.
    pub agent_hostnames: Vec<String>,
    /// Pinned external URL of the API/UI (D-167). Optional: unset means
    /// "auto", and request-time links use the address the client used.
    /// Background links (alerts, OIDC) need it, the runtime public address,
    /// or the address an administrator last signed in on.
    pub public_url: Option<String>,
    /// Agent endpoint printed in enrollment instructions.
    pub agent_public_url: String,
    /// Mark session cookies `Secure` (keep `true` outside local development).
    pub cookie_secure: bool,
    /// Serve the built UI from this directory.
    pub ui_dir: Option<PathBuf>,
    /// API URL as reachable *from managed nodes* (installer downloads).
    /// Defaults to `public_url`.
    pub node_public_url: Option<String>,
    /// Directory with `forge-agent-linux-<arch>` binaries handed to nodes.
    /// Defaults to `<data_dir>/dist`.
    pub agent_dist_dir: Option<PathBuf>,
    /// Seconds the hub holds an idle long-poll before answering.
    pub heartbeat_hold_secs: u32,
    /// Agents not seen for this long are marked offline.
    pub offline_after_secs: u32,
    /// Seconds a container shell waits for the node to start it.
    pub container_shell_open_secs: u32,
    /// Path of `forge-plugin-runner`; defaults to the binary next to this one.
    pub plugin_runner: Option<PathBuf>,
    /// Trusted plugin publishers.
    pub trusted_publishers: Vec<TrustedPublisher>,
    pub oidc: Option<OidcSettings>,
    pub jobs: JobSettings,
    /// Deployment environment (shown in the console; production warns
    /// loudly when MFA is not required).
    pub environment: DeploymentMode,
    /// **Deprecated** alias of `[security] require_mfa = false`.
    pub dev_skip_mfa: bool,
    /// MFA requirement and the login challenge.
    pub security: SecuritySettings,
    /// Lets object-store endpoints use loopback addresses (labs, development
    /// and tests). Off by default: the control plane never calls its own
    /// host for a store. Link-local and cloud-metadata addresses are refused
    /// either way.
    pub storage_allow_loopback: bool,
    /// Self-update from the Forge release feed.
    pub update: UpdateSettings,
    /// Forge-managed SFTP gateway for temporary file access.
    pub sftp: SftpSettings,
    /// `[remote_access]` from before D-100 (WireGuard remote access, gone).
    /// Accepted and ignored, with a warning, so an old `forge.toml` still
    /// loads.
    pub remote_access: Option<toml::Table>,
    /// MCP endpoint for AI agents (connectors).
    pub mcp: McpSettings,
    /// Fleet-wide local names in every node's hosts file (D-084).
    pub names: NameSettings,
    /// The server host's forge-dns, the second tier behind each node's
    /// (D-175). Off while `listen` is empty.
    pub dns: DnsSettings,
    /// Alert rules and alarms (D-087).
    pub alerts: AlertSettings,
    /// Port tunnels from the CLI (D-091, D-100).
    pub tunnels: TunnelSettings,
    /// The control-plane key agents pin and its rotation (D-167).
    pub agent_tls: AgentTlsSettings,
    /// The file this configuration was read from (set by `load`).
    #[serde(skip)]
    pub config_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct SftpSettings {
    /// Run the SFTP gateway and allow temporary access grants.
    pub enabled: bool,
    /// SFTP listener.
    pub listen: SocketAddr,
    /// Host name clients connect to (shown with each grant); defaults to the
    /// host of `public_url`.
    pub public_host: Option<String>,
    /// Port clients connect to, if a proxy maps it; defaults to `listen`'s.
    pub public_port: Option<u16>,
    /// Longest lifetime of a grant, in hours.
    pub max_hours: u32,
    /// Allow grants that never expire (issued only by callers with
    /// `file.admin`). Off by default: `max_hours` then caps every grant.
    pub allow_never_expiry: bool,
}

impl Default for SftpSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            listen: SocketAddr::from(([0, 0, 0, 0], 2022)),
            public_host: None,
            public_port: None,
            max_hours: 7 * 24,
            allow_never_expiry: false,
        }
    }
}

/// `[agent_tls]`: rotation of the control-plane key that agents pin (D-167).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct AgentTlsSettings {
    /// Days a key serves before the server switches to the next one
    /// (after `grace_days` and once every online agent has it); 0: only
    /// `forge-server agent-key rotate` switches.
    pub rotate_days: u32,
    /// Days the next key is announced to agents before a switch, and days
    /// agents keep a retired key afterwards.
    pub grace_days: u32,
}

impl Default for AgentTlsSettings {
    fn default() -> Self {
        Self { rotate_days: 365, grace_days: 30 }
    }
}

/// `[tunnels]`: limits and lifetimes for port tunnels (D-091, D-100).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct TunnelSettings {
    /// Open tunnels one user may have.
    pub max_per_user: u32,
    /// Tunnel connections to one node at once.
    pub max_connections_per_node: u32,
    /// Tunnel connections on this server at once.
    pub max_connections: u32,
    /// A connection without traffic for this long is closed (seconds).
    pub idle_timeout_secs: u32,
    /// Lifetime of a tunnel opened without `--for` (hours).
    pub default_lifetime_hours: u32,
    /// Longest lifetime a tunnel or a tunnel grant may ask for (hours, at
    /// most 168). A grant's own expiry caps tunnels opened through it.
    pub max_lifetime_hours: u32,
    /// Allow tunnels and grants that never expire (issued only by callers
    /// with `access.manage`). Off by default: `max_lifetime_hours` then caps
    /// everything.
    pub allow_never_expiry: bool,
}

impl Default for TunnelSettings {
    fn default() -> Self {
        Self {
            max_per_user: 8,
            max_connections_per_node: 32,
            max_connections: 256,
            idle_timeout_secs: 900,
            default_lifetime_hours: 8,
            max_lifetime_hours: 168,
            allow_never_expiry: false,
        }
    }
}

impl TunnelSettings {
    #[must_use]
    pub fn default_lifetime_ms(&self) -> i64 {
        i64::from(self.default_lifetime_hours) * 3_600_000
    }

    #[must_use]
    pub fn max_lifetime_ms(&self) -> i64 {
        i64::from(self.max_lifetime_hours) * 3_600_000
    }
}

/// `[alerts]`: limits for alert rules (D-087) and where notifications may
/// be sent (D-110).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct AlertSettings {
    /// Most alert rules one workspace may have; each pass costs rules × series.
    pub max_rules_per_workspace: u32,
    /// Notification channels may use `http://` URLs. Messages then travel
    /// unencrypted.
    pub webhook_allow_http: bool,
    /// Notification channels may target private networks (RFC 1918,
    /// carrier-grade NAT, IPv6 unique local). Link-local and metadata
    /// addresses stay refused.
    pub webhook_allow_private: bool,
    /// Notification channels may target this host's loopback interface
    /// (tests and single-host labs).
    pub webhook_allow_loopback: bool,
    /// Attempts per notification before it is recorded as failed.
    pub delivery_attempts: u32,
    /// Wait before the first retry, in seconds; it doubles per attempt.
    pub delivery_retry_base_secs: u64,
    /// Longest wait between two attempts, in seconds.
    pub delivery_retry_max_secs: u64,
    /// Milliseconds between two alert evaluation passes.
    pub evaluate_interval_ms: u64,
    /// Milliseconds between two notification plan-and-send passes.
    pub deliver_interval_ms: u64,
}

impl Default for AlertSettings {
    fn default() -> Self {
        Self {
            max_rules_per_workspace: 200,
            webhook_allow_http: false,
            webhook_allow_private: false,
            webhook_allow_loopback: false,
            delivery_attempts: 6,
            delivery_retry_base_secs: 30,
            delivery_retry_max_secs: 900,
            evaluate_interval_ms: 15_000,
            deliver_interval_ms: 5_000,
        }
    }
}

/// `[names]`: the fleet's names in every node's hosts file (D-084).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct NameSettings {
    /// Send every node the names of the workspace's nodes and services.
    pub enabled: bool,
    /// Suffix of every name (`<node>.<suffix>`, `<node>.<cluster>.<suffix>`).
    pub suffix: String,
    /// Also write the names into the hosts files of Forge containers.
    pub containers: bool,
}

impl Default for NameSettings {
    fn default() -> Self {
        Self { enabled: true, suffix: forge_catalog::names::DEFAULT_SUFFIX.into(), containers: true }
    }
}

/// `[dns]`: the server host's forge-dns.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct DnsSettings {
    /// Addresses nodes reach this host's forge-dns on (`192.0.2.10:53`).
    /// Empty: no server-host tier; `names.json` is not written here.
    pub listen: Vec<std::net::SocketAddrV4>,
    /// Resolvers forge-dns forwards other names to (default: the host's).
    pub upstreams: Vec<std::net::SocketAddr>,
    /// Where the server writes `names.json` for its forge-dns. Default
    /// `/var/lib/forge-dns/names.json`: a world-readable folder, because the
    /// data folder is private to the server and forge-dns runs as its own user.
    pub records_file: Option<PathBuf>,
}

impl DnsSettings {
    /// The `names.json` this host's forge-dns reads.
    #[must_use]
    pub fn records_path(&self) -> PathBuf {
        self.records_file
            .clone()
            .unwrap_or_else(|| PathBuf::from("/var/lib/forge-dns").join(forge_catalog::names::RECORDS_FILE))
    }

    /// The addresses containers and nodes use as the second resolver.
    #[must_use]
    pub fn second_tier(&self) -> Vec<std::net::Ipv4Addr> {
        self.listen.iter().map(|a| *a.ip()).collect()
    }
}

/// MCP for AI agents (issue #6): `/mcp`, OAuth discovery, dynamic client
/// registration and consent.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct McpSettings {
    /// Serve `/mcp` and let connectors register and ask for consent.
    pub enabled: bool,
    /// The highest mode a person may give a connector.
    pub max_mode: forge_core::model::AgentMode,
    /// Dynamic client registrations accepted per hour (all callers).
    pub registrations_per_hour: u64,
}

impl Default for McpSettings {
    fn default() -> Self {
        Self { enabled: false, max_mode: forge_core::model::AgentMode::Auto, registrations_per_hour: 30 }
    }
}

/// `[security]`: whether MFA is required and the login challenge (D-064).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct SecuritySettings {
    /// Opt-in hardening: MFA-guarded actions need a verified second factor
    /// (`forge-server mfa on`). Off by default: MFA is each user's choice and
    /// protects their sign-in only; sign-in always asks an enrolled user for
    /// the code.
    pub require_mfa: bool,
    /// Sign-in needs a solved proof-of-work challenge (brute-force guard).
    pub login_challenge: bool,
    /// Failed sign-ins (per sign-in name, or per client address) after which
    /// the next sign-in also needs the image CAPTCHA. `0` turns it off.
    pub captcha_after_failures: u32,
    /// Absolute lifetime of an app's refresh token in days, counted from the
    /// original sign-in; refreshing never extends it (1-365).
    pub refresh_token_days: u32,
    /// Reverse proxies (IP addresses or CIDRs) whose `X-Forwarded-For` is
    /// trusted to find the client address for per-address sign-in limits.
    pub trusted_proxies: Vec<String>,
    /// API keys and service-account tokens marked elevated may run
    /// MFA-guarded actions without a prompt (D-097). When false, no elevated
    /// token can be created and existing ones act as normal tokens.
    pub allow_elevated_tokens: bool,
    /// `[security.access]`: who may reach the control plane (D-161).
    pub access: AccessSettings,
    /// `[security.rate_limit]`: requests per client address (D-161).
    pub rate_limit: RateLimitSettings,
}

/// `[security.access]`: CIDR lists (IP addresses or `addr/prefix`). Agents
/// use their own listener and are not affected.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct AccessSettings {
    /// When not empty, only these reach the console, API, MCP and OAuth.
    pub allow: Vec<String>,
    /// Refused everywhere, before any other check.
    pub deny: Vec<String>,
    /// When not empty, only these may call deploy webhooks (`/hooks/*`).
    pub webhooks_allow: Vec<String>,
    /// When not empty, only these may fetch the node installers and agent
    /// downloads (`/install.sh`, `/install.ps1`, `/agent/download/*`).
    pub nodes_allow: Vec<String>,
}

/// `[security.rate_limit]`: a sliding minute per client address.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RateLimitSettings {
    pub enabled: bool,
    /// Requests from one address (an IPv6 /64) per minute.
    pub per_ip_per_minute: u32,
}

impl Default for RateLimitSettings {
    fn default() -> Self {
        Self { enabled: true, per_ip_per_minute: 1200 }
    }
}

impl Default for SecuritySettings {
    fn default() -> Self {
        Self {
            require_mfa: false,
            login_challenge: true,
            captcha_after_failures: 2,
            refresh_token_days: 30,
            trusted_proxies: Vec::new(),
            allow_elevated_tokens: true,
            access: AccessSettings::default(),
            rate_limit: RateLimitSettings::default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct UpdateSettings {
    /// Check the feed every six hours and offer updates in the console.
    pub enabled: bool,
    /// GitHub "latest release" API URL (or a mirror with the same shape).
    pub feed_url: String,
    /// Base64url Ed25519 key releases are signed with; defaults to Forge's.
    pub public_key: Option<String>,
    /// GitHub-compatible "list releases" URL; default: `feed_url` without
    /// its trailing `/latest`.
    pub releases_url: Option<String>,
    /// How long one batch of an agent rollout (and one node's update) may
    /// take: waiting for an offline node, the download, the swap and the
    /// reconnect. A node still offline at the end is skipped. Seconds,
    /// 1 to 86400; default 1800.
    pub agent_batch_timeout_secs: u64,
}

impl Default for UpdateSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            feed_url: crate::update::DEFAULT_FEED.into(),
            public_key: None,
            releases_url: None,
            agent_batch_timeout_secs: 1800,
        }
    }
}

impl UpdateSettings {
    /// [`agent_batch_timeout_secs`](Self::agent_batch_timeout_secs), clamped
    /// to 1 s .. 24 h.
    #[must_use]
    pub fn agent_batch_timeout(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.agent_batch_timeout_secs.clamp(1, 86_400))
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustedPublisher {
    pub publisher: String,
    /// Base64url Ed25519 public key.
    pub public_key: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OidcSettings {
    pub issuer_url: String,
    pub client_id: String,
    /// Name of the environment variable holding the client secret.
    pub client_secret_env: String,
    #[serde(default)]
    pub auto_provision: bool,
}

/// Default name of the operational database on a database server.
pub const DEFAULT_DB_NAME: &str = "forge_op";
/// Suffix of the analytics database name (`forge_op` → `forge_op_analytics`).
pub const ANALYTICS_DB_SUFFIX: &str = "_analytics";

/// `[database]`: the operational database.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct DatabaseSettings {
    /// Server address without the database name and without a password,
    /// e.g. `postgres://forge@db1:5432`. Unset: SQLite in `data_dir`.
    pub url: Option<String>,
    /// Database name on that server (default `forge_op`).
    pub name: Option<String>,
    /// File holding the password (mode 0600). `FORGE_DATABASE_PASSWORD`
    /// overrides it. Never put the password in `url`.
    pub password_file: Option<PathBuf>,
    /// Read from `password_file` or the environment by `load`.
    #[serde(skip)]
    pub password: Option<String>,
}

/// Which engine stores analytics (node history, logs).
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, serde::Serialize, clap::ValueEnum, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum AnalyticsEngine {
    /// No analytics: live views only.
    Off,
    /// The ClickHouse engine run by Forge itself on `127.0.0.1` (a separate,
    /// supervised process; the engine is a separate download). `chdb` is
    /// accepted as an alias.
    #[serde(alias = "chdb")]
    #[value(alias = "chdb")]
    Embedded,
    /// A ClickHouse server, local or remote.
    Clickhouse,
    /// DuckDB inside the analytics worker (D-143): no separate engine
    /// process, one database file per day under `<data_dir>/analytics/duckdb`.
    /// The default (D-144): small and medium installs.
    #[default]
    Duckdb,
    /// A PostgreSQL database (small installs).
    Postgres,
    /// Rows kept in the analytics worker's memory only, for tests and
    /// trials: nothing survives a restart.
    Memory,
}

/// `[analytics]`: the analytics database.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct AnalyticsSettings {
    pub engine: AnalyticsEngine,
    /// Server address for `clickhouse` and `postgres`, without a password.
    pub url: Option<String>,
    /// Database name (default: the operational name + `_analytics`).
    pub name: Option<String>,
    /// File holding the password; `FORGE_ANALYTICS_PASSWORD` overrides it.
    pub password_file: Option<PathBuf>,
    pub retention: AnalyticsRetention,
    /// `embedded`: the loopback HTTP port of the engine.
    pub embedded_port: u16,
    /// `embedded`: memory cap of the engine in MiB.
    pub embedded_memory_mb: u32,
    /// `duckdb`: the memory limit of DuckDB in MiB (setting `memory_budget_mb`).
    pub duckdb_memory_mb: u32,
    /// `embedded`: the engine binary; default `<data_dir>/analytics/bin/clickhouse`.
    pub embedded_binary: Option<PathBuf>,
    /// `embedded`: CPU cap of the engine in vCPUs, rounded up to whole
    /// CPUs it is pinned to (`0`: none).
    pub cpu_quota_vcpu: f64,
    /// Removed (D-140): the memory budget is the engine's hard cap. Still
    /// accepted so an older `forge.toml` loads; ignored with a warning.
    #[serde(rename = "memory_limit_mb")]
    pub removed_memory_limit_mb: Option<u32>,
    /// Log ingest refuses batches while the engine has less free disk, MiB.
    pub min_free_disk_mb: u32,
    pub search: AnalyticsSearch,
    pub ingest: AnalyticsIngest,
    /// The analytics worker is restarted when its resident memory passes
    /// this, MiB (`0`: no limit). The embedded engine has its own budget.
    pub worker_memory_limit_mb: u32,
    /// Path of `forge-analytics-worker`; defaults to the binary next to this one.
    pub worker_binary: Option<PathBuf>,
    /// Where the worker accepts agent spool uploads (mTLS, D-142).
    pub worker_agent_listen: SocketAddr,
    /// The URL agents upload to; default: the host of `agent_public_url`
    /// with the port of `worker_agent_listen`. Its host must be one of
    /// `agent_hostnames` (the worker's certificate names them).
    pub worker_agent_url: Option<String>,
    /// Spool files the worker imports at once, `MIN_UPLOADS` to
    /// `MAX_UPLOADS`; more get 429 at once. Higher values store more per
    /// second and use more network. Unset: `MAX_UPLOADS` for `duckdb`
    /// (more files per write batch, D-144), `MIN_UPLOADS` otherwise; see
    /// [`AnalyticsSettings::max_uploads`].
    pub worker_max_uploads: Option<u32>,
    pub spool: AnalyticsSpool,
    pub listener: AnalyticsListener,
    /// Read from `password_file` or the environment by `load`.
    #[serde(skip)]
    pub password: Option<String>,
}

impl AnalyticsSettings {
    /// Spool files the worker imports at once: `worker_max_uploads`, or the
    /// engine's default. DuckDB writes every file that waits as one batch,
    /// so more slots mean larger batches and no 429 bursts when many nodes
    /// upload at once; ClickHouse groups inserts itself.
    #[must_use]
    pub fn max_uploads(&self) -> u32 {
        use forge_analytics_proto::config::{MAX_UPLOADS, MIN_UPLOADS};
        self.worker_max_uploads.unwrap_or(if self.engine == AnalyticsEngine::Duckdb {
            MAX_UPLOADS
        } else {
            MIN_UPLOADS
        })
    }
}

impl Default for AnalyticsSettings {
    fn default() -> Self {
        Self {
            engine: AnalyticsEngine::Duckdb,
            url: None,
            name: None,
            password_file: None,
            retention: AnalyticsRetention::default(),
            embedded_port: 18123,
            embedded_memory_mb: 1024,
            duckdb_memory_mb: 256,
            embedded_binary: None,
            cpu_quota_vcpu: 0.0,
            removed_memory_limit_mb: None,
            min_free_disk_mb: 1024,
            search: AnalyticsSearch::default(),
            ingest: AnalyticsIngest::default(),
            worker_memory_limit_mb: 2048,
            worker_binary: None,
            // The installers write 3832 for new installs. The default stays 55132 so
            // an install whose forge.toml never named the port keeps it on update.
            worker_agent_listen: SocketAddr::from(([0, 0, 0, 0], 55132)),
            worker_agent_url: None,
            worker_max_uploads: None,
            spool: AnalyticsSpool::default(),
            listener: AnalyticsListener::default(),
            password: None,
        }
    }
}

/// `[analytics.spool]`: the node spool every agent keeps (D-142), sent to
/// nodes in heartbeat responses. `0` keeps the agent's default.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct AnalyticsSpool {
    /// Most disk the spool uses, MiB (default 512).
    pub max_mb: u64,
    /// Nothing is written below this much free disk, MiB (default 256).
    pub min_free_mb: u64,
    /// Files older than this are dropped, hours (default 72).
    pub max_age_hours: u64,
    /// A file is sealed at this size, KiB (default 4096) ...
    pub seal_kb: u64,
    /// ... or this long after its first row, seconds (default 10).
    pub seal_secs: u64,
    /// zstd level 1 to 3 (default 1).
    pub zstd_level: u32,
}

impl AnalyticsSpool {
    #[must_use]
    pub fn wire(&self) -> forge_proto::messages::SpoolTuning {
        const MIB: u64 = 1024 * 1024;
        forge_proto::messages::SpoolTuning {
            max_bytes: self.max_mb.saturating_mul(MIB),
            min_free_bytes: self.min_free_mb.saturating_mul(MIB),
            max_age_secs: self.max_age_hours.saturating_mul(3_600),
            seal_bytes: self.seal_kb.saturating_mul(1024),
            seal_secs: self.seal_secs,
            zstd_level: self.zstd_level,
        }
    }
}

/// `[analytics.listener]`: the local log listener on every node (D-142).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct AnalyticsListener {
    /// Largest request body, KiB (default 1024).
    pub max_body_kb: u64,
    /// Requests per second per key on addresses other than loopback (default 50).
    pub rate_per_sec: u32,
    /// Apply the rate cap on loopback too.
    pub rate_on_loopback: bool,
}

impl AnalyticsListener {
    #[must_use]
    pub fn wire(&self) -> forge_proto::messages::ListenerTuning {
        forge_proto::messages::ListenerTuning {
            max_body_bytes: self.max_body_kb.saturating_mul(1024),
            rate_per_sec: self.rate_per_sec,
            rate_on_loopback: self.rate_on_loopback,
            extra_address: String::new(),
        }
    }
}

/// `[analytics.search]`: limits for log searches (D-136). `0` keeps the
/// engine's default where noted.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct AnalyticsSearch {
    /// Searches running at once; more answer 503.
    pub max_concurrent: u32,
    /// Seconds one search may run.
    pub timeout_secs: u32,
    /// How far back a search may reach, days (`0`: no limit).
    pub max_lookback_days: u32,
    /// Threads per search (`0`: engine default).
    pub threads: u32,
    /// Memory per search, MiB (`0`: engine default).
    pub memory_mb: u32,
}

impl Default for AnalyticsSearch {
    fn default() -> Self {
        Self { max_concurrent: 8, timeout_secs: 20, max_lookback_days: 0, threads: 0, memory_mb: 0 }
    }
}

/// `[analytics.ingest]`: the log ingest queue (D-135, D-136).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct AnalyticsIngest {
    /// Bytes of log batches waiting to be stored before ingest answers 503, MiB.
    pub max_queued_mb: u32,
    /// Rows in one insert.
    pub max_batch_rows: u32,
    /// How long the writer waits for more batches before an insert, ms.
    pub flush_interval_ms: u32,
}

impl Default for AnalyticsIngest {
    fn default() -> Self {
        Self { max_queued_mb: 256, max_batch_rows: 20_000, flush_interval_ms: 100 }
    }
}

/// How long each analytics stream is kept.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct AnalyticsRetention {
    /// Per-minute node metrics.
    pub node_metrics_days: u32,
    /// Per-minute top processes (default: `node_metrics_days`).
    pub top_processes_days: Option<u32>,
    /// Hourly node rollups (trends).
    pub node_rollup_days: u32,
    /// Service logs.
    pub app_logs_days: u32,
}

impl Default for AnalyticsRetention {
    fn default() -> Self {
        Self { node_metrics_days: 7, top_processes_days: None, node_rollup_days: 365, app_logs_days: 30 }
    }
}

/// Reads a password: the environment variable first, then the file.
fn read_password(env: &str, file: Option<&std::path::Path>) -> anyhow::Result<Option<String>> {
    if let Ok(v) = std::env::var(env) {
        return Ok(Some(v));
    }

    match file {
        Some(p) => {
            let raw = std::fs::read_to_string(p)
                .map_err(|e| anyhow::anyhow!("reading password file {}: {e}", p.display()))?;
            Ok(Some(raw.lines().next().unwrap_or_default().to_string()))
        }
        None => Ok(None),
    }
}

/// `base` (no database, no password) with `name` as the path and the
/// password filled in. Fails when `base` already carries a password.
pub fn compose_url(base: &str, name: &str, password: Option<&str>) -> anyhow::Result<String> {
    let mut u = url::Url::parse(base).map_err(|e| anyhow::anyhow!("invalid database url: {e}"))?;
    if u.password().is_some() {
        anyhow::bail!("put the database password in password_file or the environment, not in the url");
    }

    if let Some(p) = password {
        u.set_password(Some(p)).map_err(|()| anyhow::anyhow!("this url cannot carry a password"))?;
    }

    u.set_path(&format!("/{name}"));

    Ok(u.to_string())
}

/// `url` with any password replaced by `***`.
#[must_use]
pub fn redact_url(raw: &str) -> String {
    match url::Url::parse(raw) {
        Ok(mut u) if u.password().is_some() => {
            let _ = u.set_password(Some("***"));
            u.to_string()
        }
        Ok(u) => u.to_string(),
        Err(_) => raw.to_string(),
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct JobSettings {
    pub lease_secs: u64,
    pub concurrency: usize,
    pub poll_ms: u64,
}

impl Default for JobSettings {
    fn default() -> Self {
        Self { lease_secs: 30, concurrency: 8, poll_ms: 500 }
    }
}

/// Fallback console URL for background node work when no `public_url` or
/// `node_public_url` is configured.
pub const DEFAULT_PUBLIC_URL: &str = "http://localhost:3880";

/// Checks that a configured console URL is one browsers and nodes can
/// reach: an `http`/`https` origin whose host is not a wildcard address
/// (`0.0.0.0`, `::`), which listeners accept but no client can dial.
pub fn check_reachable_url(url: &str) -> anyhow::Result<()> {
    let canonical = crate::public_address::validate(url).map_err(|e| anyhow::anyhow!("{e}"))?;
    let u = url::Url::parse(&canonical)?;
    let unspecified = match u.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_unspecified(),
        Some(url::Host::Ipv6(ip)) => ip.is_unspecified(),
        Some(url::Host::Domain(_)) => false,
        None => true,
    };

    if unspecified {
        anyhow::bail!(
            "`{url}` is a listen-everywhere address, not one a browser can open; use this machine's name or IP, or leave public_url unset"
        );
    }

    Ok(())
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            data_dir: PathBuf::from(".forge-data"),
            database_url: None,
            database: DatabaseSettings::default(),
            analytics: AnalyticsSettings::default(),
            api_listen: SocketAddr::from(([127, 0, 0, 1], 3880)),
            agent_listen: SocketAddr::from(([0, 0, 0, 0], 3843)),
            agent_hostnames: vec!["localhost".into(), "127.0.0.1".into()],
            public_url: None,
            agent_public_url: "https://localhost:3843".into(),
            cookie_secure: true,
            ui_dir: None,
            node_public_url: None,
            agent_dist_dir: None,
            update: UpdateSettings::default(),
            sftp: SftpSettings::default(),
            remote_access: None,
            mcp: McpSettings::default(),
            names: NameSettings::default(),
            dns: DnsSettings::default(),
            alerts: AlertSettings::default(),
            tunnels: TunnelSettings::default(),
            agent_tls: AgentTlsSettings::default(),
            config_path: None,
            heartbeat_hold_secs: 25,
            offline_after_secs: 90,
            container_shell_open_secs: 15,
            plugin_runner: None,
            trusted_publishers: vec![],
            oidc: None,
            jobs: JobSettings::default(),
            environment: DeploymentMode::Production,
            dev_skip_mfa: false,
            security: SecuritySettings::default(),
            storage_allow_loopback: false,
        }
    }
}

impl ServerConfig {
    /// Loads `path` if it exists, then applies `FORGE_*` environment overrides.
    pub fn load(path: Option<&std::path::Path>) -> anyhow::Result<Self> {
        let mut cfg: Self = match path {
            Some(p) if p.exists() => toml::from_str(&std::fs::read_to_string(p)?)?,
            Some(p) => anyhow::bail!("config file {} not found", p.display()),
            None => Self::default(),
        };

        if let Ok(v) = std::env::var("FORGE_DATA_DIR") {
            cfg.data_dir = v.into();
        }

        if let Ok(v) = std::env::var("FORGE_DATABASE_URL") {
            cfg.database_url = Some(v);
        }

        cfg.database.password = read_password("FORGE_DATABASE_PASSWORD", cfg.database.password_file.as_deref())?;
        cfg.analytics.password = read_password("FORGE_ANALYTICS_PASSWORD", cfg.analytics.password_file.as_deref())?;

        if let Some(base) = &cfg.database.url {
            compose_url(base, cfg.database_name(), None)?;
        }

        if let Some(base) = &cfg.analytics.url {
            compose_url(base, &cfg.analytics_name(), None)?;
        }

        if let Ok(v) = std::env::var("FORGE_API_LISTEN") {
            cfg.api_listen = v.parse()?;
        }

        if let Ok(v) = std::env::var("FORGE_AGENT_LISTEN") {
            cfg.agent_listen = v.parse()?;
        }

        if let Ok(v) = std::env::var("FORGE_PUBLIC_URL") {
            cfg.public_url = Some(v).filter(|v| !v.trim().is_empty());
        }

        if let Ok(v) = std::env::var("FORGE_COOKIE_SECURE") {
            cfg.cookie_secure = v != "false" && v != "0";
        }

        if let Ok(v) = std::env::var("FORGE_STORAGE_ALLOW_LOOPBACK") {
            cfg.storage_allow_loopback = v == "true" || v == "1";
        }

        if let Ok(v) = std::env::var("FORGE_MCP_ENABLED") {
            cfg.mcp.enabled = v == "true" || v == "1";
        }

        if let Ok(v) = std::env::var("FORGE_MCP_MAX_MODE") {
            cfg.mcp.max_mode = forge_core::model::AgentMode::parse(&v)
                .ok_or_else(|| anyhow::anyhow!("FORGE_MCP_MAX_MODE: read_only, ask or auto"))?;
        }

        if let Ok(v) = std::env::var("FORGE_SFTP_LISTEN") {
            cfg.sftp.listen = v.parse().map_err(|e| anyhow::anyhow!("FORGE_SFTP_LISTEN: {e}"))?;
            cfg.sftp.enabled = true;
        }

        if let Ok(v) = std::env::var("FORGE_NAMES_SUFFIX") {
            cfg.names.suffix = v;
        }

        if let Ok(v) = std::env::var("FORGE_ENVIRONMENT") {
            cfg.environment = v.parse()?;
        }

        if let Ok(v) = std::env::var("FORGE_DEV_SKIP_MFA") {
            cfg.dev_skip_mfa = v == "true" || v == "1";
        }

        if let Ok(v) = std::env::var("FORGE_REQUIRE_MFA") {
            cfg.security.require_mfa = !matches!(v.trim(), "false" | "0" | "off" | "no");
        }

        if let Ok(v) = std::env::var("FORGE_ALLOW_ELEVATED_TOKENS") {
            cfg.security.allow_elevated_tokens = !matches!(v.trim(), "false" | "0" | "off" | "no");
        }

        if let Ok(v) = std::env::var("FORGE_LOGIN_CHALLENGE") {
            cfg.security.login_challenge = !matches!(v.trim(), "false" | "0" | "off" | "no");
        }

        if let Ok(v) = std::env::var("FORGE_CAPTCHA_AFTER_FAILURES") {
            cfg.security.captcha_after_failures =
                v.trim().parse().map_err(|e| anyhow::anyhow!("FORGE_CAPTCHA_AFTER_FAILURES: {e}"))?;
        }

        if cfg.dev_skip_mfa {
            tracing::warn!("dev_skip_mfa is deprecated: use [security] require_mfa = false (`forge-server mfa off`)");
            cfg.security.require_mfa = false;
        }

        if cfg.remote_access.is_some() {
            tracing::warn!("[remote_access] is ignored: WireGuard remote access was replaced by port tunnels (D-100)");
        }

        cfg.config_path = path.map(std::path::Path::to_path_buf);
        cfg.validate()?;

        Ok(cfg)
    }

    /// Rejects unsafe combinations.
    pub fn validate(&self) -> anyhow::Result<()> {
        let limit = self.analytics.worker_memory_limit_mb;
        if limit != 0 && limit < 128 {
            anyhow::bail!("[analytics] worker_memory_limit_mb must be 0 (no limit) or at least 128");
        }

        if self.analytics.spool.zstd_level > 3 {
            anyhow::bail!("[analytics.spool] zstd_level must be 0 (default) or 1 to 3");
        }

        let (min_up, max_up) = (forge_analytics_proto::config::MIN_UPLOADS, forge_analytics_proto::config::MAX_UPLOADS);
        if !(min_up..=max_up).contains(&self.analytics.max_uploads()) {
            anyhow::bail!("[analytics] worker_max_uploads must be {min_up} to {max_up}");
        }

        if !(1..=365).contains(&self.security.refresh_token_days) {
            anyhow::bail!("[security] refresh_token_days must be 1 to 365");
        }

        if let Some(u) = &self.public_url {
            check_reachable_url(u).map_err(|e| anyhow::anyhow!("public_url: {e}"))?;
        }

        for p in &self.security.trusted_proxies {
            let net = p
                .parse::<crate::throttle::TrustedNet>()
                .map_err(|e| anyhow::anyhow!("[security] trusted_proxies: {e}"))?;

            // Trusting everyone would let any client set X-Forwarded-*.
            if net.prefix() == 0 {
                anyhow::bail!("[security] trusted_proxies: `{p}` trusts every address; list your proxies");
            }
        }

        let a = &self.security.access;

        for (key, list) in [
            ("allow", &a.allow),
            ("deny", &a.deny),
            ("webhooks_allow", &a.webhooks_allow),
            ("nodes_allow", &a.nodes_allow),
        ] {
            for n in list {
                n.parse::<crate::throttle::TrustedNet>()
                    .map_err(|e| anyhow::anyhow!("[security.access] {key}: {e}"))?;
            }
        }

        let r = &self.security.rate_limit;
        if r.enabled && r.per_ip_per_minute < 60 {
            anyhow::bail!("[security.rate_limit] per_ip_per_minute is at least 60");
        }

        // Never lock out the machine itself: with an allow list, loopback
        // and the trusted proxies must be in it.
        if !a.allow.is_empty() {
            let lists = self.access_lists();

            for must in ["127.0.0.1", "::1"].into_iter().map(str::to_owned).chain(self.security.trusted_proxies.clone())
            {
                let ip = must.split('/').next().unwrap_or(&must);
                if let Ok(ip) = ip.parse()
                    && !lists.permits(ip, "/")
                {
                    anyhow::bail!("[security.access] allow must include {must} (loopback and trusted proxies)");
                }
            }
        }

        forge_catalog::names::check_suffix(&self.names.suffix).map_err(|e| anyhow::anyhow!("[names] suffix: {e}"))?;

        if !(1..=10_000).contains(&self.alerts.max_rules_per_workspace) {
            anyhow::bail!("[alerts] max_rules_per_workspace must be 1 to 10000");
        }

        let a = &self.alerts;
        if !(1..=20).contains(&a.delivery_attempts) {
            anyhow::bail!("[alerts] delivery_attempts must be 1 to 20");
        }

        if !(1..=3600).contains(&a.delivery_retry_base_secs)
            || !(a.delivery_retry_base_secs..=86_400).contains(&a.delivery_retry_max_secs)
        {
            anyhow::bail!(
                "[alerts] delivery_retry_base_secs must be 1 to 3600, and delivery_retry_max_secs from it to 86400"
            );
        }

        if !(100..=3_600_000).contains(&a.evaluate_interval_ms) || !(100..=3_600_000).contains(&a.deliver_interval_ms) {
            anyhow::bail!("[alerts] evaluate_interval_ms and deliver_interval_ms must be 100 to 3600000");
        }

        if !(1..=300).contains(&self.container_shell_open_secs) {
            anyhow::bail!("container_shell_open_secs must be 1 to 300");
        }

        let k = &self.agent_tls;
        if k.rotate_days > 3650 || k.grace_days > 365 || (k.rotate_days > 0 && k.grace_days >= k.rotate_days) {
            anyhow::bail!(
                "[agent_tls] rotate_days must be 0 to 3650 and grace_days 0 to 365, less than rotate_days when it is set"
            );
        }

        let t = &self.tunnels;
        if t.max_per_user == 0 || t.max_connections_per_node == 0 || t.max_connections == 0 {
            anyhow::bail!("[tunnels] limits must be at least 1");
        }

        if !(60..=86_400).contains(&t.idle_timeout_secs) {
            anyhow::bail!("[tunnels] idle_timeout_secs must be 60 to 86400");
        }

        if !(1..=168).contains(&t.max_lifetime_hours) {
            anyhow::bail!("[tunnels] max_lifetime_hours must be 1 to 168");
        }

        if !(1..=t.max_lifetime_hours).contains(&t.default_lifetime_hours) {
            anyhow::bail!("[tunnels] default_lifetime_hours must be 1 to max_lifetime_hours");
        }

        Ok(())
    }

    /// `[security.access]`, parsed (checked by [`Self::validate`]).
    #[must_use]
    pub fn access_lists(&self) -> crate::net_access::Lists {
        let parse = |v: &[String]| v.iter().filter_map(|p| p.parse().ok()).collect();
        let a = &self.security.access;

        crate::net_access::Lists {
            allow: parse(&a.allow),
            deny: parse(&a.deny),
            webhooks_allow: parse(&a.webhooks_allow),
            nodes_allow: parse(&a.nodes_allow),
        }
    }

    /// `[security] trusted_proxies`, parsed (checked by [`Self::validate`]).
    #[must_use]
    pub fn trusted_proxies(&self) -> Vec<crate::throttle::TrustedNet> {
        self.security.trusted_proxies.iter().filter_map(|p| p.parse().ok()).collect()
    }

    /// Whether elevated tokens may run MFA-guarded actions (D-097).
    #[must_use]
    pub fn elevated_tokens_allowed(&self) -> bool {
        self.security.allow_elevated_tokens
    }

    /// Whether MFA-guarded actions need a verified second factor. The
    /// operator's choice per installation, in any environment (D-064).
    #[must_use]
    pub fn mfa_required(&self) -> bool {
        self.security.require_mfa && !self.dev_skip_mfa
    }

    /// Connection URL of the operational database. Contains the password
    /// when one is configured: never log it (use [`Self::database_display`]).
    #[must_use]
    pub fn database_url(&self) -> String {
        if let Some(u) = &self.database_url {
            return u.clone();
        }

        if let Some(base) = &self.database.url {
            // `load` validated it, so composing cannot fail here.
            if let Ok(u) = compose_url(base, self.database_name(), self.database.password.as_deref()) {
                return u;
            }
        }

        format!("sqlite:{}?mode=rwc", self.data_dir.join("forge.db").display().to_string().replace('\\', "/"))
    }

    /// The operational database name on a database server.
    #[must_use]
    pub fn database_name(&self) -> &str {
        self.database.name.as_deref().unwrap_or(DEFAULT_DB_NAME)
    }

    /// The analytics database name (default: operational name + `_analytics`).
    #[must_use]
    pub fn analytics_name(&self) -> String {
        self.analytics.name.clone().unwrap_or_else(|| format!("{}{ANALYTICS_DB_SUFFIX}", self.database_name()))
    }

    /// Connection URL of the analytics database (for `clickhouse` and
    /// `postgres`); `None` when the engine has no server address. Contains
    /// the password: never log it.
    #[must_use]
    pub fn analytics_url(&self) -> Option<String> {
        let base = self.analytics.url.as_deref().or(match self.analytics.engine {
            // Postgres analytics defaults to the operational server.
            AnalyticsEngine::Postgres => self.database.url.as_deref(),
            _ => None,
        })?;
        let pw = self.analytics.password.as_deref().or(match (self.analytics.engine, &self.analytics.url) {
            (AnalyticsEngine::Postgres, None) => self.database.password.as_deref(),
            _ => None,
        });

        compose_url(base, &self.analytics_name(), pw).ok()
    }

    /// The operational database without its password, for messages.
    #[must_use]
    pub fn database_display(&self) -> String {
        redact_url(&self.database_url())
    }

    /// The console URL for background node work (SSH bootstrap, agent
    /// update downloads): `node_public_url`, else `public_url`, else
    /// [`DEFAULT_PUBLIC_URL`]. Request-time links use the request's address.
    #[must_use]
    pub fn node_forge_url(&self) -> &str {
        self.node_public_url.as_deref().or(self.public_url.as_deref()).unwrap_or(DEFAULT_PUBLIC_URL)
    }

    #[must_use]
    pub fn node_agent_url(&self) -> &str {
        &self.agent_public_url
    }

    /// The port agents upload spool files to on the host of their own
    /// server URL (D-167); `None` when `[analytics] worker_agent_url` names
    /// another address, which agents then use as given.
    #[must_use]
    pub fn worker_agent_port(&self) -> Option<u16> {
        self.analytics.worker_agent_url.is_none().then(|| self.analytics.worker_agent_listen.port())
    }

    /// The URL agents upload spool files to (D-142): `[analytics]
    /// worker_agent_url`, else the agent URL's host with the worker's port.
    #[must_use]
    pub fn worker_agent_url(&self) -> String {
        if let Some(u) = &self.analytics.worker_agent_url {
            return u.clone();
        }

        let port = self.analytics.worker_agent_listen.port();
        let rest = self.agent_public_url.strip_prefix("https://").unwrap_or(&self.agent_public_url);
        let authority = rest.split('/').next().unwrap_or_default();
        let host = match authority.rsplit_once(':') {
            Some((h, p)) if p.bytes().all(|b| b.is_ascii_digit()) && (!h.ends_with(']') || h.starts_with('[')) => h,
            _ => authority,
        };

        format!("https://{host}:{port}")
    }

    #[must_use]
    pub fn agent_dist_dir(&self) -> PathBuf {
        // A release package keeps the agent binaries in `dist/` next to the server.
        self.agent_dist_dir.clone().unwrap_or_else(|| {
            let beside = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join("dist")));
            match beside {
                Some(d) if d.join("SHA256SUMS").is_file() => d,
                _ => self.data_dir.join("dist"),
            }
        })
    }

    /// Path of `forge-analytics-worker`: `[analytics] worker_binary`, else
    /// next to this executable.
    #[must_use]
    pub fn analytics_worker_path(&self) -> PathBuf {
        self.analytics.worker_binary.clone().unwrap_or_else(|| {
            let exe = if cfg!(windows) { "forge-analytics-worker.exe" } else { "forge-analytics-worker" };
            std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|d| d.join(exe)))
                .unwrap_or_else(|| PathBuf::from(exe))
        })
    }

    #[must_use]
    pub fn plugin_runner_path(&self) -> PathBuf {
        self.plugin_runner.clone().unwrap_or_else(|| {
            let exe = if cfg!(windows) { "forge-plugin-runner.exe" } else { "forge-plugin-runner" };
            std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|d| d.join(exe)))
                .unwrap_or_else(|| PathBuf::from(exe))
        })
    }
}

/// `http://localhost:8090`, `http://127.0.0.1`, `http://[::1]:8080` → true.
#[must_use]
pub fn is_loopback_url(url: &str) -> bool {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.contains('@') {
        return false;
    }

    let host = if let Some(v6) = authority.strip_prefix('[') {
        v6.split(']').next().unwrap_or_default()
    } else {
        authority.rsplit_once(':').map_or(authority, |(h, _)| h)
    };

    let host = host.to_ascii_lowercase();

    host == "localhost"
        || host.ends_with(".localhost")
        || host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

#[cfg(test)]
mod dev_tests {
    use super::*;

    #[test]
    fn dev_skip_mfa_only_on_loopback() {
        for ok in ["http://localhost:8090", "http://127.0.0.1", "http://[::1]:8080/x", "http://forge.localhost"] {
            assert!(is_loopback_url(ok), "{ok}");
        }

        for bad in [
            "https://forge.example.com",
            "http://10.0.0.5:8080",
            "http://localhost@evil.com",
            "http://localhost.evil.com",
        ] {
            assert!(!is_loopback_url(bad), "{bad}");
        }
    }
}

/// Where this control plane runs. Shown in the console; whether MFA is
/// required is a separate choice (`[security] require_mfa`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum DeploymentMode {
    #[default]
    Production,
    Staging,
    Development,
    Test,
}

impl DeploymentMode {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Production => "production",
            Self::Staging => "staging",
            Self::Development => "development",
            Self::Test => "test",
        }
    }
}

impl std::fmt::Display for DeploymentMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for DeploymentMode {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> anyhow::Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "production" | "prod" => Ok(Self::Production),
            "staging" => Ok(Self::Staging),
            "development" | "dev" => Ok(Self::Development),
            "test" => Ok(Self::Test),
            other => anyhow::bail!(
                "FORGE_ENVIRONMENT: unknown environment {other:?} (production, staging, development, test)"
            ),
        }
    }
}

#[cfg(test)]
mod environment_tests {
    use super::*;

    #[test]
    fn mfa_is_the_operators_choice_in_any_environment() {
        assert!(!ServerConfig::default().mfa_required(), "off by default: MFA is each user's choice");
        let on = ServerConfig {
            security: SecuritySettings { require_mfa: true, ..SecuritySettings::default() },
            ..ServerConfig::default()
        };
        assert!(on.mfa_required(), "an operator can still require it");

        for environment in
            [DeploymentMode::Production, DeploymentMode::Staging, DeploymentMode::Development, DeploymentMode::Test]
        {
            let off = ServerConfig {
                environment,
                security: SecuritySettings { require_mfa: false, ..SecuritySettings::default() },
                ..ServerConfig::default()
            };
            assert!(off.validate().is_ok() && !off.mfa_required(), "{environment}");
            let alias = ServerConfig { environment, dev_skip_mfa: true, ..ServerConfig::default() };
            assert!(alias.validate().is_ok() && !alias.mfa_required(), "dev_skip_mfa is an alias for off");
        }

        assert_eq!("DEV".parse::<DeploymentMode>().ok(), Some(DeploymentMode::Development));
        assert!("prd2".parse::<DeploymentMode>().is_err());
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // reason: tests
mod database_tests {
    use super::*;

    #[test]
    fn composes_name_and_password() {
        let u = compose_url("postgres://forge@db1:5432", "forge_op", Some("p@ss")).unwrap();
        assert_eq!(u, "postgres://forge:p%40ss@db1:5432/forge_op");
        assert_eq!(redact_url(&u), "postgres://forge:***@db1:5432/forge_op");
    }

    #[test]
    fn worker_max_uploads_defaults_per_engine_and_accepts_8_to_32() {
        let mut c = ServerConfig::default();
        assert_eq!(c.analytics.max_uploads(), 32, "duckdb");
        c.analytics.engine = AnalyticsEngine::Embedded;
        assert_eq!(c.analytics.max_uploads(), 8);

        for (n, ok) in [(7, false), (8, true), (32, true), (33, false)] {
            c.analytics.worker_max_uploads = Some(n);
            assert_eq!(c.validate().is_ok(), ok, "worker_max_uploads = {n}");
        }
    }

    /// D-167: `public_url` is optional, and when set it must be an address
    /// a client can open; trusting every address as a proxy is refused.
    #[test]
    fn public_url_is_optional_and_reachable_and_proxies_are_not_everyone() {
        let mut c = ServerConfig::default();
        assert!(c.public_url.is_none() && c.validate().is_ok(), "unset means auto");

        for (url, ok) in [
            ("https://forge.example.com", true),
            ("http://192.0.2.1:3880", true),
            ("http://[2001:db8::1]:3880", true),
            ("http://0.0.0.0:3880", false),
            ("http://[::]:3880", false),
            ("forge.example.com", false),
            ("https://forge.example.com/sub", false),
        ] {
            c.public_url = Some(url.into());
            assert_eq!(c.validate().is_ok(), ok, "{url}");
        }

        c.public_url = None;

        for (p, ok) in [("10.0.0.0/8", true), ("0.0.0.0/0", false), ("::/0", false), ("192.0.2.1", true)] {
            c.security.trusted_proxies = vec![p.into()];
            assert_eq!(c.validate().is_ok(), ok, "{p}");
        }
    }

    #[test]
    fn refuses_password_in_url() {
        assert!(compose_url("postgres://forge:secret@db1:5432", "forge_op", None).is_err());
    }

    #[test]
    fn names_and_defaults() {
        let mut c = ServerConfig::default();
        assert!(c.database_url().starts_with("sqlite:"));
        assert_eq!(c.analytics_name(), "forge_op_analytics");
        c.database.url = Some("postgres://forge@db1:5432".into());
        c.database.name = Some("ops".into());
        assert_eq!(c.database_url(), "postgres://forge@db1:5432/ops");
        assert_eq!(c.analytics_name(), "ops_analytics");
        c.analytics.engine = AnalyticsEngine::Postgres;
        assert_eq!(c.analytics_url().as_deref(), Some("postgres://forge@db1:5432/ops_analytics"));

        // The older one-line form wins.
        c.database_url = Some("sqlite:x.db".into());
        assert_eq!(c.database_url(), "sqlite:x.db");
    }

    #[test]
    fn parses_sections() {
        let c: ServerConfig = toml::from_str(
            "[database]\nurl = \"postgres://forge@h:5432\"\nname = \"f\"\n[analytics]\nengine = \"clickhouse\"\nurl = \"https://default@ch:8443\"\n[analytics.retention]\napp_logs_days = 14\n",
        )
        .unwrap();
        assert_eq!(c.analytics.engine, AnalyticsEngine::Clickhouse);
        assert_eq!(c.analytics.retention.app_logs_days, 14);
        assert_eq!(c.analytics_url().as_deref(), Some("https://default@ch:8443/f_analytics"));
    }
}
