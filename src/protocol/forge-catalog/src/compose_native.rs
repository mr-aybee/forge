//! Compose / Swarm format services files run by Forge itself
//! (`plans/service-orchestration.md` step 3, D-178).
//!
//! Every entry under `services:` becomes one native Forge service: an
//! [`AppSpec`](crate::AppSpec)-shaped [`NativeSpec`], plain environment
//! variables, and the [`Orchestration`] that says where and how many
//! copies run (`deploy.replicas`, `mode: global`, constraints, spread,
//! reservations, update and rollback settings, `depends_on`). No `docker
//! compose` and no Swarm are involved.
//!
//! Like `forge.stack.yaml` (D-174), the file is read strictly: a key Forge
//! does not map is refused with its dotted path (`services.web.build: …`),
//! never ignored. [`export`] writes the same format back, so the console,
//! the CLI and the file describe one thing.
//!
//! This module validates a parsed [`Value`]; the server and the CLI read
//! the YAML with [`crate::yaml::parse_document`].

use std::collections::{BTreeMap, BTreeSet};
use std::net::IpAddr;

use forge_core::model::NodeFailurePolicy;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::apps::{HealthCheck, Runtime};
use crate::container_opts::{ContainerOptions, Device, RestartPolicy, Tmpfs, Ulimit};
use crate::deploy::DocError;
use crate::placement::{Count, Rules};
use crate::secret_files::{FileKind, FileRef};
use crate::swarm_service::{FailureAction, ServiceMode, UpdateOrder, UpdatePolicy};

/// Most services one file may hold.
pub const MAX_SERVICES: usize = 100;
/// The key of [`Orchestration`] in a stored service spec.
pub const SPEC_KEY: &str = "orchestration";
/// Why a top-level secret or config with `file:` (or inline content) is refused.
pub const FILE_REFUSED: &str = "the server never reads files: store the value as a credential vault item and declare it `external: true` (with `name: <vault item>` when the names differ)";

type Res<T> = Result<T, DocError>;

fn err(path: &str, message: impl Into<String>) -> DocError {
    DocError::new(path, message)
}

/// A parsed services file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServicesFile {
    /// Top-level `name`, if set.
    pub name: Option<String>,
    /// The services, a service after the ones it `depends_on`.
    pub services: Vec<FileService>,
}

/// One entry under `services:`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileService {
    pub name: String,
    pub runtime: Runtime,
    pub spec: NativeSpec,
    /// Plain environment variables, in file order.
    pub env: Vec<(String, String)>,
    pub orchestration: Orchestration,
    /// `x-forge.on_node_failure`.
    pub on_node_failure: Option<NodeFailurePolicy>,
    /// `x-forge.mesh: false` opts out of the cluster container network.
    pub mesh: Option<bool>,
}

/// The runtime fields of a service a file sets. Field names are the ones
/// of the stored service spec, so a stored spec reads back into it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeSpec {
    #[serde(default)]
    pub image: String,
    /// Run through `/bin/sh -c` (containers) or `exec`-ed (native).
    #[serde(default)]
    pub command: String,
    /// Words separated by spaces.
    #[serde(default)]
    pub entrypoint: String,
    #[serde(default)]
    pub working_dir: String,
    /// Native services: the port the app listens on.
    #[serde(default)]
    pub listen_port: Option<u16>,
    /// `[ip:]host:container[/udp]`.
    #[serde(default)]
    pub ports: Vec<String>,
    /// `source:/target[:ro]`.
    #[serde(default)]
    pub volumes: Vec<String>,
    #[serde(default)]
    pub cpu_millis: u32,
    #[serde(default)]
    pub memory_mb: u32,
    #[serde(default)]
    pub user: String,
    #[serde(default)]
    pub hostname: String,
    #[serde(default)]
    pub labels: Vec<(String, String)>,
    /// `name:ipv4`.
    #[serde(default)]
    pub extra_hosts: Vec<String>,
    #[serde(default)]
    pub healthcheck: Option<HealthCheck>,
    #[serde(default)]
    pub stop_timeout_secs: u32,
    #[serde(default)]
    pub read_only: bool,
    /// The older restart flag; `restart_policy` wins when set.
    #[serde(default = "yes")]
    pub restart: bool,
    #[serde(flatten)]
    pub options: ContainerOptions,
    /// Secret and config files: references to vault items, never values (D-179).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secret_files: Vec<FileRef>,
}

impl Default for NativeSpec {
    fn default() -> Self {
        Self {
            image: String::new(),
            command: String::new(),
            entrypoint: String::new(),
            working_dir: String::new(),
            listen_port: None,
            ports: Vec::new(),
            volumes: Vec::new(),
            cpu_millis: 0,
            memory_mb: 0,
            user: String::new(),
            hostname: String::new(),
            labels: Vec::new(),
            extra_hosts: Vec::new(),
            healthcheck: None,
            stop_timeout_secs: 0,
            read_only: false,
            restart: true,
            options: ContainerOptions::default(),
            secret_files: Vec::new(),
        }
    }
}

const fn yes() -> bool {
    true
}

const fn one() -> u32 {
    1
}

#[allow(clippy::trivially_copy_pass_by_ref)] // serde's skip_serializing_if passes a reference
fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// Where and how a file-managed service runs, stored in its spec under
/// [`SPEC_KEY`]. One copy per node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Orchestration {
    #[serde(default)]
    pub mode: ServiceMode,
    /// Nodes in replicated mode (`deploy.replicas`).
    #[serde(default = "one")]
    pub replicas: u32,
    /// `node.hostname==…` / `node.id==…`: only these nodes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nodes: Vec<String>,
    /// `node.labels.<key>==<value>`: node tags a node must have.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub node_tags: BTreeMap<String, String>,
    /// `preferences: - spread: node.labels.<key>`: the tag key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spread: Option<String>,
    /// `deploy.resources.reservations.cpus`, thousandths of a core.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub reserve_cpu_millis: u32,
    /// `deploy.resources.reservations.memory`, MiB.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub reserve_memory_mb: u32,
    /// `deploy.update_config`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update: Option<UpdatePolicy>,
    /// `deploy.rollback_config`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rollback: Option<UpdatePolicy>,
    /// `depends_on`: services applied (and deployed) before this one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
}

impl Default for Orchestration {
    fn default() -> Self {
        Self {
            mode: ServiceMode::Replicated,
            replicas: 1,
            nodes: Vec::new(),
            node_tags: BTreeMap::new(),
            spread: None,
            reserve_cpu_millis: 0,
            reserve_memory_mb: 0,
            update: None,
            rollback: None,
            depends_on: Vec::new(),
        }
    }
}

impl Orchestration {
    /// The placer's rules for this service.
    #[must_use]
    pub fn rules(&self) -> Rules<'_> {
        Rules {
            nodes: &self.nodes,
            tags: &self.node_tags,
            count: match self.mode {
                ServiceMode::Global => Count::Global,
                ServiceMode::Replicated => Count::Replicas(self.replicas),
            },
            spread: self.spread.as_deref(),
            reserve_cpu_millis: self.reserve_cpu_millis,
            reserve_memory_mb: self.reserve_memory_mb,
        }
    }
}

// ---- reading ---------------------------------------------------------------------------

const TOP_KEYS: &[&str] = &["services", "name", "version", "volumes", "networks", "secrets", "configs"];

const SERVICE_KEYS: &[&str] = &[
    "image",
    "command",
    "entrypoint",
    "working_dir",
    "environment",
    "ports",
    "volumes",
    "healthcheck",
    "user",
    "hostname",
    "labels",
    "extra_hosts",
    "cap_add",
    "cap_drop",
    "ulimits",
    "tmpfs",
    "dns",
    "sysctls",
    "devices",
    "init",
    "read_only",
    "stop_grace_period",
    "networks",
    "restart",
    "deploy",
    "depends_on",
    "secrets",
    "configs",
    "x-forge",
];

