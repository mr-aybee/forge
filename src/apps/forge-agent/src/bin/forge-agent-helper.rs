//! `forge-agent-helper`: privileged helper with a fixed allowlist.
//!
//! * `forge-agent-helper` — reads one JSON request line from stdin (one-shot).
//! * `forge-agent-helper serve [--agent-dir /var/lib/forge-agent]` — runs as
//!   root and serves requests on a Unix socket that only root and the owner of
//!   the agent's state directory (the unprivileged `forge-agent` user) may use.
//! * `forge-agent-helper pin [--agent-dir /var/lib/forge-agent]` — (root,
//!   installer) pins Forge's command key and this node's id from a fresh
//!   enrollment (D-068).
//!
//! Requests arrive as commands signed by Forge; the helper verifies them
//! against its pin and derives what to run (`forge_agent::helper_trust`).
//! Every request is validated against the closed set in
//! `forge_agent::helper::HelperRequest` (package requests against the
//! compiled-in catalog) before anything runs.

use std::io::{BufRead, Write};

use forge_agent::helper::{HelperRequest, HelperResponse};
use forge_agent::helper_trust::{self, Trust};
use forge_agent::service;
use forge_core::{Clock, SystemClock};

fn respond(r: &HelperResponse) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{}", serde_json::to_string(r).unwrap_or_else(|_| "{\"ok\":false}".into()));
}

/// The `Capabilities` answer: container shells (pinned helpers only) and
/// whether this host can update its own agent.
async fn capabilities(container_exec: bool) -> HelperResponse {
    let caps = forge_agent::helper::HelperCapabilities {
        container_exec,
        self_update: forge_agent::self_update::capability().await,
    };
    data(serde_json::to_value(caps).map_err(|e| e.to_string()))
}

fn data(r: Result<serde_json::Value, String>) -> HelperResponse {
    match r {
        Ok(d) => HelperResponse { ok: true, state: None, error: None, output: None, details: None, data: Some(d) },
        Err(e) => HelperResponse::failed(e),
    }
}

/// Applies a control-plane key announcement (D-167) to the root-owned pin
/// file, once Forge's signature checks out against the helper's own pin.
/// A helper without a pin (a node set up before D-068) refuses: the agent
/// then keeps checking the CA and host name.
#[cfg(unix)]
async fn server_pins(pin: Option<helper_trust::Pin>, keys: String) -> HelperResponse {
    let Some(pin) = pin else {
        return HelperResponse::failed("refused: this helper has no pin of Forge's command key to check the keys with");
    };

    let run = move || -> Result<serde_json::Value, String> {
        let raw = forge_secrets::crypto::b64url_decode(&keys).ok_or("refused: the keys are not base64url")?;
        let signed = <forge_proto::messages::SignedServerKeys as prost::Message>::decode(raw.as_slice())
            .map_err(|_| "refused: malformed keys".to_owned())?;
        let public = forge_secrets::crypto::b64url_decode(&pin.command_public_key).ok_or("bad pinned command key")?;
        let path = std::path::Path::new(forge_agent::pins::SYSTEM_DIR).join(forge_agent::pins::FILE);
        let changed = forge_agent::pins::apply_signed(&path, &signed, &public, &pin.server_id, SystemClock.now())
            .map_err(|e| e.to_string())?;

        if changed {
            tracing::info!(path = %path.display(), "control-plane key pins updated from Forge's announcement");
        }

        Ok(serde_json::json!({ "changed": changed }))
    };

    match tokio::task::spawn_blocking(run).await {
        Ok(r) => data(r),
        Err(e) => HelperResponse::failed(e.to_string()),
    }
}

