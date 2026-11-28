//! Fleet-wide local names (D-084).
//!
//! Every enrolled node gets the names of its workspace's nodes and services
//! in a marked block of its hosts file:
//!
//! * a node: `<label>.<cluster>.<suffix>` in a cluster, `<label>.<suffix>`
//!   outside one;
//! * a service with an active instance: `<app>.<ns>.<cluster>.<suffix>`
//!   (`<app>.<ns>.<suffix>` without a cluster), one line per node running it.
//!
//! A node's label is the one a user set (`node_names`, unique in the
//! workspace) or one derived from its console name or hostname; derived
//! labels that collide get `-<first 6 of the id>`. Addresses are the mesh
//! address when both ends are in the same fabric, otherwise the address the
//! server sees the node's agent connect from (never what a node says about
//! itself). Ports come from the stored app specs.
//!
//! Sync: the lists are rebuilt every pass, and [`DEBOUNCE`] after a rename or
//! a change that calls [`poke`] (a burst of pokes makes one pass). A pass
//! reads each table once for the whole fleet, and nodes that see the same
//! names share one list. A node whose list changed (at most once per
//! [`CHANGE_GAP`]), that reconnected or that has not been sent its list for
//! [`RESEND`] gets a signed `names` command on its next heartbeat. The agent replaces only its block, so a
//! resend is a no-op on the node.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use forge_catalog::fabric::Cidr;
use forge_catalog::fabric::{Lease, LeaseKind};
use forge_catalog::names::{
    NameEntry, PublishedPort, RecordInput, Records, SrvPort, build_records, check_label, first_container_port,
    label_from, published_port,
};
use forge_core::model::{AgentStatus, Server, Service, ServiceInstance};
use forge_core::ports::{
    ClusterRepository, FabricRepository, ServerRepository, ServiceRepository, SettingsRepository, TenancyRepository,
};
use forge_core::tags::FactsRepository;
use forge_core::{ClusterId, ServerId, ServiceId, WorkspaceId};
use forge_proto::Command;
use forge_proto::messages::HostNames;
use serde::Serialize;
use tokio::sync::{Notify, watch};

use crate::state::AppState;

/// Who signs the command (visible in the agent's log).
pub const SYSTEM_PRINCIPAL: &str = "system:names";
/// A pass over the fleet this often (and on [`poke`]).
const TICK: Duration = Duration::from_secs(30);
/// Resend an unchanged list this often (repairs a hand-edited hosts file
/// and a container restarted since; the node skips an unchanged file).
const RESEND: Duration = Duration::from_secs(10 * 60);
/// A burst of pokes (a fleet enrolling or reconnecting) waits this long and
/// becomes one pass.
const DEBOUNCE: Duration = Duration::from_secs(2);
/// A changed list is sent at most this often per node; the last change in a
/// burst still goes out on the next heartbeat or pass after the gap.
const CHANGE_GAP: Duration = Duration::from_secs(10);
/// Longest wait after failures before trying again.
const MAX_BACKOFF_MS: i64 = 60 * 60 * 1000;

// ------------------------------------------------------------- labels

/// A node's label: the one set by a user, else derived.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NodeLabel {
    pub label: String,
    /// A user set it (`PUT /servers/{id}/dns-name`).
    pub explicit: bool,
}

/// The last `n` letters and digits of an id (the random end of a v7 UUID).
fn id_tail(id: &str, n: usize) -> String {
    let chars: Vec<char> = id.chars().filter(char::is_ascii_alphanumeric).map(|c| c.to_ascii_lowercase()).collect();
    chars[chars.len().saturating_sub(n)..].iter().collect()
}

fn short_id(id: ServerId) -> String {
    id_tail(&id.to_string(), 6)
}

/// An environment's (environment's) slug: what services' specs and node paths
/// use (`/srv/forge/apps/<slug>/…`) and what `forge.deploy.yaml` names in
/// `metadata.environment`.
#[must_use]
pub fn environment_slug(s: &str) -> String {
    let s: String = s.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let s = s.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-");
    if s.starts_with(|c: char| c.is_ascii_digit()) { format!("ns-{s}") } else { s }
}

/// The label derived from a node's console name, else its hostname's first
/// label, else `node-<id>`.
#[must_use]
pub fn derived_label(s: &Server) -> String {
    s.display_name
        .as_deref()
        .and_then(label_from)
        .or_else(|| label_from(s.hostname.split('.').next().unwrap_or_default()))
        .unwrap_or_else(|| format!("node-{}", short_id(s.id)))
}

/// Stored labels of a workspace: node → (label, set by a user).
pub type StoredLabels = HashMap<ServerId, (String, bool)>;

/// A label for `s` that is not in `taken`: its derived label, with the end
/// of its id added when that is taken.
#[must_use]
pub fn free_label(s: &Server, taken: &HashSet<String>) -> String {
    let base = derived_label(s);
    let mut label = base.clone();
    let mut n = 6;

    while taken.contains(&label) && n <= 32 {
        let tail = id_tail(&s.id.to_string(), n);
        let head: String = base.chars().take(63 - tail.len() - 1).collect();
        label = format!("{}-{tail}", head.trim_end_matches('-'));
        n += 4;
    }

    label
}

/// Labels of one workspace's nodes. A stored label (set by a user, or
/// derived once and kept) never moves; a node without one gets a derived
/// label, in enrollment order, that no other node has (review of D-084:
/// renaming one node never changes another node's name).
#[must_use]
pub fn node_labels(servers: &[&Server], stored: &StoredLabels) -> HashMap<ServerId, NodeLabel> {
    let mut out = HashMap::new();
    let mut taken: HashSet<String> = stored.values().map(|(l, _)| l.clone()).collect();

    for s in servers {
        if let Some((l, explicit)) = stored.get(&s.id).filter(|(l, _)| check_label(l).is_ok()) {
            out.insert(s.id, NodeLabel { label: l.clone(), explicit: *explicit });
        }
    }

    let mut rest: Vec<&&Server> = servers.iter().filter(|s| !out.contains_key(&s.id)).collect();
    rest.sort_by_key(|s| (s.created_at.0, s.id.to_string()));

    for s in rest {
        let label = free_label(s, &taken);
        taken.insert(label.clone());
        out.insert(s.id, NodeLabel { label, explicit: false });
    }

    out
}

