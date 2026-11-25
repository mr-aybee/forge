//! Compose stacks on Docker Swarm (D-171): one `docker stack deploy` on a
//! manager instead of one `docker compose up` per node.
//!
//! The compose policy is the one of [`crate::stacks`]: the same
//! interpolation, the same [`check`] (privileged settings, host paths,
//! sockets, `build:`). On top of it come the rules a swarm needs:
//!
//! * top-level `secrets:` and `configs:` must be `external: true`; Forge
//!   points each at the newest version it made (`<name>.v<N>`). A `file:`
//!   would be read on whichever manager runs the deploy and is refused.
//! * services that publish `ports:` (or set `x-forge.shared: true`) also
//!   join the cluster's shared overlay [`SHARED_NETWORK`], so other stacks
//!   reach them by name; every service stays on the stack's own encrypted
//!   overlay (`default`). Services without ports stay private to the stack.
//! * `depends_on` and `container_name` are dropped (Swarm ignores them);
//!   `network_mode` and relative bind mounts are refused (a task may run on
//!   any node).
//! * every service gets the `forge.stack` label.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::stacks::{
    Policy, Report, STACK_LABEL, Severity, check, escape_dollars, interpolate, is_system_path, normalize_abs,
    project_name, redact, valid_stack_name,
};

/// Where the manager keeps a swarm stack's compose file.
pub const SWARM_STACKS_DIR: &str = "/opt/forge/swarm-stacks";
/// The attachable, encrypted overlay every swarm gets for calls between
/// stacks and services by name.
pub const SHARED_NETWORK: &str = "forge-shared";
/// Label with the Forge name of a secret or config (without the version).
pub const OBJECT_NAME_LABEL: &str = "forge.object.name";
/// Label with the version of a secret or config Forge made.
pub const OBJECT_VERSION_LABEL: &str = "forge.object.version";
/// Largest secret or config value (Docker's limit is 500 KiB).
pub const MAX_OBJECT_BYTES: usize = 500 * 1024;

/// The folder of a swarm stack on the manager.
#[must_use]
pub fn stack_dir(environment: &str, stack: &str) -> String {
    format!("{SWARM_STACKS_DIR}/{environment}/{stack}")
}

/// Swarm stack namespaces are the Compose project names.
#[must_use]
pub fn namespace(environment: &str, stack: &str) -> String {
    project_name(environment, stack)
}

/// Names of secrets and configs: `[a-z0-9][a-z0-9_-]{0,50}`.
#[must_use]
pub fn object_name_ok(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= 51
        && (n.as_bytes()[0].is_ascii_lowercase() || n.as_bytes()[0].is_ascii_digit())
        && n.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

/// The Docker name of version `version` of object `name`.
#[must_use]
pub fn versioned(name: &str, version: u32) -> String {
    format!("{name}.v{version}")
}

/// A Docker secret or a Docker config.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObjectKind {
    Secret,
    Config,
}

impl ObjectKind {
    /// The Engine API path segment (`secrets`, `configs`).
    #[must_use]
    pub const fn path(self) -> &'static str {
        match self {
            Self::Secret => "secrets",
            Self::Config => "configs",
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Secret => "secret",
            Self::Config => "config",
        }
    }
}

/// One version of a secret or config in the swarm. Never carries the value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwarmObject {
    pub id: String,
    pub kind: ObjectKind,
    /// The Forge name (`db_password`), empty for one Forge did not make.
    pub name: String,
    /// The Docker name (`db_password.v3`).
    pub docker_name: String,
    pub version: u32,
    pub created_at: String,
    /// Swarm services that use this version.
    pub used_by: Vec<String>,
    /// Made by Forge (only those can be rotated or removed from Forge).
    pub managed: bool,
}