/// Service keys refused with a reason of their own.
fn refused_service_key(key: &str, v: &Value) -> Option<&'static str> {
    Some(match key {
        "build" => "Forge runs images: build and push the image, then set `image`",
        "privileged" => "privileged containers are not allowed; add the capabilities it needs with cap_add",
        "network_mode" if v.as_str() == Some("host") => "host networking is not allowed; publish ports instead",
        "network_mode" => "not supported: services join the cluster container network",
        "env_file" => {
            "the server never reads files: `forge services apply` inlines env_file; elsewhere list the variables under environment"
        }
        "container_name" => "Forge names containers itself",
        "links" | "external_links" => "services reach each other by name through the cluster DNS",
        "pid" | "ipc" | "userns_mode" | "cgroup_parent" | "uts" => "host namespaces are not allowed",
        _ => return None,
    })
}

/// Reads a services file (already parsed from YAML).
pub fn parse(tree: &Value) -> Res<ServicesFile> {
    let top = tree.as_object().ok_or_else(|| err("", "a services file is a mapping with `services:`"))?;

    for (k, _) in top {
        if !TOP_KEYS.contains(&k.as_str()) && !k.starts_with("x-") {
            return Err(err(k, "unknown key; see the services file reference"));
        }
    }

    let name = match top.get("name") {
        None | Some(Value::Null) => None,
        Some(v) => {
            let n = string(v, "name")?;

            if !crate::apps::valid_slug(&n, 40) {
                return Err(err("name", "lowercase letters, digits and dashes (max 40)"));
            }

            Some(n)
        }
    };

    check_volumes(top.get("volumes"))?;
    let declared = Declared {
        networks: top_networks(top.get("networks"))?,
        secrets: top_objects(top.get("secrets"), FileKind::Secret)?,
        configs: top_objects(top.get("configs"), FileKind::Config)?,
    };
    let services = top
        .get("services")
        .and_then(Value::as_object)
        .ok_or_else(|| err("services", "required: a mapping of services"))?;

    if services.is_empty() {
        return Err(err("services", "lists no service"));
    }

    if services.len() > MAX_SERVICES {
        return Err(err("services", format!("at most {MAX_SERVICES} services")));
    }

    let mut parsed = Vec::with_capacity(services.len());

    for (n, v) in services {
        parsed.push(service(n, v, &declared)?);
    }

    let services = dependency_order(parsed)?;

    Ok(ServicesFile { name, services })
}

/// Top-level `volumes:` declares named volumes; Forge creates them on the
/// node on first use, so only `external: true` (or nothing) is accepted.
fn check_volumes(v: Option<&Value>) -> Res<()> {
    let Some(v) = v.filter(|v| !v.is_null()) else { return Ok(()) };
    let m = v.as_object().ok_or_else(|| err("volumes", "a mapping of volume names"))?;

    for (name, def) in m {
        let path = format!("volumes.{name}");

        if !crate::swarm_service::volume_name_ok(name) {
            return Err(err(&path, "invalid volume name"));
        }

        match def {
            Value::Null => {}
            Value::Object(d) => {
                for (k, x) in d {
                    if k != "external" {
                        return Err(err(&format!("{path}.{k}"), "not supported: volumes use the node's local driver"));
                    }

                    boolean(x, &format!("{path}.{k}"))?;
                }
            }
            _ => return Err(err(&path, "a mapping or empty")),
        }
    }

    Ok(())
}

/// Top-level `networks:`: existing Docker networks (`external: true`),
/// optionally under another `name`. Returns compose name → Docker name.
fn top_networks(v: Option<&Value>) -> Res<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    let Some(v) = v.filter(|v| !v.is_null()) else { return Ok(out) };
    let m = v.as_object().ok_or_else(|| err("networks", "a mapping of network names"))?;

    for (name, def) in m {
        let path = format!("networks.{name}");
        let d =
            def.as_object().ok_or_else(|| err(&path, "set external: true (Forge connects to existing networks)"))?;
        let mut docker = name.clone();
        let mut external = false;

        for (k, x) in d {
            match k.as_str() {
                "external" => external = boolean(x, &format!("{path}.external"))?,
                "name" => docker = string(x, &format!("{path}.name"))?,
                _ => {
                    return Err(err(
                        &format!("{path}.{k}"),
                        "not supported: Forge connects to existing networks; services share the cluster container network",
                    ));
                }
            }
        }

        if !external {
            return Err(err(&format!("{path}.external"), "must be true: Forge connects to existing networks"));
        }

        out.insert(name.clone(), docker);
    }

    Ok(out)
}

/// What the top level declares: compose name → Docker network, and
/// compose name → credential vault item for secrets and configs.
struct Declared {
    networks: BTreeMap<String, String>,
    secrets: BTreeMap<String, String>,
    configs: BTreeMap<String, String>,
}

impl Declared {
    fn objects(&self, kind: FileKind) -> &BTreeMap<String, String> {
        match kind {
            FileKind::Secret => &self.secrets,
            FileKind::Config => &self.configs,
        }
    }
}

/// Top-level `secrets:` or `configs:`: each `external: true`, naming a
/// credential vault item (its own key unless `name` says otherwise), as on
/// the Swarm path (D-171). `file:`, `environment:` and `content:` are
/// refused: the server never reads files and values never sit in a file.
fn top_objects(v: Option<&Value>, kind: FileKind) -> Res<BTreeMap<String, String>> {
    let section = kind.section();
    let mut out = BTreeMap::new();
    let Some(v) = v.filter(|v| !v.is_null()) else { return Ok(out) };
    let m = v.as_object().ok_or_else(|| err(section, "a mapping of names"))?;

    for (key, def) in m {
        let path = format!("{section}.{key}");

        if !object_key_ok(key) {
            return Err(err(&path, "names use letters, digits, `.`, `_` and `-`"));
        }

        let d = def.as_object().ok_or_else(|| err(&path, "set external: true and name the vault item"))?;
        let mut item = key.clone();
        let mut external = false;

        for (k, x) in d {
            match k.as_str() {
                "external" => external = boolean(x, &format!("{path}.external"))?,
                "name" => item = string(x, &format!("{path}.name"))?,
                "file" | "environment" | "content" => return Err(err(&format!("{path}.{k}"), FILE_REFUSED)),
                _ => return Err(err(&format!("{path}.{k}"), "not supported: only `external` and `name`")),
            }
        }

        if !external {
            return Err(err(&format!("{path}.external"), "must be true: the value lives in the credential vault"));
        }

        out.insert(key.clone(), item);
    }

    Ok(out)
}