/// A workspace's stored labels.
pub async fn stored_labels(state: &AppState, ws: WorkspaceId) -> Result<StoredLabels, String> {
    Ok(state
        .store
        .list_node_labels(Some(ws))
        .await
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|(id, l, explicit)| (id, (l, explicit)))
        .collect())
}

// ------------------------------------------------------------- the fleet

/// One name in the fleet: what it is, where it points and its ports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NameRow {
    pub name: String,
    /// `node`, `service`, `stack-service` (every healthy replica of a
    /// compose service) or `replica` (one compose container).
    pub kind: &'static str,
    pub server_id: ServerId,
    /// The node's label (for a service, the node running it).
    pub node: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cluster_id: Option<ClusterId>,
    /// The address other nodes outside its fabric use.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<IpAddr>,
    /// The address nodes in the same fabric use.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mesh_address: Option<Ipv4Addr>,
    /// Service: `<environment>/<app>`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    /// Service: ports published on the node.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ports: Vec<PublishedPort>,
    /// Stack service: the port its SRV record names (forge-dns only, never
    /// the hosts file).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub srv: Option<SrvPort>,
}

/// Row kinds.
pub const KIND_NODE: &str = "node";
pub const KIND_SERVICE: &str = "service";
pub const KIND_STACK_SERVICE: &str = "stack-service";
pub const KIND_REPLICA: &str = "replica";

/// One compose container as the stack node state records it.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct StateRow {
    #[serde(default)]
    pub service: String,
    #[serde(default)]
    pub number: Option<u32>,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub health: Option<String>,
    #[serde(default)]
    pub mesh_ip: Option<Ipv4Addr>,
}

/// A deployed stack, as names need it.
#[derive(Debug, Clone)]
pub struct StackNames {
    pub workspace_id: WorkspaceId,
    pub environment_id: forge_core::EnvironmentId,
    pub stack: String,
    /// Compose services with a container port: the first one and its protocol.
    pub ports: HashMap<String, (u16, String)>,
    /// Container rows of every node part.
    pub rows: Vec<(ServerId, StateRow)>,
}

/// Where one stack's names live besides the stack itself.
#[derive(Debug)]
pub struct StackLabels<'a> {
    pub environment: &'a str,
    pub cluster: Option<ClusterId>,
}

/// Builds a full name from labels and a cluster (`None` when invalid).
pub type NameBuilder<'a> = dyn Fn(&[&str], Option<ClusterId>) -> Option<String> + 'a;

/// The `stack-service` and `replica` rows of one stack (D-175). Only running containers on the cluster network have an address,
/// and only services with a container port get a name. `name` builds a
/// full name from labels and the cluster.
pub fn stack_rows(
    st: &StackNames,
    labels: &StackLabels<'_>,
    node_label: &dyn Fn(ServerId) -> String,
    name: &NameBuilder<'_>,
) -> Vec<NameRow> {
    let mut out = Vec::new();
    let Some(stack) = label_from(&st.stack) else { return out };

    for (server, r) in &st.rows {
        let (Some(ip), Some((port, proto))) = (r.mesh_ip, st.ports.get(&r.service)) else { continue };
        let Some(svc) = label_from(&r.service) else { continue };

        if r.state != "running" {
            continue;
        }

        let row = |name: String, kind: &'static str, srv: Option<SrvPort>| NameRow {
            name,
            kind,
            server_id: *server,
            node: node_label(*server),
            cluster_id: labels.cluster,
            address: None,
            mesh_address: Some(ip),
            app: Some(format!("{}/{}", labels.environment, st.stack)),
            ports: Vec::new(),
            srv,
        };
        let healthy = r.health.as_deref().is_none_or(|h| h.is_empty() || h == "healthy");

        if healthy && let Some(n) = name(&[&svc, &stack, labels.environment], labels.cluster) {
            let srv = SrvPort { service: svc.clone(), port: *port, proto: proto.clone() };
            out.push(row(n, KIND_STACK_SERVICE, Some(srv)));
        }

        let replica = r.number.map(|n| format!("{svc}-{n}"));

        if let Some(n) = replica.and_then(|rep| name(&[&rep, &stack, labels.environment], labels.cluster)) {
            out.push(row(n, KIND_REPLICA, None));
        }
    }

    out
}

/// Everything the lists are built from, for one workspace.
#[derive(Debug, Default)]
pub struct Fleet {
    pub suffix: String,
    pub servers: Vec<Server>,
    pub labels: HashMap<ServerId, NodeLabel>,
    pub rows: Vec<NameRow>,
    /// Cluster labels, for each cluster's zone (`<cluster>.<suffix>`).
    pub clusters: HashMap<ClusterId, String>,
    /// Fabric subnets: their addresses get PTR records.
    pub subnets: HashMap<ClusterId, Cidr>,
}

impl Fleet {
    /// The list one node writes: every row with the address it should use.
    #[must_use]
    pub fn entries_for(&self, target: &Server) -> Vec<NameEntry> {
        let mut out: Vec<NameEntry> = self
            .rows
            .iter()
            .filter_map(|r| {
                let same_fabric = r.cluster_id.is_some() && r.cluster_id == target.cluster_id;
                let ip = match (same_fabric, r.mesh_address, r.address) {
                    (true, Some(m), _) => IpAddr::V4(m),
                    (_, _, Some(a)) if !a.is_loopback() || r.server_id == target.id => a,
                    _ => return None,
                };

                Some(NameEntry { name: r.name.clone(), ip })
            })
            .filter(|e| e.validate().is_ok())
            .collect();
        out.sort();
        out.dedup();
        out.truncate(forge_catalog::names::MAX_ENTRIES);

        out
    }
}

/// The cluster label of each cluster of a workspace (derived from its name;
/// a collision gets `-<id>`).
fn cluster_labels(clusters: &[forge_core::model::Cluster]) -> HashMap<ClusterId, String> {
    let mut sorted: Vec<&forge_core::model::Cluster> = clusters.iter().collect();
    sorted.sort_by_key(|c| (c.created_at.0, c.id.to_string()));
    let mut taken = HashSet::new();
    let mut out = HashMap::new();

    for c in sorted {
        let tail = id_tail(&c.id.to_string(), 6);
        let mut l = label_from(&c.name).unwrap_or_else(|| format!("cluster-{tail}"));
        if !taken.insert(l.clone()) {
            l = format!("{}-{tail}", l.chars().take(56).collect::<String>().trim_end_matches('-'));
            taken.insert(l.clone());
        }

        out.insert(c.id, l);
    }

    out
}