/// The manager's part of a swarm stack, as the helper receives it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SwarmStackPart {
    pub environment: String,
    pub stack: String,
    /// The compose tree (not yet interpolated).
    pub compose: Value,
    /// `.env` variables (may hold secrets; never written to disk).
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// `stack_dir` is [`stack_dir`]; `bind_roots` are the folders approved
    /// on every node of the swarm.
    pub policy: Policy,
    /// Newest version of every Forge-made secret, by Forge name.
    #[serde(default)]
    pub secrets: BTreeMap<String, u32>,
    /// Newest version of every Forge-made config, by Forge name.
    #[serde(default)]
    pub configs: BTreeMap<String, u32>,
}

impl SwarmStackPart {
    #[must_use]
    pub fn namespace(&self) -> String {
        namespace(&self.environment, &self.stack)
    }

    /// Checks names, policy and the swarm rules; returns the tree to deploy
    /// (`$` escaped) and the findings. `.env` values are redacted from errors.
    pub fn prepare(&self) -> Result<(Value, Report), String> {
        if !valid_stack_name(&self.environment) || !valid_stack_name(&self.stack) {
            return Err("invalid environment or stack name".into());
        }

        if self.policy.stack_dir != stack_dir(&self.environment, &self.stack) {
            return Err("the stack folder does not match the policy".into());
        }

        if self.env.keys().any(|k| {
            let u = k.to_ascii_uppercase();
            u.starts_with("COMPOSE_") || u.starts_with("DOCKER_")
        }) {
            return Err("COMPOSE_* and DOCKER_* variables are not allowed in .env".into());
        }

        let hide = |e: String| redact(&e, &self.env);
        let mut tree = interpolate(&self.compose, &self.env).map_err(hide)?;
        let mut report = check(&tree, &self.policy);
        swarm_rules(&tree, &mut report, &self.secrets, &self.configs);
        report.ensure(self.policy.allow_privileged).map_err(hide)?;
        rewrite(&mut tree, &format!("{}/{}", self.environment, self.stack), &self.secrets, &self.configs);

        Ok((escape_dollars(&tree), report))
    }
}

/// Whether a service joins [`SHARED_NETWORK`]: it publishes ports, or says so.
#[must_use]
pub fn is_shared(svc: &Value) -> bool {
    if let Some(b) = svc.pointer("/x-forge/shared").and_then(Value::as_bool) {
        return b;
    }

    svc.get("ports").and_then(Value::as_array).is_some_and(|p| !p.is_empty())
}

/// The services of a stack that join the shared network.
#[must_use]
pub fn shared_services(tree: &Value) -> BTreeSet<String> {
    tree.get("services")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter(|(_, s)| is_shared(s))
        .map(|(n, _)| n.clone())
        .collect()
}

fn swarm_rules(tree: &Value, r: &mut Report, secrets: &BTreeMap<String, u32>, configs: &BTreeMap<String, u32>) {
    let mut push = |service: &str, severity: Severity, message: String| {
        if severity == Severity::Privileged {
            r.privileged = true;
        }

        r.findings.push(crate::stacks::Finding { service: service.to_owned(), severity, message });
    };

    for (name, svc) in tree.get("services").and_then(Value::as_object).into_iter().flatten() {
        if svc.get("network_mode").is_some() {
            push(name, Severity::Blocked, "`network_mode` is not supported in a swarm; use `networks`".into());
        }

        if svc.get("depends_on").is_some() {
            push(
                name,
                Severity::Info,
                "`depends_on` is ignored in a swarm: give the service a health check and a restart policy".into(),
            );
        }

        if svc.get("container_name").is_some() {
            push(name, Severity::Info, "`container_name` is ignored in a swarm: Swarm names the tasks".into());
        }

        for v in svc.get("volumes").and_then(Value::as_array).into_iter().flatten() {
            let src = match v {
                Value::String(s) => s.split(':').next().filter(|_| s.contains(':')),
                Value::Object(m) if m.get("type").and_then(Value::as_str) == Some("bind") => {
                    m.get("source").and_then(Value::as_str)
                }
                _ => None,
            };

            if let Some(s) = src.filter(|s| s.starts_with('.') || s.starts_with('~')) {
                push(
                    name,
                    Severity::Blocked,
                    format!("bind `{s}` is relative: a task can run on any node; use an absolute folder or a volume"),
                );
            }
        }
    }

    for (section, known) in [("secrets", secrets), ("configs", configs)] {
        for (key, v) in tree.get(section).and_then(Value::as_object).into_iter().flatten() {
            let external = v.get("external").is_some_and(|e| e.as_bool() == Some(true) || e.is_object());
            if !external {
                push(
                    "",
                    Severity::Blocked,
                    format!(
                        "{section} `{key}`: create it in Forge (Swarm → Secrets & configs) and declare it \
                         `external: true`"
                    ),
                );
                continue;
            }

            let name = v.get("name").and_then(Value::as_str).unwrap_or(key);
            if !known.contains_key(name) {
                push(
                    "",
                    Severity::Blocked,
                    format!("{section} `{name}` does not exist in this swarm; create it first"),
                );
            }
        }
    }
}