fn object_key_ok(key: &str) -> bool {
    !key.is_empty() && key.len() <= 64 && key.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

/// A service's `secrets:` or `configs:` list: short (`- name`) or long
/// syntax (`source`, `target`, `mode`, `uid`, `gid`).
fn service_objects(v: &Value, path: &str, kind: FileKind, declared: &Declared) -> Res<Vec<FileRef>> {
    let mut out = Vec::new();

    for (i, e) in list(v, path)?.iter().enumerate() {
        let at = format!("{path}[{i}]");
        let mut target = None;
        let mut mode = None;
        let mut uid = None;
        let mut gid = None;

        let source = match e {
            Value::String(s) => s.clone(),
            Value::Object(m) => {
                let mut source = None;

                for (k, x) in m {
                    let p = format!("{at}.{k}");

                    match k.as_str() {
                        "source" => source = Some(string(x, &p)?),
                        "target" => target = Some(string(x, &p)?),
                        "mode" => mode = Some(file_mode(x, &p)?),
                        "uid" => uid = Some(uint(x, &p)?),
                        "gid" => gid = Some(uint(x, &p)?),
                        _ => return Err(err(&p, "unknown key; use source, target, mode, uid or gid")),
                    }
                }

                source.ok_or_else(|| err(&format!("{at}.source"), "required"))?
            }
            _ => return Err(err(&at, "a name or a mapping with `source`")),
        };

        let item = declared.objects(kind).get(&source).ok_or_else(|| {
            err(&at, format!("`{source}` is not declared under the top-level {} (external: true)", kind.section()))
        })?;

        let r = FileRef {
            kind,
            item: item.clone(),
            target: FileRef::target_for(kind, &source, target.as_deref()),
            mode,
            uid,
            gid,
        };
        r.validate().map_err(|m| err(&at, m))?;
        out.push(r);
    }

    Ok(out)
}

/// A file mode: octal text (`"0440"`, which is how a leading zero reads)
/// or a number (YAML `0o440`).
fn file_mode(v: &Value, path: &str) -> Res<u32> {
    let n = match v {
        Value::String(s) => u32::from_str_radix(s.trim().trim_start_matches("0o"), 8).ok(),
        _ => v.as_u64().and_then(|n| u32::try_from(n).ok()),
    };

    n.filter(|n| *n <= 0o777).ok_or_else(|| err(path, "octal permission bits, for example `0440`"))
}

fn service(name: &str, v: &Value, declared: &Declared) -> Res<FileService> {
    let base = format!("services.{name}");

    if !crate::apps::valid_slug(name, 40) {
        return Err(err(&base, "service names are lowercase letters, digits and dashes (max 40)"));
    }

    let m = v.as_object().ok_or_else(|| err(&base, "a mapping"))?;

    for (k, x) in m {
        if let Some(why) = refused_service_key(k, x) {
            return Err(err(&format!("{base}.{k}"), why));
        }

        if !SERVICE_KEYS.contains(&k.as_str()) && !k.starts_with("x-") {
            return Err(err(&format!("{base}.{k}"), "unknown key; see the services file reference"));
        }
    }

    let p = |k: &str| format!("{base}.{k}");
    let mut spec = NativeSpec::default();
    let mut orch = Orchestration::default();
    let mut runtime = Runtime::Container;
    let mut on_node_failure = None;
    let mut mesh = None;

    if let Some(x) = m.get("x-forge") {
        let ext = x_forge(x, &p("x-forge"))?;
        runtime = ext.runtime;
        spec.listen_port = ext.listen_port;
        on_node_failure = ext.on_node_failure;
        mesh = ext.mesh;
    }

    for (k, x) in m {
        let path = p(k);

        match k.as_str() {
            "image" => spec.image = string(x, &path)?,
            "command" => spec.command = command(x, &path)?,
            "entrypoint" => spec.entrypoint = entrypoint(x, &path)?,
            "working_dir" => spec.working_dir = string(x, &path)?,
            "ports" => {
                spec.ports = list(x, &path)?
                    .iter()
                    .enumerate()
                    .map(|(i, e)| port(e, &format!("{path}[{i}]")))
                    .collect::<Res<_>>()?
            }
            "volumes" => {
                for (i, e) in list(x, &path)?.iter().enumerate() {
                    match volume(e, &format!("{path}[{i}]"))? {
                        Mount::Volume(s) => spec.volumes.push(s),
                        Mount::Tmpfs(t) => spec.options.tmpfs.push(t),
                    }
                }
            }
            "healthcheck" => spec.healthcheck = healthcheck(x, &path)?,
            "user" => spec.user = string(x, &path)?,
            "hostname" => spec.hostname = string(x, &path)?,
            "labels" => spec.labels = pairs(x, &path, '=')?,
            "extra_hosts" => spec.extra_hosts = extra_hosts(x, &path)?,
            "cap_add" => spec.options.cap_add = strings(x, &path)?,
            "cap_drop" => spec.options.cap_drop = strings(x, &path)?,
            "ulimits" => spec.options.ulimits = ulimits(x, &path)?,
            "tmpfs" => {
                for (i, s) in strings(x, &path)?.iter().enumerate() {
                    spec.options.tmpfs.push(tmpfs(s, &format!("{path}[{i}]"))?);
                }
            }
            "dns" => {
                spec.options.dns = strings(x, &path)?
                    .iter()
                    .map(|s| s.parse::<IpAddr>().map_err(|_| err(&path, format!("`{s}` is not an IP address"))))
                    .collect::<Res<_>>()?;
            }
            "sysctls" => spec.options.sysctls = pairs(x, &path, '=')?,
            "devices" => spec.options.devices = devices(x, &path)?,
            "init" => spec.options.init = boolean(x, &path)?,
            "read_only" => spec.read_only = boolean(x, &path)?,
            "stop_grace_period" => spec.stop_timeout_secs = duration_secs(x, &path)?,
            "networks" => spec.options.networks = service_networks(x, &path, &declared.networks)?,
            "secrets" => spec.secret_files.extend(service_objects(x, &path, FileKind::Secret, declared)?),
            "configs" => spec.secret_files.extend(service_objects(x, &path, FileKind::Config, declared)?),
            "restart" => (spec.options.restart_policy, spec.options.restart_max_attempts) = restart(x, &path)?,
            "depends_on" => orch.depends_on = depends_on(x, &path)?,
            // environment, deploy and x-forge are read below; other `x-`
            // keys are Compose extension fields, ignored by design.
            _ => {}
        }
    }

    let env = match m.get("environment") {
        Some(x) => environment(x, &p("environment"))?,
        None => Vec::new(),
    };

    if let Some(d) = m.get("deploy") {
        deploy(d, &p("deploy"), &mut spec, &mut orch, m.contains_key("restart"))?;
    }

    crate::secret_files::validate_refs(&spec.secret_files).map_err(|m| err(&base, m))?;

    match runtime {
        Runtime::Container if spec.image.trim().is_empty() => return Err(err(&p("image"), "required")),
        Runtime::Systemd if !spec.image.is_empty() => {
            return Err(err(&p("image"), "a native (x-forge.runtime: systemd) service runs a command, not an image"));
        }
        Runtime::Systemd if spec.command.trim().is_empty() => {
            return Err(err(&p("command"), "required for a native service"));
        }
        _ => {}
    }

    Ok(FileService { name: name.to_owned(), runtime, spec, env, orchestration: orch, on_node_failure, mesh })
}

/// `x-forge` on a service.
struct Extensions {
    runtime: Runtime,
    listen_port: Option<u16>,
    on_node_failure: Option<NodeFailurePolicy>,
    mesh: Option<bool>,
}

fn x_forge(v: &Value, path: &str) -> Res<Extensions> {
    let m = v.as_object().ok_or_else(|| err(path, "a mapping"))?;
    let mut out = Extensions { runtime: Runtime::Container, listen_port: None, on_node_failure: None, mesh: None };

    for (k, x) in m {
        let p = format!("{path}.{k}");

        match k.as_str() {
            "runtime" => {
                out.runtime = match string(x, &p)?.as_str() {
                    "container" => Runtime::Container,
                    "systemd" => Runtime::Systemd,
                    _ => return Err(err(&p, "container or systemd")),
                };
            }
            "listen_port" => {
                let n = uint(x, &p)?;
                out.listen_port =
                    Some(u16::try_from(n).ok().filter(|n| *n > 0).ok_or_else(|| err(&p, "a port (1-65535)"))?);
            }
            "on_node_failure" => {
                out.on_node_failure = Some(match string(x, &p)?.as_str() {
                    "move" => NodeFailurePolicy::Move,
                    "alert" => NodeFailurePolicy::Alert,
                    _ => return Err(err(&p, "move or alert")),
                });
            }
            "mesh" => out.mesh = Some(boolean(x, &p)?),
            "strategy" | "health" | "console" | "ingress" | "nodes" | "artifact" => {
                return Err(err(
                    &p,
                    "not supported in a services file yet: use forge.deploy.yaml (strategy, health, artifact) or the console",
                ));
            }
            _ => return Err(err(&p, "unknown key; see the services file reference")),
        }
    }

    Ok(out)
}

fn deploy(v: &Value, path: &str, spec: &mut NativeSpec, orch: &mut Orchestration, has_restart: bool) -> Res<()> {
    let m = v.as_object().ok_or_else(|| err(path, "a mapping"))?;
    let mut replicas = None;

    for (k, x) in m {
        let p = format!("{path}.{k}");

        match k.as_str() {
            "mode" => {
                orch.mode = match string(x, &p)?.as_str() {
                    "replicated" => ServiceMode::Replicated,
                    "global" => ServiceMode::Global,
                    _ => return Err(err(&p, "replicated or global")),
                };
            }
            "replicas" => {
                let n = uint(x, &p)?;

                if !(1..=1000).contains(&n) {
                    return Err(err(&p, "1 to 1000 nodes (one copy per node; remove the service to stop it)"));
                }

                replicas = Some(n);
            }
            "placement" => placement(x, &p, orch)?,
            "resources" => resources(x, &p, spec, orch)?,
            "restart_policy" => {
                if has_restart {
                    return Err(err(&p, "set restart or deploy.restart_policy, not both"));
                }

                (spec.options.restart_policy, spec.options.restart_max_attempts) = restart_policy(x, &p)?;
            }
            "update_config" => orch.update = Some(update_policy(x, &p)?),
            "rollback_config" => orch.rollback = Some(update_policy(x, &p)?),
            "endpoint_mode" => {
                return Err(err(
                    &p,
                    "Forge has no virtual IPs or DNS round-robin: the cluster DNS name reaches the service",
                ));
            }
            "labels" => return Err(err(&p, "deploy.labels are Swarm service labels; use labels")),
            _ => return Err(err(&p, "unknown key; see the services file reference")),
        }
    }

    if let Some(n) = replicas {
        if orch.mode == ServiceMode::Global {
            return Err(err(
                &format!("{path}.replicas"),
                "a global service runs on every eligible node; remove replicas",
            ));
        }

        orch.replicas = n;
    }

    Ok(())
}

fn placement(v: &Value, path: &str, orch: &mut Orchestration) -> Res<()> {
    let m = v.as_object().ok_or_else(|| err(path, "a mapping"))?;

    for (k, x) in m {
        let p = format!("{path}.{k}");

        match k.as_str() {
            "constraints" => {
                for (i, c) in strings(x, &p)?.iter().enumerate() {
                    constraint(c, &format!("{p}[{i}]"), orch)?;
                }
            }
            "preferences" => {
                for (i, e) in list(x, &p)?.iter().enumerate() {
                    let ep = format!("{p}[{i}]");
                    let spread = e.get("spread").filter(|_| e.as_object().is_some_and(|o| o.len() == 1));
                    let key = spread
                        .map(|s| string(s, &format!("{ep}.spread")))
                        .transpose()?
                        .and_then(|s| s.strip_prefix("node.labels.").map(str::to_owned))
                        .ok_or_else(|| err(&ep, "only `spread: node.labels.<key>` is supported"))?;

                    if orch.spread.replace(key).is_some() {
                        return Err(err(&ep, "spread over one tag key at most"));
                    }
                }
            }
            "max_replicas_per_node" => {
                if uint(x, &p)? != 1 {
                    return Err(err(&p, "only 1: Forge runs one copy of a service per node"));
                }
            }
            _ => return Err(err(&p, "unknown key; see the services file reference")),
        }
    }

    Ok(())
}

fn constraint(c: &str, path: &str, orch: &mut Orchestration) -> Res<()> {
    if c.contains("!=") {
        return Err(err(path, "only == constraints are supported"));
    }

    let (lhs, rhs) = c.split_once("==").ok_or_else(|| err(path, "`<field> == <value>`"))?;
    let (lhs, rhs) = (lhs.trim(), rhs.trim());

    if rhs.is_empty() {
        return Err(err(path, "a value is required"));
    }

    if lhs == "node.hostname" || lhs == "node.id" {
        if !orch.nodes.iter().any(|n| n == rhs) {
            orch.nodes.push(rhs.to_owned());
        }

        return Ok(());
    }

    let key = lhs
        .strip_prefix("node.labels.")
        .filter(|k| !k.is_empty())
        .ok_or_else(|| err(path, "only node.hostname, node.id and node.labels.<key> constraints are supported"))?;

    if orch.node_tags.insert(key.to_owned(), rhs.to_owned()).is_some() {
        return Err(err(path, format!("node.labels.{key} is constrained twice")));
    }

    Ok(())
}

fn resources(v: &Value, path: &str, spec: &mut NativeSpec, orch: &mut Orchestration) -> Res<()> {
    let m = v.as_object().ok_or_else(|| err(path, "a mapping"))?;

    for (k, x) in m {
        let p = format!("{path}.{k}");
        let r = x.as_object().ok_or_else(|| err(&p, "a mapping"))?;

        let limits = match k.as_str() {
            "limits" => true,
            "reservations" => false,
            _ => return Err(err(&p, "unknown key; see the services file reference")),
        };

        for (rk, rv) in r {
            let rp = format!("{p}.{rk}");

            match (rk.as_str(), limits) {
                ("cpus", true) => spec.cpu_millis = cpus(rv, &rp)?,
                ("memory", true) => spec.memory_mb = memory_mb(rv, &rp)?,
                ("cpus", false) => orch.reserve_cpu_millis = cpus(rv, &rp)?,
                ("memory", false) => orch.reserve_memory_mb = memory_mb(rv, &rp)?,
                _ => return Err(err(&rp, "unknown key; see the services file reference")),
            }
        }
    }

    Ok(())
}

fn restart(v: &Value, path: &str) -> Res<(Option<RestartPolicy>, u32)> {
    let s = string(v, path)?;

    let parsed = match s.as_str() {
        "no" => (RestartPolicy::No, 0),
        "always" => (RestartPolicy::Always, 0),
        "unless-stopped" => (RestartPolicy::UnlessStopped, 0),
        "on-failure" => (RestartPolicy::OnFailure, 0),
        other => match other.strip_prefix("on-failure:").and_then(|n| n.parse::<u32>().ok()) {
            Some(n) => (RestartPolicy::OnFailure, n),
            None => return Err(err(path, "no, always, unless-stopped, on-failure or on-failure:<n>")),
        },
    };

    Ok((Some(parsed.0), parsed.1))
}

fn restart_policy(v: &Value, path: &str) -> Res<(Option<RestartPolicy>, u32)> {
    let m = v.as_object().ok_or_else(|| err(path, "a mapping"))?;
    let mut policy = RestartPolicy::Always;
    let mut attempts = 0;

    for (k, x) in m {
        let p = format!("{path}.{k}");

        match k.as_str() {
            "condition" => {
                policy = match string(x, &p)?.as_str() {
                    "none" => RestartPolicy::No,
                    "on-failure" => RestartPolicy::OnFailure,
                    "any" => RestartPolicy::Always,
                    _ => return Err(err(&p, "none, on-failure or any")),
                };
            }
            "max_attempts" => attempts = uint(x, &p)?,
            "delay" | "window" => {
                return Err(err(&p, "not supported: Docker's restart policy on a node has no delay or window"));
            }
            _ => return Err(err(&p, "unknown key; see the services file reference")),
        }
    }

    if attempts > 0 && policy != RestartPolicy::OnFailure {
        return Err(err(&format!("{path}.max_attempts"), "applies to condition: on-failure only"));
    }

    Ok((Some(policy), attempts))
}

fn update_policy(v: &Value, path: &str) -> Res<UpdatePolicy> {
    let m = v.as_object().ok_or_else(|| err(path, "a mapping"))?;
    let mut u = UpdatePolicy::default();

    for (k, x) in m {
        let p = format!("{path}.{k}");

        match k.as_str() {
            "parallelism" => u.parallelism = uint(x, &p)?,
            "delay" => u.delay_secs = duration_secs(x, &p)?,
            "monitor" => u.monitor_secs = duration_secs(x, &p)?,
            "order" => {
                u.order = match string(x, &p)?.as_str() {
                    "start-first" => UpdateOrder::StartFirst,
                    "stop-first" => UpdateOrder::StopFirst,
                    _ => return Err(err(&p, "start-first or stop-first")),
                };
            }
            "failure_action" => {
                u.failure_action = match string(x, &p)?.as_str() {
                    "rollback" => FailureAction::Rollback,
                    "pause" => FailureAction::Pause,
                    "continue" => FailureAction::Continue,
                    _ => return Err(err(&p, "rollback, pause or continue")),
                };
            }
            "max_failure_ratio" => {
                #[allow(clippy::cast_possible_truncation)] // a ratio between 0 and 1
                let r = x.as_f64().filter(|r| (0.0..=1.0).contains(r)).ok_or_else(|| err(&p, "a number from 0 to 1"))?
                    as f32;
                u.max_failure_ratio = r;
            }
            _ => return Err(err(&p, "unknown key; see the services file reference")),
        }
    }

    Ok(u)
}

fn depends_on(v: &Value, path: &str) -> Res<Vec<String>> {
    match v {
        Value::Object(m) => {
            let mut out = Vec::new();

            for (name, cond) in m {
                let p = format!("{path}.{name}");

                if let Some(c) = cond.as_object() {
                    for (k, x) in c {
                        let ok = k == "condition"
                            && matches!(
                                x.as_str(),
                                Some("service_started" | "service_healthy" | "service_completed_successfully")
                            );

                        if !ok {
                            return Err(err(
                                &format!("{p}.{k}"),
                                "only condition (service_started, service_healthy or service_completed_successfully)",
                            ));
                        }
                    }
                } else if !cond.is_null() {
                    return Err(err(&p, "a mapping with condition"));
                }

                out.push(name.clone());
            }

            Ok(out)
        }
        _ => strings(v, path),
    }
}

/// Orders services so each comes after the ones it depends on; refuses
/// unknown names and cycles.
fn dependency_order(services: Vec<FileService>) -> Res<Vec<FileService>> {
    let names: BTreeSet<String> = services.iter().map(|s| s.name.clone()).collect();

    for s in &services {
        for d in &s.orchestration.depends_on {
            if !names.contains(d) {
                return Err(err(&format!("services.{}.depends_on", s.name), format!("no service `{d}` in this file")));
            }
        }
    }

    let mut done: BTreeSet<String> = BTreeSet::new();
    let mut out = Vec::with_capacity(services.len());
    let mut rest = services;

    while !rest.is_empty() {
        let (ready, waiting): (Vec<FileService>, Vec<FileService>) =
            rest.into_iter().partition(|s| s.orchestration.depends_on.iter().all(|d| done.contains(d)));

        if ready.is_empty() {
            let first = waiting.first().map(|s| s.name.clone()).unwrap_or_default();

            return Err(err(&format!("services.{first}.depends_on"), "the services depend on each other in a cycle"));
        }

        for s in ready {
            done.insert(s.name.clone());
            out.push(s);
        }

        rest = waiting;
    }

    Ok(out)
}

fn environment(v: &Value, path: &str) -> Res<Vec<(String, String)>> {
    match v {
        Value::Object(m) => m
            .iter()
            .map(|(k, x)| {
                if x.is_null() {
                    return Err(err(&format!("{path}.{k}"), "give a value: Forge does not read the shell environment"));
                }

                Ok((k.clone(), string(x, &format!("{path}.{k}"))?))
            })
            .collect(),
        Value::Array(a) => a
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let p = format!("{path}[{i}]");
                let s = string(e, &p)?;

                s.split_once('=')
                    .map(|(k, v)| (k.to_owned(), v.to_owned()))
                    .ok_or_else(|| err(&p, "`KEY=value`: Forge does not read the shell environment"))
            })
            .collect(),
        _ => Err(err(path, "a mapping or a list of KEY=value")),
    }
}

