//! `forge-server` command line.

use std::io::BufRead;
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use forge_server::{App, ServerConfig, Services, bootstrap};

#[derive(Parser)]
#[command(name = "forge-server", version = forge_server::VERSION, about = "Orbyn Forge control plane")]
struct Cli {
    /// Path to forge.toml.
    #[arg(long, short, env = "FORGE_CONFIG", global = true)]
    config: Option<PathBuf>,
    /// Log as JSON lines.
    #[arg(long, global = true)]
    json_logs: bool,
    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Runs the control plane (default).
    Serve,
    /// Creates the first platform administrator. The password is read from
    /// FORGE_ADMIN_PASSWORD or, if unset, from the first line of stdin.
    Init {
        /// Sign-in name (default `admin` when no email is given either).
        #[arg(long)]
        username: Option<String>,
        /// Optional email; it can also be added later in the console.
        #[arg(long)]
        email: Option<String>,
        #[arg(long, default_value = "Administrator")]
        name: String,
        /// The installer generated the password and saved it to
        /// `<data_dir>/initial-admin-password`; Forge deletes that file when
        /// the administrator changes their password.
        #[arg(long)]
        generated_password: bool,
    },
    /// Recovery on the server host: removes a user's passkeys and
    /// authenticator app, revokes their API keys and signs out all their
    /// sessions, so they can sign in with their password and enroll again.
    /// Audited.
    ResetMfa {
        /// Username or email.
        login: String,
    },
    /// Prints the effective configuration.
    Config,
    /// Whether MFA is required on this server: `status`, `on` or `off`.
    /// Edits `[security] require_mfa` in forge.toml; restart to apply.
    Mfa {
        #[arg(value_enum, default_value = "status")]
        action: MfaAction,
    },
    /// Whether API keys and service-account tokens marked elevated may run
    /// MFA-guarded actions: `status`, `on` or `off`. Edits `[security]
    /// allow_elevated_tokens` in forge.toml; restart to apply.
    ElevatedTokens {
        #[arg(value_enum, default_value = "status")]
        action: MfaAction,
    },
    /// The public address people use in links (alert emails, webhooks,
    /// approvals): `get`, `set <url>` or `clear`. Stored in the database,
    /// like the console's Settings page; `clear` falls back to `public_url`.
    /// Audited as system:forge. Restart a running server to apply it there.
    PublicAddress {
        #[command(subcommand)]
        action: AddressAction,
    },
    /// Exits non-zero when the database has applied migrations this binary
    /// does not know (it is older than the database). Used by the
    /// self-update dry run; never changes the database.
    CheckMigrations,
    /// The operational database: test it, create it, show the configuration.
    #[command(subcommand)]
    Db(DbCmd),
    /// The embedded analytics engine: install its binary.
    #[command(subcommand)]
    Analytics(AnalyticsCmd),
    /// Creates an enrollment token for a node on this host (installer use).
    /// Creates the cluster if it is missing. Prints only the token on
    /// stdout. Run as root on the server host; audited as system:forge.
    EnrollToken {
        /// Cluster the node joins (created if missing).
        #[arg(long)]
        cluster: String,
        /// Expected host name of the node.
        #[arg(long)]
        hostname: String,
        /// Validity in minutes (1-1440).
        #[arg(long, default_value_t = 15)]
        ttl_minutes: u32,
    },
    /// Release signing (CI): `keygen` prints a new key pair; `sign <file>`
    /// prints the base64 signature of a file (key in FORGE_RELEASE_KEY).
    #[command(subcommand)]
    Release(ReleaseCmd),
}

#[derive(Subcommand)]
enum AnalyticsCmd {
    /// Copies the engine binary to `<data_dir>/analytics/bin/clickhouse`
    /// after checking its SHA-256. Restart forge-server to start it.
    Install {
        /// The downloaded engine binary.
        #[arg(long)]
        binary: PathBuf,
        /// Its expected SHA-256 (hex), from the release feed's SHA256SUMS.
        #[arg(long)]
        sha256: String,
    },
}