fn rewrite(tree: &mut Value, label: &str, secrets: &BTreeMap<String, u32>, configs: &BTreeMap<String, u32>) {
    let Some(top) = tree.as_object_mut() else { return };

    // External objects point at the newest Forge version.
    for (section, known) in [("secrets", secrets), ("configs", configs)] {
        for (key, v) in top.get_mut(section).and_then(Value::as_object_mut).into_iter().flatten() {
            let name = v.get("name").and_then(Value::as_str).unwrap_or(key).to_owned();

            if let Some(version) = known.get(&name) {
                *v = json!({ "external": true, "name": versioned(&name, *version) });
            }
        }
    }

    let mut any_shared = false;

    if let Some(services) = top.get_mut("services").and_then(Value::as_object_mut) {
        for svc in services.values_mut().filter_map(Value::as_object_mut) {
            let shared = is_shared(&Value::Object(svc.clone()));
            svc.remove("depends_on");
            svc.remove("container_name");
            svc.remove("x-forge");

            let deploy = svc.entry("deploy").or_insert_with(|| Value::Object(Map::new()));
            if let Some(d) = deploy.as_object_mut() {
                set_label(d.entry("labels").or_insert_with(|| Value::Object(Map::new())), label);
            }

            if shared {
                any_shared = true;
                join_shared(svc);
            }
        }
    }

    // The stack's own network: an encrypted overlay unless the file says otherwise.
    let networks = top.entry("networks").or_insert_with(|| Value::Object(Map::new()));
    if let Some(n) = networks.as_object_mut() {
        n.entry("default").or_insert_with(|| json!({ "driver": "overlay", "driver_opts": { "encrypted": "true" } }));

        if any_shared {
            n.insert(SHARED_NETWORK.to_owned(), json!({ "external": true, "name": SHARED_NETWORK }));
        }
    }
}

fn set_label(labels: &mut Value, value: &str) {
    match labels {
        Value::Object(m) => {
            m.insert(STACK_LABEL.to_owned(), Value::String(value.to_owned()));
        }
        Value::Array(a) => {
            a.retain(|l| l.as_str().is_none_or(|s| s.split('=').next() != Some(STACK_LABEL)));
            a.push(Value::String(format!("{STACK_LABEL}={value}")));
        }
        other => *other = json!({ STACK_LABEL: value }),
    }
}

/// Adds the shared network to a service, keeping the ones it lists (and
/// `default` when it lists none, as Compose would).
fn join_shared(svc: &mut Map<String, Value>) {
    match svc.entry("networks").or_insert_with(|| json!(["default"])) {
        Value::Array(a) => {
            if !a.iter().any(|n| n.as_str() == Some(SHARED_NETWORK)) {
                a.push(Value::String(SHARED_NETWORK.to_owned()));
            }
        }
        Value::Object(m) => {
            m.entry(SHARED_NETWORK).or_insert(Value::Null);
        }
        other => *other = json!(["default", SHARED_NETWORK]),
    }
}