fn command(v: &Value, path: &str) -> Res<String> {
    match v {
        Value::Array(a) => {
            let words: Vec<String> =
                a.iter().enumerate().map(|(i, w)| string(w, &format!("{path}[{i}]"))).collect::<Res<_>>()?;

            Ok(words.iter().map(|w| shell_quote(w)).collect::<Vec<_>>().join(" "))
        }
        _ => string(v, path),
    }
}

fn entrypoint(v: &Value, path: &str) -> Res<String> {
    match v {
        Value::Array(a) => {
            let words: Vec<String> =
                a.iter().enumerate().map(|(i, w)| string(w, &format!("{path}[{i}]"))).collect::<Res<_>>()?;

            if words.iter().any(|w| w.is_empty() || w.contains(char::is_whitespace)) {
                return Err(err(path, "entrypoint words may not be empty or contain spaces"));
            }

            Ok(words.join(" "))
        }
        _ => string(v, path),
    }
}

/// `word` for `/bin/sh`: as is when it is plain, else single-quoted.
fn shell_quote(word: &str) -> String {
    let plain = !word.is_empty() && word.bytes().all(|b| b.is_ascii_alphanumeric() || b"_./:=@%+,-".contains(&b));

    if plain { word.to_owned() } else { format!("'{}'", word.replace('\'', r"'\''")) }
}

