//! Compose stacks on the control plane (issue #7): parsing, `.env` sealing,
//! placement onto nodes and the preview. The policy, split and helper
//! operations are in `forge_catalog::stacks` (shared with the node helper).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use forge_catalog::fabric::{Lease, LeaseKind, interface_name, network_name};
use forge_catalog::stacks::{
    self as cat, MAX_COMPOSE_BYTES, Placement, Policy, PublishedPort, Report, Severity, StackPart,
};
use forge_core::files::{FileRepository, RootSpec};
use forge_core::model::Server;
use forge_core::ports::{ClusterRepository, FabricRepository, ServerRepository};
use forge_core::tags::FactsRepository;
use forge_core::{ServerId, StackId, WorkspaceId};
use forge_secrets::Envelope;
use serde::Serialize;
use serde_json::{Value, json};

/// Parses a compose document (YAML 1.2, merge keys) with tight limits.
pub fn parse_compose(text: &str) -> Result<Value, String> {
    if text.len() > MAX_COMPOSE_BYTES {
        return Err(format!("the compose file is larger than {} KiB", MAX_COMPOSE_BYTES / 1024));
    }

    if text.trim().is_empty() {
        return Err("the compose file is empty".into());
    }

    let mut budget = serde_saphyr::Budget::default();
    budget.max_depth = 32;
    budget.max_nodes = 50_000;
    budget.max_aliases = 2_000;
    budget.max_anchors = 2_000;
    budget.max_documents = 1;
    budget.max_events = 200_000;
    budget.max_total_scalar_bytes = MAX_COMPOSE_BYTES * 4;
    budget.max_recorded_anchor_bytes = MAX_COMPOSE_BYTES * 4;
    let mut options = serde_saphyr::Options::default();
    options.budget = Some(budget);

    // YAML 1.2: `yes`, `on` stay strings; `0755` stays a string.
    options.strict_booleans = true;
    options.leading_zero_numbers_as_strings = true;
    let mut v: Value = serde_saphyr::from_str_with_options(text, options).map_err(|e| {
        let msg = e.to_string();
        format!("invalid YAML: {}", msg.lines().next().unwrap_or(&msg))
    })?;

    if !v.is_object() {
        return Err("the compose file must be a mapping with `services`".into());
    }

    // `version` is obsolete in Compose v2 and only causes warnings.
    if let Some(m) = v.as_object_mut() {
        m.remove("version");
    }

    Ok(v)
}

/// An environment name as used in folders and project names (the same slug
/// services use: lower case, `-` for anything else).
#[must_use]
pub fn ns_slug(name: &str) -> String {
    let s: String = name.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let s = s.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-");
    if s.starts_with(|c: char| c.is_ascii_digit()) { format!("ns-{s}") } else { s }
}

/// Additional authenticated data of a stack's sealed `.env`.
#[must_use]
pub fn env_aad(stack: StackId) -> String {
    format!("stack:{stack}:env")
}

/// Seals `.env` variables (JSON map); `None` when there are none.
pub fn seal_env(
    envelope: &Envelope,
    stack: StackId,
    vars: &BTreeMap<String, String>,
) -> Result<Option<String>, String> {
    if vars.is_empty() {
        return Ok(None);
    }

    let plain = serde_json::to_vec(vars).map_err(|e| e.to_string())?;

    envelope.seal(&plain, &env_aad(stack)).map(Some).map_err(|e| e.to_string())
}

/// Opens a stack's sealed `.env`.
pub fn open_env(envelope: &Envelope, stack: StackId, sealed: Option<&str>) -> Result<BTreeMap<String, String>, String> {
    let Some(s) = sealed else { return Ok(BTreeMap::new()) };
    let plain = envelope.open(s, &env_aad(stack)).map_err(|_| "cannot unseal the stack's .env".to_owned())?;

    serde_json::from_slice(&plain).map_err(|e| e.to_string())
}

/// The repositories planning needs.
#[derive(Debug)]
pub struct Deps<'a> {
    pub servers: &'a dyn ServerRepository,
    pub clusters: &'a dyn ClusterRepository,
    pub fabrics: &'a dyn FabricRepository,
    pub files: &'a dyn FileRepository,
    pub facts: &'a dyn FactsRepository,
    /// The server host's forge-dns addresses (`[dns] listen`), after each
    /// node's own in a stack's `dns`.
    pub forge_dns: Vec<std::net::Ipv4Addr>,
}

