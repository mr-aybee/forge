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

/// `<environment>-<service>`: unique per swarm, valid as a Docker name.
#[must_use]
pub fn service_name(environment: &str, service: &str) -> String {
    format!("{environment}-{service}")
}

/// The overlay network of an environment.
#[must_use]
pub fn network_name(environment: &str) -> String {
    format!("forge-{environment}")
}

fn port(p: &str) -> Result<Value, String> {
    let (body, proto) = p.rsplit_once('/').map_or((p, "tcp"), |(b, pr)| (b, pr));
    if !matches!(proto, "tcp" | "udp") {
        return Err(format!("port `{p}`: protocol is tcp or udp"));
    }
    let parts: Vec<&str> = body.split(':').collect();
    let (published, target) = match parts.as_slice() {
        [t] => (None, *t),
        [h, t] | [_, h, t] => (Some(*h), *t),
        _ => return Err(format!("port `{p}` is not `[ip:]host:container`")),
    };
    let target: u16 = target.parse().map_err(|_| format!("port `{p}`: bad container port"))?;
    let mut v = json!({ "Protocol": proto, "TargetPort": target });
    if let Some(h) = published {
        v["PublishedPort"] = json!(h.parse::<u16>().map_err(|_| format!("port `{p}`: bad host port"))?);
    }
    Ok(v)
}

fn mount(v: &str) -> Result<Value, String> {
    let parts: Vec<&str> = v.split(':').collect();
    let (source, target, ro) = match parts.as_slice() {
        [s, t] => (*s, *t, false),
        [s, t, "ro"] => (*s, *t, true),
        [s, t, "rw"] => (*s, *t, false),
        _ => return Err(format!("volume `{v}` is not `source:target[:ro]`")),
    };
    if !target.starts_with('/') {
        return Err(format!("volume `{v}`: the container path must be absolute"));
    }
    let kind = if source.starts_with('/') { "bind" } else { "volume" };
    Ok(json!({ "Type": kind, "Source": source, "Target": target, "ReadOnly": ro }))
}

fn secs(n: u32) -> u64 {
    u64::from(n) * NANOS
}

