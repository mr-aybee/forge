//! Docker Swarm membership and nodes, run by the root helper over the local
//! Engine API (D-154). The operations are the closed set in
//! [`forge_catalog::swarm::SwarmOp`], validated before they get here.
//!
//! Join tokens appear in `Join` and in the results of `JoinTokens` and
//! `RotateTokens`; nothing here logs them.

use std::collections::BTreeMap;
use std::time::Duration;

use forge_catalog::swarm::{
    Availability, JoinTokens, SwarmNode, SwarmOp, SwarmRole, SwarmServiceStatus, SwarmStatus, SwarmTask,
};
use serde_json::{Value, json};

use forge_catalog::swarm_service::{
    MANAGED_LABEL, SERVICE_ID_LABEL, SLOT_LABEL, SPEC_LABEL, network_name, service_name,
};

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
                "Spec": { "EncryptionConfig": { "AutoLockManagers": autolock } },
            });
            if let Some(a) = advertise_addr {
                body["AdvertiseAddr"] = json!(format!("{a}:{port}"));
            }
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
            let mut body = json!({
                "ListenAddr": "0.0.0.0:2377",
                "RemoteAddrs": remote_addrs,
                "JoinToken": join_token,
            });
            if let Some(a) = advertise_addr {
                body["AdvertiseAddr"] = json!(a);
            }
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
        SwarmOp::EnsureNetwork { environment_id } => {
            let name = network_name(environment_id);
            if api("GET", &format!("/networks/{name}"), None, QUICK).await?.is_none() {
                // Encrypted (IPsec between nodes) and not attachable: only
                // this environment's services join it, never a stray container.
                let body = json!({
                    "Name": name, "Driver": "overlay", "Attachable": false, "CheckDuplicate": true,
                    "Options": { "encrypted": "true" },
                    "Labels": { MANAGED_LABEL: "true", "forge.environment.id": environment_id },
                });
                api("POST", "/networks/create", Some(&body), QUICK).await?;
            }
            Ok(json!({ "network": name }))
        }
        SwarmOp::ServiceApply { def, registry_auth } => {
            let spec = def.engine_spec();
            let auth = registry_auth.as_ref().map(|a| a.0.as_str());
            match owned(&def.service_id, &def.slot).await? {
                None => {
                    let created = api_auth("POST", "/services/create", &spec, auth).await?;
                    Ok(json!({ "id": created["ID"], "created": true }))
                }
                Some(cur) => {
                    let id = cur["ID"].as_str().ok_or("Docker did not report the service id")?;
                    // The same definition again (a retried step): no update,
                    // so Swarm's previous spec stays the release before.
                    if cur["Spec"]["Labels"][SPEC_LABEL] == spec["Labels"][SPEC_LABEL] {
                        return Ok(json!({ "id": id, "created": false, "unchanged": true }));
                    }
                    let version =
                        cur["Version"]["Index"].as_u64().ok_or("Docker did not report the service version")?;
                    api_auth("POST", &format!("/services/{id}/update?version={version}"), &spec, auth).await?;
                    Ok(json!({ "id": id, "created": false }))
                }
            }
        }
        SwarmOp::ServiceStatus { service_id, slot } => to_value(&service_status(service_id, slot).await?),
        SwarmOp::ServiceScale { service_id, slot, replicas } => {
            change_service(service_id, slot, "", |spec| {
                if spec["Mode"]["Replicated"].is_object() {
                    spec["Mode"]["Replicated"]["Replicas"] = json!(replicas);
                    Ok(())
                } else {
                    Err("a global service runs one task per node; it cannot be scaled".to_owned())
                }
            })
            .await
        }
        SwarmOp::ServiceRestart { service_id, slot } => {
            change_service(service_id, slot, "", |spec| {
                let n = spec["TaskTemplate"]["ForceUpdate"].as_u64().unwrap_or(0);
                spec["TaskTemplate"]["ForceUpdate"] = json!(n + 1);
                Ok(())
            })
            .await
        }
        SwarmOp::ServiceRollback { service_id, slot } => {
            change_service(service_id, slot, "&rollback=previous", |_| Ok(())).await
        }
        SwarmOp::ServiceRemove { service_id, slot } => {
            if let Some(cur) = owned(service_id, slot).await? {
                let id = cur["ID"].as_str().ok_or("Docker did not report the service id")?;
                api("DELETE", &format!("/services/{id}"), None, QUICK).await?;
            }
            Ok(json!({ "removed": service_id }))
        }
        SwarmOp::ServiceLogs { service_id, slot, tail } => {
            let cur = owned(service_id, slot).await?.ok_or("the service is not in the swarm")?;
            let id = cur["ID"].as_str().ok_or("Docker did not report the service id")?;
            let path = format!("/services/{id}/logs?stdout=1&stderr=1&timestamps=1&tail={tail}");
            let (status, bytes) =
                crate::docker::call_at(std::path::Path::new(crate::docker::SOCKET), "GET", &path, None, QUICK).await?;
            if !(200..300).contains(&status) {
                return Err(crate::docker::docker_error(status, &bytes));
            }
            Ok(json!({ "logs": crate::docker::demux(&bytes) }))
        }
    }
}