/// One node's share of a stack.
#[derive(Debug, Clone, Serialize)]
pub struct NodePlan {
    pub server_id: ServerId,
    pub hostname: String,
    pub services: BTreeSet<String>,
    /// Address unmarked ports bind to: the cluster network address, or loopback.
    pub internal_ip: String,
    #[serde(skip)]
    pub bind_roots: Vec<String>,
    /// The part's ports after binding.
    pub ports: Vec<PublishedPort>,
    /// Policy findings for this node's part.
    pub findings: Vec<cat::Finding>,
    /// Docker name of the cluster container network the node's services
    /// join, when the node can take it (`plans/native-mesh.md` 1.6).
    pub mesh_network: Option<String>,
    /// Resolvers for those services: the node's forge-dns (its mesh
    /// address), then the server host's.
    #[serde(skip)]
    pub dns: Option<cat::StackDns>,
}

/// A planned stack: the raw tree, its interpolated form and the nodes.
#[derive(Debug, Clone)]
pub struct Plan {
    pub tree: Value,
    pub interpolated: Value,
    pub nodes: Vec<NodePlan>,
    /// Every finding across nodes; `privileged` when any needs `stack.privileged`.
    pub report: Report,
}

impl Plan {
    #[must_use]
    pub fn node(&self, server: ServerId) -> Option<&NodePlan> {
        self.nodes.iter().find(|n| n.server_id == server)
    }

    /// Every service name.
    #[must_use]
    pub fn services(&self) -> BTreeSet<String> {
        self.tree.get("services").and_then(Value::as_object).map(|m| m.keys().cloned().collect()).unwrap_or_default()
    }
}

/// The folder of a stack on its nodes.
#[must_use]
pub fn stack_dir(dir: Option<&str>, environment: &str, stack: &str) -> String {
    dir.and_then(cat::normalize_abs).unwrap_or_else(|| cat::default_dir(environment, stack))
}

/// Cluster network addresses of nodes whose mesh is up.
pub async fn fabric_ips(fabrics: &dyn FabricRepository, ws: WorkspaceId) -> HashMap<String, String> {
    fabric_nodes(fabrics, ws).await.into_iter().filter_map(|(id, n)| Some((id, n.ip?))).collect()
}

/// A node's place in its cluster's fabric, as stack planning needs it.
#[derive(Debug, Clone, Default)]
pub struct FabricNode {
    /// Mesh address, when the node's WireGuard part is in sync.
    pub ip: Option<String>,
    /// Docker name of the cluster container network, when it is `ok` on
    /// the node.
    pub network: Option<String>,
}

/// Every fabric member of a workspace, by server id. Nodes outside any
/// fabric are absent.
pub async fn fabric_nodes(fabrics: &dyn FabricRepository, ws: WorkspaceId) -> HashMap<String, FabricNode> {
    let mut out: HashMap<String, FabricNode> = HashMap::new();

    // Planning reads fabrics best effort: without them every node simply
    // binds to loopback and joins no network, which the preview says.
    let list = fabrics.list_fabrics(Some(ws)).await.unwrap_or_default();

    for f in list {
        let field = |id: &str, k: &str| f.status.pointer(&format!("/nodes/{id}/{k}")).and_then(Value::as_str);
        let iface = interface_name(&f.cluster_id.to_string());

        for l in serde_json::from_value::<Vec<Lease>>(f.leases.clone()).unwrap_or_default() {
            if l.kind != LeaseKind::Node {
                continue;
            }

            let up = field(&l.owner, "state") == Some("ok");
            let network = field(&l.owner, "network") == Some(crate::api::fabric_reconcile::NETWORK_OK);
            let node = FabricNode {
                ip: up.then(|| l.cidr.first().to_string()),
                network: (up && network).then(|| network_name(&iface)),
            };
            out.insert(l.owner, node);
        }
    }

    out
}

/// The `dns` of a node's part on the cluster network: its own forge-dns on
/// its mesh address first. Search domains are left to the node, which
/// knows its zone from `names.json`.
fn mesh_dns(mesh: Option<&str>, internal_ip: &str, second: &[std::net::Ipv4Addr]) -> Option<cat::StackDns> {
    mesh?;
    let own: std::net::Ipv4Addr = internal_ip.parse().ok()?;
    let mut servers = vec![own];
    servers.extend(second.iter().filter(|s| **s != own));

    Some(cat::StackDns { servers, search: Vec::new() })
}