/// The folders approved on every node: a root of the first node that lies
/// inside some root of every other node.
#[must_use]
pub fn common_roots(per_node: &[Vec<String>]) -> Vec<String> {
    let Some((first, rest)) = per_node.split_first() else { return Vec::new() };
    let norm = |v: &Vec<String>| v.iter().filter_map(|r| normalize_abs(r)).collect::<Vec<_>>();
    let rest: Vec<Vec<String>> = rest.iter().map(norm).collect();

    norm(first)
        .into_iter()
        .filter(|r| !is_system_path(r))
        .filter(|r| rest.iter().all(|roots| roots.iter().any(|o| crate::stacks::within(r, o))))
        .collect()
}

/// One service of a swarm stack, as the console shows it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StackService {
    /// Compose name (`api`), from the `com.docker.stack.namespace` prefix.
    pub name: String,
    /// Docker name (`forge-prod-shop_api`).
    pub docker_name: String,
    pub id: String,
    pub image: String,
    pub mode: String,
    pub desired: u64,
    pub running: u64,
    /// Published ports (`8080:80/tcp`, mode).
    pub ports: Vec<StackPort>,
    pub update_state: String,
    pub update_message: String,
    /// Joined to the shared network.
    pub shared: bool,
    pub tasks: Vec<crate::swarm::SwarmTask>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StackPort {
    pub published: u16,
    pub target: u16,
    pub protocol: String,
    /// `ingress` (routing mesh) or `host`.
    pub mode: String,
}

/// A swarm stack's live state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StackStatus {
    pub namespace: String,
    pub services: Vec<StackService>,
}

impl StackStatus {
    /// Every service runs what it should, and no update is under way.
    #[must_use]
    pub fn converged(&self) -> bool {
        self.services
            .iter()
            .all(|s| s.running >= s.desired && !matches!(s.update_state.as_str(), "updating" | "rollback_started"))
    }

