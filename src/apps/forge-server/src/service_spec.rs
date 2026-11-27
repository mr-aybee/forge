//! A service's editable fields and how they become its stored spec
//! (`services.spec`), shared by the services API and services files
//! (D-178), so both paths validate and store a service the same way.

use std::collections::HashSet;

use forge_catalog::compose_native::{Orchestration, SPEC_KEY};
use forge_core::model::{EnvVar, Service};
use forge_core::{DomainError, DomainResult, Timestamp};
use forge_secrets::Envelope;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::jobs::app::{build_spec, env_aad};

/// One environment variable as an editor or a file sends it.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct EnvInput {
    pub key: String,
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub secret: bool,
}

/// A service's editable fields (create and update).
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct AppInput {
    #[schemars(with = "Option<String>")]
    pub environment_id: Option<forge_core::EnvironmentId>,
    pub name: Option<String>,
    pub runtime: String,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub working_dir: String,
    /// Native service: the TCP port it listens on (default health check).
    #[serde(default)]
    pub listen_port: Option<u16>,
    #[serde(default)]
    pub image: String,
    #[serde(default)]
    pub ports: Vec<String>,
    #[serde(default)]
    pub volumes: Vec<String>,
    #[serde(default)]
    pub env_files: Vec<String>,
    #[serde(default = "yes")]
    pub restart: bool,
    #[serde(default)]
    pub env: Vec<EnvInput>,
    #[serde(default)]
    #[schemars(with = "Vec<serde_json::Value>")]
    pub dependencies: Vec<forge_catalog::apps::Dependency>,
    #[serde(default)]
    pub cpu_millis: u32,
    #[serde(default)]
    pub memory_mb: u32,
    #[serde(default)]
    pub entrypoint: String,
    #[serde(default)]
    pub user: String,
    #[serde(default)]
    pub hostname: String,
    #[serde(default)]
    pub labels: Vec<(String, String)>,
    #[serde(default)]
    pub extra_hosts: Vec<String>,
    #[serde(default)]
    #[schemars(with = "Option<serde_json::Value>")]
    pub healthcheck: Option<forge_catalog::apps::HealthCheck>,
    #[serde(default)]
    pub stop_timeout_secs: u32,
    #[serde(default)]
    pub read_only: bool,
    #[serde(default)]
    pub log_max_mb: u32,
    /// Container services in a Docker Swarm cluster: replicas, placement,
    /// restart and update behaviour (D-154).
    #[serde(default)]
    #[schemars(with = "Option<Value>")]
    pub swarm: Option<forge_catalog::swarm_service::SwarmSettings>,
    /// Container: join the node's cluster container network (default on).
    #[serde(default)]
    pub mesh: Option<bool>,
    /// What the service reconciler does when the node is lost: `move` or
    /// `alert` (D-177). Absent: move a container without local data,
    /// alert otherwise.
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub on_node_failure: Option<forge_core::model::NodeFailurePolicy>,
    /// Restart policy, capabilities, ulimits, tmpfs, DNS, sysctls, devices,
    /// init and extra networks.
    #[serde(flatten)]
    pub options: forge_catalog::container_opts::ContainerOptions,
    /// Where and how many copies run, as a services file says (D-178).
    /// Absent: keep the stored one.
    #[serde(default)]
    #[schemars(with = "Option<Value>")]
    pub orchestration: Option<Orchestration>,
    /// Update only: the `version` the editor loaded. A different stored
    /// version fails with 409 instead of overwriting someone else's change.
    #[serde(default)]
    pub version: Option<i64>,
}