/// Whether a node's agent reports the `mesh_network` capability fact.
/// Missing or unreadable means an older agent.
pub async fn mesh_capable(facts: &dyn FactsRepository, server: ServerId) -> bool {
    match facts.facts_of(server).await {
        Ok(Some((f, _))) => f.get("mesh_network").and_then(Value::as_bool) == Some(true),
        Ok(None) => false,
        Err(e) => {
            tracing::warn!(error = %e, %server, "reading node facts failed; planning without the cluster network");
            false
        }
    }
}

/// Preview notes for the cluster container network (exact texts in
/// `plans/native-mesh.md` 1.5 and 1.6).
pub const NOTE_NO_FABRIC: &str = "services are reachable from other nodes only through published ports; create the cluster network to reach them by name";

fn note_not_in_sync(node: &str) -> String {
    format!("{node}: cluster network not applied yet; services will join it on the next deploy after it is in sync")
}

fn note_old_agent(node: &str) -> String {
    format!("{node}: update its agent to put services on the cluster network")
}

/// Decides a node's `mesh_network` and the note that explains a `None`.
pub fn decide_mesh(
    fabric: Option<&FabricNode>,
    capable: bool,
    stack_off: bool,
    node: &str,
) -> (Option<String>, Option<String>) {
    if stack_off {
        return (None, None);
    }

    let Some(f) = fabric else { return (None, Some(NOTE_NO_FABRIC.to_owned())) };
    let Some(net) = f.network.clone() else { return (None, Some(note_not_in_sync(node))) };

    if !capable {
        return (None, Some(note_old_agent(node)));
    }

    (Some(net), None)
}

/// Approved, writable directory roots on a node (bind mounts may use them).
pub async fn bind_roots(files: &dyn FileRepository, server: ServerId) -> Vec<String> {
    files
        .list_roots(server)
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|r| !r.read_only)
        .filter_map(|r| match r.spec {
            RootSpec::Dir { base } => Some(base.to_string()),
            _ => None,
        })
        .collect()
}

/// A node of workspace `ws` by hostname, console name or id.
pub async fn resolve_node(deps: &Deps<'_>, ws: WorkspaceId, all: &[Server], name: &str) -> Result<Server, String> {
    if let Some(s) = deps.servers.find_server_by_hostname(ws, name).await.map_err(|e| e.to_string())? {
        return Ok(s);
    }

    all.iter()
        .find(|s| s.display_name.as_deref() == Some(name) || s.id.to_string() == name)
        .cloned()
        .ok_or_else(|| format!("no node named `{name}` in this organization"))
}

/// Plans a stack: placement → nodes, then the policy for each node's part.
///
/// Findings, ports and errors are computed on the interpolated tree, so
/// `.env` values are redacted from their text (see [`cat::redact`]): plans
/// are shown to readers of the stack and recorded in audit and job errors.
#[allow(clippy::too_many_arguments)]
pub async fn plan(
    deps: &Deps<'_>,
    ws: WorkspaceId,
    default_server: ServerId,
    environment: &str,
    stack: &str,
    dir: Option<&str>,
    content: &str,
    env: &BTreeMap<String, String>,
    allow_privileged: bool,
) -> Result<Plan, String> {
    let mut p = plan_unredacted(deps, ws, default_server, environment, stack, dir, content, env, allow_privileged)
        .await
        .map_err(|e| cat::redact(&e, env))?;
    let hide = |f: &mut cat::Finding| f.message = cat::redact(&f.message, env);
    p.report.findings.iter_mut().for_each(hide);

    for n in &mut p.nodes {
        n.findings.iter_mut().for_each(hide);

        for port in &mut n.ports {
            for s in [&mut port.host_ip, &mut port.published, &mut port.target, &mut port.protocol] {
                *s = cat::redact(s, env);
            }
        }
    }

    Ok(p)
}

