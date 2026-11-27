//! Converges every cluster node to its fabric plan (WireGuard mesh), with
//! retries.
//!
//! Desired state lives in `cluster_fabrics`; this loop compares each member
//! node with it and runs idempotent agent commands until they match:
//!
//! 1. `install` WireGuard (skipped once the node reported a public key),
//! 2. `key` — the node creates its key pair and returns only the public key,
//! 3. `apply` — interface address, peers (direct / NAT / relay), routes.
//!
//! A failed node is retried with exponential back-off (30 s doubling up to
//! 15 min) and picked up again as soon as it is online. Healthy nodes are
//! re-applied every 10 minutes, which is a no-op when nothing changed and
//! repairs drift (an interface removed by hand, a reboot without the unit).
//! A node planned before some peers had keys is due again as soon as one of
//! those keys appears, and a pass that learned new keys runs again at once,
//! so a single **Apply network** on fresh nodes converges in one go.
//!
//! Commands run as `agent.command` jobs under the principal who last pressed
//! **Apply network** (with the MFA context of that request). The job runner
//! re-authorizes that principal before every step, so revoking their rights
//! stops the reconciler and the nodes show the denial.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use forge_catalog::fabric::{
    FabricMode, FabricOp, FabricSpec, HostEntry, Lease, LeaseKind, MESH_MTU, MeshNode, interface_name, plan_mesh,
};
use forge_core::model::{AgentStatus, FabricRecord, Server};
use forge_core::ports::{ClusterRepository, FabricRepository, ServerRepository, ServiceRepository, TenancyRepository};
use forge_core::tags::FactsRepository;
use forge_core::{AuthzContext, ClusterId, Principal, ServiceId, Timestamp};
use forge_proto::messages::RefusalCode;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::watch;

use super::ingress::node_address;
use super::sites::run_as;
use crate::state::AppState;

const TICK: Duration = Duration::from_secs(20);
const WAIT: Duration = Duration::from_secs(240);
const RECHECK_MS: i64 = 10 * 60 * 1000;
const MAX_BACKOFF_MS: i64 = 15 * 60 * 1000;

/// Who the reconciler acts as (set by `POST …/fabric/apply`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppliedBy {
    pub principal: Principal,
    pub context: AuthzContext,
    pub at: Timestamp,
}

/// Reconcile state of one node (inside `status.nodes`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NodeState {
    /// `pending`, `ok`, `waiting` (peers without keys), `retrying`, `failed`.
    pub state: String,
    #[serde(default)]
    pub step: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub attempts: u32,
    #[serde(default)]
    pub next_at: i64,
    #[serde(default)]
    pub checked_at: i64,
    #[serde(default)]
    pub public_key: Option<String>,
    /// Hash of the last config applied.
    #[serde(default)]
    pub applied: Option<String>,
    #[serde(default)]
    pub unreachable: Vec<String>,
    /// Peers left out of the last plan because they had no key yet.
    #[serde(default)]
    pub missing_keys: Vec<String>,
    #[serde(default)]
    pub relay: bool,
    /// The node's cluster container network (D-172):
    /// `ok`, `unsupported` (old or missing Docker; not retried with
    /// back-off), `waiting for a block`, or `None` before the first try and
    /// on Windows nodes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<String>,
    /// Why the container network is not `ok`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_error: Option<String>,
    /// Docker version the node reported with its last network answer (or
    /// its `docker_version` fact when the network was refused).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docker_version: Option<String>,
}

pub use forge_catalog::fabric::{NETWORK_FAILED, NETWORK_OK, NETWORK_UNSUPPORTED, NETWORK_WAITING_BLOCK};

/// Whether an `unsupported` container network should be tried again now
/// because the node reports a different Docker version than when refused.
fn docker_changed(ns: &NodeState, fact: Option<&str>) -> bool {
    ns.network.as_deref() == Some(NETWORK_UNSUPPORTED) && fact.is_some() && fact != ns.docker_version.as_deref()
}

/// Whether a node needs a step now: its time has come, or a peer it was
/// planned without has reported its key since.
fn due(ns: &NodeState, states: &BTreeMap<String, NodeState>, now: i64) -> bool {
    now >= ns.next_at || ns.missing_keys.iter().any(|m| states.get(m).is_some_and(|s| s.public_key.is_some()))
}

/// Passes run back to back while they keep learning keys; every pass that
/// continues learned at least one, so this bounds even a large first apply.
const MAX_PASSES: usize = 4;

