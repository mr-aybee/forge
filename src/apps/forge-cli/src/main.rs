//! `forge`: command-line client for Orbyn Forge (GitHub issue #5).
//!
//! Connects through named **contexts** (`forge contexts add`), signs in with
//! the browser (OAuth 2.1 + PKCE, `forge auth login`) and keeps tokens in the
//! OS credential store. `--output json` gives a versioned envelope for
//! scripts and LLM agents; exit codes are documented in `error.rs`.
//! Publisher tooling (`plugins keygen|sign`) works offline.

mod admincmd;
mod agentscmd;
mod alertscmd;
mod alerttemplates;
mod api;
mod apicmd;
mod appspec;
mod cmds;
mod containercmd;
mod creds;
mod ctx;
mod deploycmd;
mod dockercmd;
#[cfg(test)]
mod docs_sync;
mod error;
mod filescmd;
mod ingresscmd;
mod login;
mod notifycmd;
mod out;
mod parity;
mod profilescmd;
mod projectcmd;
#[cfg(test)]
mod queryparity;
mod rescmd;
mod schemacheck;
mod silencecmd;
mod swarmcmd;
mod termcmd;
mod tunnel_direct;
mod tunnelcmd;
mod updatecmd;

use clap::{Args, CommandFactory, Parser, Subcommand};

/// Version of this build: `FORGE_BUILD_VERSION` at compile time (releases and
/// local test feeds), otherwise the crate version. `forge update` compares it.
pub const VERSION: &str = match option_env!("FORGE_BUILD_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

use crate::api::Api;
use crate::error::{CliError, Exit, Result};
use crate::out::Format;

#[derive(Parser, Debug)]
#[command(name = "forge", version = VERSION, about = "Orbyn Forge CLI", propagate_version = true)]
pub struct Cli {
    #[command(flatten)]
    pub global: Global,
    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Args, Debug, Clone)]