#[allow(clippy::too_many_arguments)]
async fn plan_unredacted(
    deps: &Deps<'_>,
    ws: WorkspaceId,
    default_server: ServerId,
    environment: &str,
    stack: &str,
    dir: Option<&str>,
    content: &str,
    env: &BTreeMap<String, String>,
    allow_privileged: bool,
) -> Result<Plan, String> {
    let tree = parse_compose(content)?;
    let interpolated = cat::interpolate(&tree, env)?;
    let placements = cat::placements(&interpolated)?;
    let all = deps.servers.list_servers(Some(ws)).await.map_err(|e| e.to_string())?;
    let default = all.iter().find(|s| s.id == default_server).cloned().ok_or("the stack's node no longer exists")?;
    let clusters = deps.clusters.list_clusters(ws).await.map_err(|e| e.to_string())?;
    let mut assign: BTreeMap<ServerId, (Server, BTreeSet<String>)> = BTreeMap::new();

    for (service, p) in &placements {
        let targets: Vec<Server> = match p {
            Placement::Default => vec![default.clone()],
            Placement::Node(n) => vec![resolve_node(deps, ws, &all, n).await?],
            Placement::Cluster(c) => {
                let cluster =
                    clusters.iter().find(|x| x.name == *c).ok_or_else(|| format!("no cluster named `{c}`"))?;
                let nodes: Vec<Server> = all.iter().filter(|s| s.cluster_id == Some(cluster.id)).cloned().collect();
                if nodes.is_empty() {
                    return Err(format!("cluster `{c}` has no nodes"));
                }

                nodes
            }
        };

        for s in targets {
            if !s.os.eq_ignore_ascii_case("linux") {
                return Err(format!("{service}: compose stacks run on Linux nodes only ({} is {})", s.hostname, s.os));
            }

            assign.entry(s.id).or_insert_with(|| (s.clone(), BTreeSet::new())).1.insert(service.clone());
        }
    }

    let fabric = fabric_nodes(deps.fabrics, ws).await;
    let stack_off = interpolated.pointer("/x-forge/mesh") == Some(&Value::Bool(false));
    let sdir = stack_dir(dir, environment, stack);
    let label = format!("{environment}/{stack}");
    let mut report = Report::default();
    let mut nodes = Vec::new();

    for (id, (server, services)) in assign {
        let part = cat::part_for(&interpolated, &services, &label)?;
        let roots = bind_roots(deps.files, id).await;
        let policy = Policy { allow_privileged, bind_roots: roots.clone(), stack_dir: sdir.clone() };
        let mut r = cat::check(&part, &policy);
        let node_fabric = fabric.get(&id.to_string());
        let internal_ip = node_fabric.and_then(|f| f.ip.clone()).unwrap_or_else(|| "127.0.0.1".into());
        let mut bound = part;
        let ports = cat::bind_ports(&mut bound, &internal_ip, &mut r);
        let name = server.display_name.clone().unwrap_or_else(|| server.hostname.clone());
        let capable = mesh_capable(deps.facts, id).await;
        let (mesh_network, note) = decide_mesh(node_fabric, capable, stack_off, &name);

        if let Some(n) = note {
            r.push("", Severity::Info, n);
        }

        for f in &r.findings {
            if !report.findings.iter().any(|x| x == f) {
                report.findings.push(f.clone());
            }
        }

        report.privileged |= r.privileged;
        nodes.push(NodePlan {
            server_id: id,
            hostname: name,
            services,
            dns: mesh_dns(mesh_network.as_deref(), &internal_ip, &deps.forge_dns),
            internal_ip,
            bind_roots: roots,
            ports,
            findings: r.findings,
            mesh_network,
        });
    }

    // A service named like another one's replica loses that name.
    let svc_names: Vec<&str> = interpolated
        .get("services")
        .and_then(Value::as_object)
        .map(|m| m.keys().map(String::as_str).collect())
        .unwrap_or_default();

    for (svc, other, n) in forge_catalog::names::replica_clashes(&svc_names) {
        report.push("", Severity::Info, format!("service `{svc}` clashes with replica {n} of `{other}`"));
    }

    // Top-level problems (no services, unknown keys) even when nothing was placed.
    if nodes.is_empty() {
        let r = cat::check(&interpolated, &Policy { allow_privileged, bind_roots: Vec::new(), stack_dir: sdir });
        report.findings.extend(r.findings);
    }

    Ok(Plan { tree, interpolated, nodes, report })
}

/// The helper's part for one node of a plan.
pub fn node_part(
    plan: &Plan,
    node: &NodePlan,
    environment: &str,
    stack: &str,
    dir: Option<&str>,
    env: &BTreeMap<String, String>,
    allow_privileged: bool,
) -> Result<StackPart, String> {
    let sdir = stack_dir(dir, environment, stack);

    // The raw (not interpolated) part: the helper interpolates with the same `.env`.
    let compose = cat::part_for(&plan.tree, &node.services, &format!("{environment}/{stack}"))?;

    Ok(StackPart {
        environment: environment.to_owned(),
        stack: stack.to_owned(),
        dir: sdir.clone(),
        compose,
        env: env.clone(),
        policy: Policy { allow_privileged, bind_roots: node.bind_roots.clone(), stack_dir: sdir },
        internal_ip: node.internal_ip.clone(),
        mesh_network: node.mesh_network.clone(),
        dns: node.dns.clone(),
    })
}