fn running() -> &'static Mutex<HashSet<ClusterId>> {
    static R: OnceLock<Mutex<HashSet<ClusterId>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Clusters whose membership changed while a pass was running: the running
/// pass goes once more when it ends, so changes that land together are
/// handled in one extra pass (debounce) and none is lost.
fn again() -> &'static Mutex<HashSet<ClusterId>> {
    static R: OnceLock<Mutex<HashSet<ClusterId>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Issue #24: a node joined or left these clusters (enroll, move, revoke).
/// Every member of each affected fabric is marked due now and a pass starts
/// in the background, so existing members pick up the new peer list at once
/// instead of at their periodic recheck. Idempotent: it only resets
/// schedules; the pass re-plans from stored state.
pub async fn membership_changed(state: &AppState, clusters: &[ClusterId]) {
    let mut seen = HashSet::new();

    for &c in clusters {
        if !seen.insert(c) {
            continue;
        }

        let Ok(Some(mut rec)) = state.store.get_fabric(c).await else { continue };
        reset(&mut rec, None);

        if let Err(e) = state.store.put_fabric(&rec).await {
            tracing::warn!(error = %e, cluster = %c, "marking fabric members for resync failed");
            continue;
        }

        let s = state.clone();
        tokio::spawn(async move { reconcile_cluster(&s, c).await });
    }
}

fn backoff_ms(attempts: u32) -> i64 {
    (30_000_i64 << attempts.saturating_sub(1).min(10)).min(MAX_BACKOFF_MS)
}

fn fnv(s: &str) -> String {
    let h = s.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
    format!("{h:016x}")
}

pub fn node_states(status: &Value) -> BTreeMap<String, NodeState> {
    status.get("nodes").cloned().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default()
}

/// Update one node's state without clobbering other fields written meanwhile.
async fn save_node(state: &AppState, cluster: ClusterId, node: &str, ns: &NodeState) {
    let Ok(Some(mut rec)) = state.store.get_fabric(cluster).await else { return };
    if !rec.status.is_object() {
        rec.status = json!({});
    }

    if rec.status.get("nodes").is_none_or(|n| !n.is_object()) {
        rec.status["nodes"] = json!({});
    }

    rec.status["nodes"][node] = serde_json::to_value(ns).unwrap_or(Value::Null);

    if let Err(e) = state.store.put_fabric(&rec).await {
        tracing::warn!(error = %e, %cluster, "saving fabric node state failed");
    }
}

/// The answer to a fabric command: its output, or a coded refusal the
/// reconciler acts on (`jobs::agent_command` records it as the output).
enum Answer {
    Done(Value),
    Refused { code: String, message: String },
}

/// Run one fabric command on a node and return its output; a coded refusal
/// is an error here.
async fn exec(state: &AppState, by: &AppliedBy, node: &Server, op: FabricOp) -> Result<Value, String> {
    match exec_coded(state, by, node, op).await? {
        Answer::Done(v) => Ok(v),
        Answer::Refused { message, .. } => Err(message),
    }
}

/// Run one fabric command on a node, keeping a coded refusal apart.
async fn exec_coded(state: &AppState, by: &AppliedBy, node: &Server, op: FabricOp) -> Result<Answer, String> {
    let out = exec_raw(state, by, node, op).await?;

    let Some(refusal) = out.get("refusal") else { return Ok(Answer::Done(out)) };
    let code = refusal.get("code").and_then(Value::as_str).unwrap_or_default().to_owned();
    let message = refusal.get("message").and_then(Value::as_str).unwrap_or("refused").to_owned();

    Ok(Answer::Refused { code, message })
}

async fn exec_raw(state: &AppState, by: &AppliedBy, node: &Server, op: FabricOp) -> Result<Value, String> {
    let cmd = json!({ "type": "fabric", "op": op });
    let r = run_as(state, by.principal.clone(), by.context, node.id, cmd, WAIT).await.map_err(|e| e.message)?;
    match r.get("status").and_then(Value::as_str) {
        Some("succeeded") => Ok(r.get("result").cloned().unwrap_or(Value::Null)),
        Some("running") => Err("the node did not answer in time".into()),
        _ => Err(r.get("error").and_then(Value::as_str).unwrap_or("command failed").to_owned()),
    }
}