    /// A service whose update Swarm paused or rolled back, with its message.
    #[must_use]
    pub fn failure(&self) -> Option<String> {
        self.services
            .iter()
            .find(|s| matches!(s.update_state.as_str(), "paused" | "rollback_paused" | "rollback_completed"))
            .map(|s| format!("{}: Swarm {} the update: {}", s.name, s.update_state.replace('_', " "), s.update_message))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn part(compose: Value) -> SwarmStackPart {
        SwarmStackPart {
            environment: "prod".into(),
            stack: "shop".into(),
            compose,
            env: BTreeMap::from([("TAG".into(), "1.2".into())]),
            policy: Policy { allow_privileged: false, bind_roots: vec![], stack_dir: stack_dir("prod", "shop") },
            secrets: BTreeMap::from([("db_password".into(), 3)]),
            configs: BTreeMap::new(),
        }
    }

    fn shop() -> Value {
        json!({
            "services": {
                "api": { "image": "example/api:${TAG}", "ports": ["8080:80"], "depends_on": ["redis"],
                         "secrets": ["db_password"] },
                "worker": { "image": "example/worker:1", "ports": [{"target": 9000, "published": 9000}] },
                "redis": { "image": "redis:7" },
                "seq": { "image": "datalust/seq:2024", "container_name": "seq" },
            },
            "secrets": { "db_password": { "external": true } },
        })
    }

    #[test]
    fn ports_join_the_shared_network_and_private_services_do_not() {
        let (tree, report) = part(shop()).prepare().unwrap();
        let svc = |n: &str| &tree["services"][n];
        assert_eq!(svc("api")["networks"], json!(["default", SHARED_NETWORK]));
        assert_eq!(svc("worker")["networks"], json!(["default", SHARED_NETWORK]));
        assert!(svc("redis").get("networks").is_none(), "redis stays on the stack's own network");
        assert!(svc("seq").get("container_name").is_none());
        assert!(svc("api").get("depends_on").is_none());
        assert_eq!(svc("api")["image"], "example/api:1.2");
        assert_eq!(svc("redis")["deploy"]["labels"][STACK_LABEL], "prod/shop");
        assert_eq!(tree["networks"][SHARED_NETWORK], json!({"external": true, "name": SHARED_NETWORK}));
        assert_eq!(tree["networks"]["default"]["driver_opts"]["encrypted"], "true");
        assert_eq!(tree["secrets"]["db_password"], json!({"external": true, "name": "db_password.v3"}));
        assert!(report.findings.iter().any(|f| f.severity == Severity::Info && f.service == "api"));
    }

    #[test]
    fn shared_can_be_set_either_way() {
        let mut c = shop();
        c["services"]["redis"]["x-forge"] = json!({"shared": true});
        c["services"]["worker"]["x-forge"] = json!({"shared": false});
        let (tree, _) = part(c).prepare().unwrap();
        assert!(tree["services"]["redis"]["networks"].as_array().unwrap().contains(&json!(SHARED_NETWORK)));
        assert!(tree["services"]["worker"].get("networks").is_none());
        assert!(tree["services"]["redis"].get("x-forge").is_none());
    }

    #[test]
    fn file_secrets_unknown_secrets_and_host_tricks_are_refused() {
        let mut c = shop();
        c["secrets"]["db_password"] = json!({"file": "./pw.txt"});
        assert!(part(c).prepare().unwrap_err().contains("external: true"));

        let mut c = shop();
        c["secrets"]["other"] = json!({"external": true});
        assert!(part(c).prepare().unwrap_err().contains("does not exist"));

        let mut c = shop();
        c["services"]["api"]["network_mode"] = json!("bridge");
        assert!(part(c).prepare().unwrap_err().contains("network_mode"));

        let mut c = shop();
        c["services"]["api"]["volumes"] = json!(["./data:/data"]);
        assert!(part(c).prepare().unwrap_err().contains("relative"));

        let mut c = shop();
        c["services"]["api"]["volumes"] = json!(["/var/run/docker.sock:/var/run/docker.sock"]);
        assert!(part(c).prepare().is_err(), "the compose policy still applies");
    }

    #[test]
    fn env_values_never_reach_errors() {
        let mut p = part(json!({"services": {"a": {"image": "x", "network_mode": "${TAG}"}}}));
        p.env.insert("TAG".into(), "hunter2-secret".into());
        let e = p.prepare().unwrap_err();
        assert!(!e.contains("hunter2-secret"));
    }

    #[test]
    fn existing_networks_and_label_lists_are_kept() {
        let c = json!({"services": {"api": {"image": "x", "ports": ["80:80"], "networks": {"back": {}},
            "deploy": {"labels": ["team=a"]}}}, "networks": {"back": {"driver": "overlay"}}});
        let (tree, _) = part(c).prepare().unwrap();
        let api = &tree["services"]["api"];
        assert!(api["networks"].get("back").is_some() && api["networks"].get(SHARED_NETWORK).is_some());
        assert_eq!(api["deploy"]["labels"], json!(["team=a", "forge.stack=prod/shop"]));
    }

    #[test]
    fn roots_approved_on_every_node() {
        let roots = common_roots(&[
            vec!["/srv/data".into(), "/opt/apps".into()],
            vec!["/srv".into()],
            vec!["/srv/data".into(), "/opt/apps".into()],
        ]);
        assert_eq!(roots, vec!["/srv/data"]);
        assert!(common_roots(&[]).is_empty());
    }

    #[test]
    fn object_names_and_versions() {
        assert!(object_name_ok("db_password") && object_name_ok("tls-cert"));
        assert!(!object_name_ok("Db") && !object_name_ok("a.v1") && !object_name_ok(""));
        assert_eq!(versioned("db", 2), "db.v2");
    }

    #[test]
    fn status_converges_and_reports_failures() {
        let mut s = StackStatus {
            namespace: "forge-prod-shop".into(),
            services: vec![StackService { name: "api".into(), desired: 2, running: 1, ..Default::default() }],
        };
        assert!(!s.converged());
        s.services[0].running = 2;
        assert!(s.converged() && s.failure().is_none());
        s.services[0].update_state = "rollback_completed".into();
        assert!(s.failure().unwrap().contains("rollback completed"));
    }
}