/// A service create or update with Docker's `X-Registry-Auth`, so the nodes
/// that run the tasks can pull a private image (`--with-registry-auth`).
async fn api_auth(method: &str, path: &str, body: &Value, auth: Option<&str>) -> Result<Value, String> {
    let socket = std::path::Path::new(crate::docker::SOCKET);
    let (status, bytes) = crate::docker::call_with_registry_auth(socket, method, path, Some(body), auth, QUICK).await?;
    if !(200..300).contains(&status) {
        return Err(crate::docker::docker_error(status, &bytes));
    }
    // An update answers with warnings only; the create's id is read when present.
    Ok(serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

/// The Swarm service of Forge service `service_id`, or `None` when there is
/// none. A service with that name that does not carry the same Forge id is
/// refused: Forge never touches a service it did not make for this id.
async fn owned(service_id: &str, slot: &str) -> Result<Option<Value>, String> {
    let Some(cur) = api("GET", &format!("/services/{}", service_name(service_id, slot)), None, QUICK).await? else {
        return Ok(None);
    };
    check_owner(&cur, service_id, slot)?;
    Ok(Some(cur))
}

pub(crate) fn check_owner(cur: &Value, service_id: &str, slot: &str) -> Result<(), String> {
    let labels = &cur["Spec"]["Labels"];
    let slot_label = labels[SLOT_LABEL].as_str().unwrap_or_default();
    if labels[MANAGED_LABEL] != "true" || labels[SERVICE_ID_LABEL] != service_id || slot_label != slot {
        return Err("a Swarm service with this name exists but Forge did not make it for this service".into());
    }
    Ok(())
}

/// Reads the service, lets `edit` change its spec, and writes it back at the
/// version read (`extra` adds query parameters, e.g. a rollback).
async fn change_service(
    service_id: &str,
    slot: &str,
    extra: &str,
    edit: impl FnOnce(&mut Value) -> Result<(), String>,
) -> Result<Value, String> {
    let cur = owned(service_id, slot).await?.ok_or("the service is not in the swarm")?;
    let version = cur["Version"]["Index"].as_u64().ok_or("Docker did not report the service version")?;
    let id = cur["ID"].as_str().ok_or("Docker did not report the service id")?.to_owned();
    let mut spec = cur["Spec"].clone();
    edit(&mut spec)?;
    api("POST", &format!("/services/{id}/update?version={version}{extra}"), Some(&spec), QUICK).await?;
    Ok(json!({ "id": id }))
}

/// `filters={"service":{"<name>":true}}`, percent-encoded (names are
/// checked to letters, digits, `-`, `_` and `.`).
fn task_filter(name: &str) -> String {
    format!("%7B%22service%22%3A%7B%22{name}%22%3Atrue%7D%7D")
}

async fn service_status(service_id: &str, slot: &str) -> Result<SwarmServiceStatus, String> {
    let Some(svc) = owned(service_id, slot).await? else {
        return Ok(SwarmServiceStatus::default());
    };
    let name = service_name(service_id, slot);
    let tasks =
        api("GET", &format!("/tasks?filters={}", task_filter(&name)), None, QUICK).await?.unwrap_or(Value::Null);
    Ok(status_from(&svc, &tasks))
}

/// A service and its tasks as the console shows them.
pub(crate) fn status_from(svc: &Value, tasks: &Value) -> SwarmServiceStatus {
    let tasks: Vec<SwarmTask> = tasks.as_array().map(|a| a.iter().map(task_view).collect()).unwrap_or_default();
    let replicated = svc["Spec"]["Mode"]["Replicated"].is_object();
    let spec = svc["Spec"]["Labels"][SPEC_LABEL].as_str().unwrap_or_default().to_owned();
    let live = |t: &&SwarmTask| t.state == "running" && t.desired_state == "running";
    SwarmServiceStatus {
        exists: true,
        id: svc["ID"].as_str().unwrap_or_default().to_owned(),
        mode: if replicated { "replicated" } else { "global" }.to_owned(),
        desired: if replicated {
            svc["Spec"]["Mode"]["Replicated"]["Replicas"].as_u64().unwrap_or(0)
        } else {
            tasks.iter().filter(|t| t.desired_state == "running").count() as u64
        },
        running: tasks.iter().filter(live).count() as u64,
        running_current: tasks.iter().filter(live).filter(|t| !spec.is_empty() && t.spec == spec).count() as u64,
        spec,
        update_state: svc["UpdateStatus"]["State"].as_str().unwrap_or_default().to_owned(),
        update_message: svc["UpdateStatus"]["Message"].as_str().unwrap_or_default().to_owned(),
        image: svc["Spec"]["TaskTemplate"]["ContainerSpec"]["Image"].as_str().unwrap_or_default().to_owned(),
        tasks,
    }
}

fn task_view(t: &Value) -> SwarmTask {
    SwarmTask {
        id: t["ID"].as_str().unwrap_or_default().to_owned(),
        node_id: t["NodeID"].as_str().unwrap_or_default().to_owned(),
        slot: t["Slot"].as_u64().unwrap_or(0),
        state: t["Status"]["State"].as_str().unwrap_or("unknown").to_owned(),
        desired_state: t["DesiredState"].as_str().unwrap_or_default().to_owned(),
        error: t["Status"]["Err"].as_str().unwrap_or_default().to_owned(),
        image: t["Spec"]["ContainerSpec"]["Image"].as_str().unwrap_or_default().to_owned(),
        spec: t["Spec"]["ContainerSpec"]["Labels"][SPEC_LABEL].as_str().unwrap_or_default().to_owned(),
        updated_at: t["UpdatedAt"].as_str().unwrap_or_default().to_owned(),
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

    #[test]
    fn service_status_counts_running_tasks() {
        let svc = json!({"ID": "s1", "Spec": {"Mode": {"Replicated": {"Replicas": 3}}, "Labels": {"forge.spec": "new"},
            "TaskTemplate": {"ContainerSpec": {"Image": "nginx:1.27"}}},
            "UpdateStatus": {"State": "updating", "Message": "update in progress"}});
        let tasks = json!([
            {"ID": "t1", "NodeID": "n1", "Slot": 1, "DesiredState": "running", "Status": {"State": "running"},
             "Spec": {"ContainerSpec": {"Labels": {"forge.spec": "old"}}}},
            {"ID": "t2", "NodeID": "n2", "Slot": 2, "DesiredState": "running", "Status": {"State": "starting"}},
            {"ID": "t3", "NodeID": "n2", "Slot": 3, "DesiredState": "shutdown", "Status": {"State": "failed", "Err": "exit 1"}}
        ]);
        let s = status_from(&svc, &tasks);
        assert!(s.exists);
        assert_eq!((s.desired, s.running, s.running_current), (3, 1, 0), "the running task is an old one");
        assert_eq!(s.update_state, "updating");
        assert_eq!(s.tasks[2].error, "exit 1");
        assert_eq!(s.image, "nginx:1.27");
        assert_eq!(task_filter("prod-web"), "%7B%22service%22%3A%7B%22prod-web%22%3Atrue%7D%7D");
    }

    #[test]
    fn foreign_services_are_never_touched() {
        let id = "01900000-0000-7000-8000-000000000001";
        let mine = json!({"Spec": {"Labels": {"forge.managed": "true", "forge.service.id": id}}});
        assert!(check_owner(&mine, id, "").is_ok());
        assert!(check_owner(&mine, id, "a").is_err(), "another slot is another service");
        let other = json!({"Spec": {"Labels": {"forge.managed": "true", "forge.service.id": "01900000-0000-7000-8000-000000000009"}}});
        assert!(check_owner(&other, id, "").is_err());
        let outside = json!({"Spec": {"Labels": {}}});
        assert!(check_owner(&outside, id, "").is_err());
    }
}