/// One pass over every fabric.
pub async fn reconcile_all(state: &AppState) {
    let fabrics = match state.store.list_fabrics(None).await {
        Ok(f) => f,
        Err(e) => return tracing::warn!(error = %e, "listing fabrics failed"),
    };

    for f in fabrics {
        reconcile_cluster(state, f.cluster_id).await;
    }
}

/// One pass over one cluster (skipped if a pass is already running).
pub async fn reconcile_cluster(state: &AppState, cluster: ClusterId) {
    {
        let Ok(mut r) = running().lock() else { return };
        if !r.insert(cluster) {
            if let Ok(mut a) = again().lock() {
                a.insert(cluster);
            }

            return;
        }
    }

    loop {
        for _ in 0..MAX_PASSES {
            if !pass(state, cluster).await {
                break;
            }
        }

        // Take the re-run flag and release the slot under the same lock
        // order as a caller that sets it, so a request is never dropped.
        let Ok(mut r) = running().lock() else { return };
        let rerun = again().lock().is_ok_and(|mut a| a.remove(&cluster));
        if !rerun {
            r.remove(&cluster);
            return;
        }
    }
}

/// One pass over a cluster's nodes. Returns whether it learned a new key, in
/// which case nodes planned without that key are due again right away.
async fn pass(state: &AppState, cluster: ClusterId) -> bool {
    let Ok(Some(rec)) = state.store.get_fabric(cluster).await else { return false };
    let Ok(spec) = serde_json::from_value::<FabricSpec>(rec.spec.clone()) else { return false };

    let Some(by) = rec.status.get("applied_by").cloned().and_then(|v| serde_json::from_value::<AppliedBy>(v).ok())
    else {
        return false; // never applied: planning only
    };

    if spec.mode != FabricMode::Mesh {
        return false; // a stored routed network waits for the next Apply network
    }

    let leases: Vec<Lease> = serde_json::from_value(rec.leases.clone()).unwrap_or_default();
    let Ok(all) = state.store.list_servers(None).await else { return false };

    // A revoked node is no peer any more: the others drop it on this pass.
    let members: Vec<Server> =
        all.into_iter().filter(|s| s.cluster_id == Some(cluster) && s.status != AgentStatus::Revoked).collect();
    let iface = interface_name(&cluster.to_string());
    let now = state.clock.now().0;
    let hosts = cluster_hosts(state, cluster, &spec, &leases, &members).await;
    let docker_facts = docker_versions(state).await;

    let swarm = match state.store.get_cluster(cluster).await {
        Ok(c) => c.mode == forge_core::model::ClusterMode::Swarm,
        Err(e) => {
            tracing::warn!(error = %e, %cluster, "reading the cluster failed; skipping this pass");
            return false;
        }
    };

    let mut learned = false;

    for node in &members {
        let id = node.id.to_string();

        // Fresh state each time: earlier nodes may have reported keys.
        let Ok(Some(cur)) = state.store.get_fabric(cluster).await else { return learned };
        let states = node_states(&cur.status);
        let mut ns =
            states.get(&id).cloned().unwrap_or_else(|| NodeState { state: "pending".into(), ..NodeState::default() });

        let docker_fact = docker_facts.get(&node.id).map(String::as_str);

        if node.status != AgentStatus::Online || !(due(&ns, &states, now) || docker_changed(&ns, docker_fact)) {
            continue;
        }

        let had_key = ns.public_key.is_some();

        let result = converge(state, &by, &spec, &iface, &leases, &members, &hosts, &states, node, &mut ns, now).await;

        // Swarm clusters keep the fabric for their data path but have no
        // container network (conversion removed it); Windows nodes neither.
        // A network failure is recorded on the network fields only: the
        // WireGuard part stays in sync and other nodes keep their reach.
        if result.is_ok() && !swarm && !node.os.to_lowercase().starts_with("win") {
            let block = leases.iter().find(|l| l.kind == LeaseKind::Block && l.owner == id).map(|l| l.cidr);

            if let Err(e) = ensure_network(state, &by, &iface, block, node, &mut ns).await {
                tracing::info!(%cluster, node = %id, error = %e, "container network failed; retried on the repair pass");
                ns.step = None;
                ns.network = Some(NETWORK_FAILED.into());
                ns.network_error = Some(e);
            }
        }

        match result {
            Ok(()) => {
                ns.attempts = 0;
                ns.error = None;
                ns.next_at = now + RECHECK_MS;
                ns.checked_at = now;
            }
            Err(e) => {
                ns.attempts += 1;
                ns.state = if ns.attempts >= 8 { "failed".into() } else { "retrying".into() };
                ns.error = Some(e);
                ns.next_at = now + backoff_ms(ns.attempts);
                tracing::info!(%cluster, node = %id, attempts = ns.attempts, "fabric step failed; will retry");
            }
        }

        learned |= !had_key && ns.public_key.is_some();
        save_node(state, cluster, &id, &ns).await;
    }

    learned
}

