//! Adopting a running container as a `container` Service (D-180).
//!
//! Forge reads the container through the node's agent (one `inspect` with
//! environment values) into the service's stored spec: image, environment,
//! published ports, mounts, restart policy, user, host name and labels. The
//! service records the container's name (`services.adopted_from`) and gets
//! an instance on that node marked deployed, without recreating or
//! restarting anything. The node acts on that container until the first
//! deploy replaces it with the service's own container (see
//! `forge_catalog::AppSpec::adopted_container`).
//!
//! The API handler authorizes and audits; this type only reads and writes.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use forge_catalog::container_opts::{ContainerOptions, RestartPolicy};
use forge_core::model::{AgentStatus, Environment, InstanceDesired, Service, ServiceInstance};
use forge_core::ports::{ServerRepository, ServiceRepository, TenancyRepository};
use forge_core::tags::FactsRepository;
use forge_core::{Clock, DomainError, EnvironmentId, Principal, ServerId, ServiceId};
use forge_proto::Command;
use forge_proto::messages::ContainerAction;
use forge_secrets::Envelope;
use serde::Deserialize;
use serde_json::Value;

use crate::node_rpc::{NodeRpc, RpcError};
use crate::service_spec::{AppInput, EnvInput};

/// Audit action of an adoption.
pub const AUDIT_ADOPT: &str = "service.adopt";
/// Error code when the node's agent is too old to keep an adopted container.
pub const AGENT_UPDATE_REQUIRED: &str = "agent_update_required";
const INSPECT_WAIT: Duration = Duration::from_secs(20);

