//! Executes verified, policy-approved commands.

use std::time::{Duration, Instant};

use async_trait::async_trait;
use forge_proto::Command;
use forge_proto::messages::{RefusalCode, SignedCommand};
use serde_json::{Value, json};

use crate::helper::{HelperRequest, HelperResponse};
use crate::service::ServiceManager;

/// Package recipes can take a while (downloads, database initialisation).
pub(crate) const PACKAGE_TIMEOUT: Duration = Duration::from_secs(50 * 60);
/// Large copies, moves to the trash and commit hashing can take minutes;
/// file commands run in the background, so this never holds the link.
const FILES_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// A failed command, with the refusal code the control plane acts on when
/// there is one (`CommandResult::error_code`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandError {
    pub message: String,
    pub code: Option<RefusalCode>,
}

impl From<String> for CommandError {
    fn from(message: String) -> Self {
        Self { message, code: None }
    }
}

impl From<&str> for CommandError {
    fn from(message: &str) -> Self {
        Self { message: message.to_owned(), code: None }
    }
}

impl CommandError {
    /// A refusal with `code`.
    #[must_use]
    pub fn refused(code: RefusalCode, message: impl Into<String>) -> Self {
        Self { message: message.into(), code: Some(code) }
    }

    /// The helper's answer for this error: the code travels in `data`.
    #[must_use]
    pub fn into_response(self) -> HelperResponse {
        HelperResponse {
            data: self.code.map(|c| json!({ "code": c.as_str() })),
            ..HelperResponse::failed(self.message)
        }
    }
}

/// The refusal code a helper put in a failed response's `data`
/// (`{"code": "unsupported"}`); older helpers send none.
#[must_use]
pub fn refusal_code(r: &HelperResponse) -> Option<RefusalCode> {
    if r.ok {
        return None;
    }

    r.data.as_ref().and_then(|d| d.get("code")).and_then(Value::as_str).and_then(RefusalCode::parse)
}

#[async_trait]
pub trait CommandExecutor: Send + Sync + std::fmt::Debug {
    /// Runs a verified command. `signed` is the command as Forge signed it:
    /// privileged work is sent to the helper in that form (D-068).
    async fn execute(&self, cmd: &Command, signed: &SignedCommand) -> Result<Value, String>;

    /// Like [`execute`](Self::execute), keeping a refusal code. The agent
    /// uses it for commands whose refusals the control plane tells apart
    /// (agent updates).
    async fn execute_coded(&self, cmd: &Command, signed: &SignedCommand) -> Result<Value, CommandError> {
        self.execute(cmd, signed).await.map_err(CommandError::from)
    }

    /// Asks the root helper for a container shell with the command as Forge
    /// signed it (D-114). Tests replace it with an in-memory helper.
    async fn open_exec(&self, signed: &SignedCommand) -> Result<crate::container_exec::ExecPipe, String> {
        crate::container_exec::open_helper(signed).await
    }
}

/// Read-only commands run in the agent; everything privileged goes to the
/// helper service as the signed command.
#[derive(Debug)]
pub struct LocalExecutor {
    services: Box<dyn ServiceManager>,
    started: Instant,
}

impl LocalExecutor {
    #[must_use]
    pub fn new(services: Box<dyn ServiceManager>) -> Self {
        Self { services, started: Instant::now() }
    }
}

#[cfg(unix)]
async fn forge_dns_status() -> &'static str {
    crate::fabric::dns::status().await
}

#[cfg(not(unix))]
async fn forge_dns_status() -> &'static str {
    "missing"
}

/// Runs `cmd` in the helper and keeps the refusal code of a failure.
async fn helper_coded(cmd: &Command, signed: &SignedCommand) -> Result<Value, CommandError> {
    let req = HelperRequest::from_command(cmd)?.ok_or("this command needs no helper")?;
    let r = crate::helper::call_signed(&req, signed, timeout(&req)).await;
    let code = refusal_code(&r);

    result(cmd, r).map_err(|message| CommandError { message, code })
}

#[async_trait]
impl CommandExecutor for LocalExecutor {
    async fn execute_coded(&self, cmd: &Command, signed: &SignedCommand) -> Result<Value, CommandError> {
        match cmd {
            Command::AgentUpdate(_) if !cfg!(target_os = "linux") => Err(CommandError::refused(
                RefusalCode::Unsupported,
                "self-update is available for Linux nodes; reinstall the agent instead",
            )),
            // Fabric refusals (an old or missing Docker for the container
            // network) are told apart too, so the server does not retry them.
            Command::AgentUpdate(_) | Command::FabricAction(_) => helper_coded(cmd, signed).await,
            _ => self.execute(cmd, signed).await.map_err(CommandError::from),
        }
    }