/// Settings key of the suffix set at runtime (`PUT /api/v1/names/suffix`).
pub const SUFFIX_KEY: &str = "names.suffix";

/// Where the suffix in use comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SuffixSource {
    /// The runtime setting (`names.suffix` in the settings store).
    Setting,
    /// `[names] suffix` in the config file (or `FORGE_NAMES_SUFFIX`).
    Config,
    /// Neither is set: `local`.
    Default,
}

/// The suffix set at runtime, when there is a valid one.
pub async fn stored_suffix(state: &AppState) -> Option<String> {
    let all = state.store.all_settings().await.ok()?;
    all.into_iter()
        .find(|(k, _)| k == SUFFIX_KEY)
        .map(|(_, v)| v)
        .filter(|v| forge_catalog::names::check_suffix(v).is_ok())
}

/// The suffix in use and where it comes from: the runtime setting, else the
/// config file, else `local`.
pub async fn suffix_with_source(state: &AppState) -> (String, SuffixSource) {
    if let Some(s) = stored_suffix(state).await {
        return (s, SuffixSource::Setting);
    }

    let c = &state.config.names.suffix;
    let src = if c == forge_catalog::names::DEFAULT_SUFFIX { SuffixSource::Default } else { SuffixSource::Config };

    (c.clone(), src)
}

/// The suffix in use.
pub async fn suffix(state: &AppState) -> String {
    suffix_with_source(state).await.0
}

/// Rows every workspace's lists are built from, loaded once per pass
/// (not once per workspace, and not one query per service).
struct Shared {
    suffix: String,
    instances: Vec<ServiceInstance>,
    apps: HashMap<ServiceId, Service>,
    /// The address in the stored facts, for nodes with no live connection.
    stored_address: HashMap<ServerId, String>,
    /// Deployed stacks with their container rows.
    stacks: Vec<StackNames>,
}

async fn load_shared(state: &AppState, servers: &[Server]) -> Result<Shared, String> {
    let e = |e: forge_core::DomainError| e.to_string();
    let suffix = suffix(state).await;
    let instances = state.store.list_all_instances(None).await.map_err(e)?;
    let ids: Vec<ServiceId> = instances
        .iter()
        .filter(|i| i.role != "standby")
        .map(|i| i.service_id)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let apps = state.store.get_services(&ids).await.map_err(e)?.into_iter().map(|a| (a.id, a)).collect();

    // Connected nodes have a live address; the facts table (wide JSON rows)
    // is read only when some node has none.
    let stored_address = if servers.iter().any(|s| crate::facts::live_address(s.id).is_none()) {
        match state.store.all_facts().await {
            Ok(all) => {
                all.into_iter().filter_map(|(id, (f, _))| f["address"].as_str().map(|a| (id, a.to_owned()))).collect()
            }
            Err(err) => {
                // Only offline nodes lose their address until the next pass;
                // the rest of the list is still correct.
                tracing::warn!(error = %err, "reading facts for local names failed");
                HashMap::new()
            }
        }
    } else {
        HashMap::new()
    };

    let stacks = load_stacks(state).await.map_err(e)?;

    Ok(Shared { suffix, instances, apps, stored_address, stacks })
}

/// Every deployed stack with the container ports of its compose services
/// and its nodes' container rows. One revision read per deployed stack.
async fn load_stacks(state: &AppState) -> forge_core::DomainResult<Vec<StackNames>> {
    use forge_core::stacks::StackRepository;

    let mut out = Vec::new();

    for st in state.store.list_stacks(None).await?.into_iter().filter(|s| s.deployed_revision > 0) {
        let Some(rev) = state.store.get_revision(st.id, st.deployed_revision).await? else { continue };

        // A stored revision that no longer parses names nothing; its deploy
        // already failed on the same text.
        let tree = match crate::stacks::parse_compose(&rev.content) {
            Ok(t) => t,
            Err(err) => {
                tracing::debug!(stack = %st.id, error = %err, "deployed revision does not parse; no names");
                continue;
            }
        };

        let services = tree.get("services").and_then(serde_json::Value::as_object).cloned().unwrap_or_default();
        let ports = services.iter().filter_map(|(k, v)| Some((k.clone(), first_container_port(v)?))).collect();
        let mut rows = Vec::new();

        for node in state.store.list_nodes(st.id).await? {
            // Rows are what the agent reported; a malformed one is skipped.
            let list: Vec<serde_json::Value> = node.state.as_array().cloned().unwrap_or_default();
            rows.extend(list.into_iter().filter_map(|v| serde_json::from_value(v).ok()).map(|r| (node.server_id, r)));
        }

        out.push(StackNames {
            workspace_id: st.workspace_id,
            environment_id: st.environment_id,
            stack: st.name,
            ports,
            rows,
        });
    }

    Ok(out)
}

/// Builds one workspace's fleet.
pub async fn fleet(state: &AppState, ws: WorkspaceId) -> Result<Fleet, String> {
    let servers = state.store.list_servers(Some(ws)).await.map_err(|e| e.to_string())?;
    let shared = load_shared(state, &servers).await?;

    build_fleet(state, ws, servers, &shared).await
}