fn port(v: &Value, path: &str) -> Res<String> {
    if let Some(m) = v.as_object() {
        let mut target = None;
        let mut published = None;
        let mut proto = "tcp".to_owned();
        let mut ip = String::new();

        for (k, x) in m {
            let p = format!("{path}.{k}");

            match k.as_str() {
                "target" => target = Some(uint(x, &p)?),
                "published" => published = Some(string(x, &p)?),
                "protocol" => proto = string(x, &p)?,
                "host_ip" => ip = string(x, &p)?,
                "mode" if x.as_str() == Some("host") => {}
                "mode" => return Err(err(&p, "only host: Forge has no routing mesh")),
                _ => return Err(err(&p, "unknown key; see the services file reference")),
            }
        }

        let target = target.ok_or_else(|| err(&format!("{path}.target"), "required"))?;
        let published =
            published.ok_or_else(|| err(&format!("{path}.published"), "required: Forge has no routing mesh"))?;
        let host = if ip.is_empty() { published } else { format!("{ip}:{published}") };
        let suffix = if proto == "tcp" { String::new() } else { format!("/{proto}") };

        return Ok(format!("{host}:{target}{suffix}"));
    }

    let s = string(v, path)?;
    let body = s.split_once('/').map_or(s.as_str(), |(b, _)| b);

    if body.contains('-') {
        return Err(err(path, "port ranges are not supported"));
    }

    if !body.contains(':') {
        return Err(err(path, "publish as host:container (Forge has no routing mesh)"));
    }

    Ok(s.strip_suffix("/tcp").map_or_else(|| s.clone(), str::to_owned))
}

/// What a `volumes` entry becomes.
enum Mount {
    Volume(String),
    Tmpfs(Tmpfs),
}

fn volume(v: &Value, path: &str) -> Res<Mount> {
    if let Some(m) = v.as_object() {
        let mut kind = String::new();
        let mut source = String::new();
        let mut target = String::new();
        let mut read_only = false;
        let mut size_mb = 0;

        for (k, x) in m {
            let p = format!("{path}.{k}");

            match k.as_str() {
                "type" => kind = string(x, &p)?,
                "source" => source = string(x, &p)?,
                "target" => target = string(x, &p)?,
                "read_only" => read_only = boolean(x, &p)?,
                "tmpfs" => {
                    for (tk, tv) in x.as_object().ok_or_else(|| err(&p, "a mapping"))? {
                        if tk != "size" {
                            return Err(err(&format!("{p}.{tk}"), "only size is supported"));
                        }

                        size_mb = memory_mb(tv, &format!("{p}.size"))?;
                    }
                }
                _ => return Err(err(&p, "unknown key; see the services file reference")),
            }
        }

        if target.is_empty() {
            return Err(err(&format!("{path}.target"), "required"));
        }

        return match kind.as_str() {
            "tmpfs" => Ok(Mount::Tmpfs(Tmpfs { target, size_mb })),
            "bind" | "volume" => {
                let s = format!("{source}:{target}{}", if read_only { ":ro" } else { "" });

                short_volume(&s, path).map(Mount::Volume)
            }
            _ => Err(err(&format!("{path}.type"), "bind, volume or tmpfs")),
        };
    }

    short_volume(&string(v, path)?, path).map(Mount::Volume)
}