/// `pin`: adopts the control-plane key pins the enrollment staged as the
/// agent user (D-167), only when no root-owned pin file exists yet.
fn adopt_enrollment_pins(agent_dir: &std::path::Path) {
    use forge_agent::pins;

    if !pins::root_owned(agent_dir) {
        return;
    }

    let system = std::path::Path::new(pins::SYSTEM_DIR).join(pins::FILE);
    match pins::adopt_staged(&pins::staged_path(agent_dir), &system) {
        Ok(pins::Adoption::Adopted(n)) => println!("pinned {n} control-plane key(s) in {}", system.display()),
        Ok(pins::Adoption::KeptExisting) => println!("kept the existing pin file {}", system.display()),
        Ok(pins::Adoption::NothingStaged) => {}
        Err(e) => {
            eprintln!("forge-agent-helper pin: {e}");
            std::process::exit(1);
        }
    }
}

/// The agent state directory (`serve --agent-dir`).
fn agent_dir() -> std::path::PathBuf {
    AGENT_DIR.get().cloned().unwrap_or_else(|| std::path::PathBuf::from("/var/lib/forge-agent"))
}

static AGENT_DIR: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

/// What a request handler shares with the others of this helper run.
#[derive(Clone)]
struct Ctx {
    health: std::sync::Arc<forge_agent::health_exec::Runner>,
    /// Container logs and samples (`serve` only).
    #[cfg(unix)]
    containers: Option<forge_agent::container_telemetry::Collector>,
}