#[allow(clippy::too_many_arguments)]
async fn converge(
    state: &AppState,
    by: &AppliedBy,
    spec: &FabricSpec,
    iface: &str,
    leases: &[Lease],
    members: &[Server],
    hosts: &[HostEntry],
    states: &BTreeMap<String, NodeState>,
    node: &Server,
    ns: &mut NodeState,
    _now: i64,
) -> Result<(), String> {
    let id = node.id.to_string();

    if ns.public_key.is_none() {
        ns.step = Some("install".into());
        exec(state, by, node, FabricOp::Install).await?;
        ns.step = Some("key".into());
        let out = exec(state, by, node, FabricOp::Key { iface: iface.to_owned() }).await?;
        let key = out.get("public_key").and_then(Value::as_str).ok_or("the node returned no public key")?;
        forge_catalog::fabric::valid_key(key)?;
        ns.public_key = Some(key.to_owned());
    }

    // Plan with every key known so far (this node's included).
    let lease =
        |kind: LeaseKind, owner: &str| leases.iter().find(|l| l.kind == kind && l.owner == owner).map(|l| l.cidr);
    let mut mesh = Vec::new();

    for m in members {
        let mid = m.id.to_string();
        let (Some(ip), Some(block)) = (lease(LeaseKind::Node, &mid), lease(LeaseKind::Block, &mid)) else { continue };
        let public_key =
            if mid == id { ns.public_key.clone() } else { states.get(&mid).and_then(|s| s.public_key.clone()) };
        mesh.push(MeshNode {
            id: mid.clone(),
            address: node_address(state, m).await,
            endpoint: spec.endpoints.get(&mid).cloned(),
            public_key,
            mesh_ip: ip,
            block,
            can_relay: !m.os.to_lowercase().starts_with("win"),
        });
    }

    let missing_keys = mesh.iter().filter(|m| m.id != id && m.public_key.is_none()).map(|m| m.id.clone()).collect();
    let (mut config, unreachable) = plan_mesh(spec, iface, &mesh, &id)?;
    config.hosts = hosts.to_vec();
    let hash = fnv(&serde_json::to_string(&config).map_err(|e| e.to_string())?);
    ns.step = Some("apply".into());
    exec(state, by, node, FabricOp::Apply { config: Box::new(config.clone()) }).await?;
    ns.applied = Some(hash);
    ns.relay = config.forward;
    ns.step = None;
    ns.state = if unreachable.is_empty() { "ok".into() } else { "waiting".into() };
    ns.unreachable = unreachable;
    ns.missing_keys = missing_keys;

    Ok(())
}

/// Every node's `docker_version` fact. Unreadable facts count as unknown:
/// the 10-minute repair pass still retries the network.
async fn docker_versions(state: &AppState) -> HashMap<forge_core::ServerId, String> {
    let all = match state.store.all_facts().await {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!(error = %e, "reading node facts failed; Docker upgrades are noticed on the repair pass");
            return HashMap::new();
        }
    };

    all.into_iter().filter_map(|(id, (f, _))| Some((id, f.get("docker_version")?.as_str()?.to_owned()))).collect()
}

/// Runs `NetworkEnsure` after the WireGuard part is in sync. An old or
/// missing Docker marks the network `unsupported` without failing the node,
/// so it is not retried with back-off; any other error is a node failure.
async fn ensure_network(
    state: &AppState,
    by: &AppliedBy,
    iface: &str,
    block: Option<forge_catalog::fabric::Cidr>,
    node: &Server,
    ns: &mut NodeState,
) -> Result<(), String> {
    let Some(subnet) = block else {
        tracing::warn!(node = %node.id, "no instance block leased; the container network waits");
        ns.network = Some(NETWORK_WAITING_BLOCK.into());
        ns.network_error = None;

        return Ok(());
    };

    ns.step = Some("network".into());
    let op = FabricOp::NetworkEnsure { iface: iface.to_owned(), subnet, mtu: MESH_MTU };
    let answer = exec_coded(state, by, node, op).await?;
    ns.step = None;

    match answer {
        Answer::Done(out) => {
            ns.network = Some(NETWORK_OK.into());
            ns.network_error = None;
            ns.docker_version = out.get("docker_version").and_then(Value::as_str).map(str::to_owned);
        }
        Answer::Refused { code, message } if RefusalCode::parse(&code) == Some(RefusalCode::Unsupported) => {
            ns.network = Some(NETWORK_UNSUPPORTED.into());
            ns.network_error = Some(message);
            ns.docker_version = docker_versions(state).await.remove(&node.id);
        }
        Answer::Refused { message, .. } => return Err(message),
    }

    Ok(())
}