fn short_volume(s: &str, path: &str) -> Res<String> {
    let source = s.split(':').next().unwrap_or_default();

    if !s.contains(':') || source.is_empty() {
        return Err(err(path, "anonymous volumes are not supported: name the volume (`data:/path`)"));
    }

    if source.starts_with('.') || source.starts_with('~') {
        return Err(err(path, "relative host paths are not supported: use an absolute path"));
    }

    Ok(s.strip_suffix(":rw").unwrap_or(s).to_owned())
}

fn healthcheck(v: &Value, path: &str) -> Res<Option<HealthCheck>> {
    let m = v.as_object().ok_or_else(|| err(path, "a mapping"))?;
    let mut hc =
        HealthCheck { command: String::new(), interval_secs: 30, timeout_secs: 5, retries: 3, start_period_secs: 0 };
    let mut disabled = false;

    for (k, x) in m {
        let p = format!("{path}.{k}");

        match k.as_str() {
            "test" => match x {
                Value::Array(a) => {
                    let words: Vec<String> =
                        a.iter().enumerate().map(|(i, w)| string(w, &format!("{p}[{i}]"))).collect::<Res<_>>()?;

                    match words.split_first() {
                        Some((first, _)) if first == "NONE" => disabled = true,
                        Some((first, rest)) if first == "CMD-SHELL" => hc.command = rest.join(" "),
                        Some((first, rest)) if first == "CMD" => {
                            hc.command = rest.iter().map(|w| shell_quote(w)).collect::<Vec<_>>().join(" ");
                        }
                        _ => return Err(err(&p, "a command, or a list starting with CMD, CMD-SHELL or NONE")),
                    }
                }
                _ => {
                    let s = string(x, &p)?;

                    if s == "NONE" {
                        disabled = true;
                    } else {
                        hc.command = s;
                    }
                }
            },
            "interval" => hc.interval_secs = duration_secs(x, &p)?,
            "timeout" => hc.timeout_secs = duration_secs(x, &p)?,
            "retries" => hc.retries = uint(x, &p)?,
            "start_period" => hc.start_period_secs = duration_secs(x, &p)?,
            "disable" => disabled = boolean(x, &p)?,
            _ => return Err(err(&p, "unknown key; see the services file reference")),
        }
    }

    if disabled {
        return Ok(None);
    }

    if hc.command.trim().is_empty() {
        return Err(err(&format!("{path}.test"), "required"));
    }

    Ok(Some(hc))
}

fn extra_hosts(v: &Value, path: &str) -> Res<Vec<String>> {
    let entries = match v {
        Value::Object(_) => pairs(v, path, '=')?,
        _ => strings(v, path)?
            .iter()
            .map(|s| {
                // `name:ip` (Compose) or `name=ip` (newer Compose).
                let (n, ip) = s
                    .split_once('=')
                    .or_else(|| s.split_once(':'))
                    .ok_or_else(|| err(path, format!("`{s}`: name:ip")))?;

                Ok((n.to_owned(), ip.to_owned()))
            })
            .collect::<Res<_>>()?,
    };

    Ok(entries.into_iter().map(|(n, ip)| format!("{n}:{ip}")).collect())
}

fn ulimits(v: &Value, path: &str) -> Res<Vec<Ulimit>> {
    let m = v.as_object().ok_or_else(|| err(path, "a mapping of ulimit names"))?;
    let num = |x: &Value, p: &str| x.as_i64().ok_or_else(|| err(p, "a whole number"));

    m.iter()
        .map(|(name, x)| {
            let p = format!("{path}.{name}");

            let (soft, hard) = match x {
                Value::Object(o) => {
                    for k in o.keys() {
                        if k != "soft" && k != "hard" {
                            return Err(err(&format!("{p}.{k}"), "soft or hard"));
                        }
                    }

                    let soft =
                        num(o.get("soft").ok_or_else(|| err(&format!("{p}.soft"), "required"))?, &format!("{p}.soft"))?;
                    let hard =
                        num(o.get("hard").ok_or_else(|| err(&format!("{p}.hard"), "required"))?, &format!("{p}.hard"))?;

                    (soft, hard)
                }
                _ => {
                    let n = num(x, &p)?;

                    (n, n)
                }
            };

            Ok(Ulimit { name: name.clone(), soft, hard })
        })
        .collect()
}

fn tmpfs(s: &str, path: &str) -> Res<Tmpfs> {
    let (target, opts) = s.split_once(':').unwrap_or((s, ""));
    let mut size_mb = 0;

    for o in opts.split(',').filter(|o| !o.is_empty()) {
        let size = o.strip_prefix("size=").ok_or_else(|| err(path, format!("`{o}`: only size= is supported")))?;
        size_mb = memory_mb(&Value::String(size.to_owned()), path)?;
    }

    Ok(Tmpfs { target: target.to_owned(), size_mb })
}

fn devices(v: &Value, path: &str) -> Res<Vec<Device>> {
    list(v, path)?
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let p = format!("{path}[{i}]");

            if let Some(m) = e.as_object() {
                let mut d = Device { host: String::new(), container: String::new(), permissions: String::new() };

                for (k, x) in m {
                    match k.as_str() {
                        "source" => d.host = string(x, &format!("{p}.source"))?,
                        "target" => d.container = string(x, &format!("{p}.target"))?,
                        "permissions" => d.permissions = string(x, &format!("{p}.permissions"))?,
                        _ => return Err(err(&format!("{p}.{k}"), "source, target or permissions")),
                    }
                }

                return Ok(d);
            }

            let s = string(e, &p)?;
            let mut parts = s.splitn(3, ':');
            let host = parts.next().unwrap_or_default().to_owned();
            let container = parts.next().unwrap_or_default().to_owned();
            let permissions = parts.next().unwrap_or_default().to_owned();

            Ok(Device { host, container, permissions })
        })
        .collect()
}

fn service_networks(v: &Value, path: &str, declared: &BTreeMap<String, String>) -> Res<Vec<String>> {
    let names: Vec<String> = match v {
        Value::Object(m) => {
            for (k, x) in m {
                if !x.is_null() && x.as_object().is_none_or(|o| !o.is_empty()) {
                    return Err(err(&format!("{path}.{k}"), "network options (aliases, addresses) are not supported"));
                }
            }

            m.keys().cloned().collect()
        }
        _ => strings(v, path)?,
    };

    let mut out = Vec::new();

    for n in names.iter().filter(|n| *n != "default") {
        let docker = declared
            .get(n)
            .ok_or_else(|| err(path, format!("`{n}` is not declared under the top-level networks (external: true)")))?;
        out.push(docker.clone());
    }

    Ok(out)
}

// ---- scalar helpers ----------------------------------------------------------------------

fn list<'a>(v: &'a Value, path: &str) -> Res<&'a Vec<Value>> {
    v.as_array().ok_or_else(|| err(path, "a list"))
}

/// A scalar as text (numbers and booleans written as YAML spells them).
fn string(v: &Value, path: &str) -> Res<String> {
    match v {
        Value::String(s) => Ok(s.clone()),
        Value::Number(n) => Ok(n.to_string()),
        Value::Bool(b) => Ok(b.to_string()),
        _ => Err(err(path, "a single value")),
    }
}

/// One value or a list of values.
fn strings(v: &Value, path: &str) -> Res<Vec<String>> {
    match v {
        Value::Array(a) => a.iter().enumerate().map(|(i, e)| string(e, &format!("{path}[{i}]"))).collect(),
        _ => Ok(vec![string(v, path)?]),
    }
}