/// One workspace's fleet from its servers and the rows of this pass.
async fn build_fleet(
    state: &AppState,
    ws: WorkspaceId,
    mut servers: Vec<Server>,
    shared: &Shared,
) -> Result<Fleet, String> {
    let e = |e: forge_core::DomainError| e.to_string();
    let suffix = shared.suffix.clone();

    // A revoked node has no name (its stored label stays taken, so it is
    // never handed to another node by accident).
    servers.retain(|s| s.status != AgentStatus::Revoked);
    let stored = stored_labels(state, ws).await?;
    let refs: Vec<&Server> = servers.iter().collect();
    let labels = node_labels(&refs, &stored);

    // Keep each new node's derived label from now on. A clash with a label
    // stored meanwhile is retried on the next pass.
    for (id, l) in labels.iter().filter(|(id, _)| !stored.contains_key(*id)) {
        if let Err(err) = state.store.claim_node_label(*id, ws, &l.label, state.clock.now()).await {
            tracing::debug!(server = %id, error = %err, "storing a derived node label failed; will retry");
        }
    }

    let clusters = cluster_labels(&state.store.list_clusters(ws).await.map_err(e)?);
    let address = |id: ServerId| -> Option<IpAddr> {
        crate::facts::live_address(id)
            .or_else(|| shared.stored_address.get(&id).cloned())
            .and_then(|a| a.parse::<IpAddr>().ok())
            .map(|a| a.to_canonical())
            .filter(|a| !a.is_unspecified() && !a.is_multicast())
    };
    let cluster_of: HashMap<ServerId, Option<ClusterId>> = servers.iter().map(|s| (s.id, s.cluster_id)).collect();

    // Mesh addresses of nodes in applied fabrics.
    let mut mesh: HashMap<ServerId, Ipv4Addr> = HashMap::new();

    for f in state.store.list_fabrics(Some(ws)).await.map_err(e)? {
        let leases: Vec<Lease> = serde_json::from_value(f.leases.clone()).unwrap_or_default();

        for l in leases.iter().filter(|l| l.kind == LeaseKind::Node) {
            let Ok(id) = l.owner.parse::<ServerId>() else { continue };
            if cluster_of.get(&id).copied().flatten() == Some(f.cluster_id) {
                mesh.entry(id).or_insert_with(|| l.cidr.first());
            }
        }
    }

    let name = |labels: &[&str], cluster: Option<ClusterId>| -> Option<String> {
        let mut parts: Vec<&str> = labels.to_vec();
        let cl = cluster.and_then(|c| clusters.get(&c));
        if let Some(c) = cl {
            parts.push(c);
        }

        parts.push(&suffix);
        let n = parts.join(".");
        forge_catalog::names::valid_name(&n).then_some(n)
    };
    let mut rows = Vec::new();

    for s in &servers {
        let Some(l) = labels.get(&s.id) else { continue };
        if let Some(n) = name(&[&l.label], s.cluster_id) {
            rows.push(NameRow {
                name: n,
                kind: KIND_NODE,
                server_id: s.id,
                node: l.label.clone(),
                cluster_id: s.cluster_id,
                address: address(s.id),
                mesh_address: mesh.get(&s.id).copied(),
                app: None,
                ports: Vec::new(),
                srv: None,
            });
        }
    }

    // Services: active instances of the workspace's services.
    let environments: HashMap<_, _> =
        state.store.list_environments(ws, None).await.map_err(e)?.into_iter().map(|n| (n.id, n)).collect();

    for i in shared.instances.iter().filter(|i| i.role != "standby") {
        let Some(cluster_id) = cluster_of.get(&i.server_id).copied() else { continue };
        let Some(app) = shared.apps.get(&i.service_id) else { continue };
        let Some(ns) = environments.get(&app.environment_id) else { continue };
        let (Some(a), Some(n)) = (label_from(&app.name), label_from(&ns.name)) else { continue };
        let Some(full) = name(&[&a, &n], ns.cluster_id) else { continue };
        let ports = app
            .spec
            .get("ports")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(serde_json::Value::as_str)
            .filter_map(published_port)
            .collect();
        rows.push(NameRow {
            name: full,
            kind: KIND_SERVICE,
            server_id: i.server_id,
            node: labels.get(&i.server_id).map(|l| l.label.clone()).unwrap_or_default(),
            // The service's address follows its node's fabric.
            cluster_id,
            address: address(i.server_id),
            mesh_address: mesh.get(&i.server_id).copied(),
            app: Some(format!("{}/{}", ns.name, app.name)),
            ports,
            srv: None,
        });
    }

    // Stacks: every running container on the cluster network.
    let node_label = |id: ServerId| labels.get(&id).map(|l| l.label.clone()).unwrap_or_default();

    for st in shared.stacks.iter().filter(|s| s.workspace_id == ws) {
        let Some(ns) = environments.get(&st.environment_id) else { continue };
        let Some(env) = label_from(&ns.name) else { continue };
        let l = StackLabels { environment: &env, cluster: ns.cluster_id };
        rows.extend(stack_rows(st, &l, &node_label, &name));
    }

    resolve_clashes(&mut rows);
    rows.sort_by(|a, b| a.name.cmp(&b.name).then(a.node.cmp(&b.node)).then(a.mesh_address.cmp(&b.mesh_address)));
    rows.dedup_by(|a, b| a.name == b.name && a.mesh_address == b.mesh_address && a.address == b.address);

    let subnets = fabric_subnets(&state.store.list_fabrics(Some(ws)).await.map_err(e)?);

    Ok(Fleet { suffix, servers, labels, rows, clusters, subnets })
}

/// A service name that equals a node name: the node keeps it. A replica
/// name that equals a service or stack-service name: the service keeps it.
pub fn resolve_clashes(rows: &mut Vec<NameRow>) {
    let nodes: HashSet<String> = rows.iter().filter(|r| r.kind == KIND_NODE).map(|r| r.name.clone()).collect();
    rows.retain(|r| r.kind == KIND_NODE || !nodes.contains(&r.name));

    let services: HashSet<String> = rows
        .iter()
        .filter(|r| r.kind == KIND_SERVICE || r.kind == KIND_STACK_SERVICE)
        .map(|r| r.name.clone())
        .collect();
    rows.retain(|r| r.kind != KIND_REPLICA || !services.contains(&r.name));
}

/// Each applied fabric's subnet.
fn fabric_subnets(fabrics: &[forge_core::model::FabricRecord]) -> HashMap<ClusterId, Cidr> {
    fabrics
        .iter()
        .filter_map(|f| {
            let spec: forge_catalog::fabric::FabricSpec = serde_json::from_value(f.spec.clone()).ok()?;

            Some((f.cluster_id, spec.subnet))
        })
        .collect()
}

/// A node's full name in its workspace, for the API.
#[must_use]
pub fn node_name(fleet: &Fleet, id: ServerId) -> Option<&NameRow> {
    fleet.rows.iter().find(|r| r.kind == "node" && r.server_id == id)
}

// ------------------------------------------------------------- sync

/// What one node is sent: its hosts-file list and, inside a fabric, its
/// cluster's record set for forge-dns.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NodeNames {
    pub entries: Vec<NameEntry>,
    /// `None` outside a fabric (no forge-dns there).
    pub records: Option<Records>,
    /// The zone `records` answer for (`<cluster>.<suffix>`); empty without.
    pub zone: String,
}

/// A node's names and their hash.
type Desired = (u64, Arc<NodeNames>);