/// A DNS label from a free-form name (`Node_1` → `node-1`), or `None`; the
/// same rule as the fleet's names (D-084).
fn label(name: &str) -> Option<String> {
    forge_catalog::names::label_from(name)
}

/// Names on the cluster network: every node (`<host>.<domain>`) and every
/// service with an active instance (`<app>.<environment>.<domain>`, one line
/// per node running it).
async fn cluster_hosts(
    state: &AppState,
    cluster: ClusterId,
    spec: &FabricSpec,
    leases: &[Lease],
    members: &[Server],
) -> Vec<HostEntry> {
    let ip_of = |id: &str| leases.iter().find(|l| l.kind == LeaseKind::Node && l.owner == id).map(|l| l.cidr.first());

    // The same labels as the fleet's names (D-084), so a rename applies here too.
    let labels = match members.first() {
        Some(m) => {
            let ws = m.workspace_id;
            let all = state.store.list_servers(Some(ws)).await.unwrap_or_default();
            let stored = crate::names::stored_labels(state, ws).await.unwrap_or_default();
            crate::names::node_labels(&all.iter().collect::<Vec<_>>(), &stored)
        }
        None => HashMap::new(),
    };

    let mut out = Vec::new();

    for m in members {
        if let (Some(ip), Some(l)) = (ip_of(&m.id.to_string()), labels.get(&m.id)) {
            out.push(HostEntry { name: format!("{}.{}", l.label, spec.domain), ip });
        }
    }

    let Ok(instances) = state.store.list_all_instances(None).await else { return out };
    let mut apps: HashMap<ServiceId, Option<String>> = HashMap::new();

    for i in instances.iter().filter(|i| i.role != "standby") {
        let Some(ip) = ip_of(&i.server_id.to_string()) else { continue };

        if !members.iter().any(|m| m.id == i.server_id) {
            continue;
        }

        let name = match apps.get(&i.service_id) {
            Some(n) => n.clone(),
            None => {
                let n = async {
                    let a = state.store.get_service(i.service_id).await.ok()??;
                    let ns = state.store.get_environment(a.environment_id).await.ok()?;
                    (ns.cluster_id == Some(cluster)).then_some(())?;
                    Some(format!("{}.{}.{}", label(&a.name)?, label(&ns.name)?, spec.domain))
                }
                .await;
                apps.insert(i.service_id, n.clone());
                n
            }
        };

        if let Some(n) = name {
            out.push(HostEntry { name: n, ip });
        }
    }

    out.retain(|h| forge_catalog::fabric::valid_hostname(&h.name));
    out.sort_by(|a, b| a.name.cmp(&b.name).then(a.ip.cmp(&b.ip)));
    out.dedup();

    out
}

/// Background loop: a pass over every fabric every 20 seconds.
pub(crate) async fn run(state: AppState, mut shutdown: watch::Receiver<bool>) {
    let mut tick = tokio::time::interval(TICK);

    loop {
        tokio::select! {
            _ = tick.tick() => {}
            _ = shutdown.changed() => break,
        }
        reconcile_all(&state).await;
    }
}