async fn handle(
    req: HelperRequest,
    progress: Option<tokio::sync::mpsc::UnboundedSender<String>>,
    ctx: Ctx,
) -> HelperResponse {
    let health = ctx.health.clone();
    let manager = service::platform();

    match req {
        HelperRequest::RestartService { name } => match manager.restart(&name).await {
            Ok(()) => HelperResponse { ok: true, state: None, error: None, output: None, details: None, data: None },
            Err(e) => HelperResponse::failed(e),
        },
        HelperRequest::ServiceStatus { name } => match manager.status(&name).await {
            Ok(s) => HelperResponse {
                ok: true,
                state: serde_json::to_value(s).ok().and_then(|v| v.as_str().map(str::to_owned)),
                error: None,
                output: None,
                details: None,
                data: None,
            },
            Err(e) => HelperResponse::failed(e),
        },
        HelperRequest::Container { container } => container_request(&container).await,
        #[cfg(unix)]
        HelperRequest::ContainerState { name } => data(forge_agent::docker::container_state(&name).await),
        #[cfg(not(unix))]
        HelperRequest::ContainerState { .. } => HelperResponse::failed("containers are not supported on this OS yet"),
        HelperRequest::App { action, spec, cursor, .. } if action == "follow" => {
            follow_request(&spec, &cursor, progress.as_ref()).await
        }
        HelperRequest::App { action, spec, tail, .. } => {
            let r = app_request(&action, &spec, tail).await;

            // The command checks the helper may run follow what is deployed (D-111).
            if r.ok {
                let (dir, now_ms) = (forge_agent::health_exec::state_dir(), SystemClock.now().0);

                // Best effort and logged inside: it never fails the app action.
                let _ = tokio::task::spawn_blocking(move || {
                    forge_agent::health_exec::on_app_action(&dir, &action, &spec, now_ms);
                })
                .await;
            }

            r
        }
        HelperRequest::HealthPolicy { health: policy, issued_at_ms } => {
            let at = forge_agent::health_exec::Times { issued_at_ms, now_ms: SystemClock.now().0 };
            let stored = tokio::task::spawn_blocking(move || {
                forge_agent::health_exec::store(&forge_agent::health_exec::state_dir(), &policy, at)
            })
            .await;

            match stored {
                Ok(Ok(applied)) => data(serde_json::to_value(applied).map_err(|e| e.to_string())),
                Ok(Err(e)) => HelperResponse::failed(e.to_string()),
                Err(e) => HelperResponse::failed(e.to_string()),
            }
        }
        HelperRequest::HealthCheck { check } => {
            data(serde_json::to_value(health.run(&check).await).map_err(|e| e.to_string()))
        }
        HelperRequest::Site { action, spec } => data(forge_agent::sites::site_action(&action, &spec).await),
        HelperRequest::Dns { dns } => data(forge_agent::sites::dns_action(&dns).await),
        HelperRequest::Ingress { ingress } => data(forge_agent::ingress::run_op(&ingress).await),
        // A refusal code travels in `data` (`executor::refusal_code`).
        HelperRequest::Fabric { fabric } => match forge_agent::fabric::run_op(&fabric).await {
            Ok(v) => data(Ok(v)),
            Err(e) => e.into_response(),
        },
        HelperRequest::Names { suffix, entries, containers } => {
            data(forge_agent::host_names::apply(suffix, entries, containers).await)
        }
        HelperRequest::Storage { storage } => store_request(&storage, progress.as_ref()).await,
        HelperRequest::Files { files } => data(forge_agent::files::run_async(files).await),
        HelperRequest::UpdateAgent { update } => {
            let dir = agent_dir();
            match tokio::task::spawn_blocking(move || forge_agent::self_update::apply(&update, &dir)).await {
                Ok(Ok(v)) => data(Ok(v)),
                // A refusal code travels in `data` (`executor::refusal_code`).
                Ok(Err(e)) => e.into_response(),
                Err(e) => HelperResponse::failed(e.to_string()),
            }
        }
        HelperRequest::ConfirmUpdate {} => data(forge_agent::self_update::confirm(&agent_dir())),
        #[cfg(unix)]
        HelperRequest::Stack { stack } => data(forge_agent::stacks::run_op(&stack).await),
        #[cfg(not(unix))]
        HelperRequest::Stack { .. } => HelperResponse::failed("compose stacks need a Linux node"),
        #[cfg(unix)]
        HelperRequest::DockerTls { docker_tls } => data(forge_agent::docker_tls::run(&docker_tls).await),
        #[cfg(not(unix))]
        HelperRequest::DockerTls { .. } => HelperResponse::failed("the Docker TLS endpoint needs a Linux node"),
        #[cfg(unix)]
        HelperRequest::Docker { docker } => data(forge_agent::docker_ops::run(&docker).await),
        #[cfg(not(unix))]
        HelperRequest::Docker { .. } => HelperResponse::failed("Docker resources need a Linux node"),
        #[cfg(unix)]
        HelperRequest::Swarm { swarm } => data(forge_agent::swarm::run(&swarm).await),
        #[cfg(not(unix))]
        HelperRequest::Swarm { .. } => HelperResponse::failed("Docker Swarm needs a Linux node"),
        #[cfg(unix)]
        HelperRequest::LoginAccounts {} => data(Ok(serde_json::json!(forge_agent::terminal_accounts::gather().await))),
        #[cfg(unix)]
        HelperRequest::TerminalAccount { name, sudo } => {
            data(forge_agent::terminal_accounts::ensure(&name, sudo).await)
        }
        #[cfg(not(unix))]
        HelperRequest::LoginAccounts {} | HelperRequest::TerminalAccount { .. } => {
            HelperResponse::failed("terminal accounts need a Linux node")
        }
        HelperRequest::Process { process } => data(
            tokio::task::spawn_blocking(move || forge_agent::processes::run(&process))
                .await
                .unwrap_or_else(|e| Err(e.to_string())),
        ),
        HelperRequest::Package { package, version, action, params } => {
            package_request(&package, &version, &action, &params, progress.as_ref()).await
        }
        // `Trust::admit` replaces it with the request derived from the command.
        HelperRequest::Signed { .. } => HelperResponse::failed("internal: signed request not admitted"),
        // One-shot runs never serve shells.
        HelperRequest::Capabilities {} => capabilities(false).await,
        HelperRequest::ServerPins { .. } => {
            HelperResponse::failed("key announcements are applied by the helper service")
        }
        #[cfg(unix)]
        HelperRequest::ContainerTelemetry { telemetry } => match &ctx.containers {
            Some(c) => data(c.handle(telemetry).await.and_then(|r| serde_json::to_value(r).map_err(|e| e.to_string()))),
            None => HelperResponse::failed("container telemetry needs `forge-agent-helper serve`"),
        },
        #[cfg(not(unix))]
        HelperRequest::ContainerTelemetry { .. } => {
            HelperResponse::failed("containers are not supported on this OS yet")
        }
        // Served on the socket itself by `serve` (it streams).
        HelperRequest::ContainerExec { .. } => {
            HelperResponse::failed("container shells need `forge-agent-helper serve`")
        }
    }
}