    async fn execute(&self, cmd: &Command, signed: &SignedCommand) -> Result<Value, String> {
        match cmd {
            Command::Ping(_) => Ok(json!({ "pong": true, "uptime_secs": self.started.elapsed().as_secs() })),
            Command::CollectFacts(_) => {
                let (caps, helper_ok) = helper_capabilities().await;
                Ok(json!({
                // Accounts the browser terminal can log in as (D-078).
                "login_accounts": login_accounts().await,
                "hostname": sysinfo::System::host_name(),
                "os": std::env::consts::OS,
                "os_version": sysinfo::System::long_os_version(),
                "kernel": sysinfo::System::kernel_version(),
                "arch": std::env::consts::ARCH,
                "cpus": std::thread::available_parallelism().map(std::num::NonZero::get).unwrap_or(1),
                "agent_version": crate::VERSION,
                // Public host keys (world-readable), so the console can pin
                // this node's SSH key for the browser terminal without
                // trusting whatever answers on the network first.
                "ssh_host_keys": ssh_host_keys(),
                // sshd accepts only certificates bound to this node (D-069).
                "ssh_principals": crate::ssh_principals::configured(),
                // Direct port tunnels can log in over SSH (#35 phase 2).
                "ssh_tunnel": crate::ssh_principals::tunnel_ready(),
                // IPv4 interface addresses the node is reachable on (shown
                // in the console; tunnels may dial them, D-100).
                "interfaces": interface_addresses(),
                // Container shells through the helper, no terminal account
                // (D-114), and whether the node can update its own agent.
                "container_exec": caps.container_exec,
                "self_update": caps.self_update,
                // Understands the cluster container network: stack parts
                // with `mesh_network`, `NetworkEnsure`/`NetworkRemove`
                // (D-172). Older agents omit it.
                "mesh_network": cfg!(unix),
                // Restart policy, capabilities, ulimits, tmpfs, DNS, sysctls,
                // devices, init and extra networks on app specs
                // (plans/service-orchestration.md step 1).
                "container_options": true,
                // Pushes native service container states for the service
                // reconciler (D-177).
                "app_states": cfg!(unix),
                "docker_version": caps.docker_version,
                // forge-dns on fabric nodes: running, stopped or missing.
                "forge_dns": forge_dns_status().await,
                "helper_ok": helper_ok,
                }))
            }
            Command::ContainerExecOpen(_) => Err("container shells are handled by the agent loop".into()),
            Command::ServiceStatus(s) => {
                let state = self.services.status(&s.name).await?;
                Ok(json!({ "name": s.name, "state": state }))
            }
            Command::NetProbe(p) => crate::probe::run(&p.targets, p.count).await,
            // Handled by the agent loop, which owns the running operations.
            Command::CancelAction(_) => Err("cancel is handled by the agent, not the executor".into()),
            Command::ProcessList(p) => {
                let (sort, limit, filter) = (p.sort.clone(), p.limit, p.filter.chars().take(100).collect::<String>());
                tokio::task::spawn_blocking(move || crate::processes::snapshot(&sort, limit, &filter))
                    .await
                    .map_err(|e| e.to_string())
            }
            // A new health policy for a deployed app (D-085). The agent keeps
            // it for its own HTTP and TCP checks; the helper gets the signed
            // command too and keeps the exec and shell checks, which only
            // it may run (D-111). A policy with such checks is not applied
            // unless the helper took it, so Forge sends it again.
            Command::AppAction(a) if a.action == forge_catalog::health::APPLY_ACTION => {
                if cfg!(target_os = "linux") {
                    let envelope = HelperRequest::Signed {
                        command: forge_secrets::crypto::b64url(&prost::Message::encode_to_vec(signed)),
                    };
                    let r = crate::helper::call(&envelope, Duration::from_secs(30)).await;
                    if !r.ok && crate::health::helper_must_take(&a.spec_json) {
                        return Err(format!(
                            "the node's helper did not take the policy, which adds or removes command checks: {}",
                            r.error.unwrap_or_default()
                        ));
                    }
                }

                crate::health::apply(&a.spec_json)
            }
            _ => {
                // Never forward anything the helper would refuse.
                let req = HelperRequest::from_command(cmd)?.ok_or("this command needs no helper")?;
                let r = crate::helper::call_signed(&req, signed, timeout(&req)).await;

                // Default health checks follow what is deployed (D-085).
                if let HelperRequest::App { action, spec, .. } = &req
                    && r.ok
                {
                    // A slot (#73) takes over the checks only when traffic
                    // moves to it: the rollout sends its policy then.
                    match action.as_str() {
                        "deploy" if spec.slot.is_empty() => crate::health::on_deployed(spec),
                        "remove" if spec.slot.is_empty() => crate::health::on_removed(spec),
                        _ => {}
                    }
                }
                result(cmd, r)
            }
        }
    }
}

/// Terminal login accounts: root's view from the helper, else what
/// `/etc/passwd` shows. Windows nodes report none yet.
async fn login_accounts() -> Vec<String> {
    if !cfg!(unix) {
        return Vec::new();
    }

    let r = crate::helper::call(&HelperRequest::LoginAccounts {}, Duration::from_secs(10)).await;
    match r.data.and_then(|d| serde_json::from_value::<Vec<String>>(d).ok()) {
        Some(list) if r.ok => list,
        _ => crate::terminal_accounts::gather_unprivileged(),
    }
}

