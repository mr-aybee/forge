//! Protocol of the privileged helper (`forge-agent-helper`).
//!
//! The helper runs as root/SYSTEM. It reads one JSON request per line: per
//! connection on its Unix socket (`serve`), or from stdin when run one-shot.
//! It accepts only the variants below. Arguments are validated before use;
//! there is no generic "run command" request. Package requests name a
//! catalog recipe compiled into the helper; parameters are re-validated here.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::service::valid_service_name;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum HelperRequest {
    RestartService {
        name: String,
    },
    ServiceStatus {
        name: String,
    },
    Package {
        package: String,
        version: String,
        action: String,
        #[serde(default)]
        params: BTreeMap<String, String>,
    },
    Container {
        container: ContainerOp,
    },
    App {
        action: String,
        spec: Box<forge_catalog::AppSpec>,
        #[serde(default)]
        tail: u32,
        #[serde(default)]
        cursor: String,
        /// `deploy` of a container: Docker's `X-Registry-Auth` for the
        /// image's private registry, from the signed command. Never logged
        /// (`Debug` prints `***`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        registry_auth: Option<forge_catalog::swarm::RegistryAuth>,
    },
    Site {
        action: String,
        spec: Box<forge_catalog::SiteSpec>,
    },
    Dns {
        dns: crate::sites::DnsOp,
    },
    Ingress {
        ingress: forge_catalog::ingress::IngressOp,
    },
    Fabric {
        fabric: forge_catalog::fabric::FabricOp,
    },
    /// File explorer operation (confined and policy-checked in `crate::files`).
    /// Install a new agent + helper (issue #4); see `self_update`.
    UpdateAgent {
        update: crate::self_update::UpdateSpec,
    },
    /// The new agent reconnected: keep it (clears the rollback marker).
    ConfirmUpdate {},
    Files {
        files: forge_core::files::FileOp,
    },
    /// Compose stack (`crate::stacks`; issue #7).
    Stack {
        stack: forge_catalog::stacks::StackOp,
    },
    /// dockerd's TCP listener with mutual TLS (`crate::docker_tls`, D-125).
    DockerTls {
        docker_tls: forge_catalog::docker_tls::DockerTlsOp,
    },
    /// Docker Swarm membership and nodes (`crate::swarm`, D-154).
    Swarm {
        swarm: forge_catalog::swarm::SwarmOp,
    },
    /// Docker images, volumes and networks (`crate::docker_ops`, D-157).
    Docker {
        docker: forge_catalog::docker_ops::DockerOp,
    },
    /// Accounts a terminal can log in as (`crate::terminal_accounts`).
    /// Read-only, so the agent may ask without a signed command.
    LoginAccounts {},
    /// Whether a Forge app container runs, for its default health check
    /// (D-085). Read-only and limited to `forge-app-*` names, returning only
    /// `{id, running, status, health}`, so the agent may ask without a signed
    /// command (D-068).
    ContainerState {
        name: String,
    },
    /// The command checks of an app as Forge signed them (the app command
    /// `health`, D-111). Only ever derived from a signed command: the helper
    /// keeps them in its root-only folder.
    HealthPolicy {
        health: Box<forge_catalog::health::ApplySpec>,
        /// When Forge issued the signed command (epoch ms), filled in by
        /// the helper from the verified command, never by the agent: an
        /// older command than the one the helper holds is refused.
        #[serde(default)]
        issued_at_ms: i64,
    },
    /// Run one command check the helper holds, by reference (D-111). The
    /// agent asks on its own, at the check's period, so this is allowed
    /// without a signed command: it names a check, never a command.
    HealthCheck {
        check: crate::health_exec::CheckRef,
    },
    /// Logs and samples of every Docker container (`crate::container_telemetry`).
    /// Read-only and limited to collection on/off, the exclude list and the
    /// acknowledgement of a batch, so the agent may ask without a signed
    /// command: what it reads goes to Forge through the agent anyway.
    ContainerTelemetry {
        telemetry: crate::container_telemetry::TelemetryRequest,
    },
    /// The node's compose containers by stack (`crate::stacks::states`):
    /// read-only, and Forge learns the same rows from every deploy, so the
    /// agent may ask without a signed command.
    StackStates {},
    /// The node's native service containers (`crate::apps::app_states`):
    /// read-only, so the agent may ask without a signed command (D-177).
    AppStates {},
    /// What this helper can do, for the agent's facts: `container_exec`
    /// is true only when the helper verifies commands with a pin (D-114).
    /// Read-only, so the agent may ask without a signed command.
    Capabilities {},
    /// Pin the control-plane keys Forge announced (D-167): base64url of a
    /// `SignedServerKeys`. The helper checks Forge's signature with its own
    /// pin before it writes the root-owned pin file, so the agent cannot
    /// pin a key of its choosing.
    ServerPins {
        keys: String,
    },
    /// Create or keep a terminal account (D-078).
    TerminalAccount {
        name: String,
        sudo: bool,
    },
    /// End a process (tree) or change its priority (D-080).
    Process {
        process: crate::processes::ProcessOp,
    },
    /// The fleet's names in the hosts file (D-084).
    Names {
        suffix: String,
        entries: Vec<forge_catalog::names::NameEntry>,
        containers: bool,
        /// The cluster's record set for forge-dns (`names.json`); `None`
        /// outside a fabric and from older servers.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        records: Option<Box<forge_catalog::names::Records>>,
    },
    /// A command exactly as Forge signed it (base64url of the protobuf
    /// `SignedCommand`). The helper verifies the signature with the key it
    /// pinned itself and derives the request from the command with
    /// [`HelperRequest::from_command`], so the agent cannot choose what runs
    /// as root (D-068).
    Signed {
        command: String,
    },
    /// A container shell (D-114). Never parsed from a request line: only
    /// derived from a signed `ContainerExecOpen` (an unpinned helper takes
    /// it only in the signed form the agent verified). The helper answers
    /// one line, then the connection carries `exec_frame` frames.
    #[serde(skip)]
    ContainerExec {
        exec: crate::container_exec::ExecSpec,
    },
}