#[derive(Subcommand)]
enum DbCmd {
    /// Tests the connection and reports what the database holds (missing,
    /// empty, Forge, newer Forge or other tables). Exits 0 when Forge can
    /// use it, 4 when it cannot. Never changes anything unless `--create`.
    Check {
        /// Create the PostgreSQL database when it is missing.
        #[arg(long)]
        create: bool,
        /// Accept a database that also holds tables that are not Forge's.
        #[arg(long)]
        allow_shared: bool,
        /// Print the report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Prints the operational and analytics database settings (no passwords).
    Show,
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum MfaAction {
    Status,
    On,
    Off,
}

#[derive(Subcommand)]
enum AddressAction {
    /// Prints the address links use and where it comes from.
    Get,
    /// Sets it: an absolute http(s) origin such as https://forge.example.com.
    Set { url: String },
    /// Removes the setting; links use `public_url` from forge.toml.
    Clear,
}

#[derive(Subcommand)]
enum ReleaseCmd {
    /// New Ed25519 key: the private key (keep secret) and the public key.
    Keygen,
    /// Signs a file (e.g. SHA256SUMS) with FORGE_RELEASE_KEY.
    Sign { file: PathBuf },
    /// Prints the public key of FORGE_RELEASE_KEY.
    Pubkey,
}

fn init_logging(json: bool, stderr: bool) {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,sqlx=warn".into());
    if stderr {
        // Commands whose stdout is read by scripts (`enroll-token`) log to stderr.
        let builder = tracing_subscriber::fmt().with_ansi(false).with_env_filter(filter).with_writer(std::io::stderr);
        if json {
            builder.json().init();
        } else {
            builder.init();
        }
        return;
    }
    // No colour codes in journald or redirected logs.
    let builder = tracing_subscriber::fmt()
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stdout()))
        .with_env_filter(filter);
    if json {
        builder.json().init();
    } else {
        builder.init();
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Files the server creates (database, WAL, backups, keys) are owner-only.
    forge_server::perms::owner_only_umask();
    // Before an update can rename the running file (`*.prev`).
    forge_server::update::remember_install_path();
    let cli = Cli::parse();
    init_logging(cli.json_logs, matches!(cli.command, Some(Cmd::EnrollToken { .. } | Cmd::CheckMigrations)));
    let config = ServerConfig::load(cli.config.as_deref())?;

    match cli.command.unwrap_or(Cmd::Serve) {
        Cmd::Serve => {
            // A freshly installed update that keeps failing is rolled back here.
            forge_server::update::startup_guard(&config.data_dir, &config.agent_dist_dir());
            let data_dir = config.data_dir.clone();
            // Off is the default: MFA is each user's choice and guards their
            // sign-in. `forge-server mfa on` opts in to guarded actions.
            let mfa_required = config.mfa_required();
            tracing::info!(required_for_guarded_actions = mfa_required, "MFA policy");
            let app = App::start(config).await?;
            tracing::info!(api = %app.api_addr, agents = %app.agent_addr, version = forge_server::VERSION, "Orbyn Forge control plane started");
            if let Some((old, new)) = forge_server::mfa_config::changed_since_last_start(&data_dir, mfa_required) {
                let name = |v: bool| if v { "required" } else { "off" };
                tracing::info!(old = old.map(name), new = name(new), "MFA setting");
                app.state
                    .audit_system("config.mfa", serde_json::json!({ "old": old.map(name), "new": name(new) }))
                    .await;
            }
            if let Some(a) = forge_server::update::mark_serving(&data_dir) {
                tracing::info!(from = %a.from, to = %a.to, "update completed");
                app.state
                    .audit_system(
                        "system.update.completed",
                        serde_json::json!({ "from": a.from, "to": a.to, "backup": a.backup }),
                    )
                    .await;
                // Agents follow the server when the update asked for it.
                forge_server::api::system_update::start_agent_follow_up(&app.state, &a);
            } else if let Some(a) = forge_server::update::detect_stale_restart(&data_dir) {
                tracing::error!(from = %a.from, to = %a.to, "update installed, but the server restarted into the old version");
                app.state
                    .audit_system(
                        "system.update.failed",
                        serde_json::json!({ "from": a.from, "to": a.to, "reason": "restarted into the old version" }),
                    )
                    .await;
            }
            // An update restart needs a known install path; without one the
            // server keeps serving and the update is marked failed.
            let restart = loop {
                let wants_restart = tokio::select! {
                    () = shutdown_signal() => false,
                    () = forge_server::update::restart_signal().notified() => true,
                };
                if !wants_restart {
                    break None;
                }
                match forge_server::update::restart_target() {
                    Ok(exe) => break Some(exe),
                    Err(e) => {
                        tracing::error!(error = %e, "update installed, but the server cannot restart itself");
                        app.state
                            .audit_system(
                                "system.update.failed",
                                serde_json::json!({ "to": forge_server::update::status().latest.map(|r| r.version), "reason": e }),
                            )
                            .await;
                    }
                }
            };
            tracing::info!(restart = restart.is_some(), "shutting down");
            app.stop().await;
            if let Some(exe) = restart {
                forge_server::update::restart_into_self(&exe);
            }
        }
        Cmd::ResetMfa { login } => {
            use forge_core::ports::UserRepository;
            let state = Services::build(config).await?;
            let login = login.trim().to_lowercase();
            let user = if login.contains('@') {
                state.store.find_user_by_email(&login).await?
            } else {
                state.store.find_user_by_username(&login).await?
            }
            .ok_or_else(|| anyhow::anyhow!("no user `{login}`"))?;
            let removed = state.passkeys.remove_all(user.id).await.map_err(|e| anyhow::anyhow!("{e}"))?;
            state.store.set_totp_secret(user.id, None).await?;
            state.identity.revoke_all(user.id).await.map_err(|e| anyhow::anyhow!("{e}"))?;
            // A reset is account-compromise recovery: API keys go too.
            let keys = forge_core::ports::ApiKeyRepository::delete_user_api_keys(&state.store, user.id).await?;
            state
                .audit_system(
                    "auth.mfa.reset",
                    serde_json::json!({
                        "user": user.id,
                        "passkeys_removed": removed,
                        "api_keys_revoked": keys,
                        "via": "forge-server reset-mfa",
                    }),
                )
                .await;
            println!(
                "{login}: removed {removed} passkey(s) and the authenticator app, revoked {keys} API key(s); all sessions signed out. \
                 Sign in with the password and enroll MFA again."
            );
        }
        Cmd::Init { username, email, name, generated_password } => {
            // A named PostgreSQL database is created on first use (never recreated).
            if forge_store::provision::create_if_missing(&config.database_url())
                .await
                .map_err(|e| anyhow::anyhow!("creating the database: {e}"))?
            {
                println!("created database {}", config.database_name());
            }
            let password = match std::env::var("FORGE_ADMIN_PASSWORD") {
                Ok(p) => p,
                Err(_) => {
                    let mut line = String::new();
                    std::io::stdin().lock().read_line(&mut line)?;
                    line.trim_end_matches(['\r', '\n']).to_owned()
                }
            };
            let username = match (&username, &email) {
                (None, None) => Some("admin".to_owned()),
                _ => username,
            };
            // Check everything before the database is created, so a rejected
            // password leaves no half-initialised installation behind.
            let (username, email) = forge_identity::sign_in_names(username.as_deref(), email.as_deref())?;
            forge_identity::password::validate(&password).map_err(anyhow::Error::msg)?;
            let state = Services::build(config).await?;
            let r = bootstrap::init_admin(
                &state,
                username.as_deref(),
                email.as_deref(),
                &name,
                &password,
                generated_password,
            )
            .await?;
            let login = username.or(email).unwrap_or_default();
            println!("created platform admin {login} ({}) and default org {}", r.user_id, r.workspace_id);
        }
        Cmd::Config => println!("{config:#?}"),
        Cmd::Mfa { action } => {
            let path = cli.config.as_deref().ok_or_else(|| {
                anyhow::anyhow!("pass --config /etc/forge/forge.toml (or set FORGE_CONFIG): the setting lives there")
            })?;
            let text = std::fs::read_to_string(path)?;
            let in_file = forge_server::mfa_config::read(&text)?;
            let word = |v: bool| if v { "required" } else { "not required" };
            match action {
                MfaAction::Status => {
                    println!("MFA is {} ({}).", word(config.mfa_required()), path.display());
                    if in_file != config.mfa_required() {
                        println!("The file says {}; an environment variable overrides it.", word(in_file));
                    }
                }
                MfaAction::On | MfaAction::Off => {
                    let required = matches!(action, MfaAction::On);
                    if in_file == required {
                        println!("MFA is already {} in {}.", word(required), path.display());
                    } else {
                        let new = forge_server::mfa_config::set(&text, required)?;
                        forge_server::mfa_config::write_atomic(path, &new)?;
                        println!("MFA is now {} in {}.", word(required), path.display());
                    }
                    println!("Restart forge-server to apply it: systemctl restart forge-server");
                    if std::env::var("FORGE_REQUIRE_MFA").is_ok() || std::env::var("FORGE_DEV_SKIP_MFA").is_ok() {
                        println!("Note: FORGE_REQUIRE_MFA or FORGE_DEV_SKIP_MFA is set and overrides the file.");
                    }
                }
            }
        }
        Cmd::ElevatedTokens { action } => {
            let path = cli.config.as_deref().ok_or_else(|| {
                anyhow::anyhow!("pass --config /etc/forge/forge.toml (or set FORGE_CONFIG): the setting lives there")
            })?;
            let text = std::fs::read_to_string(path)?;
            let in_file = forge_server::mfa_config::read_elevated(&text)?;
            let word = |v: bool| if v { "allowed" } else { "not allowed" };
            match action {
                MfaAction::Status => {
                    println!("Elevated tokens are {} ({}).", word(config.elevated_tokens_allowed()), path.display());
                    if in_file != config.elevated_tokens_allowed() {
                        println!("The file says {}; FORGE_ALLOW_ELEVATED_TOKENS overrides it.", word(in_file));
                    }
                }
                MfaAction::On | MfaAction::Off => {
                    let allowed = matches!(action, MfaAction::On);
                    if in_file == allowed {
                        println!("Elevated tokens are already {} in {}.", word(allowed), path.display());
                    } else {
                        let new = forge_server::mfa_config::set_elevated(&text, allowed)?;
                        forge_server::mfa_config::write_atomic(path, &new)?;
                        println!("Elevated tokens are now {} in {}.", word(allowed), path.display());
                    }
                    println!("Restart forge-server to apply it: systemctl restart forge-server");
                }
            }
        }
        Cmd::PublicAddress { action } => public_address(config, action).await?,
        Cmd::Db(cmd) => db_command(&config, cmd).await?,
        Cmd::Analytics(AnalyticsCmd::Install { binary, sha256 }) => {
            let paths = forge_server::analytics_install::paths(&config);
            let target = paths.binary.clone();
            tokio::task::spawn_blocking(move || forge_server::analytics_install::install(&paths, &binary, &sha256))
                .await??;
            println!("installed the analytics engine at {}", target.display());
            println!("set [analytics] engine = \"embedded\" and restart forge-server: systemctl restart forge-server");
        }
        Cmd::CheckMigrations => {
            let unknown = forge_store::Store::unknown_migrations(&config.database_url())
                .await
                .map_err(|e| anyhow::anyhow!("cannot read the database: {e}"))?;
            if !unknown.is_empty() {
                let list: Vec<String> = unknown.iter().map(i64::to_string).collect();
                eprintln!(
                    "the database has migrations this forge-server {} does not know: {}; it is older than the database",
                    forge_server::VERSION,
                    list.join(", ")
                );
                std::process::exit(3);
            }
            println!("every applied migration is known to forge-server {}", forge_server::VERSION);
        }
        Cmd::EnrollToken { cluster, hostname, ttl_minutes } => {
            let state = Services::build(config).await?;
            let token = bootstrap::local_enroll_token(&state, &cluster, &hostname, ttl_minutes).await?;
            println!("{token}");
        }
        Cmd::Release(ReleaseCmd::Keygen) => {
            let (private, public) = forge_server::update::keys::generate().map_err(anyhow::Error::msg)?;
            println!("private (FORGE_RELEASE_KEY, keep secret): {private}");
            println!("public  (update::RELEASE_PUBLIC_KEY):     {public}");
        }
        Cmd::Release(ReleaseCmd::Pubkey) => {
            let key = std::env::var("FORGE_RELEASE_KEY").map_err(|_| anyhow::anyhow!("set FORGE_RELEASE_KEY"))?;
            println!("{}", forge_server::update::keys::public(&key).map_err(anyhow::Error::msg)?);
        }
        Cmd::Release(ReleaseCmd::Sign { file }) => {
            let key = std::env::var("FORGE_RELEASE_KEY").map_err(|_| anyhow::anyhow!("set FORGE_RELEASE_KEY"))?;
            let data = std::fs::read(&file)?;
            println!("{}", forge_server::update::keys::sign(&key, &data).map_err(anyhow::Error::msg)?);
        }
    }
    Ok(())
}

/// `forge-server public-address get|set|clear`.
async fn public_address(config: ServerConfig, action: AddressAction) -> anyhow::Result<()> {
    use forge_server::api::server_settings::{AUDIT_PUBLIC_ADDRESS_CLEAR, AUDIT_PUBLIC_ADDRESS_UPDATE};
    let state = Services::build(config).await?;
    let addr = &state.public_address;
    let (audit, value) = match action {
        AddressAction::Get => {
            match addr.runtime() {
                Some(v) => println!("{v} (setting; public_url in forge.toml: {})", addr.configured()),
                None => println!("{} (public_url in forge.toml; no setting)", addr.configured()),
            }
            return Ok(());
        }
        AddressAction::Set { url } => {
            forge_server::public_address::validate(&url).map_err(|e| anyhow::anyhow!("{url}: {e}"))?;
            (AUDIT_PUBLIC_ADDRESS_UPDATE, Some(url))
        }
        AddressAction::Clear => (AUDIT_PUBLIC_ADDRESS_CLEAR, None),
    };
    let from = addr.get();
    forge_server::public_address::save(&state.store, addr, value.as_deref(), state.clock.now())
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let to = addr.get();
    state
        .audit_system(audit, serde_json::json!({ "from": from, "to": to, "via": "forge-server public-address" }))
        .await;
    println!(
        "Links now use {to}. Restart forge-server to apply it to a running server: systemctl restart forge-server"
    );
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {}
        () = terminate => {}
    }
}

