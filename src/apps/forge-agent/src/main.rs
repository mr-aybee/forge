//! `forge-agent` command line.

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use forge_agent::enroll::{HostFacts, enroll_with};
use forge_agent::{Agent, AgentConfig, StatePaths};

#[derive(Parser)]
#[command(name = "forge-agent", version = forge_agent::VERSION, about = "Orbyn Forge node agent")]
struct Cli {
    /// State directory (identity, keys, config).
    #[arg(long, env = "FORGE_AGENT_STATE_DIR", global = true)]
    state_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Enrolls this machine with a one-time token.
    Enroll {
        /// Agent endpoint of the control plane, e.g. https://forge.example.com:7443
        #[arg(long, env = "FORGE_SERVER")]
        server: String,
        /// Enrollment token. Prefer the environment variable so it stays out of shell history.
        #[arg(long, env = "FORGE_ENROLL_TOKEN", hide_env_values = true)]
        token: String,
        /// Override the reported hostname.
        #[arg(long)]
        hostname: Option<String>,
        /// Terminal account the installer created on this machine; Forge
        /// saves it to the node's login list.
        #[arg(long, value_name = "NAME")]
        terminal_account: Option<String>,
    },
    /// Runs the agent in the foreground until it is stopped.
    ///
    /// On Linux the installer's systemd unit runs it; on Windows the
    /// ForgeAgent service runs it (`service install`).
    Run,
    /// The Windows service: install, uninstall, or run (Windows only; on
    /// Linux the installer sets up a systemd unit).
    #[command(subcommand)]
    Service(ServiceCmd),
    /// Prints the enrolled identity.
    Status,
    /// Allows the command kinds this release adds that were never offered
    /// before; kinds the operator removed stay removed (run by the installer).
    OfferCommands,
}

#[derive(Subcommand)]
enum ServiceCmd {
    /// Registers the ForgeAgent service (automatic start, LocalSystem,
    /// restarted after a failure) for this binary and starts it. Run as
    /// Administrator; the install.ps1 installer runs it. Re-running updates it.
    Install,
    /// Stops and deletes the ForgeAgent service; the enrollment stays.
    Uninstall,
    /// The service's entry point; only the Service Control Manager starts it.
    Run,
}

/// Runs the agent until `stop` turns true, on its own single-threaded runtime
/// (also called from the Windows service thread).
fn run_agent(paths: StatePaths, stop: tokio::sync::watch::Receiver<bool>) -> anyhow::Result<()> {
    // A self-updated agent offers the command kinds its release adds, like
    // the installer does; kinds the operator removed stay removed (D-070).
    if let Ok(mut config) = paths.load_config() {
        let offered_before = config.offered_commands.len();
        let added = config.offer_new_commands();
        if config.offered_commands.len() != offered_before {
            match paths.save_config(&config) {
                Ok(()) if !added.is_empty() => tracing::info!(added = %added.join(", "), "new command kinds allowed"),
                Ok(()) => {}
                Err(e) => tracing::warn!(error = %e, "could not record offered command kinds"),
            }
        }
    }
    let agent = Agent::load(paths)?;
    tracing::info!(server_id = agent.server_id(), "agent starting");
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    rt.block_on(agent.run(stop));
    // Dropping a runtime waits for its blocking tasks without limit; a stop
    // must not hang on a slow command.
    rt.shutdown_timeout(std::time::Duration::from_secs(5));
    Ok(())
}

/// Logs to `<state>/logs/forge-agent.log` (a service has no console); the
/// file is moved to `forge-agent.log.1` when it passes 10 MB at start-up.
#[cfg(windows)]
fn log_to_file(state_dir: &std::path::Path) -> anyhow::Result<()> {
    let dir = state_dir.join("logs");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("forge-agent.log");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 10 * 1024 * 1024) {
        let _ = std::fs::rename(&path, dir.join("forge-agent.log.1"));
    }
    let file = std::fs::OpenOptions::new().create(true).append(true).open(&path)?;
    tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(std::sync::Mutex::new(file))
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    Ok(())
}