/// A mapping, or a list of `key<sep>value`.
fn pairs(v: &Value, path: &str, sep: char) -> Res<Vec<(String, String)>> {
    match v {
        Value::Object(m) => m.iter().map(|(k, x)| Ok((k.clone(), string(x, &format!("{path}.{k}"))?))).collect(),
        Value::Array(_) => strings(v, path)?
            .iter()
            .map(|s| {
                s.split_once(sep)
                    .map(|(k, v)| (k.to_owned(), v.to_owned()))
                    .ok_or_else(|| err(path, format!("`{s}`: key{sep}value")))
            })
            .collect(),
        _ => Err(err(path, format!("a mapping or a list of key{sep}value"))),
    }
}

fn boolean(v: &Value, path: &str) -> Res<bool> {
    v.as_bool().ok_or_else(|| err(path, "true or false"))
}

fn uint(v: &Value, path: &str) -> Res<u32> {
    let n = match v {
        Value::String(s) => s.trim().parse::<u64>().ok(),
        _ => v.as_u64(),
    };

    n.and_then(|n| u32::try_from(n).ok()).ok_or_else(|| err(path, "a whole number"))
}

/// A Compose duration (`1m30s`, `10s`, `500ms`, `2h`) or a number of
/// seconds, rounded up to whole seconds.
pub fn duration_secs(v: &Value, path: &str) -> Res<u32> {
    if let Some(n) = v.as_u64() {
        return u32::try_from(n).map_err(|_| err(path, "too long"));
    }

    let s = string(v, path)?;
    let mut total_ms: u64 = 0;
    let mut num = String::new();
    let mut chars = s.trim().chars().peekable();
    let bad = || err(path, format!("`{s}` is not a duration (e.g. 30s, 1m30s, 500ms)"));

    if s.trim().is_empty() {
        return Err(bad());
    }

    while let Some(c) = chars.next() {
        if c.is_ascii_digit() {
            num.push(c);
            continue;
        }

        let unit = match (c, chars.peek()) {
            ('m', Some('s')) => {
                chars.next();
                1
            }
            ('u' | 'µ', Some('s')) | ('n', Some('s')) => {
                chars.next();
                0
            }
            ('h', _) => 3_600_000,
            ('m', _) => 60_000,
            ('s', _) => 1000,
            _ => return Err(bad()),
        };

        let n: u64 = num.parse().map_err(|_| bad())?;
        total_ms = total_ms.saturating_add(n.saturating_mul(unit));
        num.clear();
    }

    if !num.is_empty() {
        return Err(bad());
    }

    u32::try_from(total_ms.div_ceil(1000)).map_err(|_| err(path, "too long"))
}

/// A Docker memory size (`512M`, `1g`, `256mb`, `1073741824`) in MiB,
/// rounded up.
pub fn memory_mb(v: &Value, path: &str) -> Res<u32> {
    let s = string(v, path)?.trim().to_ascii_lowercase();
    let digits: String = s.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    let unit = s[digits.len()..].trim_end_matches('b').trim_end_matches('i');
    let bad = || err(path, format!("`{s}` is not a size (e.g. 512M, 1G)"));
    let n: f64 = digits.parse().map_err(|_| bad())?;

    let bytes = match unit {
        "" => n,
        "k" => n * 1024.0,
        "m" => n * 1024.0 * 1024.0,
        "g" => n * 1024.0 * 1024.0 * 1024.0,
        _ => return Err(bad()),
    };

    let mb = (bytes / (1024.0 * 1024.0)).ceil();

    if !(0.0..=f64::from(u32::MAX)).contains(&mb) {
        return Err(bad());
    }

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // checked range above
    Ok(mb as u32)
}

/// CPUs (`0.5`, `"2"`) in thousandths of a core.
pub fn cpus(v: &Value, path: &str) -> Res<u32> {
    let n = match v {
        Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => v.as_f64(),
    };
    let millis = n
        .filter(|n| *n >= 0.0 && *n <= 4096.0)
        .map(|n| (n * 1000.0).round())
        .ok_or_else(|| err(path, "a number of CPUs (e.g. 0.5)"))?;

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // checked range above
    Ok(millis as u32)
}

// ---- writing ---------------------------------------------------------------------------

/// A stored service as a file entry: its spec (with the orchestration
/// under [`SPEC_KEY`]) and its plain variables. Secret variables are never
/// written.
pub fn from_stored(name: &str, runtime: &str, spec: &Value, env: Vec<(String, String)>) -> Result<FileService, String> {
    let runtime = match runtime {
        "container" => Runtime::Container,
        "systemd" => Runtime::Systemd,
        other => return Err(format!("a {other} service cannot be written as a services file")),
    };

    let native: NativeSpec = serde_json::from_value(spec.clone()).map_err(|e| format!("stored spec: {e}"))?;

    let orchestration = match spec.get(SPEC_KEY).filter(|v| !v.is_null()) {
        Some(v) => serde_json::from_value(v.clone()).map_err(|e| format!("stored orchestration: {e}"))?,
        None => Orchestration::default(),
    };

    let on_node_failure = spec.get("on_node_failure").and_then(|v| serde_json::from_value(v.clone()).ok());
    let mesh = spec.get("mesh").and_then(Value::as_bool);

    Ok(FileService { name: name.to_owned(), runtime, spec: native, env, orchestration, on_node_failure, mesh })
}

/// Writes services as a services file that [`parse`] reads back.
#[must_use]
pub fn export(name: Option<&str>, services: &[FileService]) -> String {
    let mut top = Map::new();

    if let Some(n) = name {
        top.insert("name".into(), json!(n));
    }

    let keys = object_keys(services);
    let mut list = Map::new();

    for s in services {
        list.insert(s.name.clone(), service_value(s, &keys));
    }

    top.insert("services".into(), Value::Object(list));

    // Secrets and configs name their vault items.
    for kind in [FileKind::Secret, FileKind::Config] {
        let m: Map<String, Value> = keys
            .iter()
            .filter(|((k, _), _)| *k == kind)
            .map(|((_, item), key)| {
                let def =
                    if key == item { json!({ "external": true }) } else { json!({ "external": true, "name": item }) };

                (key.clone(), def)
            })
            .collect();

        if !m.is_empty() {
            top.insert(kind.section().into(), Value::Object(m));
        }
    }

    // Networks the services connect to are declared as external.
    let networks: BTreeSet<&String> = services.iter().flat_map(|s| &s.spec.options.networks).collect();

    if !networks.is_empty() {
        let m: Map<String, Value> = networks.into_iter().map(|n| (n.clone(), json!({ "external": true }))).collect();
        top.insert("networks".into(), Value::Object(m));
    }

    crate::stacks::to_yaml(&Value::Object(top))
}

/// The compose key of every referenced vault item: the item name when it
/// is a valid key, otherwise a cleaned form made unique.
fn object_keys(services: &[FileService]) -> BTreeMap<(FileKind, String), String> {
    let mut out: BTreeMap<(FileKind, String), String> = BTreeMap::new();

    for r in services.iter().flat_map(|s| &s.spec.secret_files) {
        if out.contains_key(&(r.kind, r.item.clone())) {
            continue;
        }

        let base = if object_key_ok(&r.item) {
            r.item.clone()
        } else {
            let cleaned: String =
                r.item.chars().map(|c| if c.is_ascii_alphanumeric() || "._-".contains(c) { c } else { '_' }).collect();
            cleaned.chars().take(56).collect()
        };

        let taken = |k: &str| out.iter().any(|((kind, _), v)| *kind == r.kind && v == k);
        let mut key = base.clone();
        let mut n = 2;

        while taken(&key) {
            key = format!("{base}-{n}");
            n += 1;
        }

        out.insert((r.kind, r.item.clone()), key);
    }

    out
}

