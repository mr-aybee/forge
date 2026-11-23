//! Docker Swarm membership and nodes, run by the root helper over the local
//! Engine API (D-154). The operations are the closed set in
//! [`forge_catalog::swarm::SwarmOp`], validated before they get here.
//!
//! Join tokens appear in `Join` and in the results of `JoinTokens` and
//! `RotateTokens`; nothing here logs them.

use std::collections::BTreeMap;
use std::time::Duration;

use forge_catalog::swarm::{Availability, JoinTokens, SwarmNode, SwarmOp, SwarmRole, SwarmStatus};
use serde_json::{Value, json};

use crate::docker::api;

const QUICK: Duration = Duration::from_secs(15);
/// Init and join wait for the managers' Raft consensus.
const CONSENSUS: Duration = Duration::from_secs(120);

pub async fn run(op: &SwarmOp) -> Result<Value, String> {
    match op {
        SwarmOp::Status {} => to_value(&status().await),
        SwarmOp::Init { advertise_addr, port, default_addr_pool, autolock } => {
            let now = status().await;
            if now.state == "active" {
                // Idempotent: a retried step finds the swarm it made.
                return to_value(&now);
            }
            let mut body = json!({
                "ListenAddr": format!("0.0.0.0:{port}"),
                "AdvertiseAddr": format!("{advertise_addr}:{port}"),
                "Spec": { "EncryptionConfig": { "AutoLockManagers": autolock } },
            });
            if !default_addr_pool.is_empty() {
                body["DefaultAddrPool"] = json!(default_addr_pool);
            }
            api("POST", "/swarm/init", Some(&body), CONSENSUS).await?;
            to_value(&status().await)
        }
        SwarmOp::Join { remote_addrs, join_token, advertise_addr } => {
            let now = status().await;
            if now.state == "active" {
                return to_value(&now);
            }
            let body = json!({
                "ListenAddr": "0.0.0.0:2377",
                "AdvertiseAddr": advertise_addr.to_string(),
                "RemoteAddrs": remote_addrs,
                "JoinToken": join_token,
            });
            api("POST", "/swarm/join", Some(&body), CONSENSUS).await.map_err(|e| without_token(&e, join_token))?;
            to_value(&status().await)
        }
        SwarmOp::Leave { force } => {
            if status().await.state == "inactive" {
                return Ok(json!({ "left": true }));
            }
            api("POST", &format!("/swarm/leave?force={force}"), None, CONSENSUS).await?;
            Ok(json!({ "left": true }))
        }
        SwarmOp::JoinTokens {} => to_value(&tokens(&swarm().await?)),
        SwarmOp::RotateTokens { worker, manager } => {
            let s = swarm().await?;
            let version = s["Version"]["Index"].as_u64().ok_or("Docker did not report the swarm version")?;
            let path =
                format!("/swarm/update?version={version}&rotateWorkerToken={worker}&rotateManagerToken={manager}");
            api("POST", &path, Some(&s["Spec"]), QUICK).await?;
            to_value(&tokens(&swarm().await?))
        }
        SwarmOp::Nodes {} => {
            let list = api("GET", "/nodes", None, QUICK).await?.unwrap_or(Value::Null);
            let nodes: Vec<SwarmNode> = list.as_array().map(|a| a.iter().map(node_view).collect()).unwrap_or_default();
            to_value(&nodes)
        }
        SwarmOp::NodeUpdate { node_id, role, availability, labels } => {
            let node = api("GET", &format!("/nodes/{node_id}"), None, QUICK)
                .await?
                .ok_or_else(|| format!("swarm node {node_id} not found"))?;
            let version = node["Version"]["Index"].as_u64().ok_or("Docker did not report the node version")?;
            let spec = updated_spec(&node["Spec"], *role, *availability, labels.as_ref());
            api("POST", &format!("/nodes/{node_id}/update?version={version}"), Some(&spec), QUICK).await?;
            let node = api("GET", &format!("/nodes/{node_id}"), None, QUICK).await?.unwrap_or(Value::Null);
            to_value(&node_view(&node))
        }
        SwarmOp::NodeRemove { node_id, force } => {
            api("DELETE", &format!("/nodes/{node_id}?force={force}"), None, QUICK).await?;
            Ok(json!({ "removed": node_id }))
        }
    }
}

fn to_value<T: serde::Serialize>(v: &T) -> Result<Value, String> {
    serde_json::to_value(v).map_err(|e| e.to_string())
}

/// Docker may echo the request in an error; never pass a token on.
fn without_token(err: &str, token: &str) -> String {
    err.replace(token, "<join-token>")
}

async fn swarm() -> Result<Value, String> {
    api("GET", "/swarm", None, QUICK).await?.ok_or_else(|| "this node is not a swarm manager".to_owned())
}

async fn status() -> SwarmStatus {
    match api("GET", "/info", None, QUICK).await {
        Ok(Some(info)) => status_of(&info),
        _ => SwarmStatus { docker: false, state: "inactive".into(), ..SwarmStatus::default() },
    }
}

fn opt_str(v: &Value) -> Option<String> {
    v.as_str().filter(|s| !s.is_empty()).map(str::to_owned)
}

/// The swarm part of `GET /info`.
pub(crate) fn status_of(info: &Value) -> SwarmStatus {
    let s = &info["Swarm"];
    SwarmStatus {
        docker: true,
        state: s["LocalNodeState"].as_str().unwrap_or("inactive").to_owned(),
        swarm_id: opt_str(&s["Cluster"]["ID"]),
        node_id: opt_str(&s["NodeID"]),
        manager: s["ControlAvailable"].as_bool().unwrap_or(false),
        node_addr: opt_str(&s["NodeAddr"]),
        managers: s["RemoteManagers"]
            .as_array()
            .map(|a| a.iter().filter_map(|m| opt_str(&m["Addr"])).collect())
            .unwrap_or_default(),
    }
}