fn service(cmd: ServiceCmd, state_dir: Option<PathBuf>) -> anyhow::Result<()> {
    #[cfg(windows)]
    {
        use forge_agent::winservice;
        match cmd {
            ServiceCmd::Install => {
                let exe = std::env::current_exe()?;
                winservice::require_protected_location(&exe)?;
                let dir = state_dir.clone().unwrap_or_else(StatePaths::default_dir);
                forge_agent::config::lock_windows_data(&dir)?;
                winservice::install(&exe, state_dir.as_deref())?;
                println!("service {} installed and running ({})", winservice::SERVICE_NAME, exe.display());
            }
            ServiceCmd::Uninstall => {
                if winservice::uninstall()? {
                    println!("service {} removed", winservice::SERVICE_NAME);
                } else {
                    println!("service {} is not installed", winservice::SERVICE_NAME);
                }
            }
            ServiceCmd::Run => {
                let dir = state_dir.unwrap_or_else(StatePaths::default_dir);
                // Again at every start, in case the ACL was loosened since.
                let locked = forge_agent::config::lock_windows_data(&dir);
                log_to_file(&dir)?;
                if let Err(e) = locked {
                    tracing::warn!(error = %e, "could not restrict access to the data folder");
                }
                winservice::run(dir, run_agent)?;
            }
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = (cmd, state_dir);
        anyhow::bail!(
            "`forge-agent service` is for Windows; on Linux the installer sets up the forge-agent systemd unit"
        )
    }
}

fn init_console_log() {
    tracing_subscriber::fmt()
        // No colour codes in journald or redirected logs.
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stdout()))
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    // The Windows service logs to a file and has no async main.
    let command = match cli.command {
        Cmd::Service(cmd) => {
            if !matches!(cmd, ServiceCmd::Run) {
                init_console_log();
            }
            return service(cmd, cli.state_dir);
        }
        other => other,
    };
    init_console_log();
    let paths = StatePaths::new(cli.state_dir.unwrap_or_else(StatePaths::default_dir));
    if matches!(command, Cmd::Run) {
        let (tx, rx) = tokio::sync::watch::channel(false);
        let ctrl_c = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build();
            if let Ok(rt) = rt {
                rt.block_on(async {
                    let _ = tokio::signal::ctrl_c().await;
                });
            }
            let _ = tx.send(true);
        });
        let r = run_agent(paths, rx);
        drop(ctrl_c);
        return r;
    }

    // A single-threaded runtime keeps the idle footprint small.
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    rt.block_on(async move {
        match command {
            Cmd::Enroll { server, token, hostname, terminal_account } => {
                // Keys go into a folder only SYSTEM and Administrators can read.
                #[cfg(windows)]
                forge_agent::config::lock_windows_data(&paths.dir)?;
                let mut facts = HostFacts::detect();
                if let Some(h) = hostname {
                    facts.hostname = h;
                }
                let id =
                    enroll_with(&paths, AgentConfig::new(server), &token, facts, terminal_account.as_deref()).await?;
                println!("enrolled as server {}", id.server_id);
            }
            Cmd::Run | Cmd::Service(_) => {}
            Cmd::OfferCommands => {
                let mut config = paths.load_config()?;
                let added = config.offer_new_commands();
                paths.save_config(&config)?;
                println!(
                    "{}",
                    if added.is_empty() {
                        "no new commands".to_owned()
                    } else {
                        format!("allowed: {}", added.join(", "))
                    }
                );
            }
            Cmd::Status => {
                let id = paths.load_identity()?;
                println!("{}", serde_json::to_string_pretty(&id)?);
            }
        }
        Ok(())
    })
}

/// Fails when a command or flag of this binary is missing from its reference
/// page (`docs/src/content/docs/reference/cli/forge-agent.md`): every subcommand has a
/// `## forge-agent <path>` (or `### …`) heading and every flag and argument appears.
#[cfg(test)]
mod docs_coverage {
    use clap::CommandFactory;

    fn page() -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../docs/src/content/docs/reference/cli/forge-agent.md");
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
        walk(&super::Cli::command(), "forge-agent", &doc, &mut missing);
        assert!(missing.is_empty(), "undocumented in reference/cli/forge-agent.md: {missing:?}");
    }
}