/// Mark nodes for an immediate retry (all, or one).
pub fn reset(rec: &mut FabricRecord, only: Option<&str>) {
    let mut states = node_states(&rec.status);

    for (id, s) in &mut states {
        if only.is_none_or(|o| o == id) {
            s.next_at = 0;
            s.attempts = 0;

            if s.state == "failed" {
                s.state = "retrying".into();
            }
        }
    }

    if !rec.status.is_object() {
        rec.status = json!({});
    }

    rec.status["nodes"] = serde_json::to_value(states).unwrap_or(json!({}));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_to_fifteen_minutes() {
        assert_eq!(backoff_ms(1), 30_000);
        assert_eq!(backoff_ms(2), 60_000);
        assert_eq!(backoff_ms(5), 480_000);
        assert_eq!(backoff_ms(6), MAX_BACKOFF_MS);
        assert_eq!(backoff_ms(40), MAX_BACKOFF_MS);
    }

    fn node(next_at: i64, key: Option<&str>, missing: &[&str]) -> NodeState {
        NodeState {
            state: if missing.is_empty() { "ok" } else { "waiting" }.into(),
            next_at,
            public_key: key.map(str::to_owned),
            missing_keys: missing.iter().map(|m| (*m).to_owned()).collect(),
            ..NodeState::default()
        }
    }

    /// Issue #10: two fresh nodes, one apply. Node `b` is planned before `c`
    /// has a key; it must be due again in the same apply once `c` reports
    /// its key, not ten minutes later.
    #[test]
    fn a_node_planned_without_a_peer_key_is_due_once_the_key_appears() {
        let now = 1_000;
        let later = now + RECHECK_MS;

        // After b's step: b waits for c, c has not run yet.
        let mut states =
            BTreeMap::from([("b".to_owned(), node(later, Some("KB"), &["c"])), ("c".to_owned(), node(0, None, &[]))]);
        assert!(!due(&states["b"], &states, now), "nothing new for b yet");
        assert!(due(&states["c"], &states, now));

        // c's step reports its key (and c was planned with b's).
        states.insert("c".to_owned(), node(later, Some("KC"), &[]));
        assert!(due(&states["b"], &states, now), "c's key arrived: b is due now");
        assert!(!due(&states["c"], &states, now), "c had every key");

        // b re-planned with c's key: both settle until the recheck.
        states.insert("b".to_owned(), node(later, Some("KB"), &[]));
        assert!(!due(&states["b"], &states, now) && !due(&states["c"], &states, now));
        assert!(due(&states["b"], &states, later), "the periodic recheck still runs");
    }

    #[test]
    fn a_peer_unreachable_for_other_reasons_does_not_retrigger() {
        // NAT-only peers are `unreachable` but not missing a key: no busy loop.
        let mut b = node(5_000, Some("KB"), &[]);
        b.unreachable = vec!["c".into()];
        let states = BTreeMap::from([("b".to_owned(), b.clone()), ("c".to_owned(), node(5_000, Some("KC"), &[]))]);
        assert!(!due(&b, &states, 1_000));
    }

    #[test]
    fn an_unsupported_network_is_retried_only_when_docker_changes() {
        let ns = NodeState {
            network: Some(NETWORK_UNSUPPORTED.into()),
            docker_version: Some("27.5.1".into()),
            ..NodeState::default()
        };

        assert!(!docker_changed(&ns, Some("27.5.1")));
        assert!(!docker_changed(&ns, None));
        assert!(docker_changed(&ns, Some("28.1.0")));

        let ok = NodeState { network: Some(NETWORK_OK.into()), ..ns };
        assert!(!docker_changed(&ok, Some("28.1.0")));
    }

    #[test]
    fn old_status_without_missing_keys_still_parses() {
        let s = node_states(&json!({ "nodes": { "a": { "state": "waiting", "next_at": 7, "unreachable": ["b"] } } }));
        assert!(s["a"].missing_keys.is_empty());
    }

    #[test]
    fn reset_retries_failed_nodes_now_and_keeps_their_keys() {
        let mut rec = FabricRecord {
            cluster_id: ClusterId::new(),
            spec: json!({}),
            pins: json!([]),
            leases: json!([]),
            status: json!({ "nodes": {
                "a": { "state": "failed", "attempts": 8, "next_at": 9_999_999, "public_key": "K" },
                "b": { "state": "ok", "attempts": 0, "next_at": 5_000 }
            }}),
            updated_at: Timestamp(0),
        };
        reset(&mut rec, Some("a"));
        let s = node_states(&rec.status);
        assert_eq!(s["a"].state, "retrying");
        assert_eq!((s["a"].attempts, s["a"].next_at), (0, 0));
        assert_eq!(s["a"].public_key.as_deref(), Some("K"), "the node keeps its key");
        assert_eq!(s["b"].next_at, 5_000, "other nodes untouched");
        reset(&mut rec, None);
        assert_eq!(node_states(&rec.status)["b"].next_at, 0);
    }
}