fn agent_dir_arg(args: &[String]) -> String {
    args.iter()
        .position(|a| a == "--agent-dir")
        .and_then(|i| args.get(i + 1))
        .map_or("/var/lib/forge-agent", String::as_str)
        .to_owned()
}

#[cfg(unix)]
async fn package_request(
    package: &str,
    version: &str,
    action: &str,
    params: &std::collections::BTreeMap<String, String>,
    progress: Option<&tokio::sync::mpsc::UnboundedSender<String>>,
) -> HelperResponse {
    match forge_catalog::validate(package, version, action, params) {
        Ok(v) => forge_agent::recipes::run(&v, progress).await,
        Err(e) => HelperResponse::failed(e),
    }
}

async fn store_request(
    op: &forge_catalog::storage::StoreOp,
    progress: Option<&tokio::sync::mpsc::UnboundedSender<String>>,
) -> HelperResponse {
    forge_agent::recipes::run_store(op, progress).await
}

#[cfg(not(unix))]
async fn package_request(
    _: &str,
    _: &str,
    _: &str,
    _: &std::collections::BTreeMap<String, String>,
    _: Option<&tokio::sync::mpsc::UnboundedSender<String>>,
) -> HelperResponse {
    HelperResponse::failed("packages are not supported on this OS yet")
}

#[cfg(unix)]
async fn app_request(action: &str, spec: &forge_catalog::AppSpec, tail: u32) -> HelperResponse {
    match forge_agent::apps::run(action, spec, tail).await {
        Ok(data) => {
            HelperResponse { ok: true, state: None, error: None, output: None, details: None, data: Some(data) }
        }
        Err(e) => HelperResponse::failed(e),
    }
}

#[cfg(unix)]
async fn follow_request(
    spec: &forge_catalog::AppSpec,
    cursor: &str,
    progress: Option<&tokio::sync::mpsc::UnboundedSender<String>>,
) -> HelperResponse {
    let emit = |l: &str| {
        if let Some(p) = progress {
            let _ = p.send(l.to_owned());
        }
    };

    match forge_agent::apps::follow(spec, cursor, &emit).await {
        Ok(data) => {
            HelperResponse { ok: true, state: None, error: None, output: None, details: None, data: Some(data) }
        }
        Err(e) => HelperResponse::failed(e),
    }
}

#[cfg(not(unix))]
async fn follow_request(
    _: &forge_catalog::AppSpec,
    _: &str,
    _: Option<&tokio::sync::mpsc::UnboundedSender<String>>,
) -> HelperResponse {
    HelperResponse::failed("services are not supported on this OS yet")
}

#[cfg(not(unix))]
async fn app_request(_: &str, _: &forge_catalog::AppSpec, _: u32) -> HelperResponse {
    HelperResponse::failed("services are not supported on this OS yet")
}

#[cfg(unix)]
async fn container_request(op: &forge_agent::helper::ContainerOp) -> HelperResponse {
    match forge_agent::docker::run(op).await {
        Ok(data) => {
            HelperResponse { ok: true, state: None, error: None, output: None, details: None, data: Some(data) }
        }
        Err(e) => HelperResponse::failed(e),
    }
}

#[cfg(not(unix))]
async fn container_request(_: &forge_agent::helper::ContainerOp) -> HelperResponse {
    HelperResponse::failed("containers are not supported on this OS yet")
}