/// Closed set of Docker Engine operations (see `crate::docker`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ContainerOp {
    Info,
    List,
    Images,
    Inspect {
        id: String,
        /// Also return environment values (a signed, MFA-guarded command on
        /// the server side); names only when false.
        #[serde(default)]
        reveal_env: bool,
    },
    Start {
        id: String,
    },
    Stop {
        id: String,
    },
    Restart {
        id: String,
    },
    Remove {
        id: String,
    },
    Logs {
        id: String,
        tail: u32,
        /// Only lines after this Unix time (decimal seconds); live follow.
        #[serde(default)]
        since: Option<String>,
    },
    Stats {
        id: String,
    },
    Pull {
        image: String,
    },
}

/// A decimal Unix time: `1731234567` or `1731234567.123456789`.
#[must_use]
pub fn valid_since(s: &str) -> bool {
    let (secs, frac) = s.split_once('.').unwrap_or((s, ""));
    !secs.is_empty()
        && s.len() <= 24
        && secs.bytes().all(|b| b.is_ascii_digit())
        && frac.bytes().all(|b| b.is_ascii_digit())
}

/// Container ids and names: `[A-Za-z0-9][A-Za-z0-9_.-]{0,127}`.
#[must_use]
pub fn valid_container_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.as_bytes()[0].is_ascii_alphanumeric()
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}

/// Image references: `registry/name:tag` or `name@sha256:...`.
#[must_use]
pub fn valid_image(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 255
        && s.as_bytes()[0].is_ascii_alphanumeric()
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b':' | b'.' | b'_' | b'-' | b'@'))
}

