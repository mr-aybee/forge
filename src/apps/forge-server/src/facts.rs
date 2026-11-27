//! Node facts: OS name and version, kernel, architecture and CPU count,
//! collected from the agent (`collect_facts`, allowed by default) when a node
//! connects and once a day, then saved so the console shows them even while
//! the node is offline. Works with existing agents: no protocol change.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use forge_core::ServerId;
use forge_core::tags::FactsRepository;
use forge_proto::Command;
use forge_proto::messages::CollectFacts;
use serde_json::Value;

use crate::state::AppState;

/// Who signs the command (visible in the agent's log).
pub const SYSTEM_PRINCIPAL: &str = "system:facts";
/// Collect again after this long.
const REFRESH: Duration = Duration::from_secs(24 * 3600);
/// Retry after a failure no sooner than this.
const RETRY: Duration = Duration::from_secs(10 * 60);

fn addresses() -> &'static Mutex<HashMap<ServerId, std::net::IpAddr>> {
    static A: OnceLock<Mutex<HashMap<ServerId, std::net::IpAddr>>> = OnceLock::new();
    A.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Records the address a node's agent connects from (every heartbeat).
pub fn note_address(server: ServerId, ip: std::net::IpAddr) {
    if let Ok(mut a) = addresses().lock() {
        a.insert(server, ip.to_canonical());
    }
}

/// The address a node is reached on by default: where its agent connects
/// from (live), else the address in its saved facts. `None` for a node that
/// never connected. A host name is not used: nothing guarantees that the
/// control plane or the other nodes can resolve it.
pub async fn node_address(state: &AppState, server: ServerId) -> forge_core::DomainResult<Option<String>> {
    if let Some(ip) = live_address(server) {
        return Ok(Some(ip));
    }

    let facts = forge_core::tags::FactsRepository::all_facts(&state.store).await?;

    Ok(facts.get(&server).and_then(|(f, _)| f["address"].as_str()).map(str::to_owned))
}

/// The address the node's agent last connected from (this process).
#[must_use]
pub fn live_address(server: ServerId) -> Option<String> {
    addresses().lock().ok().and_then(|a| a.get(&server).map(ToString::to_string))
}

fn attempts() -> &'static Mutex<HashMap<ServerId, i64>> {
    static A: OnceLock<Mutex<HashMap<ServerId, i64>>> = OnceLock::new();
    A.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Whether a collection is due: none tried within the retry gap, or the
/// agent has just changed version (its facts, such as node-bound SSH
/// certificates, may have changed with it).
fn due(last: Option<i64>, now: i64, agent_changed: bool) -> bool {
    let gap = i64::try_from(RETRY.as_millis()).unwrap_or(i64::MAX);
    agent_changed || last.is_none_or(|t| now - t >= gap)
}

/// Called on every heartbeat: starts a background collection when due.
/// Cheap when nothing is due (one map lookup). `agent_changed`: this
/// heartbeat reports another agent version than the one on record.
pub fn maybe_collect(state: &AppState, server: ServerId, agent_changed: bool) {
    let now = state.clock.now().0;

    let due = {
        let Ok(mut a) = attempts().lock() else { return };
        if due(a.get(&server).copied(), now, agent_changed) {
            a.insert(server, now);
            true
        } else {
            false
        }
    };

    if !due {
        return;
    }

    let state = state.clone();
    tokio::spawn(async move {
        match collect(&state, server).await {
            Ok(()) => {
                // Next run after a full day, not after the retry gap.
                let next = now + i64::try_from(REFRESH.as_millis()).unwrap_or(0)
                    - i64::try_from(RETRY.as_millis()).unwrap_or(0);

                if let Ok(mut a) = attempts().lock() {
                    a.insert(server, next);
                }
            }
            Err(e) => tracing::debug!(%server, error = %e, "collecting node facts failed; will retry"),
        }
    });
}

/// Keeps the useful facts and drops large or sensitive ones (host keys).
fn keep(v: &Value) -> Value {
    let mut out = serde_json::Map::new();

    for k in [
        "hostname",
        "os",
        "os_version",
        "kernel",
        "arch",
        "cpus",
        "agent_version",
        "address",
        "ssh_principals",
        // Direct port tunnels over the node's SSH (#35 phase 2).
        "ssh_tunnel",
        // Accounts the terminal can log in as (D-078).
        "login_accounts",
        // IPv4 interface addresses (remote-access gateways, D-092).
        "interfaces",
        // Container shells through the agent and its helper (D-114).
        "container_exec",
        // Whether the node can update its own agent (agent update verdicts),
        // and whether its helper answered at all (after an update).
        "self_update",
        "helper_ok",
        // Cluster container network (D-172): the capability,
        // the Docker version its retry follows, and forge-dns's state.
        "mesh_network",
        // Typed container options on app specs (service orchestration step 1).
        "container_options",
        // Pushes native service container states (D-177).
        "app_states",
        "docker_version",
        "forge_dns",
    ] {
        if let Some(x) = v.get(k).filter(|x| !x.is_null()) {
            out.insert(k.to_owned(), x.clone());
        }
    }

    Value::Object(out)
}

