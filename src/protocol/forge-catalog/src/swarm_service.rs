//! A Forge container service as a Docker Swarm service (D-154).
//!
//! In a Swarm cluster a container service is one Swarm service named
//! `<environment>-<service>`, scheduled by the swarm. [`SwarmSettings`] holds
//! what Swarm needs beyond the container itself (replicas, placement,
//! restart and update behaviour); [`service_spec`] turns an [`AppSpec`] and
//! its settings into the Engine API's `ServiceSpec`.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::apps::{AppSpec, Runtime};

/// Swarm-managed label marking services Forge owns.
pub const MANAGED_LABEL: &str = "forge.managed";
const MAX_REPLICAS: u32 = 1000;
const MAX_CONSTRAINTS: usize = 32;
const NANOS: u64 = 1_000_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceMode {
    /// `replicas` tasks, placed by the scheduler.
    #[default]
    Replicated,
    /// One task on every node that matches the constraints.
    Global,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RestartCondition {
    #[default]
    Any,
    OnFailure,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UpdateOrder {
    /// New task first, then the old one stops (no gap; needs room).
    #[default]
    StartFirst,
    /// Old task stops first (a short gap per task).
    StopFirst,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureAction {
    /// Roll back to the previous spec (Forge's default).
    #[default]
    Rollback,
    Pause,
    Continue,
}

impl FailureAction {
    const fn as_docker(self) -> &'static str {
        match self {
            Self::Rollback => "rollback",
            Self::Pause => "pause",
            Self::Continue => "continue",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublishMode {
    /// The routing mesh: every node answers on the port.
    #[default]
    Ingress,
    /// Only nodes running a task answer (one task per node per port).
    Host,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RestartPolicy {
    #[serde(default)]
    pub condition: RestartCondition,
    /// `0` = unlimited.
    #[serde(default)]
    pub max_attempts: u32,
    #[serde(default = "five")]
    pub delay_secs: u32,
}

impl Default for RestartPolicy {
    fn default() -> Self {
        Self { condition: RestartCondition::Any, max_attempts: 0, delay_secs: 5 }
    }
}

/// How Swarm rolls out a new spec (also used, reversed, for rollbacks).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdatePolicy {
    /// Tasks updated at once (`0` = all).
    #[serde(default = "one")]
    pub parallelism: u32,
    #[serde(default)]
    pub delay_secs: u32,
    #[serde(default)]
    pub order: UpdateOrder,
    #[serde(default)]
    pub failure_action: FailureAction,
    /// How long each task is watched for failure after it starts.
    #[serde(default = "ten")]
    pub monitor_secs: u32,
    /// Share of failed tasks tolerated (0.0–1.0).
    #[serde(default)]
    pub max_failure_ratio: f32,
}

impl Eq for UpdatePolicy {}

impl Default for UpdatePolicy {
    fn default() -> Self {
        Self {
            parallelism: 1,
            delay_secs: 0,
            order: UpdateOrder::StartFirst,
            failure_action: FailureAction::Rollback,
            monitor_secs: 10,
            max_failure_ratio: 0.0,
        }
    }
}

/// What Swarm needs beyond the container itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SwarmSettings {
    #[serde(default)]
    pub mode: ServiceMode,
    /// Tasks in replicated mode.
    #[serde(default = "one")]
    pub replicas: u32,
    /// Placement constraints, e.g. `node.role==worker`, `node.labels.zone==a`.
    #[serde(default)]
    pub constraints: Vec<String>,
    /// Spread tasks evenly over the values of these node labels, e.g.
    /// `node.labels.zone`.
    #[serde(default)]
    pub spread: Vec<String>,
    /// Tasks per node at most (`0` = no limit).
    #[serde(default)]
    pub max_per_node: u32,
    #[serde(default)]
    pub restart: RestartPolicy,
    #[serde(default)]
    pub update: UpdatePolicy,
    #[serde(default)]
    pub publish: PublishMode,
}

impl Default for SwarmSettings {
    fn default() -> Self {
        Self {
            mode: ServiceMode::Replicated,
            replicas: 1,
            constraints: Vec::new(),
            spread: Vec::new(),
            max_per_node: 0,
            restart: RestartPolicy::default(),
            update: UpdatePolicy::default(),
            publish: PublishMode::Ingress,
        }
    }
}

const fn one() -> u32 {
    1
}
const fn five() -> u32 {
    5
}
const fn ten() -> u32 {
    10
}

/// `node.<field>`, `engine.labels.<x>` or `node.labels.<x>`, `==` or `!=`, a value.
fn constraint_ok(c: &str) -> bool {
    let Some((key, value)) = c.split_once("==").or_else(|| c.split_once("!=")) else { return false };
    let key = key.trim();
    let value = value.trim();
    let key_ok = ["node.id", "node.hostname", "node.role", "node.platform.os", "node.platform.arch"].contains(&key)
        || ["node.labels.", "engine.labels."].iter().any(|p| key.strip_prefix(p).is_some_and(label_name_ok));
    key_ok && !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

fn label_name_ok(k: &str) -> bool {
    (1..=128).contains(&k.len()) && k.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
}

impl SwarmSettings {
    pub fn validate(&self) -> Result<(), String> {
        if self.mode == ServiceMode::Replicated && self.replicas > MAX_REPLICAS {
            return Err(format!("replicas: at most {MAX_REPLICAS}"));
        }
        if self.constraints.len() > MAX_CONSTRAINTS || !self.constraints.iter().all(|c| constraint_ok(c)) {
            return Err("constraints look like `node.role==worker` or `node.labels.zone==a`".into());
        }
        if self.spread.len() > 4
            || !self.spread.iter().all(|s| s.strip_prefix("node.labels.").is_some_and(label_name_ok))
        {
            return Err("spread uses node labels, e.g. `node.labels.zone` (at most 4)".into());
        }
        if !(0.0..=1.0).contains(&self.update.max_failure_ratio) {
            return Err("max failure ratio is between 0 and 1".into());
        }
        Ok(())
    }
}

/// A Forge id as Swarm object names carry it: the UUID's 32 hex digits.
fn compact(id: &str) -> Option<String> {
    let hex: String = id.chars().filter(|c| *c != '-').collect();
    (id.len() == 36 && hex.len() == 32 && hex.bytes().all(|b| b.is_ascii_hexdigit())).then(|| hex.to_ascii_lowercase())
}

/// Whether `id` is a Forge id (a canonical UUID).
#[must_use]
pub fn id_ok(id: &str) -> bool {
    compact(id).is_some()
}

/// The Swarm service of Forge service `service_id` in `slot`: `forge-<id>`
/// (unslotted) or `forge-<id>-a` / `-b`. Made from the id, so two
/// environments can never name the same service.
#[must_use]
pub fn service_name(service_id: &str, slot: &str) -> String {
    let base = format!("forge-{}", compact(service_id).unwrap_or_default());
    if slot.is_empty() { base } else { format!("{base}-{slot}") }
}

/// A slot Forge uses: `""` (unslotted), `a` or `b`.
#[must_use]
pub fn slot_ok(slot: &str) -> bool {
    matches!(slot, "" | "a" | "b")
}

/// Label carrying the slot on a Swarm service.
pub const SLOT_LABEL: &str = "forge.slot";

/// Label (on the service and on its containers) with the fingerprint of the
/// definition: an identical re-send is skipped, and tasks of the current
/// definition can be told from older ones.
pub const SPEC_LABEL: &str = "forge.spec";

/// FNV-1a (64 bit): a stable fingerprint, not a security measure.
fn fingerprint(v: &Value) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in v.to_string().bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// The overlay network of environment `environment_id`.
#[must_use]
pub fn network_name(environment_id: &str) -> String {
    format!("forge-net-{}", compact(environment_id).unwrap_or_default())
}

/// Label carrying the Forge service id on a Swarm service: every operation
/// checks it before it touches the service.
pub const SERVICE_ID_LABEL: &str = "forge.service.id";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    Tcp,
    Udp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortDef {
    pub protocol: Protocol,
    pub target: u16,
    #[serde(default)]
    pub published: Option<u16>,
}

/// A mount: a named Docker volume, or a host folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum MountDef {
    Volume { name: String, target: String, read_only: bool },
    Bind { source: String, target: String, read_only: bool },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HealthDef {
    pub command: String,
    pub interval_secs: u32,
    pub timeout_secs: u32,
    pub retries: u32,
    pub start_period_secs: u32,
}

/// Everything a node needs to create or update one Swarm service. A closed
/// set: the node builds Docker's `ServiceSpec` from it, so a command can ask
/// for nothing else (no privileges, capabilities, plugins, runtimes, other
/// networks or volume drivers).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceDef {
    pub service_id: String,
    /// `""`, `a` or `b` (Blue-Green and Canary run two slots).
    #[serde(default)]
    pub slot: String,
    pub environment_id: String,
    /// Display names, for labels only.
    pub environment: String,
    pub service: String,
    pub image: String,
    /// Resolved environment (secrets included): never logged.
    pub env: Vec<(String, String)>,
    pub command: Vec<String>,
    pub args: Vec<String>,
    pub user: String,
    pub hostname: String,
    pub mounts: Vec<MountDef>,
    pub read_only: bool,
    pub stop_grace_secs: u32,
    pub healthcheck: Option<HealthDef>,
    pub cpu_millis: u32,
    pub memory_mb: u32,
    pub log_max_mb: u32,
    /// `name:ip` host entries.
    pub extra_hosts: Vec<String>,
    /// User labels (never `forge.*`).
    pub labels: Vec<(String, String)>,
    pub ports: Vec<PortDef>,
    pub settings: SwarmSettings,
}

impl std::fmt::Debug for ServiceDef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServiceDef")
            .field("service_id", &self.service_id)
            .field("environment_id", &self.environment_id)
            .field("image", &self.image)
            .field("env_keys", &self.env.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>())
            .field("mounts", &self.mounts)
            .field("ports", &self.ports)
            .field("settings", &self.settings)
            .finish_non_exhaustive()
    }
}

fn port(p: &str) -> Result<PortDef, String> {
    let (body, proto) = match p.rsplit_once('/') {
        Some((b, "tcp")) => (b, Protocol::Tcp),
        Some((b, "udp")) => (b, Protocol::Udp),
        Some(_) => return Err(format!("port `{p}`: protocol is tcp or udp")),
        None => (p, Protocol::Tcp),
    };
    let parts: Vec<&str> = body.split(':').collect();
    let (published, target) = match parts.as_slice() {
        [t] => (None, *t),
        [h, t] | [_, h, t] => (Some(*h), *t),
        _ => return Err(format!("port `{p}` is not `[ip:]host:container`")),
    };
    let target = target.parse().map_err(|_| format!("port `{p}`: bad container port"))?;
    let published = published.map(|h| h.parse().map_err(|_| format!("port `{p}`: bad host port"))).transpose()?;
    Ok(PortDef { protocol: proto, target, published })
}

fn mount(v: &str) -> Result<MountDef, String> {
    let parts: Vec<&str> = v.split(':').collect();
    let (source, target, read_only) = match parts.as_slice() {
        [s, t] | [s, t, "rw"] => (*s, *t, false),
        [s, t, "ro"] => (*s, *t, true),
        _ => return Err(format!("volume `{v}` is not `source:target[:ro]`")),
    };
    let (source, target) = (source.to_owned(), target.to_owned());
    Ok(if source.starts_with('/') {
        MountDef::Bind { source, target, read_only }
    } else {
        MountDef::Volume { name: source, target, read_only }
    })
}

fn volume_name_ok(n: &str) -> bool {
    (1..=64).contains(&n.len())
        && n.bytes().next().is_some_and(|b| b.is_ascii_alphanumeric())
        && n.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}

fn container_path_ok(t: &str) -> bool {
    crate::stacks::normalize_abs(t).is_some_and(|n| n != "/") && t.len() <= 1024 && !t.chars().any(char::is_control)
}

/// Builds the definition of `app` (a container service) for the swarm.
pub fn service_def(
    app: &AppSpec,
    settings: &SwarmSettings,
    service_id: &str,
    environment_id: &str,
) -> Result<ServiceDef, String> {
    if app.runtime != Runtime::Container {
        return Err("only container services run on Docker Swarm".into());
    }
    let (command, args) = match (app.entrypoint.trim(), app.command.trim()) {
        ("", "") => (Vec::new(), Vec::new()),
        ("", cmd) => (vec!["/bin/sh".to_owned(), "-c".to_owned(), cmd.to_owned()], Vec::new()),
        (ep, "") => (ep.split_whitespace().map(str::to_owned).collect(), Vec::new()),
        (ep, cmd) => (
            ep.split_whitespace().map(str::to_owned).collect(),
            vec!["/bin/sh".to_owned(), "-c".to_owned(), cmd.to_owned()],
        ),
    };
    let def = ServiceDef {
        service_id: service_id.to_owned(),
        slot: String::new(),
        environment_id: environment_id.to_owned(),
        environment: app.environment.clone(),
        service: app.name.clone(),
        image: app.image.trim().to_owned(),
        env: app.env.clone(),
        command,
        args,
        user: app.user.trim().to_owned(),
        hostname: app.hostname.trim().to_owned(),
        mounts: app.volumes.iter().map(|v| mount(v)).collect::<Result<_, _>>()?,
        read_only: app.read_only,
        stop_grace_secs: app.stop_timeout_secs,
        healthcheck: app.healthcheck.as_ref().map(|h| HealthDef {
            command: h.command.clone(),
            interval_secs: h.interval_secs,
            timeout_secs: h.timeout_secs,
            retries: h.retries,
            start_period_secs: h.start_period_secs,
        }),
        cpu_millis: app.cpu_millis,
        memory_mb: app.memory_mb,
        log_max_mb: app.log_max_mb,
        extra_hosts: app.extra_hosts.clone(),
        labels: app.labels.clone(),
        ports: app.ports.iter().map(|p| port(p)).collect::<Result<_, _>>()?,
        settings: settings.clone(),
    };
    def.validate()?;
    Ok(def)
}

impl ServiceDef {
    /// The definition for `slot`, publishing each container port on the
    /// slot's own port from `ports` (container port → published port), so
    /// two slots run side by side. The edge sends traffic to those ports.
    pub fn into_slot(mut self, slot: &str, ports: &std::collections::BTreeMap<u16, u16>) -> Result<Self, String> {
        if !slot_ok(slot) || slot.is_empty() {
            return Err("a slot is `a` or `b`".into());
        }
        for p in &mut self.ports {
            p.published =
                Some(*ports.get(&p.target).ok_or_else(|| format!("no slot port for container port {}", p.target))?);
        }
        self.slot = slot.to_owned();
        self.validate()?;
        Ok(self)
    }

    /// Host folders this service binds (normalized).
    #[must_use]
    pub fn bind_sources(&self) -> Vec<String> {
        self.mounts
            .iter()
            .filter_map(|m| match m {
                MountDef::Bind { source, .. } => crate::stacks::normalize_abs(source),
                MountDef::Volume { .. } => None,
            })
            .collect()
    }

    /// Checks everything before a node runs it. Runs on the control plane
    /// and again in the node's root helper.
    pub fn validate(&self) -> Result<(), String> {
        if !id_ok(&self.service_id) || !id_ok(&self.environment_id) || !slot_ok(&self.slot) {
            return Err("invalid service id, environment id or slot".into());
        }
        if self.image.is_empty() || !crate::apps::valid_image_ref(&self.image) {
            return Err("a container service needs a valid image reference".into());
        }
        if self.env.len() > 500 || !self.env.iter().all(|(k, v)| crate::apps::valid_env_key(k) && !v.contains('\0')) {
            return Err("invalid environment variable".into());
        }
        for word in self.command.iter().chain(&self.args) {
            if word.len() > 16 * 1024 || word.contains('\0') {
                return Err("command words must be text without NUL, up to 16 KiB".into());
            }
        }
        for (what, v) in [("user", &self.user), ("hostname", &self.hostname)] {
            if v.len() > 253 || v.chars().any(|c| c.is_control() || c.is_whitespace()) {
                return Err(format!("invalid {what}"));
            }
        }
        if self.mounts.len() > 64 {
            return Err("at most 64 mounts".into());
        }
        for m in &self.mounts {
            match m {
                MountDef::Volume { name, target, .. } => {
                    if !volume_name_ok(name) || !container_path_ok(target) {
                        return Err(format!("volume `{name}:{target}` is not allowed"));
                    }
                }
                MountDef::Bind { source, target, .. } => {
                    if let Some(why) = crate::stacks::protected_host_path(source) {
                        return Err(format!("binding `{source}` is not allowed: {why}"));
                    }
                    if !container_path_ok(target) {
                        return Err(format!("mount target `{target}` must be an absolute container path"));
                    }
                }
            }
        }
        if let Some(h) = &self.healthcheck
            && (h.command.trim().is_empty() || h.command.len() > 4096 || h.command.contains('\0'))
        {
            return Err("invalid health check command".into());
        }
        if !self.extra_hosts.iter().all(|h| {
            h.split_once(':').is_some_and(|(n, ip)| volume_name_ok(n) && ip.parse::<std::net::IpAddr>().is_ok())
        }) {
            return Err("extra hosts are `name:ip`".into());
        }
        if self.labels.len() > 64
            || !self.labels.iter().all(|(k, v)| {
                label_name_ok(k) && !k.starts_with("forge.") && v.len() <= 1024 && !v.chars().any(char::is_control)
            })
        {
            return Err("labels: letters, digits, `.` `-` `_`; `forge.*` is reserved".into());
        }
        if self.ports.len() > 64 || self.ports.iter().any(|p| p.target == 0 || p.published == Some(0)) {
            return Err("invalid port".into());
        }
        // Slot ports are Forge's: a slot publishes only there, and nothing
        // else may, so one service can never take another's slot port.
        let slot_range = crate::deploy::SLOT_PORTS;
        for p in self.ports.iter().filter_map(|p| p.published) {
            if self.slot.is_empty() == slot_range.contains(&p) {
                return Err(if self.slot.is_empty() {
                    format!(
                        "port {p} is reserved for Forge's deployment slots ({}-{})",
                        slot_range.start(),
                        slot_range.end()
                    )
                } else {
                    format!("slot port {p} is outside Forge's slot range")
                });
            }
        }
        self.settings.validate()
    }

    /// The Engine API `ServiceSpec`.
    #[must_use]
    pub fn engine_spec(&self) -> Value {
        let s = &self.settings;
        let mut labels = Map::new();
        for (k, v) in &self.labels {
            labels.insert(k.clone(), json!(v));
        }
        let mounts: Vec<Value> = self
            .mounts
            .iter()
            .map(|m| match m {
                MountDef::Volume { name, target, read_only } => {
                    json!({ "Type": "volume", "Source": name, "Target": target, "ReadOnly": read_only })
                }
                MountDef::Bind { source, target, read_only } => {
                    json!({ "Type": "bind", "Source": source, "Target": target, "ReadOnly": read_only })
                }
            })
            .collect();
        let mut container = json!({
            "Image": self.image,
            "Env": self.env.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>(),
            "Labels": labels,
            "Mounts": mounts,
            "ReadOnly": self.read_only,
            "Hosts": self.extra_hosts.iter().filter_map(|h| h.split_once(':').map(|(n, ip)| format!("{ip} {n}"))).collect::<Vec<_>>(),
        });
        if !self.command.is_empty() {
            container["Command"] = json!(self.command);
        }
        if !self.args.is_empty() {
            container["Args"] = json!(self.args);
        }
        for (key, value) in [("User", &self.user), ("Hostname", &self.hostname)] {
            if !value.is_empty() {
                container[key] = json!(value);
            }
        }
        if self.stop_grace_secs > 0 {
            container["StopGracePeriod"] = json!(secs(self.stop_grace_secs));
        }
        if let Some(hc) = &self.healthcheck {
            container["Healthcheck"] = json!({
                "Test": ["CMD-SHELL", hc.command],
                "Interval": secs(hc.interval_secs),
                "Timeout": secs(hc.timeout_secs),
                "Retries": hc.retries,
                "StartPeriod": secs(hc.start_period_secs),
            });
        }
        let mut limits = Map::new();
        if self.cpu_millis > 0 {
            limits.insert("NanoCPUs".into(), json!(u64::from(self.cpu_millis) * 1_000_000));
        }
        if self.memory_mb > 0 {
            limits.insert("MemoryBytes".into(), json!(u64::from(self.memory_mb) * 1024 * 1024));
        }
        let mut placement = json!({
            "Constraints": s.constraints,
            "Preferences": s.spread.iter().map(|l| json!({ "Spread": { "SpreadDescriptor": l } })).collect::<Vec<_>>(),
        });
        if s.max_per_node > 0 {
            placement["MaxReplicas"] = json!(s.max_per_node);
        }
        let restart = json!({
            "Condition": match s.restart.condition {
                RestartCondition::Any => "any",
                RestartCondition::OnFailure => "on-failure",
                RestartCondition::None => "none",
            },
            "Delay": secs(s.restart.delay_secs),
            "MaxAttempts": s.restart.max_attempts,
        });
        let mut task = json!({
            "ContainerSpec": container,
            "Resources": { "Limits": limits },
            "RestartPolicy": restart,
            "Placement": placement,
            "Networks": [{ "Target": network_name(&self.environment_id) }],
        });
        if self.log_max_mb > 0 {
            task["LogDriver"] =
                json!({ "Name": "json-file", "Options": { "max-size": format!("{}m", self.log_max_mb) } });
        }
        let update = json!({
            "Parallelism": s.update.parallelism,
            "Delay": secs(s.update.delay_secs),
            "FailureAction": s.update.failure_action.as_docker(),
            "Monitor": secs(s.update.monitor_secs),
            "MaxFailureRatio": s.update.max_failure_ratio,
            "Order": match s.update.order { UpdateOrder::StartFirst => "start-first", UpdateOrder::StopFirst => "stop-first" },
        });
        let mut rollback = update.clone();
        rollback["FailureAction"] = json!("pause");
        let mode = match s.mode {
            ServiceMode::Replicated => json!({ "Replicated": { "Replicas": s.replicas } }),
            ServiceMode::Global => json!({ "Global": {} }),
        };
        let publish = match s.publish {
            PublishMode::Ingress => "ingress",
            PublishMode::Host => "host",
        };
        let ports: Vec<Value> = self
            .ports
            .iter()
            .map(|p| {
                let mut v = json!({
                    "Protocol": match p.protocol { Protocol::Tcp => "tcp", Protocol::Udp => "udp" },
                    "TargetPort": p.target,
                    "PublishMode": publish,
                });
                if let Some(h) = p.published {
                    v["PublishedPort"] = json!(h);
                }
                v
            })
            .collect();
        let mut spec = json!({
            "Name": service_name(&self.service_id, &self.slot),
            "Labels": {
                MANAGED_LABEL: "true",
                SERVICE_ID_LABEL: self.service_id,
                SLOT_LABEL: self.slot,
                "forge.environment.id": self.environment_id,
                "forge.environment": self.environment,
                "forge.service": self.service,
            },
            "TaskTemplate": task,
            "Mode": mode,
            "UpdateConfig": update,
            "RollbackConfig": rollback,
            "EndpointSpec": { "Mode": "vip", "Ports": ports },
        });
        let print = fingerprint(&spec);
        spec["Labels"][SPEC_LABEL] = json!(print);
        spec["TaskTemplate"]["ContainerSpec"]["Labels"][SPEC_LABEL] = json!(print);
        spec
    }
}

fn secs(n: u32) -> u64 {
    u64::from(n) * NANOS
}

/// One high-availability check of a Swarm service.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HaCheck {
    pub name: String,
    pub ok: bool,
    /// What to change when it fails.
    pub hint: String,
}

/// How well a service survives losing a node or a manager.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HaReport {
    /// Share of checks that pass, 0–100.
    pub score: u8,
    pub checks: Vec<HaCheck>,
}