fn secretish(key: &str) -> bool {
    let k = key.to_ascii_uppercase();
    ["PASS", "SECRET", "TOKEN", "KEY", "CREDENTIAL", "PRIVATE", "AUTH"].iter().any(|s| k.contains(s))
}

/// Environment for display: values of secret-looking keys are masked;
/// `${VAR}` references stay as written (never the `.env` values).
fn masked_env(svc: &Value) -> Vec<Value> {
    let mask =
        |k: &str, v: String| if secretish(k) && !v.starts_with("${") { "••••••".to_owned() } else { v };

    match svc.get("environment") {
        Some(Value::Object(m)) => m
            .iter()
            .map(|(k, v)| {
                let v = match v {
                    Value::String(s) => s.clone(),
                    Value::Null => String::new(),
                    other => other.to_string(),
                };

                json!({ "key": k, "value": mask(k, v) })
            })
            .collect(),
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(Value::as_str)
            .map(|e| {
                let (k, v) = e.split_once('=').unwrap_or((e, ""));
                json!({ "key": k, "value": mask(k, v.to_owned()) })
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// What the designer shows: services with their details, node and policy
/// result, and the services grouped by node. Every field comes from the
/// raw tree (`${VAR}` as written), never from the interpolated one, so
/// `.env` values are not shown; findings and ports are redacted by [`plan`].
#[must_use]
pub fn preview(plan: &Plan) -> Value {
    let raw = plan.tree.get("services").and_then(Value::as_object).cloned().unwrap_or_default();
    let mut services = Vec::new();

    for (name, svc) in &raw {
        let i = svc;
        let nodes: Vec<Value> = plan
            .nodes
            .iter()
            .filter(|n| n.services.contains(name))
            .map(|n| json!({ "server_id": n.server_id, "hostname": n.hostname }))
            .collect();
        let findings: Vec<&cat::Finding> = plan.report.findings.iter().filter(|f| f.service == *name).collect();
        let ports: Vec<&PublishedPort> =
            plan.nodes.iter().flat_map(|n| n.ports.iter()).filter(|p| p.service == *name).collect();
        let blocked = findings.iter().any(|f| f.severity == Severity::Blocked);
        let privileged = findings.iter().any(|f| f.severity == Severity::Privileged);
        services.push(json!({
            "name": name,
            "image": i.get("image"),
            "command": svc.get("command"),
            "entrypoint": svc.get("entrypoint"),
            "ports": ports,
            "volumes": i.get("volumes").cloned().unwrap_or(json!([])),
            "environment": masked_env(svc),
            "env_file": svc.get("env_file"),
            "depends_on": cat::dependencies(svc),
            "healthcheck": i.get("healthcheck").map(|h| json!({ "test": h.get("test"), "interval": h.get("interval") })),
            "restart": i.get("restart"),
            "expose": i.pointer("/labels/forge.expose").or_else(|| i.pointer("/x-forge/expose")),
            "nodes": nodes,
            "policy": if blocked { "blocked" } else if privileged { "privileged" } else { "allowed" },
            "findings": findings,
            "mesh": mesh_column(plan, name, svc),
        }));
    }

    let top: Vec<&cat::Finding> = plan.report.findings.iter().filter(|f| f.service.is_empty()).collect();

    json!({
        "services": services,
        "nodes": plan.nodes,
        "findings": top,
        "privileged": plan.report.privileged,
        "blocked": plan.report.findings.iter().any(|f| f.severity == Severity::Blocked),
    })
}

/// The preview's per-service `mesh` value: `true`/`false`, or why it is
/// off (`"off (x-forge.mesh)"`, `"off (network_mode)"`).
fn mesh_column(plan: &Plan, name: &str, svc: &Value) -> Value {
    let off = |v: &Value| v.pointer("/x-forge/mesh") == Some(&Value::Bool(false));

    if off(&plan.tree) || off(svc) {
        return json!("off (x-forge.mesh)");
    }

    if svc.get("network_mode").is_some() {
        return json!("off (network_mode)");
    }

    let joins = plan.nodes.iter().filter(|n| n.services.contains(name)).any(|n| n.mesh_network.is_some());

    json!(joins)
}

/// A compose document for one container from the **Create container** form.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContainerForm {
    pub name: String,
    pub image: String,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub entrypoint: Option<String>,
    #[serde(default)]
    pub ports: Vec<FormPort>,
    /// Plain variables (values go into the stack's sealed `.env`).
    #[serde(default)]
    pub env: Vec<FormEnv>,
    #[serde(default)]
    pub volumes: Vec<FormVolume>,
    #[serde(default = "unless_stopped")]
    pub restart: String,
    #[serde(default)]
    pub cpus: Option<f64>,
    #[serde(default)]
    pub memory_mb: Option<u64>,
    #[serde(default)]
    pub network: Option<String>,
    #[serde(default)]
    pub healthcheck: Option<FormHealth>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    /// `http` or `https` to offer **Expose via Nginx** for the first TCP port.
    #[serde(default)]
    pub expose: Option<String>,
}

fn unless_stopped() -> String {
    "unless-stopped".into()
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FormPort {
    pub container: u16,
    #[serde(default)]
    pub host: Option<u16>,
    #[serde(default = "tcp")]
    pub protocol: String,
    #[serde(default)]
    pub public: bool,
}

fn tcp() -> String {
    "tcp".into()
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FormEnv {
    pub key: String,
    #[serde(default)]
    pub value: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FormVolume {
    /// A volume name, or a host path inside an approved root.
    pub source: String,
    pub target: String,
    #[serde(default)]
    pub read_only: bool,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FormHealth {
    pub test: String,
    #[serde(default = "thirty")]
    pub interval: String,
}

fn thirty() -> String {
    "30s".into()
}

impl ContainerForm {
    /// The generated compose document and the `.env` variables. Variables
    /// are referenced as `${KEY}` so their values live only in the sealed `.env`.
    pub fn to_compose(&self) -> Result<(String, BTreeMap<String, String>), String> {
        if !cat::valid_service_name(&self.name) {
            return Err("the name uses letters, digits, `.`, `_` and `-`".into());
        }

        if self.image.trim().is_empty() || self.image.contains(char::is_whitespace) {
            return Err("enter an image such as nginx:1.27".into());
        }

        if !matches!(self.restart.as_str(), "no" | "always" | "on-failure" | "unless-stopped") {
            return Err("restart: no, always, on-failure or unless-stopped".into());
        }

        let mut svc = serde_json::Map::new();
        svc.insert("image".into(), json!(self.image.trim()));

        if let Some(c) = self.command.as_deref().filter(|c| !c.trim().is_empty()) {
            svc.insert("command".into(), json!(c));
        }

        if let Some(e) = self.entrypoint.as_deref().filter(|e| !e.trim().is_empty()) {
            svc.insert("entrypoint".into(), json!(e));
        }

        let mut ports = Vec::new();
        let mut public = Vec::new();

        for p in &self.ports {
            if p.container == 0 || !matches!(p.protocol.as_str(), "tcp" | "udp") {
                return Err("ports: container port 1–65535, tcp or udp".into());
            }

            let host = p.host.unwrap_or(p.container);
            ports.push(json!(format!("{host}:{}/{}", p.container, p.protocol)));

            if p.public {
                public.push(json!(host));
            }
        }

        if !ports.is_empty() {
            svc.insert("ports".into(), json!(ports));
        }

        let mut env = BTreeMap::new();
        let mut environment = serde_json::Map::new();

        for e in &self.env {
            let k = e.key.trim();
            if k.is_empty()
                || !k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                || k.as_bytes()[0].is_ascii_digit()
            {
                return Err(format!("`{k}` is not a valid variable name"));
            }

            env.insert(k.to_owned(), e.value.clone());
            environment.insert(k.to_owned(), json!(format!("${{{k}}}")));
        }

        if !environment.is_empty() {
            svc.insert("environment".into(), Value::Object(environment));
        }

        let mut vols = Vec::new();
        let mut named = serde_json::Map::new();

        for v in &self.volumes {
            if !v.target.starts_with('/')
                || v.source.trim().is_empty()
                || v.source.contains(':')
                || v.target.contains(':')
            {
                return Err("volumes: a name or host path, and an absolute container path".into());
            }

            let ro = if v.read_only { ":ro" } else { "" };
            vols.push(json!(format!("{}:{}{ro}", v.source, v.target)));

            if !v.source.starts_with('/') && !v.source.starts_with('.') {
                named.insert(v.source.clone(), json!({}));
            }
        }

        if !vols.is_empty() {
            svc.insert("volumes".into(), json!(vols));
        }

        svc.insert("restart".into(), json!(self.restart));
        let mut limits = serde_json::Map::new();

        if let Some(c) = self.cpus.filter(|c| *c > 0.0) {
            limits.insert("cpus".into(), json!(c.to_string()));
        }

        if let Some(m) = self.memory_mb.filter(|m| *m > 0) {
            limits.insert("memory".into(), json!(format!("{m}M")));
        }

        if !limits.is_empty() {
            svc.insert("deploy".into(), json!({ "resources": { "limits": limits } }));
        }

        if let Some(h) = &self.healthcheck {
            svc.insert("healthcheck".into(), json!({ "test": ["CMD-SHELL", h.test], "interval": h.interval }));
        }

        let mut labels = serde_json::Map::new();

        for (k, v) in &self.labels {
            labels.insert(k.clone(), json!(v));
        }

        if let Some(e) = &self.expose {
            if !matches!(e.as_str(), "http" | "https") {
                return Err("expose: http or https".into());
            }

            labels.insert("forge.expose".into(), json!(e));
        }

        if !labels.is_empty() {
            svc.insert("labels".into(), Value::Object(labels));
        }

        if !public.is_empty() {
            svc.insert("x-forge".into(), json!({ "public": public }));
        }

        let mut doc = serde_json::Map::new();

        match self.network.as_deref().filter(|n| !n.is_empty()) {
            None => {}
            // As `network_mode`, so the policy sees them: `host` needs
            // `stack.privileged` like any other host-access setting.
            Some(m @ ("host" | "none")) => {
                svc.insert("network_mode".into(), json!(m));
            }
            Some(n) if cat::valid_service_name(n) => {
                svc.insert("networks".into(), json!([n]));
                doc.insert("networks".into(), json!({ n: { "external": true } }));
            }
            Some(_) => return Err("network: a Docker network name".into()),
        }

        doc.insert("services".into(), json!({ &self.name: svc }));

        if !named.is_empty() {
            doc.insert("volumes".into(), Value::Object(named));
        }

        Ok((cat::to_yaml(&Value::Object(doc)), env))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn compose_parses_with_merge_keys_and_yaml_1_2_scalars() {
        let v = parse_compose(
            "version: '3.8'\nx-common: &c\n  restart: always\nservices:\n  web:\n    <<: *c\n    image: nginx:1.27\n    environment:\n      DEBUG: yes\n      MODE: '0755'\n      N: 0755\n",
        )
        .unwrap();
        assert!(v.get("version").is_none());
        assert_eq!(v.pointer("/services/web/restart").unwrap(), "always");
        assert_eq!(v.pointer("/services/web/environment/DEBUG").unwrap(), "yes");
        assert_eq!(v.pointer("/services/web/environment/N").unwrap(), "0755");
    }

    #[test]
    fn hostile_yaml_is_refused() {
        let mut laughs = String::from("a: &a [x,x,x,x,x,x,x,x,x]\n");
        let mut p = 'a';

        for c in "bcdefghij".chars() {
            laughs += &format!("{c}: &{c} [*{p},*{p},*{p},*{p},*{p},*{p},*{p},*{p},*{p}]\n");
            p = c;
        }

        assert!(parse_compose(&laughs).is_err());
        assert!(parse_compose(&("[".repeat(10_000) + &"]".repeat(10_000))).is_err());
        assert!(parse_compose(&"a: 1\n".repeat(200_000)).is_err(), "size cap");
        assert!(parse_compose("a: 1\na: 2\n").is_err(), "duplicate keys");
        assert!(parse_compose("- a\n- b\n").is_err());
        assert!(parse_compose("services: {}\n---\nservices: {}\n").is_err(), "one document");
        assert!(parse_compose("").is_err());
    }

    #[test]
    fn env_values_are_masked_in_previews() {
        let svc = json!({ "environment": { "DB_PASSWORD": "hunter2", "API_KEY": "${API_KEY}", "MODE": "prod" } });
        let e = masked_env(&svc);
        let get = |k: &str| e.iter().find(|x| x["key"] == k).map(|x| x["value"].clone()).unwrap();
        assert_eq!(get("API_KEY"), "${API_KEY}");
        assert_eq!(get("DB_PASSWORD"), "••••••");
        assert_eq!(get("MODE"), "prod");
        let list = json!({ "environment": ["TOKEN=abc", "X=1"] });
        assert_eq!(masked_env(&list)[0]["value"], "••••••");
    }

    #[test]
    fn container_form_generates_a_checked_one_service_compose() {
        let form: ContainerForm = serde_json::from_value(json!({
            "name": "web", "image": "nginx:1.27",
            "ports": [{ "container": 80, "host": 8080 }, { "container": 443, "public": true }],
            "env": [{ "key": "DB_PASSWORD", "value": "s3cret" }],
            "volumes": [{ "source": "data", "target": "/data" }, { "source": "/srv/data/www", "target": "/www", "read_only": true }],
            "cpus": 0.5, "memory_mb": 256, "expose": "http",
            "healthcheck": { "test": "curl -f http://localhost/" }
        }))
        .unwrap();
        let (yaml, env) = form.to_compose().unwrap();
        assert_eq!(env["DB_PASSWORD"], "s3cret");
        assert!(!yaml.contains("s3cret"), "values live in .env only: {yaml}");
        let v = parse_compose(&yaml).unwrap();
        assert_eq!(v.pointer("/services/web/environment/DB_PASSWORD").unwrap(), "${DB_PASSWORD}");
        assert_eq!(v.pointer("/services/web/ports/0").unwrap(), "8080:80/tcp");
        assert_eq!(v.pointer("/services/web/x-forge/public/0").unwrap(), 443);
        assert_eq!(v.pointer("/services/web/labels/forge.expose").unwrap(), "http");
        assert_eq!(v.pointer("/services/web/deploy/resources/limits/memory").unwrap(), "256M");
        assert_eq!(v.pointer("/volumes/data").unwrap(), &json!({}));
        let policy = Policy {
            allow_privileged: false,
            bind_roots: vec!["/srv/data".into()],
            stack_dir: "/opt/forge/stacks/a/web".into(),
        };
        let i = cat::interpolate(&v, &env).unwrap();
        assert!(cat::check(&i, &policy).blocking(false).is_empty());
        let bad: ContainerForm = serde_json::from_value(json!({ "name": "x", "image": "a b" })).unwrap();
        assert!(bad.to_compose().is_err());
        let bad: ContainerForm =
            serde_json::from_value(json!({ "name": "x", "image": "a", "env": [{ "key": "A-B" }] })).unwrap();
        assert!(bad.to_compose().is_err());
        assert!(
            serde_json::from_value::<ContainerForm>(json!({ "name": "x", "image": "a", "privileged": true })).is_err()
        );

        // The host network needs stack.privileged, however it is spelled.
        let host: ContainerForm =
            serde_json::from_value(json!({ "name": "x", "image": "a", "network": "host" })).unwrap();
        let v = parse_compose(&host.to_compose().unwrap().0).unwrap();
        let r = cat::check(&v, &policy);
        assert!(r.privileged && !r.blocking(false).is_empty(), "{v}");
        let bridge: ContainerForm =
            serde_json::from_value(json!({ "name": "x", "image": "a", "network": "proxy" })).unwrap();
        assert!(!cat::check(&parse_compose(&bridge.to_compose().unwrap().0).unwrap(), &policy).privileged);
        let bad: ContainerForm =
            serde_json::from_value(json!({ "name": "x", "image": "a", "network": "a b" })).unwrap();
        assert!(bad.to_compose().is_err());
    }

    #[test]
    fn mesh_is_decided_from_fabric_fact_and_opt_out() {
        let up = FabricNode { ip: Some("10.64.0.2".into()), network: Some("forge-forgea1b2c3".into()) };
        let synced_without_net = FabricNode { ip: Some("10.64.0.2".into()), network: None };

        assert_eq!(decide_mesh(Some(&up), true, false, "lab-b"), (Some("forge-forgea1b2c3".into()), None));
        assert_eq!(decide_mesh(Some(&up), true, true, "lab-b"), (None, None));
        assert_eq!(decide_mesh(None, true, false, "lab-b"), (None, Some(NOTE_NO_FABRIC.to_owned())));
        assert_eq!(
            decide_mesh(Some(&synced_without_net), true, false, "lab-b").1.unwrap(),
            "lab-b: cluster network not applied yet; services will join it on the next deploy after it is in sync"
        );
        assert_eq!(
            decide_mesh(Some(&up), false, false, "lab-b"),
            (None, Some("lab-b: update its agent to put services on the cluster network".to_owned()))
        );
    }
}