pub struct Global {
    /// Context (connection profile) to use for this command only.
    #[arg(long, short = 'C', env = "FORGE_CONTEXT", global = true)]
    pub context: Option<String>,
    /// Server URL; with FORGE_TOKEN this targets a server without a context.
    #[arg(long, env = "FORGE_SERVER", global = true)]
    pub server: Option<String>,
    /// Bearer token (one-shot; never stored). Requires --server or a context.
    #[arg(long, env = "FORGE_TOKEN", hide_env_values = true, global = true)]
    pub token: Option<String>,
    /// Output format: table (for people), json, yaml or xml.
    #[arg(long, short = 'o', value_enum, default_value_t = Format::Table, global = true)]
    pub output: Format,
    /// Colour: auto (on a terminal, unless NO_COLOR is set), always or never.
    #[arg(long, value_enum, default_value_t = out::ColorChoice::Auto, global = true)]
    pub color: out::ColorChoice,
    /// Print long output directly instead of opening a pager.
    #[arg(long, global = true)]
    pub no_pager: bool,
    /// Never prompt; fail with a typed error where input would be needed.
    #[arg(long, global = true)]
    pub non_interactive: bool,
    /// Refuse to run unless the server's workspace id matches (production guard).
    #[arg(long, global = true)]
    pub expected_instance_id: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Manage local connection profiles (contexts).
    #[command(subcommand)]
    Contexts(cmds::ContextsCmd),
    /// Sign in, sign out and show the current identity.
    #[command(subcommand)]
    Auth(cmds::AuthCmd),
    /// Client and server versions.
    Version,
    /// Server feature flags.
    Capabilities,
    /// Nodes (servers running the Forge agent).
    #[command(subcommand)]
    Nodes(cmds::NodesCmd),
    /// Agent versions and self-update.
    #[command(subcommand)]
    Agents(agentscmd::AgentsCmd),
    /// Clusters.
    #[command(subcommand)]
    Clusters(cmds::ClustersCmd),
    /// Port tunnels to services, databases and SSH on nodes (like ssh -L).
    #[command(subcommand)]
    Tunnel(tunnelcmd::TunnelCmd),
    /// A node's Docker Engine API over mutual TLS: the endpoint, the call catalog and raw calls.
    #[command(subcommand)]
    Docker(dockercmd::DockerCmd),
    /// Container registries and their credentials (D-158).
    #[command(subcommand)]
    Registries(containercmd::RegistriesCmd),
    /// Deploy webhooks for CI/CD (D-159).
    #[command(subcommand)]
    Webhooks(containercmd::WebhooksCmd),
    /// Releases and rollouts of services from forge.deploy.yaml: validate, plan, apply, rollback.
    #[command(subcommand)]
    Deploy(deploycmd::DeployCmd),
    /// Alert rules and alarms.
    #[command(subcommand)]
    Alerts(alertscmd::AlertsCmd),
    /// Local names (`<node>.local`) of nodes and services.
    #[command(subcommand)]
    Dns(cmds::DnsCmd),
    /// Durable jobs.
    #[command(subcommand)]
    Jobs(cmds::JobsCmd),
    /// Audit log.
    #[command(subcommand)]
    Audit(cmds::AuditCmd),
    /// Plugins and publisher tooling.
    #[command(subcommand, alias = "plugin")]
    Plugins(cmds::PluginsCmd),
    /// Compose stacks: list, show, deploy, roll back, take down.
    #[command(subcommand)]
    Stacks(rescmd::StacksCmd),
    /// Docker Swarm: nodes, stacks, secrets, configs, networks, volumes, services.
    #[command(subcommand)]
    Swarm(swarmcmd::SwarmCmd),
    /// Projects: the products whose environments hold your services.
    #[command(subcommand)]
    Projects(projectcmd::ProjectsCmd),
    /// Services: what runs in an environment, as a systemd unit or a container.
    #[command(subcommand)]
    Services(rescmd::ServicesCmd),
    /// Changes your AI connectors asked for (MCP Ask mode): list, show, deny.
    #[command(subcommand)]
    Approvals(admincmd::ApprovalsCmd),
    /// People with a Forge account.
    #[command(subcommand)]
    Users(admincmd::UsersCmd),
    /// Role bindings: who may do what, where.
    #[command(subcommand)]
    Access(admincmd::AccessCmd),
    /// Your personal API keys.
    #[command(subcommand, name = "api-keys")]
    ApiKeys(admincmd::ApiKeysCmd),
    /// Service accounts: non-person identities with tokens, for CI and automation.
    #[command(subcommand, name = "service-accounts")]
    ServiceAccounts(profilescmd::ServiceAccountsCmd),
    /// AI apps connected through MCP, and their OAuth clients.
    #[command(subcommand)]
    Connectors(admincmd::ConnectorsCmd),
    /// Workspace settings.
    #[command(subcommand)]
    Settings(admincmd::SettingsCmd),
    /// Tenants.
    #[command(subcommand)]
    Tenants(admincmd::TenantsCmd),
    /// Environments: where services run (in a cluster, optionally in a project).
    #[command(subcommand)]
    Environments(admincmd::EnvironmentsCmd),
    /// Sites: domains routed through the cluster ingress.
    #[command(subcommand)]
    Sites(admincmd::SitesCmd),
    /// TLS certificates.
    #[command(subcommand)]
    Certificates(admincmd::CertificatesCmd),
    /// Object storage (S3-compatible stores, buckets, access keys).
    #[command(subcommand)]
    Storage(admincmd::StorageCmd),
    /// The password vault.
    #[command(subcommand)]
    Vault(admincmd::VaultCmd),
    /// Forge server update.
    #[command(subcommand)]
    System(admincmd::SystemCmd),
    /// App Store packages: catalog, installations, updates.
    #[command(subcommand)]
    Packages(admincmd::PackagesCmd),
    /// Open terminal and exec sessions: list them, or end one.
    #[command(subcommand)]
    Sessions(termcmd::SessionsCmd),
    /// Update this CLI to the latest signed release.
    Update {
        /// Only report whether a newer release exists.
        #[arg(long)]
        check: bool,
        /// Install an older release when the feed offers one.
        #[arg(long)]
        allow_downgrade: bool,
        #[arg(long)]
        yes: bool,
    },
    /// Classify every API operation: typed command, `forge api` only, or excluded.
    Parity {
        /// Only operations that are not covered by a typed command.
        #[arg(long)]
        gaps: bool,
    },
    /// Any API operation, from the server's OpenAPI document (`forge api operations`).
    #[command(subcommand)]
    Api(apicmd::ApiCmd),
    /// Print a shell completion script (bash, zsh, fish, powershell, elvish).
    Completion { shell: clap_complete::Shell },
    /// Describe every command and argument as JSON (for scripts and LLM agents).
    Commands,
    /// (deprecated) Same as `forge auth whoami`.
    #[command(hide = true)]
    Whoami,
    /// (deprecated) Same as `forge nodes list`.
    #[command(hide = true)]
    Servers,
    /// (deprecated) Same as `forge nodes command`.
    #[command(hide = true)]
    Command {
        server: String,
        kind: String,
        #[arg(long)]
        name: Option<String>,
    },
}