impl Fleet {
    /// Every node's list. Two nodes get the same list unless they sit in
    /// different fabrics or one has a loopback address of its own (only it
    /// sees that row), so a list is built and hashed once per (fabric,
    /// loopback owner) and shared: a few lists for a thousand nodes, not a
    /// thousand lists of a thousand names.
    #[must_use]
    fn lists(&self) -> HashMap<ServerId, Desired> {
        let loopback: HashSet<ServerId> =
            self.rows.iter().filter(|r| r.address.is_some_and(|a| a.is_loopback())).map(|r| r.server_id).collect();
        let mut built: HashMap<(Option<ClusterId>, Option<ServerId>), Desired> = HashMap::new();
        let mut out = HashMap::with_capacity(self.servers.len());

        for s in &self.servers {
            let key = (s.cluster_id, loopback.contains(&s.id).then_some(s.id));
            let d = built
                .entry(key)
                .or_insert_with(|| {
                    let names = NodeNames { entries: self.entries_for(s), ..self.records_for(s.cluster_id) };
                    (hash(&names), Arc::new(names))
                })
                .clone();
            out.insert(s.id, d);
        }

        out
    }

    /// The record set of a cluster with a fabric: every row in it with a
    /// mesh address. `generated_at_ms` is set when it is sent, so the hash
    /// changes only with the records.
    fn records_for(&self, cluster: Option<ClusterId>) -> NodeNames {
        let Some(c) = cluster else { return NodeNames::default() };

        let (Some(subnet), Some(label)) = (self.subnets.get(&c), self.clusters.get(&c)) else {
            return NodeNames::default();
        };

        let zone = format!("{label}.{}", self.suffix);
        let inputs: Vec<RecordInput> = self
            .rows
            .iter()
            .filter(|r| r.cluster_id == Some(c))
            .filter_map(|r| {
                Some(RecordInput {
                    name: r.name.clone(),
                    ip: r.mesh_address?,
                    srv: r.srv.clone(),
                    ptr_preferred: r.kind == KIND_REPLICA || r.kind == KIND_NODE,
                })
            })
            .collect();
        let records = build_records(std::slice::from_ref(&zone), &inputs, &[*subnet], 0);

        NodeNames { entries: Vec::new(), records: Some(records), zone }
    }
}

#[derive(Debug, Default)]
struct Sync {
    /// What each node should have: hash and list.
    desired: HashMap<ServerId, Desired>,
    /// The suffix the lists were built with (sent along; read once per pass,
    /// not once per send).
    suffix: String,
    /// What each node was last sent successfully, and when.
    sent: HashMap<ServerId, (u64, i64)>,
    inflight: HashSet<ServerId>,
    /// Failures in a row and when to try again.
    failed: HashMap<ServerId, (u32, i64, String)>,
}

fn sync() -> &'static Mutex<Sync> {
    static S: OnceLock<Mutex<Sync>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(Sync::default()))
}

fn wake() -> &'static Notify {
    static N: OnceLock<Notify> = OnceLock::new();
    N.get_or_init(Notify::new)
}

/// Rebuild the lists soon (after a rename, enrollment or deployment). Pokes
/// within [`DEBOUNCE`] of each other make one pass.
pub fn poke() {
    wake().notify_one();
}