#[cfg(unix)]
async fn serve(agent_dir: &str) -> std::io::Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    use forge_agent::helper::SOCKET;
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

    // The agent's state directory is owned by the unprivileged agent user.
    let meta = std::fs::metadata(agent_dir)?;
    let _ = AGENT_DIR.set(std::path::PathBuf::from(agent_dir));

    // Self-update: roll back if a new agent never confirms (issue #4).
    // A swap a crash interrupted is finished or undone before the watchdog
    // looks at the pending marker.
    {
        let dir = std::path::PathBuf::from(agent_dir);

        // Logged inside; nothing else to do here either way.
        let _ = tokio::task::spawn_blocking(move || forge_agent::self_update::recover(&dir)).await;
    }

    tokio::spawn(forge_agent::self_update::watchdog(std::path::PathBuf::from(agent_dir)));
    let (agent_uid, agent_gid) = (meta.uid(), meta.gid());

    // Forge's command key and this node's id, pinned in the helper's
    // root-only folder (on first use for nodes enrolled before D-068).
    let trust = std::sync::Arc::new(Trust::open(
        &helper_trust::state_dir(),
        std::path::Path::new(agent_dir),
        true,
        SystemClock.now(),
    ));

    match trust.pinned() {
        Some(p) => {
            eprintln!("forge-agent-helper: verifying commands for server {} (pin: {})", p.server_id, p.source);

            // Browser terminal certificates bound to this node (D-069).
            match forge_agent::ssh_principals::ensure(&p.server_id).await {
                Ok(true) => eprintln!("forge-agent-helper: sshd now accepts only certificates for this node"),
                Ok(false) => {}
                Err(e) => eprintln!("forge-agent-helper: node-bound SSH certificates not set up: {e}"),
            }
        }
        None => eprintln!("forge-agent-helper: no pin; requests are not verified (enroll the node)"),
    }

    // Command health checks: who runs now and what could not be ended (D-111).
    let health = std::sync::Arc::new(forge_agent::health_exec::Runner::new(forge_agent::health_exec::state_dir()));

    // Container shells of this run, counted for the per-node limit (D-114).
    let shells = forge_agent::container_exec::ShellCount::default();

    // Logs and samples of every container; cursors in the root-only folder.
    let ctx = Ctx {
        health,
        containers: Some(forge_agent::container_telemetry::Collector::new(
            std::path::Path::new(forge_agent::docker::SOCKET),
            &helper_trust::state_dir(),
        )),
    };
    let _ = std::fs::remove_file(SOCKET);
    let listener = tokio::net::UnixListener::bind(SOCKET)?;
    std::os::unix::fs::chown(SOCKET, Some(0), Some(agent_gid))?;
    std::fs::set_permissions(SOCKET, std::fs::Permissions::from_mode(0o660))?;
    eprintln!("forge-agent-helper: serving on {SOCKET} for uid {agent_uid}");

    loop {
        let (stream, _) = listener.accept().await?;
        let allowed = stream.peer_cred().is_ok_and(|c| c.uid() == 0 || c.uid() == agent_uid);
        let trust = trust.clone();
        let ctx = ctx.clone();
        let shells = shells.clone();
        tokio::spawn(async move {
            let (read, mut write) = stream.into_split();

            let resp = if allowed {
                let mut line = String::new();
                let mut reader = BufReader::new(read.take(forge_agent::helper::MAX_REQUEST_BYTES as u64 + 2));
                match reader.read_line(&mut line).await {
                    Ok(_) => {
                        match HelperRequest::parse(line.trim()).and_then(|req| trust.admit(req, SystemClock.now())) {
                            // A container shell streams frames on this
                            // connection after its answer line (D-114).
                            Ok(HelperRequest::ContainerExec { exec }) => {
                                if !reader.buffer().is_empty() {
                                    let _ = write.write_all(b"{\"ok\":false,\"error\":\"unexpected data\"}\n").await;
                                    return;
                                }

                                let read = reader.into_inner().into_inner();
                                let docker = std::path::Path::new(forge_agent::docker::SOCKET);
                                forge_agent::container_exec::serve(exec, docker, &shells, read, write).await;

                                return;
                            }
                            // Shells only with a pin: `admit` refuses them otherwise.
                            Ok(HelperRequest::Capabilities {}) => capabilities(trust.pinned().is_some()).await,
                            // Checked against Forge's key, never the agent's word.
                            Ok(HelperRequest::ServerPins { keys }) => server_pins(trust.pinned().cloned(), keys).await,
                            Ok(req) => {
                                // Forward live output lines while the request runs.
                                let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
                                let work = tokio::spawn(handle(req, Some(tx), ctx));

                                // The agent closing the connection means the
                                // operation was cancelled: stop the work, which
                                // kills the recipe's process group.
                                let disconnected = async {
                                    let mut b = [0u8; 64];

                                    loop {
                                        match reader.read(&mut b).await {
                                            Ok(0) | Err(_) => break,
                                            Ok(_) => {}
                                        }
                                    }
                                };
                                tokio::pin!(disconnected);

                                loop {
                                    tokio::select! {
                                        l = rx.recv() => match l {
                                            Some(l) => {
                                                let progress = forge_agent::helper::Progress { progress: l };
                                                if let Ok(mut out) = serde_json::to_string(&progress) {
                                                    out.push('\n');
                                                    let _ = write.write_all(out.as_bytes()).await;
                                                }
                                            }
                                            None => break,
                                        },
                                        () = &mut disconnected => {
                                            eprintln!("forge-agent-helper: the agent cancelled the operation");
                                            work.abort();
                                            break;
                                        }
                                    }
                                }

                                work.await.unwrap_or_else(|e| {
                                    HelperResponse::failed(if e.is_cancelled() {
                                        "cancelled".to_owned()
                                    } else {
                                        e.to_string()
                                    })
                                })
                            }
                            Err(e) => HelperResponse::failed(e),
                        }
                    }
                    Err(e) => HelperResponse::failed(e.to_string()),
                }
            } else {
                HelperResponse::failed("caller is not the forge-agent user")
            };

            let mut out = serde_json::to_string(&resp).unwrap_or_else(|_| "{\"ok\":false}".into());
            out.push('\n');
            let _ = write.write_all(out.as_bytes()).await;
        });
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    // The self-update dry run of a staged helper (`forge-agent-helper 1.2.3`).
    if matches!(args.get(1).map(String::as_str), Some("--version" | "-V" | "version")) {
        println!("forge-agent-helper {}", forge_agent::VERSION);
        return;
    }

    let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            respond(&HelperResponse::failed(e.to_string()));
            std::process::exit(1);
        }
    };

    #[cfg(target_os = "linux")]
    if args.get(1).map(String::as_str) == Some("file-worker") {
        // One file operation as a native app's account (D-083); started by
        // the helper itself with the app's uid and gid.
        std::process::exit(forge_agent::files::app_worker::worker_main(&args[2..]));
    }
    #[cfg(target_os = "linux")]
    if args.get(1).map(String::as_str) == Some("health-worker") {
        // One command health check as a native app's account (D-111);
        // started by the helper itself with the app's uid and gid.
        std::process::exit(forge_agent::health_exec::worker_main(&args[2..]));
    }
    #[cfg(unix)]
    if args.get(1).map(String::as_str) == Some("supervise") {
        let Some(spec) = args.get(2) else { std::process::exit(2) };
        if let Err(e) = rt.block_on(forge_agent::apps::supervise(spec)) {
            eprintln!("forge-agent-helper supervise: {e}");
            std::process::exit(1);
        }

        return;
    }
    // sshd `AuthorizedPrincipalsCommand` (runs as `nobody`): the one
    // principal this node accepts for the user (D-069).
    if args.get(1).map(String::as_str) == Some("ssh-principals") {
        let user = args.get(2).map_or("", String::as_str);

        // The certificate's key id (`%i`), signed by Forge's CA: names the
        // one destination of a direct tunnel (#35 phase 2).
        let key_id = args.get(3).map_or("", String::as_str);
        let node = std::fs::read_to_string(forge_agent::ssh_principals::NODE_FILE).unwrap_or_default();
        if let Some(p) = forge_agent::ssh_principals::principals_for(&node, user, key_id) {
            println!("{p}");
        }

        return;
    }

    // Installer, as root: create or keep a terminal account (D-078).
    #[cfg(unix)]
    if args.get(1).map(String::as_str) == Some("terminal-account") {
        let name = args.get(2).map_or("", String::as_str);
        let sudo = args.iter().skip(3).any(|a| a == "--sudo");

        if !rustix::process::geteuid().is_root() {
            eprintln!("forge-agent-helper terminal-account: run as root");
            std::process::exit(1);
        }

        match rt.block_on(forge_agent::terminal_accounts::ensure(name, sudo)) {
            Ok(v) => println!(
                "terminal account {name}: {}{}",
                if v["created"] == true { "created" } else { "kept" },
                if sudo { ", passwordless sudo" } else { ", no sudo" }
            ),
            Err(e) => {
                eprintln!("forge-agent-helper terminal-account: {e}");
                std::process::exit(1);
            }
        }

        return;
    }
    // Installer, as root, right after a fresh enrollment.
    if args.get(1).map(String::as_str) == Some("pin") {
        let dir = agent_dir_arg(&args);
        match helper_trust::pin_from_agent(std::path::Path::new(&dir), "installer")
            .and_then(|p| helper_trust::save_pin(&helper_trust::state_dir(), &p).map(|()| p))
        {
            Ok(p) => println!("pinned Forge's command key for server {}", p.server_id),
            Err(e) => {
                eprintln!("forge-agent-helper pin: {e}");
                std::process::exit(1);
            }
        }

        adopt_enrollment_pins(std::path::Path::new(&dir));

        return;
    }

    if args.get(1).map(String::as_str) == Some("serve") {
        #[cfg(unix)]
        {
            let dir = agent_dir_arg(&args);
            if let Err(e) = rt.block_on(serve(&dir)) {
                eprintln!("forge-agent-helper: {e}");
                std::process::exit(1);
            }
            return;
        }
        #[cfg(not(unix))]
        {
            eprintln!("forge-agent-helper: serve is not supported on this OS yet");
            std::process::exit(2);
        }
    }

    let mut line = String::new();
    if std::io::stdin().lock().read_line(&mut line).is_err() {
        respond(&HelperResponse::failed("no request"));
        std::process::exit(2);
    }

    // One-shot: runs with the caller's own rights; signed requests are
    // checked against a pin if this node has one.
    let trust =
        Trust::open(&helper_trust::state_dir(), std::path::Path::new(&agent_dir_arg(&args)), false, SystemClock.now());

    let req = match HelperRequest::parse(line.trim()).and_then(|r| trust.admit(r, SystemClock.now())) {
        Ok(r) => r,
        Err(e) => {
            respond(&HelperResponse::failed(e));
            std::process::exit(2);
        }
    };

    let health = std::sync::Arc::new(forge_agent::health_exec::Runner::new(forge_agent::health_exec::state_dir()));
    let ctx = Ctx {
        health,
        #[cfg(unix)]
        containers: None,
    };
    let resp = rt.block_on(handle(req, None, ctx));
    respond(&resp);
    std::process::exit(i32::from(!resp.ok));
}

/// Fails when a helper command or option is missing from
/// `docs/src/content/docs/reference/cli/forge-agent-helper.md`. Keep the list
/// in step with `main` (argv is parsed by hand).
#[cfg(test)]
mod docs_coverage {
    const COMMANDS: &[&str] =
        &["serve", "pin", "ssh-principals", "supervise", "terminal-account", "file-worker", "health-worker"];
    const OPTIONS: &[&str] = &["--agent-dir", "--sudo"];

    #[test]
    fn every_command_and_option_is_documented() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../docs/src/content/docs/reference/cli/forge-agent-helper.md");
        let doc = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{} is missing: {e}", path.display()));
        let missing: Vec<&str> = COMMANDS
            .iter()
            .filter(|c| !doc.contains(&format!("## {c}")) && !doc.contains(&format!("## forge-agent-helper {c}")))
            .chain(OPTIONS.iter().filter(|o| !doc.contains(*o)))
            .copied()
            .collect();
        assert!(missing.is_empty(), "undocumented in reference/cli/forge-agent-helper.md: {missing:?}");
    }
}