/// What the helper can do, asked once per fact collection: container
/// shells (D-114: Linux and a helper that verifies commands with its own
/// pin) and self-update. An unreachable, unpinned or older helper says no
/// shells and `None` (unknown) for self-update.
/// Also whether the helper answered at all (`None` off Linux, where there is
/// none): after an update, a silent helper means no watchdog.
async fn helper_capabilities() -> (crate::helper::HelperCapabilities, Option<bool>) {
    if !cfg!(target_os = "linux") {
        return (crate::helper::HelperCapabilities::default(), None);
    }

    let r = crate::helper::call(&HelperRequest::Capabilities {}, Duration::from_secs(10)).await;
    let caps = match r.data.filter(|_| r.ok).map(serde_json::from_value::<crate::helper::HelperCapabilities>) {
        Some(Ok(c)) => c,
        Some(Err(e)) => {
            tracing::warn!(error = %e, "unreadable helper capabilities");
            crate::helper::HelperCapabilities::default()
        }
        // No helper, or one that refused: nothing is offered.
        None => crate::helper::HelperCapabilities::default(),
    };

    (caps, Some(r.ok))
}

/// How long the agent waits for the helper.
fn timeout(req: &HelperRequest) -> Duration {
    use forge_catalog::stacks::StackOp;

    match req {
        HelperRequest::Dns { .. } => Duration::from_secs(60),
        HelperRequest::UpdateAgent { .. } => Duration::from_secs(15 * 60),
        HelperRequest::Files { .. } | HelperRequest::Stack { stack: StackOp::Ps { .. } | StackOp::Logs { .. } } => {
            FILES_TIMEOUT
        }
        _ => PACKAGE_TIMEOUT,
    }
}

/// The command result reported to the control plane.
fn result(cmd: &Command, r: HelperResponse) -> Result<Value, String> {
    match cmd {
        Command::PackageAction(p) => {
            let body = json!({
                "package": p.package,
                "version": p.version,
                "action": p.action,
                "state": r.state,
                "env": r.details,
                "output": r.output,
            });
            if r.ok { Ok(body) } else { Err(json!({ "error": r.error, "output": r.output }).to_string()) }
        }
        // `credential` only for `adopt`; the job seals it and never keeps it
        // in its state.
        Command::StorageAction(_) if r.ok => {
            Ok(json!({ "report": r.data, "output": r.output, "credential": r.details }))
        }
        Command::StorageAction(_) => Err(json!({ "error": r.error, "output": r.output }).to_string()),
        _ if r.ok => Ok(r.data.unwrap_or(Value::Null)),
        _ => Err(r.error.unwrap_or_else(|| format!("{} failed", cmd.name()))),
    }
}

/// `type base64` of the node's SSH host public keys, strongest first.
fn ssh_host_keys() -> Vec<String> {
    ["ed25519", "ecdsa", "rsa"]
        .iter()
        .filter_map(|t| std::fs::read_to_string(format!("/etc/ssh/ssh_host_{t}_key.pub")).ok())
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            let (kind, key) = (f.next()?, f.next()?);
            (kind.len() < 64 && key.len() < 4096).then(|| format!("{kind} {key}"))
        })
        .collect()
}

/// IPv4 addresses of the node's interfaces, without loopback, Forge's own
/// tunnels and container bridges.
fn interface_addresses() -> Vec<Value> {
    let nets = sysinfo::Networks::new_with_refreshed_list();
    let mut out: Vec<Value> = nets
        .iter()
        .filter(|(name, _)| forge_catalog::fabric::offerable_interface(name))
        .flat_map(|(name, data)| {
            data.ip_networks().iter().filter_map(move |n| match n.addr {
                std::net::IpAddr::V4(a) if !a.is_loopback() && !a.is_link_local() && !a.is_unspecified() => {
                    Some(json!({ "interface": name, "address": a.to_string(), "prefix": n.prefix }))
                }
                _ => None,
            })
        })
        .collect();
    out.sort_by_key(ToString::to_string);

    out
}

#[cfg(test)]
mod refusal_tests {
    use super::*;

    #[test]
    fn a_refusal_code_travels_through_the_helper_response() {
        let r = CommandError::refused(RefusalCode::Unsupported, "no systemd").into_response();
        assert!(!r.ok);
        assert_eq!(r.error.as_deref(), Some("no systemd"));
        assert_eq!(refusal_code(&r), Some(RefusalCode::Unsupported));

        // Through the wire (JSON line) too.
        let back: HelperResponse = serde_json::from_str(&serde_json::to_string(&r).unwrap_or_default())
            .unwrap_or_else(|_| HelperResponse::failed("x"));
        assert_eq!(refusal_code(&back), Some(RefusalCode::Unsupported));

        // A plain failure, an older helper and a success carry none.
        assert_eq!(refusal_code(&CommandError::from("boom").into_response()), None);
        assert_eq!(refusal_code(&HelperResponse::failed("old helper")), None);
        let ok = HelperResponse { ok: true, data: Some(json!({ "code": "busy" })), ..HelperResponse::failed("") };
        assert_eq!(refusal_code(&ok), None);
    }
}