/// The stored form of a service spec (`services.spec`): the runtime fields
/// of `forge_catalog::AppSpec` without name, runtime and environment
/// variables, plus Forge's own `health`, `mesh`, `on_node_failure` and
/// `orchestration`.
#[derive(Debug, Serialize)]
struct StoredSpec<'a> {
    environment: &'a str,
    command: &'a str,
    working_dir: &'a str,
    listen_port: Option<u16>,
    image: &'a str,
    ports: &'a [String],
    volumes: &'a [String],
    env_files: &'a [String],
    restart: bool,
    dependencies: &'a [forge_catalog::apps::Dependency],
    cpu_millis: u32,
    memory_mb: u32,
    entrypoint: &'a str,
    user: &'a str,
    hostname: &'a str,
    labels: &'a [(String, String)],
    extra_hosts: &'a [String],
    healthcheck: &'a Option<forge_catalog::apps::HealthCheck>,
    stop_timeout_secs: u32,
    read_only: bool,
    log_max_mb: u32,
    swarm: &'a Option<forge_catalog::swarm_service::SwarmSettings>,
    #[serde(flatten)]
    options: &'a forge_catalog::container_opts::ContainerOptions,
    #[serde(skip_serializing_if = "Option::is_none")]
    health: Option<Value>,
    /// Only the opt-out is stored: absent means on.
    #[serde(skip_serializing_if = "Option::is_none")]
    mesh: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    on_node_failure: Option<forge_core::model::NodeFailurePolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    orchestration: Option<Value>,
}

const fn yes() -> bool {
    true
}

/// Applies `input` to `app` (new or existing), sealing secrets, and
/// validates the result. `Invalid` names the problem.
pub fn apply_input(
    envelope: &Envelope,
    now: Timestamp,
    app: &mut Service,
    ns_slug: &str,
    input: AppInput,
) -> DomainResult<()> {
    if !matches!(input.runtime.as_str(), "systemd" | "container") {
        return Err(DomainError::Invalid("runtime must be systemd or container".into()));
    }

    if let Some(s) = &input.swarm {
        s.validate().map_err(DomainError::Invalid)?;
    }

    let mut seen = HashSet::new();
    let mut env = Vec::new();

    for e in input.env {
        let key = e.key.trim().to_owned();
        if !seen.insert(key.clone()) {
            return Err(DomainError::Invalid(format!("duplicate variable `{key}`")));
        }

        if e.secret {
            let sealed = match e.value.filter(|v| !v.is_empty()) {
                Some(v) => Some(
                    envelope
                        .seal(v.as_bytes(), &env_aad(app.id, &key))
                        .map_err(|_| DomainError::Invalid("cannot seal secret".into()))?,
                ),
                // Empty means "keep the stored value".
                None => app.env.iter().find(|x| x.key == key && x.secret).and_then(|x| x.sealed.clone()),
            };

            if sealed.is_none() {
                return Err(DomainError::Invalid(format!("secret `{key}` needs a value")));
            }

            env.push(EnvVar { key, value: None, secret: true, sealed });
        } else {
            env.push(EnvVar { key, value: Some(e.value.unwrap_or_default()), secret: false, sealed: None });
        }
    }

    app.runtime = input.runtime;
    app.env = env;

    // The health policy has its own endpoint (`PUT /services/{id}/health`)
    // and the orchestration comes from services files; an ordinary update
    // keeps both.
    let health = app.spec.get("health").cloned().filter(|v| !v.is_null());

    let orchestration = match &input.orchestration {
        Some(o) => Some(serde_json::to_value(o).map_err(DomainError::internal)?),
        None => app.spec.get(SPEC_KEY).cloned().filter(|v| !v.is_null()),
    };

    let mesh = (input.mesh == Some(false) && app.runtime == "container").then_some(false);
    let stored = StoredSpec {
        environment: ns_slug,
        command: input.command.trim(),
        working_dir: input.working_dir.trim(),
        listen_port: input.listen_port,
        image: input.image.trim(),
        ports: &input.ports,
        volumes: &input.volumes,
        env_files: &input.env_files,
        restart: input.restart,
        dependencies: &input.dependencies,
        cpu_millis: input.cpu_millis,
        memory_mb: input.memory_mb,
        entrypoint: input.entrypoint.trim(),
        user: input.user.trim(),
        hostname: input.hostname.trim(),
        labels: &input.labels,
        extra_hosts: &input.extra_hosts,
        healthcheck: &input.healthcheck,
        stop_timeout_secs: input.stop_timeout_secs,
        read_only: input.read_only,
        log_max_mb: input.log_max_mb,
        swarm: &input.swarm,
        options: &input.options,
        health,
        mesh,
        on_node_failure: input.on_node_failure,
        orchestration,
    };
    app.spec = serde_json::to_value(&stored).map_err(DomainError::internal)?;
    app.updated_at = now;
    build_spec(app, None).map_err(DomainError::Invalid)?;

    Ok(())
}