impl ContainerOp {
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Info | Self::List | Self::Images => Ok(()),
            Self::Inspect { id, .. }
            | Self::Start { id }
            | Self::Stop { id }
            | Self::Restart { id }
            | Self::Remove { id }
            | Self::Stats { id } => {
                if valid_container_id(id) {
                    Ok(())
                } else {
                    Err(format!("invalid container id `{id}`"))
                }
            }
            Self::Logs { id, tail, since } => {
                if !valid_container_id(id) {
                    Err(format!("invalid container id `{id}`"))
                } else if since.as_deref().is_some_and(|s| !valid_since(s)) {
                    Err("`since` must be a Unix time in seconds".into())
                } else if *tail == 0 || *tail > 5000 {
                    Err("tail must be 1-5000 lines".into())
                } else {
                    Ok(())
                }
            }
            Self::Pull { image } => {
                if valid_image(image) {
                    Ok(())
                } else {
                    Err(format!("invalid image reference `{image}`"))
                }
            }
        }
    }

    /// Builds an operation from the wire command's fields.
    pub fn from_wire(
        op: &str,
        id: &str,
        image: &str,
        tail: u32,
        since: &str,
        reveal_env: bool,
    ) -> Result<Self, String> {
        let id = id.to_owned();

        let v = match op {
            "info" => Self::Info,
            "list" => Self::List,
            "images" => Self::Images,
            "inspect" => Self::Inspect { id, reveal_env },
            "start" => Self::Start { id },
            "stop" => Self::Stop { id },
            "restart" => Self::Restart { id },
            "remove" => Self::Remove { id },
            "logs" => Self::Logs {
                id,
                tail: if tail == 0 { 200 } else { tail },
                since: (!since.is_empty()).then(|| since.to_owned()),
            },
            "stats" => Self::Stats { id },
            "pull" => Self::Pull { image: image.to_owned() },
            other => return Err(format!("unknown container operation `{other}`")),
        };

        v.validate()?;

        Ok(v)
    }
}

/// Unix socket served by `forge-agent-helper serve` (root). Only root and the
/// `forge-agent` user may connect.
pub const SOCKET: &str = "/run/forge-agent-helper.sock";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelperResponse {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Tail of the recipe's output (package requests).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    /// The package's env file with secret values redacted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<BTreeMap<String, String>>,
    /// Structured result (container operations).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

/// The helper's `Capabilities` answer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelperCapabilities {
    /// Container shells: only with a pinned helper (D-114).
    #[serde(default)]
    pub container_exec: bool,
    /// Self-update on this host; `None` from older helpers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub self_update: Option<forge_proto::facts::SelfUpdateCapability>,
    /// Docker Engine version; `None` without Docker or from older helpers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docker_version: Option<String>,
}

impl HelperResponse {
    #[must_use]
    pub fn failed(error: impl Into<String>) -> Self {
        Self { ok: false, state: None, error: Some(error.into()), output: None, details: None, data: None }
    }
}

/// Largest request line the helper accepts. It must hold a base64 upload
/// chunk or editor save (256 KiB of data), and a compose file (512 KiB of
/// YAML, as JSON) with its `.env`.
pub const MAX_REQUEST_BYTES: usize = 4 * 1024 * 1024;

const _: () = assert!(
    MAX_REQUEST_BYTES
        > 2 * (forge_catalog::stacks::MAX_COMPOSE_BYTES + forge_catalog::stacks::MAX_ENV_BYTES)
            + forge_core::files::MAX_CHUNK_BYTES.div_ceil(3) * 4
);

impl HelperRequest {
    /// Parses and validates a request line.
    pub fn parse(line: &str) -> Result<Self, String> {
        if line.len() > MAX_REQUEST_BYTES {
            return Err("request too large".into());
        }

        let req: Self = serde_json::from_str(line).map_err(|e| format!("rejected: {e}"))?;

        req.validate()
    }