/// The connection a command runs against, resolved once at start.
#[derive(Debug, Clone)]
pub struct Target {
    pub context: Option<String>,
    pub server: String,
    pub ca_file: Option<String>,
    pub instance_id: Option<String>,
    pub default_cluster: Option<String>,
}

/// Resolves flags → environment → current context (snapshotted once).
pub fn target(g: &Global) -> Result<Target> {
    let cfg = ctx::Config::load()?;

    // An explicit --server / FORGE_SERVER stands on its own, unless it is the
    // current context's server: then that context still applies (its CA, its
    // remembered tunnel ports), so a shell with FORGE_SERVER set behaves like
    // one that selected the context.
    let same_as_current = || {
        let s = g.server.as_deref()?;
        let name = cfg.current.clone()?;
        let c = cfg.contexts.get(&name)?;
        (ctx::normalise_server(s).ok()? == c.server).then_some(name)
    };
    let name = g.context.clone().or_else(|| if g.server.is_some() { same_as_current() } else { cfg.current.clone() });
    if let Some(name) = name {
        let c = cfg.get(&name)?.clone();

        if let Some(s) = &g.server
            && ctx::normalise_server(s)? != c.server
        {
            return Err(CliError::usage(format!(
                "--server {s} does not match context `{name}` ({}); drop one of them",
                c.server
            )));
        }

        return Ok(Target {
            context: Some(name),
            server: c.server,
            ca_file: c.ca_file,
            instance_id: c.instance_id,
            default_cluster: c.default_cluster,
        });
    }

    let server = match (&g.server, std::env::var("FORGE_URL").ok().filter(|s| !s.is_empty())) {
        (Some(s), _) => s.clone(),
        (None, Some(legacy)) => {
            eprintln!("forge: FORGE_URL is deprecated; use FORGE_SERVER or a context");
            legacy
        }
        (None, None) => {
            return Err(CliError::usage("no context selected: run `forge contexts add <name> --server <url>` first"));
        }
    };

    Ok(Target {
        context: None,
        server: ctx::normalise_server(&server)?,
        ca_file: None,
        instance_id: None,
        default_cluster: None,
    })
}