/// The Engine API `ServiceSpec` for a container service.
pub fn service_spec(spec: &AppSpec, s: &SwarmSettings, env_slug: &str) -> Result<Value, String> {
    if spec.runtime != Runtime::Container {
        return Err("only container services run on Docker Swarm".into());
    }
    if spec.image.trim().is_empty() {
        return Err("a container service needs an image".into());
    }
    s.validate()?;
    let mut labels = Map::new();
    for (k, v) in &spec.labels {
        labels.insert(k.clone(), json!(v));
    }
    let mut container = json!({
        "Image": spec.image,
        "Env": spec.env.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>(),
        "Labels": labels,
        "Mounts": spec.volumes.iter().map(|v| mount(v)).collect::<Result<Vec<_>, _>>()?,
        "ReadOnly": spec.read_only,
        "Hosts": spec.extra_hosts.iter().filter_map(|h| h.split_once(':').map(|(n, ip)| format!("{ip} {n}"))).collect::<Vec<_>>(),
    });
    if !spec.command.trim().is_empty() {
        container["Command"] = json!(["/bin/sh", "-c", spec.command.trim()]);
    }
    if !spec.entrypoint.trim().is_empty() {
        container["Command"] = json!(spec.entrypoint.split_whitespace().collect::<Vec<_>>());
        if !spec.command.trim().is_empty() {
            container["Args"] = json!(["/bin/sh", "-c", spec.command.trim()]);
        }
    }
    for (key, value) in [("User", &spec.user), ("Hostname", &spec.hostname)] {
        if !value.trim().is_empty() {
            container[key] = json!(value);
        }
    }
    if spec.stop_timeout_secs > 0 {
        container["StopGracePeriod"] = json!(secs(spec.stop_timeout_secs));
    }
    if let Some(hc) = &spec.healthcheck {
        container["Healthcheck"] = json!({
            "Test": ["CMD-SHELL", hc.command],
            "Interval": secs(hc.interval_secs),
            "Timeout": secs(hc.timeout_secs),
            "Retries": hc.retries,
            "StartPeriod": secs(hc.start_period_secs),
        });
    }
    let mut limits = Map::new();
    if spec.cpu_millis > 0 {
        limits.insert("NanoCPUs".into(), json!(u64::from(spec.cpu_millis) * 1_000_000));
    }
    if spec.memory_mb > 0 {
        limits.insert("MemoryBytes".into(), json!(u64::from(spec.memory_mb) * 1024 * 1024));
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
        "Networks": [{ "Target": network_name(env_slug) }],
    });
    if spec.log_max_mb > 0 {
        task["LogDriver"] = json!({ "Name": "json-file", "Options": { "max-size": format!("{}m", spec.log_max_mb) } });
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
    let ports: Vec<Value> = spec
        .ports
        .iter()
        .map(|p| {
            port(p).map(|mut v| {
                v["PublishMode"] = json!(publish);
                v
            })
        })
        .collect::<Result<_, _>>()?;
    let mut svc_labels = Map::new();
    svc_labels.insert(MANAGED_LABEL.into(), json!("true"));
    svc_labels.insert("forge.environment".into(), json!(env_slug));
    svc_labels.insert("forge.service".into(), json!(spec.name));
    Ok(json!({
        "Name": service_name(env_slug, &spec.name),
        "Labels": svc_labels,
        "TaskTemplate": task,
        "Mode": mode,
        "UpdateConfig": update,
        "RollbackConfig": rollback,
        "EndpointSpec": { "Mode": "vip", "Ports": ports },
    }))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

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

    #[test]
    fn a_container_service_becomes_a_swarm_service() {
        let s = SwarmSettings {
            replicas: 3,
            spread: vec!["node.labels.zone".into()],
            max_per_node: 2,
            ..Default::default()
        };
        let v = service_spec(&app(), &s, "prod").unwrap();
        assert_eq!(v["Name"], "prod-web");
        assert_eq!(v["Labels"][MANAGED_LABEL], "true");
        assert_eq!(v["Mode"]["Replicated"]["Replicas"], 3);
        let c = &v["TaskTemplate"]["ContainerSpec"];
        assert_eq!(c["Image"], "nginx:1.27");
        assert_eq!(c["Env"][0], "MODE=prod");
        assert_eq!(c["Command"][0], "/bin/sh");
        assert_eq!(c["Mounts"][0]["Type"], "volume");
        assert_eq!(c["Mounts"][1]["Type"], "bind");
        assert_eq!(c["Mounts"][1]["ReadOnly"], true);
        assert_eq!(c["Healthcheck"]["Interval"], 30 * NANOS);
        assert_eq!(v["TaskTemplate"]["Resources"]["Limits"]["NanoCPUs"], 500_000_000u64);
        assert_eq!(v["TaskTemplate"]["Placement"]["MaxReplicas"], 2);
        assert_eq!(v["TaskTemplate"]["Placement"]["Preferences"][0]["Spread"]["SpreadDescriptor"], "node.labels.zone");
        assert_eq!(v["TaskTemplate"]["Networks"][0]["Target"], "forge-prod");
        assert_eq!(v["UpdateConfig"]["Order"], "start-first");
        assert_eq!(v["UpdateConfig"]["FailureAction"], "rollback");
        assert_eq!(v["EndpointSpec"]["Ports"][0]["PublishedPort"], 8080);
        assert_eq!(v["EndpointSpec"]["Ports"][1]["Protocol"], "udp");
    }

    #[test]
    fn global_mode_and_host_ports() {
        let s = SwarmSettings { mode: ServiceMode::Global, publish: PublishMode::Host, ..Default::default() };
        let v = service_spec(&app(), &s, "prod").unwrap();
        assert!(v["Mode"]["Global"].is_object());
        assert_eq!(v["EndpointSpec"]["Ports"][0]["PublishMode"], "host");
    }

    #[test]
    fn settings_are_checked() {
        let bad = |s: SwarmSettings| service_spec(&app(), &s, "prod").is_err();
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
        assert!(service_spec(&native, &SwarmSettings::default(), "prod").is_err());
    }

    #[test]
    fn settings_round_trip_with_defaults() {
        let s: SwarmSettings = serde_json::from_value(json!({"replicas": 2})).unwrap();
        assert_eq!(s.update.order, UpdateOrder::StartFirst);
        assert_eq!(s.restart.delay_secs, 5);
        assert!(serde_json::from_value::<SwarmSettings>(json!({"replica": 2})).is_err());
    }
}