/// The agent's `inspect` answer with environment values.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Inspected {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub state: Option<Value>,
    #[serde(default)]
    pub ports: Option<BTreeMap<String, Option<Vec<PortBinding>>>>,
    #[serde(default)]
    pub mounts: Option<Vec<InspectedMount>>,
    #[serde(default)]
    pub networks: Option<Vec<String>>,
    #[serde(default)]
    pub restart_policy: Option<String>,
    #[serde(default)]
    pub labels: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub user: Option<String>,
    /// Present only when the agent was asked for values.
    #[serde(default)]
    pub env: Option<Vec<EnvPair>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct PortBinding {
    #[serde(rename = "HostIp", default)]
    pub host_ip: String,
    #[serde(rename = "HostPort", default)]
    pub host_port: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct InspectedMount {
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub destination: String,
    #[serde(default)]
    pub mode: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct EnvPair {
    pub key: String,
    #[serde(default)]
    pub value: String,
}

/// Why an adoption was refused.
#[derive(Debug, thiserror::Error)]
pub enum AdoptError {
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Conflict(String),
    #[error("this node's agent is too old to keep an adopted container; update the agent first")]
    AgentTooOld,
    #[error(transparent)]
    Domain(#[from] DomainError),
}

/// What to adopt.
#[derive(Debug, Clone)]
pub struct AdoptRequest {
    pub environment_id: EnvironmentId,
    pub server_id: ServerId,
    pub container: String,
    /// The service name; derived from the container name when absent.
    pub name: Option<String>,
    pub principal: Principal,
    /// Stored in the spec as the **Open console** link.
    pub console: Option<forge_catalog::console_link::ConsoleLink>,
}

/// The adoption service.
#[derive(Debug, Clone)]
pub struct AdoptService {
    pub rpc: NodeRpc,
    pub services: Arc<dyn ServiceRepository>,
    pub servers: Arc<dyn ServerRepository>,
    pub tenancy: Arc<dyn TenancyRepository>,
    pub facts: Arc<dyn FactsRepository>,
    pub envelope: Envelope,
    pub clock: Arc<dyn Clock>,
}

/// A service name from a container name: lowercase, `[a-z0-9-]`, starting
/// with a letter, at most 40 characters.
#[must_use]
pub fn name_for(container: &str) -> String {
    let mut s: String = container
        .trim_start_matches('/')
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .skip_while(|c| !c.is_ascii_lowercase())
        .collect();
    s = s.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-");
    s.truncate(40);

    while s.ends_with('-') {
        s.pop();
    }

    if s.is_empty() { "adopted".into() } else { s }
}

/// Whether an environment variable is stored sealed (a secret).
#[must_use]
pub fn secret_name(key: &str) -> bool {
    let k = key.to_ascii_uppercase();

    ["KEY", "SECRET", "PASSWORD", "PASSWD", "TOKEN", "CREDENTIAL"].iter().any(|m| k.contains(m))
}

/// A mount as a service volume: `name:dest` for a Docker named volume
/// (under `/var/lib/docker/volumes/<name>/_data`), `host:dest[:ro]` else.
#[must_use]
pub fn volume_of(m: &InspectedMount) -> String {
    let named = m
        .source
        .strip_prefix("/var/lib/docker/volumes/")
        .and_then(|r| r.strip_suffix("/_data"))
        .filter(|n| !n.contains('/'));
    let source = named.unwrap_or(&m.source);
    let ro = m.mode.split(',').any(|o| o == "ro");

    if ro { format!("{source}:{}:ro", m.destination) } else { format!("{source}:{}", m.destination) }
}

/// Published ports as `[ip:]host:container[/udp]`, sorted.
#[must_use]
pub fn ports_of(i: &Inspected) -> Vec<String> {
    let mut out = Vec::new();

    for (key, bindings) in i.ports.iter().flatten() {
        let (ctr, proto) = key.split_once('/').unwrap_or((key.as_str(), "tcp"));
        let suffix = if proto == "tcp" { String::new() } else { format!("/{proto}") };

        for b in bindings.iter().flatten().filter(|b| !b.host_port.is_empty()) {
            // Docker lists the IPv6 twin of every IPv4 binding.
            if b.host_ip == "::" {
                continue;
            }

            let ip =
                if b.host_ip.is_empty() || b.host_ip == "0.0.0.0" { String::new() } else { format!("{}:", b.host_ip) };
            out.push(format!("{ip}{}:{ctr}{suffix}", b.host_port));
        }
    }

    out.sort();
    out.dedup();

    out
}

/// The service fields of an inspected container. `Invalid` when it cannot
/// be represented (host network, no image).
pub fn input_from(i: &Inspected, environment: EnvironmentId, name: &str) -> Result<AppInput, String> {
    let image = i.image.clone().filter(|s| !s.is_empty()).ok_or("the container has no image")?;

    if i.networks.iter().flatten().any(|n| n == "host") {
        return Err("the container uses the host network, which a Forge service cannot represent".into());
    }

    let env = i
        .env
        .iter()
        .flatten()
        .filter(|e| e.key != "PATH")
        .map(|e| EnvInput { key: e.key.clone(), value: Some(e.value.clone()), secret: secret_name(&e.key) })
        .collect();
    let labels = i
        .labels
        .iter()
        .flatten()
        .filter(|(k, _)| {
            !["com.docker.", "org.opencontainers.", "forge.", "org.orbyn."].iter().any(|p| k.starts_with(p))
        })
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    let restart_policy = match i.restart_policy.as_deref() {
        Some("always") => Some(RestartPolicy::Always),
        Some("on-failure") => Some(RestartPolicy::OnFailure),
        Some("no" | "") => Some(RestartPolicy::No),
        _ => Some(RestartPolicy::UnlessStopped),
    };

    let networks = i
        .networks
        .iter()
        .flatten()
        .filter(|n| !matches!(n.as_str(), "bridge" | "none") && !n.starts_with("forge-forge"))
        .cloned()
        .collect();

    let input = serde_json::json!({ "runtime": "container" });
    let mut input: AppInput = serde_json::from_value(input).map_err(|e| e.to_string())?;
    input.environment_id = Some(environment);
    input.name = Some(name.to_owned());
    input.image = image;
    input.ports = ports_of(i);
    input.volumes = i.mounts.iter().flatten().map(volume_of).collect();
    input.env = env;
    input.labels = labels;
    input.user = i.user.clone().unwrap_or_default();
    input.options = ContainerOptions { restart_policy, networks, ..ContainerOptions::default() };

    Ok(input)
}

/// What was found on the node.
#[derive(Debug, Clone, Copy)]
struct Found<'a> {
    server: ServerId,
    name: &'a str,
    adopted_from: &'a str,
    /// Docker's state (`running`, `exited`, …).
    status: &'a str,
}

fn inspect_command(container: &str) -> Command {
    Command::ContainerAction(ContainerAction {
        op: "inspect".into(),
        id: container.into(),
        image: String::new(),
        tail: 0,
        since: String::new(),
        reveal_env: true,
    })
}

impl AdoptService {
    /// Reads `req.container` on its node and records the service and its
    /// instance. Multi-step write without a transaction: the service is
    /// written first; if the instance write fails the service stays without
    /// a placement (nothing changed on the node) and the operator deletes it.
    pub async fn adopt(&self, req: &AdoptRequest) -> Result<(Service, ServiceInstance), AdoptError> {
        let container = req.container.trim_start_matches('/');

        if !forge_catalog::apps::valid_container_name(container) {
            return Err(AdoptError::Invalid("container: a Docker container name or id".into()));
        }

        if container.starts_with("forge-app-") {
            return Err(AdoptError::Invalid("that container already runs a Forge service".into()));
        }

        let ns = self.tenancy.get_environment(req.environment_id).await?;
        let server = self.servers.get_server(req.server_id).await?;

        if server.workspace_id != ns.workspace_id {
            return Err(AdoptError::Invalid("the node and the environment belong to different workspaces".into()));
        }

        if server.status != AgentStatus::Online {
            return Err(AdoptError::Conflict("the node is not connected".into()));
        }

        let facts = self.facts.facts_of(server.id).await?;
        if facts.as_ref().and_then(|(f, _)| f.get(crate::jobs::app::ADOPTED_FACT)).and_then(Value::as_bool)
            != Some(true)
        {
            return Err(AdoptError::AgentTooOld);
        }

        let answer = self
            .rpc
            .call(server.id, &req.principal.to_string(), inspect_command(container), INSPECT_WAIT)
            .await
            .map_err(|e| match e {
                RpcError::Failed(m) => AdoptError::Conflict(format!("cannot read the container: {m}")),
                other => AdoptError::Conflict(other.to_string()),
            })?;
        let inspected: Inspected = serde_json::from_value(answer)
            .map_err(|e| AdoptError::Conflict(format!("the node's inspect answer is not readable: {e}")))?;

        if inspected.env.is_none() {
            return Err(AdoptError::AgentTooOld);
        }

        // Docker reports the name; an id adopts under the container's name.
        let adopted_from = if inspected.name.is_empty() { container.to_owned() } else { inspected.name.clone() };

        for other in self.services.list_server_services(server.id).await? {
            if other.adopted_from.as_deref() == Some(adopted_from.as_str()) {
                return Err(AdoptError::Conflict(format!("`{adopted_from}` is already adopted as `{}`", other.name)));
            }
        }

        let name = match &req.name {
            Some(n) => n.trim().to_lowercase(),
            None => name_for(&adopted_from),
        };

        if !forge_catalog::apps::valid_slug(&name, 40) {
            return Err(AdoptError::Invalid("name: lowercase letters, digits and dashes (max 40)".into()));
        }

        if self.services.list_services(Some(ns.id)).await?.iter().any(|a| a.name == name) {
            return Err(AdoptError::Conflict(format!("`{name}` already exists in this environment")));
        }

        let mut input = input_from(&inspected, ns.id, &name).map_err(AdoptError::Invalid)?;
        input.console = req.console.clone();

        let status = inspected.state.as_ref().and_then(|s| s.get("Status")).and_then(Value::as_str).unwrap_or("");
        let found = Found { server: server.id, name: &name, adopted_from: &adopted_from, status };
        let (service, instance) = self.record(&ns, req, input, &found)?;
        self.services.put_service(&service).await?;
        self.services.put_instance(&instance).await?;

        Ok((service, instance))
    }

    fn record(
        &self,
        ns: &Environment,
        req: &AdoptRequest,
        input: AppInput,
        found: &Found<'_>,
    ) -> Result<(Service, ServiceInstance), AdoptError> {
        let Found { server, name, adopted_from, status } = *found;
        let now = self.clock.now();
        let mut service = Service {
            id: ServiceId::new(),
            workspace_id: ns.workspace_id,
            environment_id: ns.id,
            name: name.to_owned(),
            runtime: String::new(),
            spec: Value::Null,
            env: Vec::new(),
            created_by: req.principal.clone(),
            created_at: now,
            updated_at: now,
            version: 0,
            source: String::new(),
            secrets_changed_at: None,
            adopted_from: Some(adopted_from.to_owned()),
        };
        crate::service_spec::apply_input(
            &self.envelope,
            now,
            &mut service,
            &crate::names::environment_slug(&ns.name),
            input,
        )
        .map_err(|e| match e {
            DomainError::Invalid(m) => AdoptError::Invalid(format!("the container cannot be adopted: {m}")),
            other => AdoptError::Domain(other),
        })?;

        let running = status == "running";
        let instance = ServiceInstance {
            service_id: service.id,
            server_id: server,
            role: "active".into(),
            status: "deployed".into(),
            state: Some(if running { "active".into() } else { status.to_owned() }),
            last_job_id: None,
            last_error: None,
            updated_at: now,
            desired: if running { InstanceDesired::Running } else { InstanceDesired::Stopped },
            deployed_release: None,
            exit_code: None,
        };

        Ok((service, instance))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // reason: test fixtures
mod tests {
    use super::*;

    fn rustfs() -> Inspected {
        serde_json::from_value(serde_json::json!({
            "id": "35dfaf1f3aba", "name": "forge-objstore-media", "image": "rustfs/rustfs:1.0.6",
            "state": { "Status": "running" }, "restart_policy": "unless-stopped",
            "ports": {
                "9000/tcp": [{ "HostIp": "0.0.0.0", "HostPort": "19000" }, { "HostIp": "::", "HostPort": "19000" }],
                "9001/tcp": [{ "HostIp": "127.0.0.1", "HostPort": "19001" }],
                "53/udp": [{ "HostIp": "", "HostPort": "5353" }],
                "7000/tcp": null
            },
            "mounts": [
                { "source": "/var/lib/docker/volumes/media-data/_data", "destination": "/data", "mode": "z" },
                { "source": "/srv/conf", "destination": "/conf", "mode": "ro" }
            ],
            "networks": ["bridge", "backend"],
            "labels": { "org.orbyn.forge.objstore": "media", "com.docker.compose.project": "x", "team": "infra" },
            "user": "10001",
            "env": [
                { "key": "PATH", "value": "/usr/bin" },
                { "key": "RUSTFS_ACCESS_KEY", "value": "admin" },
                { "key": "RUSTFS_SECRET_KEY", "value": "hunter2hunter2" },
                { "key": "RUSTFS_VOLUMES", "value": "/data" }
            ]
        }))
        .unwrap()
    }

    #[test]
    fn inspect_maps_onto_the_service_fields() {
        let input = input_from(&rustfs(), EnvironmentId::new(), "media").unwrap();
        assert_eq!(input.image, "rustfs/rustfs:1.0.6");
        assert_eq!(input.ports, ["127.0.0.1:19001:9001", "19000:9000", "5353:53/udp"]);
        assert_eq!(input.volumes, ["media-data:/data", "/srv/conf:/conf:ro"]);
        let env: Vec<(&str, bool)> = input.env.iter().map(|e| (e.key.as_str(), e.secret)).collect();
        assert_eq!(env, [("RUSTFS_ACCESS_KEY", true), ("RUSTFS_SECRET_KEY", true), ("RUSTFS_VOLUMES", false)]);
        assert_eq!(input.labels, [("team".to_owned(), "infra".to_owned())]);
        assert_eq!(input.user, "10001");
        assert_eq!(input.options.restart_policy, Some(RestartPolicy::UnlessStopped));
        assert_eq!(input.options.networks, ["backend"]);
    }

    #[test]
    fn host_network_and_missing_image_are_refused() {
        let mut host = rustfs();
        host.networks = Some(vec!["host".into()]);
        assert!(input_from(&host, EnvironmentId::new(), "x").is_err());
        let mut bare = rustfs();
        bare.image = None;
        assert!(input_from(&bare, EnvironmentId::new(), "x").is_err());
    }

    #[test]
    fn names_come_from_container_names() {
        assert_eq!(name_for("/My_RustFS"), "my-rustfs");
        assert_eq!(name_for("123"), "adopted");
        assert_eq!(name_for("9web__1"), "web-1");
        assert_eq!(name_for(&"a".repeat(60)).len(), 40);
        assert!(secret_name("db_password") && !secret_name("RUSTFS_VOLUMES"));
    }
}