/// The HA readiness of a service with `settings`, a health check or not,
/// in a swarm with `managers` managers.
#[must_use]
pub fn ha_report(settings: &SwarmSettings, has_healthcheck: bool, managers: usize) -> HaReport {
    let check = |name: &str, ok: bool, hint: &str| HaCheck { name: name.into(), ok, hint: hint.into() };
    let global = settings.mode == ServiceMode::Global;
    let checks = vec![
        check("More than one task", global || settings.replicas >= 2, "Run at least 2 replicas, or global mode"),
        check(
            "Tasks spread over nodes",
            global || !settings.spread.is_empty() || settings.max_per_node == 1,
            "Spread over a node label (e.g. node.labels.zone) or allow 1 task per node",
        ),
        check("Health check", has_healthcheck, "Add a health check so Swarm replaces unhealthy tasks"),
        check(
            "Restarts on failure",
            settings.restart.condition != RestartCondition::None,
            "Set the restart condition to any or on-failure",
        ),
        check(
            "Rolls back a failed update",
            settings.update.failure_action == FailureAction::Rollback,
            "Set the update failure action to rollback",
        ),
        check("Manager quorum survives a failure", managers >= 3, "Run 3 or 5 managers"),
    ];
    let passed = checks.iter().filter(|c| c.ok).count();
    let score = u8::try_from(passed * 100 / checks.len()).unwrap_or(100);
    HaReport { score, checks }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const SVC: &str = "01900000-0000-7000-8000-000000000001";
    const ENV: &str = "01900000-0000-7000-8000-000000000002";

    fn app() -> AppSpec {
        serde_json::from_value(json!({
            "environment": "prod", "name": "web", "runtime": "container",
            "image": "nginx:1.27", "ports": ["8080:80", "9000:9000/udp"],
            "volumes": ["data:/var/lib/data", "/srv/conf:/etc/app:ro"],
            "env": [["MODE", "prod"]], "command": "nginx -g 'daemon off;'",
            "cpu_millis": 500, "memory_mb": 256,
            "healthcheck": {"command": "curl -f http://localhost/"}
        }))
        .unwrap()
    }

    fn def(s: &SwarmSettings) -> Result<ServiceDef, String> {
        service_def(&app(), s, SVC, ENV)
    }

    #[test]
    fn a_container_service_becomes_a_swarm_service() {
        let s = SwarmSettings {
            replicas: 3,
            spread: vec!["node.labels.zone".into()],
            max_per_node: 2,
            ..Default::default()
        };
        let d = def(&s).unwrap();
        assert_eq!(d.bind_sources(), vec!["/srv/conf"]);
        let v = d.engine_spec();
        assert_eq!(v["Name"], "forge-01900000000070008000000000000001");
        assert_eq!(v["Labels"][MANAGED_LABEL], "true");
        assert_eq!(v["Labels"][SERVICE_ID_LABEL], SVC);
        let print = v["Labels"][SPEC_LABEL].as_str().unwrap().to_owned();
        assert_eq!(v["TaskTemplate"]["ContainerSpec"]["Labels"][SPEC_LABEL], print.as_str());
        assert_eq!(d.engine_spec()["Labels"][SPEC_LABEL], print.as_str(), "stable");
        assert_eq!(v["Mode"]["Replicated"]["Replicas"], 3);
        let c = &v["TaskTemplate"]["ContainerSpec"];
        assert_eq!(c["Image"], "nginx:1.27");
        assert_eq!(c["Env"][0], "MODE=prod");
        assert_eq!(c["Command"][0], "/bin/sh");
        assert_eq!(c["Mounts"][0]["Type"], "volume");
        assert_eq!(c["Mounts"][1]["Type"], "bind");
        assert_eq!(c["Mounts"][1]["ReadOnly"], true);
        assert_eq!(c["Healthcheck"]["Interval"], 30 * NANOS);
        assert!(c.get("CapabilityAdd").is_none() && c.get("Privileges").is_none());
        assert_eq!(v["TaskTemplate"]["Resources"]["Limits"]["NanoCPUs"], 500_000_000u64);
        assert_eq!(v["TaskTemplate"]["Placement"]["MaxReplicas"], 2);
        assert_eq!(v["TaskTemplate"]["Networks"][0]["Target"], "forge-net-01900000000070008000000000000002");
        assert_eq!(v["UpdateConfig"]["FailureAction"], "rollback");
        assert_eq!(v["EndpointSpec"]["Ports"][0]["PublishedPort"], 8080);
        assert_eq!(v["EndpointSpec"]["Ports"][1]["Protocol"], "udp");
    }

    #[test]
    fn names_come_from_ids_so_environments_never_collide() {
        assert_ne!(service_name(SVC, ""), service_name(ENV, ""));
        assert_eq!(service_name(SVC, "a"), "forge-01900000000070008000000000000001-a");
        assert!(id_ok(SVC));
        assert!(!id_ok("a-b-c"));
        assert!(!id_ok("../../etc/passwd-0000-0000-0000-00000000"));
    }

    #[test]
    fn global_mode_and_host_ports() {
        let s = SwarmSettings { mode: ServiceMode::Global, publish: PublishMode::Host, ..Default::default() };
        let v = def(&s).unwrap().engine_spec();
        assert!(v["Mode"]["Global"].is_object());
        assert_eq!(v["EndpointSpec"]["Ports"][0]["PublishMode"], "host");
    }

    #[test]
    fn protected_host_paths_never_become_binds() {
        for src in [
            "/",
            "/etc/cron.d",
            "/var/run",
            "/run/docker.sock",
            "/var/lib/docker",
            "/srv/../etc",
            "/root/.ssh",
            "/opt/forge",
        ] {
            let mut a = app();
            a.volumes = vec![format!("{src}:/x")];
            assert!(service_def(&a, &SwarmSettings::default(), SVC, ENV).is_err(), "{src}");
        }
        let mut a = app();
        a.volumes = vec!["/srv/data:relative".into()];
        assert!(service_def(&a, &SwarmSettings::default(), SVC, ENV).is_err(), "relative target");
    }

    #[test]
    fn settings_and_fields_are_checked() {
        let bad = |s: SwarmSettings| def(&s).is_err();
        assert!(bad(SwarmSettings { constraints: vec!["rm -rf /".into()], ..Default::default() }));
        assert!(bad(SwarmSettings { constraints: vec!["node.labels.zone=a".into()], ..Default::default() }));
        assert!(!bad(SwarmSettings {
            constraints: vec!["node.labels.zone==a".into(), "node.role!=manager".into()],
            ..Default::default()
        }));
        assert!(bad(SwarmSettings { spread: vec!["zone".into()], ..Default::default() }));
        assert!(bad(SwarmSettings { replicas: 5000, ..Default::default() }));
        let mut native = app();
        native.runtime = Runtime::Systemd;
        assert!(service_def(&native, &SwarmSettings::default(), SVC, ENV).is_err());
        let mut forged = app();
        forged.labels = vec![("forge.service.id".into(), "other".into())];
        assert!(service_def(&forged, &SwarmSettings::default(), SVC, ENV).is_err(), "forge.* labels are reserved");
    }

    #[test]
    fn definitions_are_closed() {
        let d = def(&SwarmSettings::default()).unwrap();
        let mut v = serde_json::to_value(&d).unwrap();
        v["privileged"] = json!(true);
        assert!(serde_json::from_value::<ServiceDef>(v).is_err());
        let dbg = format!("{d:?}");
        assert!(!dbg.contains("prod\""), "env values never print: {dbg}");
    }

    #[test]
    fn settings_round_trip_with_defaults() {
        let s: SwarmSettings = serde_json::from_value(json!({"replicas": 2})).unwrap();
        assert_eq!(s.update.order, UpdateOrder::StartFirst);
        assert_eq!(s.restart.delay_secs, 5);
        assert!(serde_json::from_value::<SwarmSettings>(json!({"replica": 2})).is_err());
    }

    #[test]
    fn slots_publish_their_own_ports() {
        let d = def(&SwarmSettings::default()).unwrap();
        let ports = std::collections::BTreeMap::from([(80, 21000), (9000, 21001)]);
        let mut reserved = app();
        reserved.ports = vec!["21000:80".into()];
        assert!(service_def(&reserved, &SwarmSettings::default(), SVC, ENV).is_err(), "slot ports are Forge's");
        let a = d.clone().into_slot("a", &ports).unwrap();
        let v = a.engine_spec();
        assert_eq!(v["Name"], "forge-01900000000070008000000000000001-a");
        assert_eq!(v["Labels"][SLOT_LABEL], "a");
        assert_eq!(v["EndpointSpec"]["Ports"][0]["PublishedPort"], 21000);
        assert!(d.clone().into_slot("c", &ports).is_err());
        assert!(d.into_slot("b", &std::collections::BTreeMap::new()).is_err(), "every port needs a slot port");
    }

    #[test]
    fn ha_report_scores_the_checks() {
        let weak = ha_report(&SwarmSettings::default(), false, 1);
        assert!(weak.score < 50, "{weak:?}");
        let strong = SwarmSettings { replicas: 3, spread: vec!["node.labels.zone".into()], ..Default::default() };
        let r = ha_report(&strong, true, 3);
        assert_eq!(r.score, 100, "{r:?}");
        assert!(ha_report(&SwarmSettings { mode: ServiceMode::Global, ..Default::default() }, true, 3).checks[0].ok);
    }
}