    /// Checks a request's arguments (also for requests derived from a signed
    /// command).
    pub fn validate(self) -> Result<Self, String> {
        let req = self;
        match &req {
            Self::RestartService { name } | Self::ServiceStatus { name } if !valid_service_name(name) => {
                Err(format!("rejected: invalid service name `{name}`"))
            }
            Self::Container { container } => {
                container.validate().map_err(|e| format!("rejected: {e}"))?;
                Ok(req)
            }
            Self::Site { action, spec } => {
                if !matches!(
                    action.as_str(),
                    "apply" | "remove" | "status" | "renew" | "clear_cache" | "access_log" | "error_log"
                ) {
                    return Err(format!("rejected: unknown site action `{action}`"));
                }

                spec.validate().map_err(|e| format!("rejected: {e}"))?;
                Ok(req)
            }
            Self::Dns { dns } => {
                dns.validate().map_err(|e| format!("rejected: {e}"))?;
                Ok(req)
            }
            Self::Ingress { ingress } => {
                ingress.validate().map_err(|e| format!("rejected: {e}"))?;
                Ok(req)
            }
            Self::Fabric { fabric } => {
                fabric.validate().map_err(|e| format!("rejected: {e}"))?;
                Ok(req)
            }
            Self::Files { files } => {
                files.validate().map_err(|e| format!("rejected: {e}"))?;
                Ok(req)
            }
            Self::Stack { stack } => {
                stack.validate().map_err(|e| format!("rejected: {e}"))?;
                Ok(req)
            }
            Self::DockerTls { docker_tls } => {
                docker_tls.validate().map_err(|e| format!("rejected: {e}"))?;
                Ok(req)
            }
            Self::Swarm { swarm } => {
                swarm.validate().map_err(|e| format!("rejected: {e}"))?;
                Ok(req)
            }
            Self::Docker { docker } => {
                docker.validate().map_err(|e| format!("rejected: {e}"))?;
                Ok(req)
            }
            Self::Process { process } => {
                process.validate().map_err(|e| format!("rejected: {e}"))?;
                Ok(req)
            }
            Self::Names { suffix, entries, records, .. } => {
                crate::host_names::check_list(suffix, entries).map_err(|e| format!("rejected: {e}"))?;

                if let Some(r) = records {
                    crate::host_names::check_records(suffix, r).map_err(|e| format!("rejected: {e}"))?;
                }

                Ok(req)
            }
            Self::HealthPolicy { health, .. } => {
                if !forge_catalog::apps::valid_slug(&health.environment, 40)
                    || !forge_catalog::apps::valid_slug(&health.name, 40)
                {
                    return Err("rejected: invalid app name".into());
                }

                if let Some(e) = health.policy.validate().first() {
                    return Err(format!("rejected: invalid health policy: {}: {}", e.field, e.message));
                }

                Ok(req)
            }
            Self::ContainerExec { exec } => {
                exec.validate().map_err(|e| format!("rejected: {e}"))?;
                Ok(req)
            }
            Self::HealthCheck { check } => {
                check.validate().map_err(|e| format!("rejected: {e}"))?;
                Ok(req)
            }
            Self::ContainerTelemetry { telemetry } => {
                telemetry.validate().map_err(|e| format!("rejected: {e}"))?;
                Ok(req)
            }
            Self::ContainerState { name } if !valid_app_container(name) => {
                Err(format!("rejected: `{name}` is not a Forge app container"))
            }
            Self::TerminalAccount { name, .. } if !crate::terminal_accounts::valid_account_name(name) => {
                Err(format!("rejected: `{name}` is not an allowed terminal account name"))
            }
            Self::App { action, spec, registry_auth, .. } => {
                if !forge_catalog::apps::APP_ACTIONS.contains(&action.as_str())
                    && !forge_catalog::apps::ROLLOUT_ACTIONS.contains(&action.as_str())
                {
                    return Err(format!("rejected: unknown app action `{action}`"));
                }

                if registry_auth.as_ref().is_some_and(|a| !a.valid()) {
                    return Err("rejected: invalid registry credential".into());
                }

                spec.validate().map_err(|e| format!("rejected: {e}"))?;
                Ok(req)
            }
            Self::Package { package, version, action, params } => {
                forge_catalog::validate(package, version, action, params).map_err(|e| format!("rejected: {e}"))?;
                Ok(req)
            }
            _ => Ok(req),
        }
    }