/// A service's `secrets:` or `configs:` entries in long syntax, leaving
/// out what equals the default.
fn objects_value(s: &FileService, kind: FileKind, keys: &BTreeMap<(FileKind, String), String>) -> Value {
    let list: Vec<Value> = s
        .spec
        .secret_files
        .iter()
        .filter(|r| r.kind == kind)
        .map(|r| {
            let key = keys.get(&(kind, r.item.clone())).cloned().unwrap_or_else(|| r.item.clone());
            let mut m = Map::new();
            m.insert("source".into(), json!(key));

            if r.target != FileRef::target_for(kind, &key, None) {
                m.insert("target".into(), json!(r.target));
            }

            if let Some(mode) = r.mode {
                m.insert("mode".into(), json!(format!("0{mode:o}")));
            }

            if let Some(uid) = r.uid {
                m.insert("uid".into(), json!(uid.to_string()));
            }

            if let Some(gid) = r.gid {
                m.insert("gid".into(), json!(gid.to_string()));
            }

            Value::Object(m)
        })
        .collect();

    Value::Array(list)
}

fn put(m: &mut Map<String, Value>, k: &str, v: Value) {
    let empty = match &v {
        Value::Null => true,
        Value::String(s) => s.is_empty(),
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
        Value::Bool(b) => !b,
        Value::Number(_) => false,
    };

    if !empty {
        m.insert(k.to_owned(), v);
    }
}

fn secs(n: u32) -> String {
    format!("{n}s")
}

fn millis_cpus(n: u32) -> String {
    let whole = n / 1000;
    let frac = n % 1000;

    if frac == 0 { whole.to_string() } else { format!("{whole}.{}", format!("{frac:03}").trim_end_matches('0')) }
}

fn service_value(s: &FileService, keys: &BTreeMap<(FileKind, String), String>) -> Value {
    let spec = &s.spec;
    let o = &spec.options;
    let mut m = Map::new();
    put(&mut m, "image", json!(spec.image));
    put(&mut m, "command", json!(spec.command));
    put(&mut m, "entrypoint", json!(spec.entrypoint));
    put(&mut m, "working_dir", json!(spec.working_dir));

    let env: Map<String, Value> = s.env.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
    put(&mut m, "environment", Value::Object(env));
    put(&mut m, "ports", json!(spec.ports));

    let mut volumes: Vec<Value> = spec.volumes.iter().map(|v| json!(v)).collect();
    volumes.extend(
        o.tmpfs
            .iter()
            .filter(|t| t.size_mb > 0)
            .map(|t| json!({ "type": "tmpfs", "target": t.target, "tmpfs": { "size": format!("{}M", t.size_mb) } })),
    );
    put(&mut m, "volumes", Value::Array(volumes));
    put(
        &mut m,
        "tmpfs",
        json!(o.tmpfs.iter().filter(|t| t.size_mb == 0).map(|t| t.target.clone()).collect::<Vec<_>>()),
    );

    if let Some(hc) = &spec.healthcheck {
        m.insert(
            "healthcheck".into(),
            json!({
                "test": ["CMD-SHELL", hc.command],
                "interval": secs(hc.interval_secs),
                "timeout": secs(hc.timeout_secs),
                "retries": hc.retries,
                "start_period": secs(hc.start_period_secs),
            }),
        );
    }

    put(&mut m, "user", json!(spec.user));
    put(&mut m, "hostname", json!(spec.hostname));
    let labels: Map<String, Value> = spec.labels.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
    put(&mut m, "labels", Value::Object(labels));
    put(&mut m, "extra_hosts", json!(spec.extra_hosts));
    put(&mut m, "cap_add", json!(o.cap_add));
    put(&mut m, "cap_drop", json!(o.cap_drop));

    let ulimits: Map<String, Value> =
        o.ulimits.iter().map(|u| (u.name.clone(), json!({ "soft": u.soft, "hard": u.hard }))).collect();
    put(&mut m, "ulimits", Value::Object(ulimits));
    put(&mut m, "dns", json!(o.dns.iter().map(ToString::to_string).collect::<Vec<_>>()));

    let sysctls: Map<String, Value> = o.sysctls.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
    put(&mut m, "sysctls", Value::Object(sysctls));

    let devices: Vec<Value> = o
        .devices
        .iter()
        .map(|d| json!({ "source": d.host, "target": d.container, "permissions": d.permissions }))
        .map(|mut v| {
            if let Some(obj) = v.as_object_mut() {
                obj.retain(|_, x| x.as_str().is_some_and(|s| !s.is_empty()));
            }

            v
        })
        .collect();
    put(&mut m, "devices", Value::Array(devices));
    put(&mut m, "init", json!(o.init));
    put(&mut m, "read_only", json!(spec.read_only));

    if spec.stop_timeout_secs > 0 {
        m.insert("stop_grace_period".into(), json!(secs(spec.stop_timeout_secs)));
    }

    put(&mut m, "networks", json!(o.networks));

    let restart = match (o.restart_policy, o.restart_max_attempts) {
        (Some(RestartPolicy::OnFailure), n) if n > 0 => Some(format!("on-failure:{n}")),
        (Some(p), _) => Some(p.docker_name().to_owned()),
        (None, _) if !spec.restart => Some("no".to_owned()),
        (None, _) => None,
    };

    if let Some(r) = restart {
        m.insert("restart".into(), json!(r));
    }

    put(&mut m, "secrets", objects_value(s, FileKind::Secret, keys));
    put(&mut m, "configs", objects_value(s, FileKind::Config, keys));
    put(&mut m, "deploy", deploy_value(s));
    put(&mut m, "depends_on", json!(s.orchestration.depends_on));

    let mut ext = Map::new();

    if s.runtime == Runtime::Systemd {
        ext.insert("runtime".into(), json!("systemd"));
    }

    if let Some(p) = spec.listen_port {
        ext.insert("listen_port".into(), json!(p));
    }

    if let Some(p) = s.on_node_failure {
        ext.insert("on_node_failure".into(), json!(p));
    }

    if let Some(mesh) = s.mesh {
        ext.insert("mesh".into(), json!(mesh));
    }

    put(&mut m, "x-forge", Value::Object(ext));

    Value::Object(m)
}

fn deploy_value(s: &FileService) -> Value {
    let o = &s.orchestration;
    let mut d = Map::new();

    match o.mode {
        ServiceMode::Global => {
            d.insert("mode".into(), json!("global"));
        }
        ServiceMode::Replicated => {
            d.insert("replicas".into(), json!(o.replicas));
        }
    }

    let mut constraints: Vec<String> = o.nodes.iter().map(|n| format!("node.hostname == {n}")).collect();
    constraints.extend(o.node_tags.iter().map(|(k, v)| format!("node.labels.{k} == {v}")));
    let mut placement = Map::new();
    put(&mut placement, "constraints", json!(constraints));

    if let Some(k) = &o.spread {
        placement.insert("preferences".into(), json!([{ "spread": format!("node.labels.{k}") }]));
    }

    put(&mut d, "placement", Value::Object(placement));

    let mut resources = Map::new();
    let mut limits = Map::new();

    if s.spec.cpu_millis > 0 {
        limits.insert("cpus".into(), json!(millis_cpus(s.spec.cpu_millis)));
    }

    if s.spec.memory_mb > 0 {
        limits.insert("memory".into(), json!(format!("{}M", s.spec.memory_mb)));
    }

    let mut reservations = Map::new();

    if o.reserve_cpu_millis > 0 {
        reservations.insert("cpus".into(), json!(millis_cpus(o.reserve_cpu_millis)));
    }

    if o.reserve_memory_mb > 0 {
        reservations.insert("memory".into(), json!(format!("{}M", o.reserve_memory_mb)));
    }

    put(&mut resources, "limits", Value::Object(limits));
    put(&mut resources, "reservations", Value::Object(reservations));
    put(&mut d, "resources", Value::Object(resources));

    for (key, policy) in [("update_config", &o.update), ("rollback_config", &o.rollback)] {
        if let Some(u) = policy {
            d.insert(
                key.into(),
                json!({
                    "parallelism": u.parallelism,
                    "delay": secs(u.delay_secs),
                    "order": u.order,
                    "failure_action": u.failure_action,
                    "monitor": secs(u.monitor_secs),
                    "max_failure_ratio": u.max_failure_ratio,
                }),
            );
        }
    }

    Value::Object(d)
}

#[cfg(test)]
mod tests;