/// Fails when a command or flag of this binary is missing from its reference
/// page (`docs/src/content/docs/reference/cli/forge-server.md`): every subcommand has a
/// `## forge-server <path>` (or `### …`) heading and every flag and argument appears.
async fn db_command(config: &ServerConfig, cmd: DbCmd) -> anyhow::Result<()> {
    use forge_store::provision::{self, Contents};
    match cmd {
        DbCmd::Show => {
            println!("operational: {}", config.database_display());
            println!("analytics:   engine={:?} database={}", config.analytics.engine, config.analytics_name());
            if let Some(u) = config.analytics_url() {
                println!("             {}", forge_server::config::redact_url(&u));
            }
        }
        DbCmd::Check { create, allow_shared, json } => {
            let url = config.database_url();
            let mut report = provision::check(&url)
                .await
                .map_err(|e| anyhow::anyhow!("cannot reach {}: {e}", config.database_display()))?;
            let mut created = false;
            if create && report.contents == Contents::Missing && report.can_create != Some(false) {
                created = provision::create_if_missing(&url)
                    .await
                    .map_err(|e| anyhow::anyhow!("creating the database: {e}"))?;
                report = provision::check(&url).await.map_err(|e| anyhow::anyhow!("{e}"))?;
            }
            let usable = report.usable(allow_shared);
            if json {
                println!("{}", serde_json::json!({ "report": report, "created": created, "usable": usable }));
            } else {
                println!("database: {}", config.database_display());
                if let Some(v) = &report.server_version {
                    println!("server:   {v}");
                }
                let what = match report.contents {
                    Contents::Missing if report.can_create == Some(false) => format!(
                        "missing, and this role may not create databases. Ask an administrator to run: CREATE DATABASE {} OWNER <role>;",
                        config.database_name()
                    ),
                    Contents::Missing => "missing; `--create` (or the first start) creates it".into(),
                    Contents::Empty => "empty; Forge will create its tables".into(),
                    Contents::Forge => {
                        format!("Forge database with {} tables; migrations bring it up to date", report.tables)
                    }
                    Contents::ForgeNewer => "Forge tables from a newer Forge build; upgrade this server first".into(),
                    Contents::Foreign => format!(
                        "{} tables that are not Forge's; pass --allow-shared to put Forge's tables next to them",
                        report.tables
                    ),
                };
                if created {
                    println!("created:  {}", config.database_name());
                }
                println!("contents: {what}");
                println!("usable:   {}", if usable { "yes" } else { "no" });
            }
            if !usable {
                std::process::exit(4);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod docs_coverage {
    use clap::CommandFactory;

    fn page() -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../docs/src/content/docs/reference/cli/forge-server.md");
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{} is missing: {e}", path.display()))
    }

    fn walk(cmd: &clap::Command, path: &str, doc: &str, missing: &mut Vec<String>) {
        for a in cmd.get_arguments() {
            let id = a.get_id().as_str();
            if matches!(id, "help" | "version") {
                continue;
            }
            let shown = match a.get_long() {
                Some(l) => format!("--{l}"),
                None => format!("<{}>", id.to_uppercase()),
            };
            let alt = format!("[{}]", id.to_uppercase());
            if !doc.contains(&shown) && !doc.contains(&alt) {
                missing.push(format!("{path}: {shown}"));
            }
        }
        for sub in cmd.get_subcommands().filter(|s| s.get_name() != "help") {
            let p = format!("{path} {}", sub.get_name());
            if !doc.contains(&format!("## {p}\n")) && !doc.contains(&format!("### {p}\n")) {
                missing.push(format!("heading `## {p}`"));
            }
            walk(sub, &p, doc, missing);
        }
    }

    #[test]
    fn every_command_and_flag_is_documented() {
        let doc = page();
        let mut missing = Vec::new();
        walk(&super::Cli::command(), "forge-server", &doc, &mut missing);
        assert!(missing.is_empty(), "undocumented in reference/cli/forge-server.md: {missing:?}");
    }
}