    /// The privileged request a command needs, validated; `None` for
    /// commands the agent runs itself (ping, facts, probes, processes). The
    /// single translation used by the agent and, for signed commands, by the
    /// helper.
    pub fn from_command(cmd: &forge_proto::Command) -> Result<Option<Self>, String> {
        use forge_proto::Command as C;

        let req = match cmd {
            C::PackageAction(p) => Self::Package {
                package: p.package.clone(),
                version: p.version.clone(),
                action: p.action.clone(),
                params: p.params.clone(),
            },
            // The health policy travels as an app command (D-105); its
            // spec is an `ApplySpec`, not an app.
            C::AppAction(a) if a.action == forge_catalog::health::APPLY_ACTION => Self::HealthPolicy {
                health: Box::new(serde_json::from_str(&a.spec_json).map_err(|e| format!("invalid health spec: {e}"))?),
                // `Trust::admit` sets it from the signed body.
                issued_at_ms: 0,
            },
            C::AppAction(a) => Self::App {
                action: a.action.clone(),
                spec: Box::new(serde_json::from_str(&a.spec_json).map_err(|e| format!("invalid app spec: {e}"))?),
                tail: a.tail,
                cursor: a.cursor.clone(),
                registry_auth: Some(&a.registry_auth)
                    .filter(|r| !r.is_empty())
                    .map(|r| forge_catalog::swarm::RegistryAuth(r.clone())),
            },
            C::SiteAction(s) => Self::Site {
                action: s.action.clone(),
                spec: Box::new(serde_json::from_str(&s.spec_json).map_err(|e| format!("invalid site: {e}"))?),
            },
            C::DnsAction(d) => {
                Self::Dns { dns: serde_json::from_str(&d.op_json).map_err(|e| format!("invalid DNS operation: {e}"))? }
            }
            C::IngressAction(i) => Self::Ingress {
                ingress: serde_json::from_str(&i.op_json).map_err(|e| format!("invalid ingress operation: {e}"))?,
            },
            C::AgentUpdate(u) => {
                let update = crate::self_update::UpdateSpec {
                    version: u.version.clone(),
                    download_base: u.download_base.clone(),
                    arch: u.arch.clone(),
                    agent_sha256: u.agent_sha256.clone(),
                    helper_sha256: u.helper_sha256.clone(),
                    allow_downgrade: u.allow_downgrade,
                };
                crate::self_update::validate(&update)?;
                Self::UpdateAgent { update }
            }
            C::FileAction(f) => Self::Files {
                files: serde_json::from_str(&f.op_json).map_err(|e| format!("invalid file operation: {e}"))?,
            },
            C::StackAction(s) => Self::Stack {
                stack: serde_json::from_str(&s.op_json).map_err(|e| format!("invalid stack operation: {e}"))?,
            },
            C::DockerTlsAction(d) => Self::DockerTls {
                docker_tls: serde_json::from_str(&d.op_json)
                    .map_err(|e| format!("invalid Docker TLS operation: {e}"))?,
            },
            C::DockerAction(d) => Self::Docker {
                docker: serde_json::from_str(&d.op_json).map_err(|e| format!("invalid Docker operation: {e}"))?,
            },
            C::SwarmAction(s) => Self::Swarm {
                swarm: serde_json::from_str(&s.op_json).map_err(|e| format!("invalid swarm operation: {e}"))?,
            },
            C::TerminalAccount(t) => Self::TerminalAccount { name: t.name.clone(), sudo: t.sudo },
            C::ProcessAction(p) => Self::Process { process: crate::processes::ProcessOp::from_wire(p)? },
            C::HostNames(h) => Self::Names {
                suffix: h.suffix.clone(),
                entries: serde_json::from_str(&h.entries_json).map_err(|e| format!("invalid name list: {e}"))?,
                containers: h.containers,
                records: if h.records_json.is_empty() {
                    None
                } else {
                    Some(serde_json::from_str(&h.records_json).map_err(|e| format!("invalid records: {e}"))?)
                },
            },
            C::FabricAction(f) => Self::Fabric {
                fabric: serde_json::from_str(&f.op_json).map_err(|e| format!("invalid fabric operation: {e}"))?,
            },
            C::ContainerAction(c) => Self::Container {
                container: ContainerOp::from_wire(&c.op, &c.id, &c.image, c.tail, &c.since, c.reveal_env)?,
            },
            C::ContainerExecOpen(x) => Self::ContainerExec { exec: crate::container_exec::ExecSpec::from_wire(x) },
            C::Ping(_)
            | C::CollectFacts(_)
            | C::ServiceStatus(_)
            | C::NetProbe(_)
            | C::CancelAction(_)
            | C::TunnelOpen(_)
            | C::ProcessList(_) => return Ok(None),
        };

        req.validate().map(Some)
    }