/// An authenticated client for `t`.
pub async fn connect(g: &Global, t: &Target) -> Result<Api> {
    let mut api = if let Some(token) = &g.token {
        Api::with_token(t.server.clone(), t.ca_file.as_deref(), token.clone())?
    } else if let Some(name) = &t.context {
        let tokens = creds::for_server(name, &t.server)?
            .ok_or_else(|| CliError::auth(format!("not signed in to `{name}`: run `forge auth login`")))?;
        Api::with_context(t.server.clone(), t.ca_file.as_deref(), name.clone(), tokens)?
    } else {
        return Err(CliError::auth(
            "no credentials: sign in with a context, or set FORGE_TOKEN together with FORGE_SERVER",
        ));
    };

    api.set_interactive(!g.non_interactive && std::io::IsTerminal::is_terminal(&std::io::stdin()));

    if let Some(expected) = &g.expected_instance_id {
        let ws = api.get("/api/v1/workspace").await?;
        let actual = ws.get("id").and_then(serde_json::Value::as_str).unwrap_or_default();
        if actual != expected {
            return Err(CliError::new(
                Exit::Conflict,
                format!("server {} is workspace {actual}, not the expected {expected}; nothing was done", t.server),
            )
            .with_code("instance_mismatch"));
        }
    } else if let (Some(recorded), Some(_)) = (&t.instance_id, &g.token) {
        // A one-shot token must reach the same deployment the context recorded.
        let ws = api.get("/api/v1/workspace").await?;
        if ws.get("id").and_then(serde_json::Value::as_str) != Some(recorded.as_str()) {
            return Err(CliError::new(
                Exit::Conflict,
                "FORGE_TOKEN belongs to a different deployment than this context",
            )
            .with_code("instance_mismatch"));
        }
    }

    Ok(api)
}

fn command_tree(c: &clap::Command) -> serde_json::Value {
    let args: Vec<serde_json::Value> = c
        .get_arguments()
        .filter(|a| !a.is_hide_set() && a.get_id() != "help" && a.get_id() != "version")
        .map(|a| {
            serde_json::json!({
                "name": a.get_id().as_str(),
                "long": a.get_long(),
                "short": a.get_short().map(|s| s.to_string()),
                "positional": a.is_positional(),
                "required": a.is_required_set(),
                "global": a.is_global_set(),
                "help": a.get_help().map(ToString::to_string),
                "values": a.get_possible_values().iter().map(|v| v.get_name().to_owned()).collect::<Vec<_>>(),
            })
        })
        .collect();
    let subs: Vec<serde_json::Value> = c.get_subcommands().filter(|s| !s.is_hide_set()).map(command_tree).collect();

    serde_json::json!({
        "name": c.get_name(),
        "about": c.get_about().map(ToString::to_string),
        "arguments": args,
        "subcommands": subs,
    })
}