fn tokens(swarm: &Value) -> JoinTokens {
    JoinTokens {
        worker: swarm["JoinTokens"]["Worker"].as_str().unwrap_or_default().to_owned(),
        manager: swarm["JoinTokens"]["Manager"].as_str().unwrap_or_default().to_owned(),
    }
}

fn labels_of(v: &Value) -> BTreeMap<String, String> {
    v.as_object()
        .map(|o| o.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_owned())).collect())
        .unwrap_or_default()
}

/// One entry of `GET /nodes`.
pub(crate) fn node_view(n: &Value) -> SwarmNode {
    let role = if n["Spec"]["Role"] == "manager" { SwarmRole::Manager } else { SwarmRole::Worker };
    let availability = match n["Spec"]["Availability"].as_str() {
        Some("drain") => Availability::Drain,
        Some("pause") => Availability::Pause,
        _ => Availability::Active,
    };
    SwarmNode {
        id: n["ID"].as_str().unwrap_or_default().to_owned(),
        hostname: n["Description"]["Hostname"].as_str().unwrap_or_default().to_owned(),
        role,
        availability,
        state: n["Status"]["State"].as_str().unwrap_or("unknown").to_owned(),
        addr: n["Status"]["Addr"].as_str().unwrap_or_default().to_owned(),
        manager_status: n["ManagerStatus"]["Reachability"].as_str().unwrap_or_default().to_owned(),
        leader: n["ManagerStatus"]["Leader"].as_bool().unwrap_or(false),
        engine_version: n["Description"]["Engine"]["EngineVersion"].as_str().unwrap_or_default().to_owned(),
        labels: labels_of(&n["Spec"]["Labels"]),
    }
}

/// The node spec with the requested changes; fields not asked for keep their
/// current value (the update API replaces the whole spec).
pub(crate) fn updated_spec(
    current: &Value,
    role: Option<SwarmRole>,
    availability: Option<Availability>,
    labels: Option<&BTreeMap<String, String>>,
) -> Value {
    let mut spec = if current.is_object() { current.clone() } else { json!({}) };
    if let Some(r) = role {
        spec["Role"] = json!(r.as_str());
    }
    if let Some(a) = availability {
        spec["Availability"] = json!(a.as_str());
    }
    if let Some(l) = labels {
        spec["Labels"] = json!(l);
    }
    spec
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn status_reads_the_swarm_part_of_info() {
        let info = json!({"Swarm": {
            "LocalNodeState": "active", "NodeID": "abc123", "NodeAddr": "192.0.2.10",
            "ControlAvailable": true, "Cluster": {"ID": "swarm1"},
            "RemoteManagers": [{"NodeID": "abc123", "Addr": "192.0.2.10:2377"}]
        }});
        let s = status_of(&info);
        assert!(s.docker && s.manager);
        assert_eq!(s.state, "active");
        assert_eq!(s.swarm_id.as_deref(), Some("swarm1"));
        assert_eq!(s.managers, vec!["192.0.2.10:2377"]);
        let off = status_of(&json!({"Swarm": {"LocalNodeState": "inactive", "NodeID": ""}}));
        assert_eq!(off.state, "inactive");
        assert!(off.node_id.is_none() && !off.manager);
    }

    #[test]
    fn node_view_maps_roles_availability_and_leader() {
        let n = json!({
            "ID": "n1", "Description": {"Hostname": "lab-a", "Engine": {"EngineVersion": "27.3.1"}},
            "Spec": {"Role": "manager", "Availability": "drain", "Labels": {"zone": "a"}},
            "Status": {"State": "ready", "Addr": "192.0.2.10"},
            "ManagerStatus": {"Leader": true, "Reachability": "reachable"}
        });
        let v = node_view(&n);
        assert_eq!(v.role, SwarmRole::Manager);
        assert_eq!(v.availability, Availability::Drain);
        assert!(v.leader);
        assert_eq!(v.labels.get("zone").map(String::as_str), Some("a"));
        let w = node_view(&json!({"ID": "n2", "Spec": {"Role": "worker"}}));
        assert_eq!((w.role, w.availability, w.state.as_str()), (SwarmRole::Worker, Availability::Active, "unknown"));
    }

    #[test]
    fn updated_spec_keeps_what_was_not_asked_for() {
        let cur = json!({"Role": "worker", "Availability": "active", "Labels": {"zone": "a"}, "Name": "keep"});
        let s = updated_spec(&cur, None, Some(Availability::Drain), None);
        assert_eq!(s["Availability"], "drain");
        assert_eq!(s["Role"], "worker");
        assert_eq!(s["Labels"]["zone"], "a");
        assert_eq!(s["Name"], "keep");
        let l = BTreeMap::from([("rack".to_owned(), "2".to_owned())]);
        let s = updated_spec(&cur, Some(SwarmRole::Manager), None, Some(&l));
        assert_eq!(s["Role"], "manager");
        assert_eq!(s["Labels"], json!({"rack": "2"}));
    }

    #[test]
    fn tokens_never_leak_through_errors() {
        let e = without_token("Docker: join failed for SWMTKN-1-abc-def", "SWMTKN-1-abc-def");
        assert!(!e.contains("SWMTKN"));
    }
}