fn hash(names: &NodeNames) -> u64 {
    // FNV-1a over the rendered lines and records: stable across restarts.
    let lines = names.entries.iter().flat_map(|e| format!("{}\t{}\n", e.ip, e.name).into_bytes());

    // Records serialise deterministically (sorted); an error hashes as empty.
    let records = names.records.as_ref().and_then(|r| serde_json::to_vec(&r.records).ok()).unwrap_or_default();

    lines
        .chain(names.zone.bytes())
        .chain(records)
        .fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

/// Sync state of one node, for the API.
#[derive(Debug, Clone, Serialize)]
pub struct NodeSync {
    pub names: usize,
    pub in_sync: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub synced_at_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Where one node stands.
#[must_use]
pub fn node_sync(id: ServerId) -> Option<NodeSync> {
    let s = sync().lock().ok()?;
    let (h, list) = s.desired.get(&id)?;
    let sent = s.sent.get(&id);

    Some(NodeSync {
        names: list.entries.len(),
        in_sync: sent.is_some_and(|(sh, _)| sh == h),
        synced_at_ms: sent.map(|(_, at)| *at),
        error: s.failed.get(&id).map(|f| f.2.clone()),
    })
}

/// Called on every heartbeat: sends the node its list when due. A node
/// that comes back online gets it again. Cheap when nothing is due.
pub fn on_heartbeat(state: &AppState, server: ServerId, reconnected: bool) {
    if !state.config.names.enabled {
        return;
    }

    let now = state.clock.now().0;

    let due = {
        let Ok(mut s) = sync().lock() else { return };

        if reconnected {
            s.sent.remove(&server);
        }

        if !s.desired.contains_key(&server) {
            // A node just enrolled: build its list on the next (debounced)
            // pass, not on the 30 s tick.
            poke();
        }

        due_now(&mut s, server, now).map(|d| (d, s.suffix.clone()))
    };

    if let Some((list, suffix)) = due {
        spawn_send(state, server, list, suffix);
    }
}

fn due_now(s: &mut Sync, server: ServerId, now: i64) -> Option<Desired> {
    let (h, list) = s.desired.get(&server)?.clone();

    if s.inflight.contains(&server) || s.failed.get(&server).is_some_and(|f| f.1 > now) {
        return None;
    }

    let ms = |d: Duration| i64::try_from(d.as_millis()).unwrap_or(i64::MAX);

    match s.sent.get(&server) {
        // In sync: only the periodic repair resend.
        Some((sh, at)) if *sh == h && now - at < ms(RESEND) => return None,
        // Changed, but sent a moment ago: while a fleet enrolls every list
        // changes on every pass; a node gets at most one list per gap.
        Some((sh, at)) if *sh != h && now - at < ms(CHANGE_GAP) => return None,
        _ => {}
    }

    s.inflight.insert(server);

    Some((h, list))
}

fn spawn_send(state: &AppState, server: ServerId, (h, list): Desired, suffix: String) {
    let state = state.clone();
    tokio::spawn(async move {
        let r = send(&state, server, &list, suffix).await;
        let now = state.clock.now().0;
        let Ok(mut s) = sync().lock() else { return };
        s.inflight.remove(&server);

        match r {
            Ok(()) => {
                s.sent.insert(server, (h, now));
                s.failed.remove(&server);
            }
            Err(e) => {
                let n = s.failed.get(&server).map_or(1, |f| f.0 + 1);
                let wait = (30_000_i64 << (n - 1).min(10)).min(MAX_BACKOFF_MS);
                tracing::debug!(%server, error = %e, "sending names failed; will retry");
                s.failed.insert(server, (n, now + wait, e));
            }
        }
    });
}

async fn send(state: &AppState, server: ServerId, names: &NodeNames, suffix: String) -> Result<(), String> {
    let entries_json = serde_json::to_string(&names.entries).map_err(|e| e.to_string())?;

    let records_json = match &names.records {
        Some(r) => {
            let stamped = Records { generated_at_ms: state.clock.now().0, ..r.clone() };
            serde_json::to_string(&stamped).map_err(|e| e.to_string())?
        }
        None => String::new(),
    };

    let cmd = Command::HostNames(HostNames {
        entries_json,
        containers: state.config.names.containers,
        suffix,
        records_json,
        zone: names.zone.clone(),
        upstream_forge_dns: state.config.dns.second_tier().iter().map(ToString::to_string).collect(),
    });
    let (body, signed) =
        state.signer.issue(&server.to_string(), SYSTEM_PRINCIPAL, cmd, state.clock.now(), Duration::from_secs(120));
    state.hub.enqueue(server, &body.operation_id, signed).map_err(|e| e.to_string())?;
    let r = state.hub.wait(&body.operation_id, Duration::from_secs(90)).await.ok_or("the node did not answer")?;
    if r.ok { Ok(()) } else { Err(r.error) }
}

/// Rebuilds every node's list; nodes online with a changed list are sent
/// it at once (the held heartbeat delivers it). One query per table for the
/// whole fleet plus a few per workspace, whatever the number of nodes.
pub async fn refresh(state: &AppState) {
    if !state.config.names.enabled {
        return;
    }

    let all = match state.store.list_servers(None).await {
        Ok(all) => all,
        Err(e) => {
            tracing::warn!(error = %e, "listing nodes for local names failed; skipping this pass");
            return;
        }
    };

    let shared = match load_shared(state, &all).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, "loading local names failed; skipping this pass");
            return;
        }
    };

    let online: Vec<ServerId> = all.iter().filter(|s| s.status == AgentStatus::Online).map(|s| s.id).collect();
    let mut by_ws: BTreeMap<String, (WorkspaceId, Vec<Server>)> = BTreeMap::new();

    for s in all {
        by_ws.entry(s.workspace_id.to_string()).or_insert_with(|| (s.workspace_id, Vec::new())).1.push(s);
    }

    let mut desired = HashMap::new();
    let mut all_records: Vec<Records> = Vec::new();
    let serve_here = !state.config.dns.listen.is_empty();

    for (ws, servers) in by_ws.into_values() {
        match build_fleet(state, ws, servers, &shared).await {
            Ok(f) => {
                desired.extend(f.lists());

                if serve_here {
                    all_records.extend(f.clusters.keys().filter_map(|c| f.records_for(Some(*c)).records));
                }
            }
            Err(e) => tracing::warn!(error = %e, "building local names failed"),
        }
    }

    if serve_here {
        write_server_records(state, all_records).await;
    }

    let now = state.clock.now().0;

    let due: Vec<(ServerId, Desired)> = {
        let Ok(mut s) = sync().lock() else { return };
        s.desired = desired;
        s.suffix.clone_from(&shared.suffix);
        let known: HashSet<ServerId> = s.desired.keys().copied().collect();
        s.sent.retain(|k, _| known.contains(k));
        s.failed.retain(|k, _| known.contains(k));
        online.iter().filter_map(|id| due_now(&mut s, *id, now).map(|d| (*id, d))).collect()
    };

    for (id, d) in due {
        spawn_send(state, id, d, shared.suffix.clone());
    }
}

/// One record set for the server host's forge-dns: every cluster of every
/// workspace (D-175), written atomically to
/// `[dns] records_file` (world-readable, 0644, so forge-dns running as its
/// own user can read it). A failure is logged; the next pass retries.
async fn write_server_records(state: &AppState, sets: Vec<Records>) {
    let merged = merge_records(sets, state.clock.now().0);
    let path = state.config.dns.records_path();

    let r = tokio::task::spawn_blocking(move || -> Result<(), String> {
        let text = serde_json::to_vec_pretty(&merged).map_err(|e| e.to_string())?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;

        // The server's umask is 0077; forge-dns is another user
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o644))
                .map_err(|e| format!("{}: {e}", tmp.display()))?;
        }

        std::fs::rename(&tmp, &path).map_err(|e| format!("{}: {e}", path.display()))
    })
    .await;

    match r {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::warn!(error = %e, "writing the server's names.json failed"),
        Err(e) => tracing::warn!(error = %e, "writing the server's names.json failed"),
    }
}

/// A compose stack's environment and the `app` value of its name rows
/// (`<environment label>/<stack>`); `None` when the stack does not exist.
pub async fn stack_app(
    stacks: &dyn forge_core::stacks::StackRepository,
    tenancy: &dyn forge_core::ports::TenancyRepository,
    id: forge_core::StackId,
) -> forge_core::DomainResult<Option<(forge_core::EnvironmentId, String)>> {
    let Some(s) = stacks.get_stack(id).await? else { return Ok(None) };
    let env = tenancy.get_environment(s.environment_id).await?;
    let label = forge_catalog::names::label_from(&env.name).unwrap_or_default();

    Ok(Some((s.environment_id, format!("{label}/{}", s.name))))
}

/// Several clusters' record sets as one.
#[must_use]
pub fn merge_records(sets: Vec<Records>, now_ms: i64) -> Records {
    let mut zones: Vec<String> = Vec::new();
    let mut records = Vec::new();

    for s in sets {
        zones.extend(s.zones);
        records.extend(s.records);
    }

    zones.sort();
    zones.dedup();
    records.sort();
    records.dedup();

    Records {
        version: forge_catalog::names::RECORDS_VERSION,
        generated_at_ms: now_ms,
        zones,
        ttl: forge_catalog::names::RECORDS_TTL,
        records,
    }
}