    /// Whether the helper runs this request without a signed command once it
    /// has pinned Forge's key (D-068): the agent's own confirmation after a
    /// self-update, which no command carries, and two read-only lookups the
    /// agent needs on its own: login accounts, and a Forge app container's
    /// run state for health checks (`forge-app-*` only, four fields). And
    /// one request that runs code: a command check named by reference
    /// (D-111), which the helper finds in the policy Forge signed and runs
    /// as the app's own account or in its container.
    #[must_use]
    pub fn allowed_unsigned(&self) -> bool {
        matches!(
            self,
            Self::ConfirmUpdate {}
                | Self::ServerPins { .. }
                | Self::LoginAccounts {}
                | Self::Capabilities {}
                | Self::ContainerState { .. }
                | Self::HealthCheck { .. }
                | Self::ContainerTelemetry { .. }
                | Self::StackStates {}
                | Self::AppStates {}
        )
    }
}

/// A Forge app container name: `forge-app-` followed by `<ns>-<app>` slug
/// characters (`[a-z0-9-]`), at most 100 bytes.
#[must_use]
pub fn valid_app_container(name: &str) -> bool {
    name.len() <= 100
        && name.strip_prefix("forge-app-").is_some_and(|rest| {
            !rest.is_empty()
                && !rest.starts_with('-')
                && rest.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
}

/// Sends a command's privileged request as the signed command itself, so a
/// helper that pinned Forge's key verifies it and derives the request on its
/// own. Only a helper older than this protocol, which cannot know signed
/// requests and so has no pin either, gets the plain request instead.
pub async fn call_signed(
    req: &HelperRequest,
    signed: &forge_proto::messages::SignedCommand,
    timeout: std::time::Duration,
) -> HelperResponse {
    if cfg!(not(unix)) {
        // Windows: requests run in the agent's own process (LocalSystem).
        return call(req, timeout).await;
    }

    let envelope =
        HelperRequest::Signed { command: forge_secrets::crypto::b64url(&prost::Message::encode_to_vec(signed)) };
    let r = call(&envelope, timeout).await;
    if !r.ok && r.error.as_deref().is_some_and(is_old_helper) {
        return call(req, timeout).await;
    }

    r
}

/// The answer of a helper that predates signed requests.
fn is_old_helper(error: &str) -> bool {
    error.contains("unknown variant `signed`")
}

/// Sends one request to the helper service and waits for its answer.
#[cfg(unix)]
pub async fn call(req: &HelperRequest, timeout: std::time::Duration) -> HelperResponse {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let run = async {
        let mut stream = tokio::net::UnixStream::connect(SOCKET)
            .await
            .map_err(|e| format!("forge-agent-helper is not running ({SOCKET}: {e}); re-run the node installer"))?;
        let mut line = serde_json::to_string(req).map_err(|e| e.to_string())?;
        line.push('\n');
        stream.write_all(line.as_bytes()).await.map_err(|e| e.to_string())?;
        let mut reader = BufReader::new(stream);

        // Zero or more `{"progress": "<line>"}` lines, then the response.
        loop {
            let mut resp = String::new();
            if reader.read_line(&mut resp).await.map_err(|e| e.to_string())? == 0 {
                return Err("helper closed the connection".into());
            }

            if let Ok(p) = serde_json::from_str::<Progress>(&resp) {
                crate::progress::push(&p.progress);
                continue;
            }

            return serde_json::from_str::<HelperResponse>(&resp).map_err(|e| format!("bad helper response: {e}"));
        }
    };

    match tokio::time::timeout(timeout, run).await {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => HelperResponse::failed(e),
        Err(_) => HelperResponse::failed("helper timed out"),
    }
}

/// A line of live output sent by the helper before its response.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Progress {
    pub progress: String,
}

/// Windows: the agent service runs as LocalSystem, so package requests run
/// in-process (validated against the catalog again first). Other requests
/// have no Windows implementation yet.
#[cfg(not(unix))]
pub async fn call(req: &HelperRequest, timeout: std::time::Duration) -> HelperResponse {
    match req {
        HelperRequest::Package { package, version, action, params } => {
            match forge_catalog::validate(package, version, action, params) {
                Ok(v) => {
                    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
                    let forward = tokio::spawn(async move {
                        while let Some(l) = rx.recv().await {
                            crate::progress::push(&l);
                        }
                    });
                    let r = tokio::time::timeout(timeout, crate::recipes::run(&v, Some(&tx))).await;
                    drop(tx);
                    let _ = forward.await;
                    r.unwrap_or_else(|_| HelperResponse::failed("package action timed out"))
                }
                Err(e) => HelperResponse::failed(format!("rejected: {e}")),
            }
        }
        HelperRequest::Files { files } => {
            match tokio::time::timeout(timeout, crate::files::run_async(files.clone())).await {
                Ok(Ok(d)) => {
                    HelperResponse { ok: true, state: None, error: None, output: None, details: None, data: Some(d) }
                }
                Ok(Err(e)) => HelperResponse::failed(e),
                Err(_) => HelperResponse::failed("file operation timed out"),
            }
        }
        HelperRequest::Process { process } => {
            let op = process.clone();
            let run = tokio::task::spawn_blocking(move || crate::processes::run(&op));
            match tokio::time::timeout(timeout, run).await {
                Ok(Ok(Ok(d))) => {
                    HelperResponse { ok: true, state: None, error: None, output: None, details: None, data: Some(d) }
                }
                Ok(Ok(Err(e))) => HelperResponse::failed(e),
                Ok(Err(e)) => HelperResponse::failed(e.to_string()),
                Err(_) => HelperResponse::failed("process action timed out"),
            }
        }
        // DNS records feed forge-dns, which runs on Linux nodes only, so
        // Windows writes the hosts file and ignores them.
        HelperRequest::Names { suffix, entries, containers, .. } => {
            match tokio::time::timeout(timeout, crate::host_names::apply(suffix.clone(), entries.clone(), *containers))
                .await
            {
                Ok(Ok(d)) => {
                    HelperResponse { ok: true, state: None, error: None, output: None, details: None, data: Some(d) }
                }
                Ok(Err(e)) => HelperResponse::failed(e),
                Err(_) => HelperResponse::failed("writing the hosts file timed out"),
            }
        }
        _ => HelperResponse::failed("this action is not supported on Windows yet"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ContainerState exception to D-068: unsigned, but only for Forge
    /// app containers.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn container_state_is_unsigned_only_for_forge_app_containers() {
        for ok in ["forge-app-uat-web", "forge-app-a-1"] {
            assert!(valid_app_container(ok), "{ok}");
            let req = HelperRequest::parse(&format!(r#"{{"op":"container_state","name":"{ok}"}}"#)).unwrap();
            assert!(req.allowed_unsigned());
        }

        for bad in [
            "postgres",
            "forge-app-",
            "forge-app--x",
            "forge-app-Web",
            "forge-app-a/../b",
            "forge-app-a b",
            &format!("forge-app-{}", "a".repeat(100)),
        ] {
            assert!(!valid_app_container(bad), "{bad}");
            assert!(HelperRequest::parse(&format!(r#"{{"op":"container_state","name":"{bad}"}}"#)).is_err(), "{bad}");
        }

        // Other container operations still need a signed command.
        let inspect = HelperRequest::Container {
            container: ContainerOp::Inspect { id: "forge-app-uat-web".into(), reveal_env: false },
        };
        assert!(!inspect.allowed_unsigned());
    }
    #[test]
    #[allow(clippy::unwrap_used)] // reason: tests
    fn container_telemetry_is_a_closed_unsigned_request() {
        let req = HelperRequest::parse(
            r#"{"op":"container_telemetry","telemetry":{"enabled":true,"exclude":["db"],"ack":4}}"#,
        )
        .unwrap();
        assert!(req.allowed_unsigned());
        assert!(
            HelperRequest::parse(r#"{"op":"container_telemetry","telemetry":{"enabled":true,"cmd":"x"}}"#).is_err()
        );
        let many: Vec<String> = (0..65).map(|i| format!("\"c{i}\"")).collect();
        let line =
            format!(r#"{{"op":"container_telemetry","telemetry":{{"enabled":true,"exclude":[{}]}}}}"#, many.join(","));
        assert!(HelperRequest::parse(&line).is_err(), "bounded");
    }

    #[test]
    fn a_full_upload_chunk_fits_in_one_request() {
        let data = "A".repeat(forge_core::files::MAX_CHUNK_BYTES.div_ceil(3) * 4);
        let line = format!(
            r#"{{"op":"files","files":{{"op":"upload_chunk","root":{{"kind":"node"}},"path":"/tmp/x","upload":"0123456789abcdef0123456789abcdef","offset":0,"data_b64":"{data}"}}}}"#
        );
        assert!(line.len() > 256 * 1024);
        assert!(HelperRequest::parse(&line).is_ok(), "{:?}", HelperRequest::parse(&line).err());
        let huge = format!(r#"{{"op":"restart_service","name":"{}"}}"#, "a".repeat(MAX_REQUEST_BYTES));
        assert_eq!(HelperRequest::parse(&huge).err().as_deref(), Some("request too large"));
    }

    #[test]
    fn accepts_only_allowlisted_operations() {
        assert!(HelperRequest::parse(r#"{"op":"restart_service","name":"nginx"}"#).is_ok());
        assert!(HelperRequest::parse(r#"{"op":"exec","cmd":"rm -rf /"}"#).is_err());
        assert!(HelperRequest::parse(r#"{"op":"restart_service","name":"x; reboot"}"#).is_err());
        assert!(HelperRequest::parse(r#"{"op":"restart_service","name":"nginx","extra":1}"#).is_err());
        assert!(HelperRequest::parse("not json").is_err());
        assert!(
            HelperRequest::parse(r#"{"op":"package","package":"redis","version":"distro","action":"install"}"#).is_ok()
        );
        assert!(
            HelperRequest::parse(
                r#"{"op":"package","package":"redis","version":"distro","action":"install","params":{"port":"1;id"}}"#
            )
            .is_err()
        );
        assert!(HelperRequest::parse(r#"{"op":"package","package":"evil","version":"1","action":"install"}"#).is_err());
        assert!(
            HelperRequest::parse(r#"{"op":"container","container":{"kind":"logs","id":"web","tail":100}}"#).is_ok()
        );
        assert!(HelperRequest::parse(r#"{"op":"container","container":{"kind":"stop","id":"../../x"}}"#).is_err());
        assert!(HelperRequest::parse(r#"{"op":"container","container":{"kind":"exec","id":"web"}}"#).is_err());

        // D-114: a container shell is never a plain request line.
        assert!(
            HelperRequest::parse(r#"{"op":"container_exec","exec":{"container":"web","cols":80,"rows":24}}"#).is_err()
        );
        let shell = HelperRequest::from_command(&forge_proto::Command::ContainerExecOpen(
            forge_proto::messages::ContainerExecOpen {
                container: "web".into(),
                cols: 80,
                rows: 24,
                idle_secs: 60,
                lifetime_secs: 60,
                ..Default::default()
            },
        ));
        assert!(matches!(&shell, Ok(Some(HelperRequest::ContainerExec { exec })) if exec.container == "web"));
        assert!(shell.is_ok_and(|r| r.is_some_and(|r| !r.allowed_unsigned())));
        assert!(
            HelperRequest::from_command(&forge_proto::Command::ContainerExecOpen(
                forge_proto::messages::ContainerExecOpen { container: "web;id".into(), ..Default::default() }
            ))
            .is_err()
        );
        assert!(
            HelperRequest::parse(r#"{"op":"container","container":{"kind":"pull","image":"nginx:1.27 --x"}}"#).is_err()
        );
        assert!(ContainerOp::from_wire("pull", "", "ghcr.io/org/app:1.2", 0, "", false).is_ok());
        assert!(ContainerOp::from_wire("logs", "web", "", 100, "1731234567.123456789", false).is_ok());
        assert!(ContainerOp::from_wire("logs", "web", "", 100, "1731234567&until=1", false).is_err());
        assert_eq!(
            ContainerOp::from_wire("inspect", "web", "", 0, "", true),
            Ok(ContainerOp::Inspect { id: "web".into(), reveal_env: true })
        );

        // Other operations never carry values, whatever the flag says.
        assert_eq!(
            ContainerOp::from_wire("stats", "web", "", 0, "", true),
            Ok(ContainerOp::Stats { id: "web".into() })
        );
    }
}