async fn run(cli: Cli) -> Result<()> {
    let g = &cli.global;

    match cli.cmd {
        Cmd::Contexts(c) => cmds::contexts(g, c).await,
        Cmd::Auth(c) => cmds::auth(g, c).await,
        Cmd::Version => cmds::version(g).await,
        Cmd::Capabilities => cmds::simple_get(g, "/api/capabilities").await,
        Cmd::Nodes(c) => cmds::nodes(g, c).await,
        Cmd::Clusters(c) => cmds::clusters(g, c).await,
        Cmd::Dns(c) => cmds::dns(g, c).await,
        Cmd::Alerts(c) => alertscmd::run(g, c).await,
        Cmd::Tunnel(c) => tunnelcmd::run(g, c).await,
        Cmd::Docker(c) => dockercmd::run(g, c).await,
        Cmd::Registries(c) => containercmd::registries(g, c).await,
        Cmd::Webhooks(c) => containercmd::webhooks(g, c).await,
        Cmd::Deploy(c) => deploycmd::run(g, c).await,
        Cmd::Agents(c) => agentscmd::run(g, c).await,
        Cmd::Jobs(c) => cmds::jobs(g, c).await,
        Cmd::Audit(c) => cmds::audit(g, c).await,
        Cmd::Plugins(c) => cmds::plugins(g, c).await,
        Cmd::Stacks(c) => rescmd::stacks(g, c).await,
        Cmd::Swarm(c) => swarmcmd::run(g, c).await,
        Cmd::Projects(c) => projectcmd::run(g, c).await,
        Cmd::Services(c) => rescmd::services(g, c).await,
        Cmd::Approvals(c) => admincmd::approvals(g, c).await,
        Cmd::Users(c) => admincmd::users(g, c).await,
        Cmd::Access(c) => admincmd::access(g, c).await,
        Cmd::ApiKeys(c) => admincmd::api_keys(g, c).await,
        Cmd::ServiceAccounts(c) => profilescmd::service_accounts(g, c).await,
        Cmd::Connectors(c) => admincmd::connectors(g, c).await,
        Cmd::Settings(c) => admincmd::settings(g, c).await,
        Cmd::Tenants(c) => admincmd::tenants(g, c).await,
        Cmd::Environments(c) => admincmd::environments(g, c).await,
        Cmd::Sites(c) => admincmd::sites(g, c).await,
        Cmd::Certificates(c) => admincmd::certificates(g, c).await,
        Cmd::Storage(c) => admincmd::storage(g, c).await,
        Cmd::Vault(c) => admincmd::vault(g, c).await,
        Cmd::System(c) => admincmd::system(g, c).await,
        Cmd::Packages(c) => admincmd::packages(g, c).await,
        Cmd::Sessions(c) => termcmd::sessions(g, c).await,
        Cmd::Update { check, allow_downgrade, yes } => updatecmd::run(g, check, yes, allow_downgrade).await,
        Cmd::Parity { gaps } => parity::run(g, gaps).await,
        Cmd::Api(c) => apicmd::run(g, c).await,
        Cmd::Completion { shell } => {
            let mut buf = Vec::new();
            clap_complete::generate(shell, &mut Cli::command(), "forge", &mut buf);

            // A closed pipe (`| head`) is not an error.
            let _ = std::io::Write::write_all(&mut std::io::stdout().lock(), &buf);
            Ok(())
        }
        Cmd::Commands => {
            out::result(Format::Json, &command_tree(&Cli::command()), None);
            Ok(())
        }
        Cmd::Whoami => cmds::auth(g, cmds::AuthCmd::Whoami).await,
        Cmd::Servers => {
            cmds::nodes(
                g,
                cmds::NodesCmd::List {
                    cluster: None,
                    workspace: None,
                    paging: out::Paging { p: out::PageArgs { limit: 0, page: 1 }, all: true },
                },
            )
            .await
        }
        Cmd::Command { server, kind, name } => {
            cmds::nodes(g, cmds::NodesCmd::Command { node: server, kind, name, no_wait: false, timeout_secs: 25 }).await
        }
    }
}

/// Stack for the CLI thread. Parsing the derived command tree and the large
/// `run` future overflow the 1 MiB main-thread stack of Windows in debug builds.
const STACK_BYTES: usize = 16 * 1024 * 1024;

fn main() {
    // Read while the process is still single-threaded (Unix requirement).
    out::capture_local_offset();
    let worker = std::thread::Builder::new().name("forge".into()).stack_size(STACK_BYTES).spawn(cli_main);
    match worker {
        Ok(handle) => {
            if handle.join().is_err() {
                std::process::exit(Exit::General as i32);
            }
        }
        Err(_) => cli_main(),
    }
}

fn cli_main() {
    let cli = Cli::parse();
    let fmt = cli.global.output;
    out::init(fmt, cli.global.color, cli.global.no_pager);

    let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            out::error(fmt, &CliError::new(Exit::General, e.to_string()));
            std::process::exit(Exit::General as i32);
        }
    };

    if let Err(e) = rt.block_on(run(cli)) {
        out::error(fmt, &e);
        std::process::exit(e.exit as i32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn command_tree_lists_groups() {
        let t = command_tree(&Cli::command());
        let names: Vec<&str> =
            t["subcommands"].as_array().into_iter().flatten().filter_map(|s| s["name"].as_str()).collect();

        for g in ["contexts", "auth", "nodes", "jobs", "audit", "plugins", "api", "completion", "commands"] {
            assert!(names.contains(&g), "missing {g}");
        }

        assert!(!names.contains(&"servers"), "deprecated aliases stay hidden");
    }
}