/// Background loop: a pass every [`TICK`], and [`DEBOUNCE`] after a
/// [`poke`]. Pokes during the wait or the pass fold into one more pass, so
/// a thousand nodes enrolling cause a pass every few seconds, not a
/// thousand.
pub(crate) async fn run(state: AppState, mut shutdown: watch::Receiver<bool>) {
    let mut tick = tokio::time::interval(TICK);

    loop {
        tokio::select! {
            _ = tick.tick() => {}
            () = wake().notified() => {
                tokio::select! {
                    () = tokio::time::sleep(DEBOUNCE) => {}
                    _ = shutdown.changed() => break,
                }
            }
            _ = shutdown.changed() => break,
        }
        refresh(&state).await;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use forge_core::model::NodeRole;
    use forge_core::{EnvironmentId, Timestamp};

    use super::*;

    fn server(host: &str, display: Option<&str>, created: i64, cluster: Option<ClusterId>) -> Server {
        Server {
            id: ServerId::new(),
            workspace_id: WorkspaceId::new(),
            environment_id: None::<EnvironmentId>,
            cluster_id: cluster,
            role: NodeRole::Worker,
            display_name: display.map(str::to_owned),
            hostname: host.into(),
            os: "linux".into(),
            arch: "x86_64".into(),
            agent_version: "0".into(),
            status: AgentStatus::Online,
            last_seen_at: None,
            cert_serial: None,
            cert_not_after: None,
            drained: false,
            created_at: Timestamp(created),
        }
    }

    #[test]
    fn labels_prefer_what_users_set_and_never_collide() {
        let a = server("web.example.com", None, 1, None);
        let b = server("WEB", None, 2, None);
        let c = server("db", Some("Primary DB"), 3, None);
        let d = server("x", None, 4, None);
        let e = server("!!!", None, 5, None);
        let set = HashMap::from([(d.id, ("web".to_owned(), true))]);
        let l = node_labels(&[&a, &b, &c, &d, &e], &set);
        assert_eq!(l[&d.id], NodeLabel { label: "web".into(), explicit: true }, "set labels win");
        assert!(
            l[&a.id].label.starts_with("web-")
                && l[&b.id].label.starts_with("web-")
                && l[&a.id].label != l[&b.id].label
        );
        assert_eq!(l[&c.id].label, "primary-db", "from the console name");
        assert!(l[&e.id].label.starts_with("node-"));
        assert!(l.values().all(|x| check_label(&x.label).is_ok()));

        // A bad stored label is ignored rather than written.
        let l = node_labels(&[&a], &HashMap::from([(a.id, ("Bad Label".to_owned(), true))]));
        assert_eq!(l[&a.id].label, "web");
    }

    /// Review of D-084: stored labels never move. Renaming node a away from
    /// `web-1` leaves b's `-<id>` label alone, and `web-1` stays free.
    #[test]
    fn labels_are_sticky_and_freed_names_stay_free() {
        let a = server("web-1", None, 1, None);
        let b = server("web-1", None, 2, None);
        let first = node_labels(&[&a, &b], &HashMap::new());
        assert_eq!(first[&a.id].label, "web-1");
        let b_label = first[&b.id].label.clone();
        assert!(b_label.starts_with("web-1-"));

        // Both are stored after the first pass; a is renamed by a user.
        let mut stored: StoredLabels = first.iter().map(|(id, l)| (*id, (l.label.clone(), false))).collect();
        stored.insert(a.id, ("front".into(), true));
        let after = node_labels(&[&a, &b], &stored);
        assert_eq!(after[&b.id].label, b_label, "b keeps its label");
        assert!(after.values().all(|l| l.label != "web-1"), "web-1 stays free");

        // A console rename does not move a stored label either.
        let mut renamed = b.clone();
        renamed.display_name = Some("Totally New".into());
        assert_eq!(node_labels(&[&a, &renamed], &stored)[&b.id].label, b_label);

        // A new node never takes a stored label.
        let c = server("front", None, 3, None);
        assert_ne!(node_labels(&[&a, &b, &c], &stored)[&c.id].label, "front");
    }

    #[test]
    fn each_node_gets_mesh_addresses_only_inside_its_fabric() {
        let c = ClusterId::new();
        let (a, b, x) = (server("a", None, 1, Some(c)), server("b", None, 2, Some(c)), server("x", None, 3, None));
        let row = |s: &Server, name: &str, addr: &str, mesh: Option<[u8; 4]>| NameRow {
            name: name.into(),
            kind: "node",
            server_id: s.id,
            node: s.hostname.clone(),
            cluster_id: s.cluster_id,
            address: Some(addr.parse().unwrap()),
            mesh_address: mesh.map(Ipv4Addr::from),
            app: None,
            ports: vec![],
            srv: None,
        };
        let fleet = Fleet {
            suffix: "local".into(),
            servers: vec![],
            labels: HashMap::new(),
            clusters: HashMap::new(),
            subnets: HashMap::new(),
            rows: vec![
                row(&a, "a.lab.local", "192.168.1.10", Some([10, 64, 0, 1])),
                row(&b, "b.lab.local", "192.168.1.11", Some([10, 64, 0, 2])),
                row(&x, "x.local", "127.0.0.1", None),
            ],
        };
        let for_a = fleet.entries_for(&a);
        assert!(for_a.contains(&NameEntry { name: "b.lab.local".into(), ip: "10.64.0.2".parse().unwrap() }));
        assert!(!for_a.iter().any(|e| e.name == "x.local"), "a loopback address is useless to other nodes");
        let for_x = fleet.entries_for(&x);
        assert!(for_x.contains(&NameEntry { name: "b.lab.local".into(), ip: "192.168.1.11".parse().unwrap() }));
        assert!(for_x.contains(&NameEntry { name: "x.local".into(), ip: "127.0.0.1".parse().unwrap() }), "itself");
        let names = |e: Vec<NameEntry>| NodeNames { entries: e, ..NodeNames::default() };
        assert_eq!(hash(&names(for_a)), hash(&names(fleet.entries_for(&a))), "stable");
    }

    #[test]
    fn sends_are_due_on_change_reconnect_and_after_the_resend_gap() {
        let id = ServerId::new();
        let mut s = Sync::default();
        let list = Arc::new(NodeNames::default());
        s.desired.insert(id, (7, list.clone()));
        assert!(due_now(&mut s, id, 0).is_some());
        assert!(due_now(&mut s, id, 0).is_none(), "in flight");
        s.inflight.clear();
        s.sent.insert(id, (7, 0));
        assert!(due_now(&mut s, id, 1_000).is_none(), "in sync");
        let gap = i64::try_from(RESEND.as_millis()).unwrap();
        assert!(due_now(&mut s, id, gap).is_some(), "resent after the gap");
        s.inflight.clear();
        s.desired.insert(id, (8, list));
        assert!(due_now(&mut s, id, gap + 1).is_some(), "changed");
        s.inflight.clear();
        s.failed.insert(id, (1, gap + 60_000, "x".into()));
        assert!(due_now(&mut s, id, gap + 2).is_none(), "backing off");
    }

    /// The shared lists are exactly what `entries_for` gives each node.
    #[test]
    fn shared_lists_equal_per_node_lists() {
        let (c1, c2) = (ClusterId::new(), ClusterId::new());
        let servers = vec![
            server("a", None, 1, Some(c1)),
            server("b", None, 2, Some(c1)),
            server("c", None, 3, Some(c2)),
            server("d", None, 4, None),
            server("e", None, 5, None),
            server("lo", None, 6, None),
        ];
        let rows = servers
            .iter()
            .enumerate()
            .map(|(i, s)| NameRow {
                name: format!("{}.local", s.hostname),
                kind: "node",
                server_id: s.id,
                node: s.hostname.clone(),
                cluster_id: s.cluster_id,
                address: Some(if s.hostname == "lo" {
                    "127.0.0.1".parse().unwrap()
                } else {
                    format!("192.168.1.{}", i + 10).parse().unwrap()
                }),
                mesh_address: s.cluster_id.map(|_| Ipv4Addr::new(10, 64, 0, u8::try_from(i + 1).unwrap())),
                app: None,
                ports: vec![],
                srv: None,
            })
            .collect();
        let fleet = Fleet {
            suffix: "local".into(),
            servers: servers.clone(),
            labels: HashMap::new(),
            rows,
            clusters: HashMap::new(),
            subnets: HashMap::new(),
        };
        let lists = fleet.lists();

        for s in &servers {
            let own = fleet.entries_for(s);
            assert_eq!(lists[&s.id].1.entries, own, "{}", s.hostname);
            assert_eq!(lists[&s.id].0, hash(&NodeNames { entries: own, ..NodeNames::default() }));
        }

        assert!(Arc::ptr_eq(&lists[&servers[0].id].1, &lists[&servers[1].id].1), "same fabric shares one list");
        assert!(Arc::ptr_eq(&lists[&servers[3].id].1, &lists[&servers[4].id].1), "no fabric shares one list");
        assert!(!Arc::ptr_eq(&lists[&servers[3].id].1, &lists[&servers[5].id].1), "a loopback node gets its own");
    }

    #[test]
    fn a_changed_list_waits_for_the_change_gap() {
        let id = ServerId::new();
        let mut s = Sync::default();
        s.desired.insert(id, (2, Arc::new(NodeNames::default())));
        s.sent.insert(id, (1, 0));
        let gap = i64::try_from(CHANGE_GAP.as_millis()).unwrap();
        assert!(due_now(&mut s, id, gap - 1).is_none(), "sent a moment ago");
        assert!(due_now(&mut s, id, gap).is_some(), "after the gap");

        // A first send never waits.
        let new = ServerId::new();
        s.desired.insert(new, (2, Arc::new(NodeNames::default())));
        assert!(due_now(&mut s, new, 0).is_some());
    }

    fn state_row(service: &str, n: u32, ip: &str, health: &str, state: &str) -> (ServerId, StateRow) {
        let row = StateRow {
            service: service.into(),
            number: Some(n),
            state: state.into(),
            health: Some(health.into()),
            mesh_ip: Some(ip.parse().unwrap()),
        };

        (ServerId::new(), row)
    }

    #[test]
    fn stack_rows_name_healthy_replicas_and_every_running_one() {
        let st = StackNames {
            workspace_id: WorkspaceId::new(),
            environment_id: forge_core::EnvironmentId::new(),
            stack: "orders".into(),
            ports: HashMap::from([("api".to_owned(), (8080, "tcp".to_owned()))]),
            rows: vec![
                state_row("api", 1, "10.64.10.2", "healthy", "running"),
                state_row("api", 2, "10.64.12.4", "", "running"),
                state_row("api", 3, "10.64.12.5", "unhealthy", "running"),
                state_row("api", 4, "10.64.12.6", "", "exited"),
                state_row("redis", 1, "10.64.12.7", "", "running"),
            ],
        };
        let c = ClusterId::new();
        let labels = StackLabels { environment: "prod", cluster: Some(c) };
        let name = |l: &[&str], _: Option<ClusterId>| Some(format!("{}.c1.local", l.join(".")));
        let rows = stack_rows(&st, &labels, &|_| "lab-b".into(), &name);
        let svc: Vec<_> = rows.iter().filter(|r| r.kind == KIND_STACK_SERVICE).collect();
        let reps: Vec<&str> = rows.iter().filter(|r| r.kind == KIND_REPLICA).map(|r| r.name.as_str()).collect();

        assert_eq!(svc.len(), 2, "two healthy or unchecked replicas");
        assert!(svc.iter().all(|r| r.name == "api.orders.prod.c1.local" && r.address.is_none()));
        assert_eq!(svc[0].srv.as_ref().unwrap().port, 8080);
        assert_eq!(
            reps,
            vec!["api-1.orders.prod.c1.local", "api-2.orders.prod.c1.local", "api-3.orders.prod.c1.local"]
        );
        assert!(!rows.iter().any(|r| r.name.starts_with("redis")), "no port, no name");
    }

    #[test]
    fn a_replica_name_loses_to_a_service_name() {
        let mk = |name: &str, kind: &'static str| NameRow {
            name: name.into(),
            kind,
            server_id: ServerId::new(),
            node: String::new(),
            cluster_id: None,
            address: None,
            mesh_address: None,
            app: None,
            ports: vec![],
            srv: None,
        };
        let mut rows = vec![
            mk("api-2.orders.prod.local", KIND_STACK_SERVICE),
            mk("api-2.orders.prod.local", KIND_REPLICA),
            mk("api-1.orders.prod.local", KIND_REPLICA),
        ];
        resolve_clashes(&mut rows);

        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.kind != KIND_REPLICA || r.name == "api-1.orders.prod.local"));
    }
}