/// Collects and stores the node's facts now (also used when a stored fact
/// may predate a change on the node, such as direct-tunnel support).
pub(crate) async fn collect(state: &AppState, server: ServerId) -> Result<(), String> {
    let (body, signed) = state.signer.issue(
        &server.to_string(),
        SYSTEM_PRINCIPAL,
        Command::CollectFacts(CollectFacts {}),
        state.clock.now(),
        Duration::from_secs(120),
    );
    state.hub.enqueue(server, &body.operation_id, signed).map_err(|e| e.to_string())?;
    let r = state.hub.wait(&body.operation_id, Duration::from_secs(90)).await.ok_or("the node did not answer")?;
    if !r.ok {
        return Err(r.error);
    }

    let mut v: Value = serde_json::from_str(&r.output).map_err(|_| "invalid facts".to_owned())?;

    // Where the agent connects from: the default address for SSH (terminal).
    if let Some(ip) = live_address(server) {
        v["address"] = Value::from(ip);
    }

    state.store.put_facts(server, &keep(&v), state.clock.now()).await.map_err(|e| e.to_string())
}

/// Facts for the API: stored facts plus live totals (RAM, disk, CPUs) from
/// the latest telemetry sample when there is one.
#[must_use]
pub fn with_live(state: &AppState, server: ServerId, stored: Option<&(Value, forge_core::Timestamp)>) -> Value {
    let mut f = stored.map(|(v, _)| v.clone()).unwrap_or_else(|| Value::Object(serde_json::Map::new()));

    if let Some((_, at)) = stored {
        f["collected_at_ms"] = Value::from(at.0);
    }

    if let Some(ip) = live_address(server) {
        f["address"] = Value::from(ip);
    }

    if let Some(sample) = state.telemetry.latest(server)
        && let Ok(Value::Object(m)) = serde_json::to_value(sample)
    {
        for (from, to) in [("cpus", "cpus"), ("mem_total", "mem_total_bytes"), ("disk_total", "disk_total_bytes")] {
            if let Some(v) = m.get(from).filter(|v| v.as_u64().is_some_and(|n| n > 0)) {
                f[to] = v.clone();
            }
        }
    }

    f
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// Found on the lab: after an agent update, facts (and with them
    /// node-bound SSH certificates, D-069) stayed stale for up to a day, so
    /// Forge kept signing certificates the updated node refused.
    #[test]
    fn an_agent_update_refreshes_facts_at_once() {
        let now = 10_000_000;
        let just = Some(now - 1000);
        assert!(!due(just, now, false), "collected a moment ago");
        assert!(due(just, now, true), "the agent version changed");
        assert!(due(None, now, false));
        assert!(due(Some(now - i64::try_from(RETRY.as_millis()).unwrap()), now, false));
    }

    #[test]
    fn host_keys_and_unknown_fields_are_dropped() {
        let v = serde_json::json!({ "os_version": "Debian 13", "ssh_host_keys": ["ssh-ed25519 AAA"], "x": 1, "kernel": null });
        let k = keep(&v);
        assert_eq!(k["os_version"], "Debian 13");
        assert!(k.get("ssh_host_keys").is_none() && k.get("x").is_none() && k.get("kernel").is_none());
    }

    #[test]
    fn the_self_update_capability_is_kept() {
        let cap =
            serde_json::json!({ "supervisor": "systemd", "ready": true, "reasons": [], "helper_version": "1.0.0" });
        let k = keep(&serde_json::json!({ "self_update": cap.clone() }));
        assert_eq!(k["self_update"], cap);
    }

    /// Found on the lab: stack planning, native containers and the network
    /// retry read these facts, and dropping them left every node without
    /// the cluster container network.
    #[test]
    fn the_cluster_network_facts_are_kept() {
        let v = serde_json::json!({ "mesh_network": true, "docker_version": "29.1.0", "forge_dns": "running" });
        let k = keep(&v);
        assert_eq!(k["mesh_network"], true);
        assert_eq!(k["docker_version"], "29.1.0");
        assert_eq!(k["forge_dns"], "running");
    }
}
