//! Swarm stacks, the shared network, networks and secrets/configs on a
//! manager (root, inside `forge-agent-helper`; D-171). See
//! `forge_catalog::swarm_stack` for the model and the policy.
//!
//! * A stack deploy re-checks the stack with the same policy as the control
//!   plane, writes `compose.yaml` (0600, root) under
//!   `/opt/forge/swarm-stacks/<environment>/<stack>/` and runs
//!   `docker stack deploy` with a fixed argument list and a cleared
//!   environment. `.env` values are interpolated before the file is written
//!   and never stored separately.
//! * Secret and config values arrive base64-encoded in the signed command and
//!   go straight to the Engine API; nothing here logs or returns them.
//!
//! Linux only.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use forge_catalog::stacks::to_yaml;
use forge_catalog::swarm::{ObjectData, StackServiceAction, SwarmNetwork};
use forge_catalog::swarm_service::MANAGED_LABEL;
use forge_catalog::swarm_stack::{
    OBJECT_NAME_LABEL, OBJECT_VERSION_LABEL, ObjectKind, SHARED_NETWORK, SWARM_STACKS_DIR, StackPort, StackService,
    StackStatus, SwarmObject, SwarmStackPart, shared_services,
};
use serde_json::{Value, json};

use crate::docker::api;

const QUICK: Duration = Duration::from_secs(15);
/// `docker stack deploy` returns once the services are created or updated;
/// image pulls happen on the nodes afterwards.
const DEPLOY: Duration = Duration::from_secs(5 * 60);
const NAMESPACE_LABEL: &str = "com.docker.stack.namespace";

/// Creates the shared overlay when it is missing. Attachable, so Forge's own
/// containers and other stacks can join it; encrypted between nodes.
pub async fn ensure_shared_network() -> Result<Value, String> {
    if api("GET", &format!("/networks/{SHARED_NETWORK}"), None, QUICK).await?.is_none() {
        let body = json!({
            "Name": SHARED_NETWORK, "Driver": "overlay", "Attachable": true, "CheckDuplicate": true,
            "Options": { "encrypted": "true" },
            "Labels": { MANAGED_LABEL: "true" },
        });
        api("POST", "/networks/create", Some(&body), QUICK).await?;
    }

    Ok(json!({ "network": SHARED_NETWORK }))
}

/// The swarm's networks (swarm scope, plus the local bridges that matter)
/// with subnets and the services attached to them.
pub async fn networks() -> Result<Value, String> {
    let nets = api("GET", "/networks", None, QUICK).await?.unwrap_or(Value::Null);
    let services = api("GET", "/services", None, QUICK).await?.unwrap_or(Value::Null);

    to_value(&network_views(&nets, &services))
}

pub(crate) fn network_views(nets: &Value, services: &Value) -> Vec<SwarmNetwork> {
    let mut attached: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

    for svc in services.as_array().into_iter().flatten() {
        let name = svc["Spec"]["Name"].as_str().unwrap_or_default();

        for vip in svc["Endpoint"]["VirtualIPs"].as_array().into_iter().flatten() {
            if let Some(id) = vip["NetworkID"].as_str() {
                attached.entry(id.to_owned()).or_default().insert(name.to_owned());
            }
        }

        for n in svc["Spec"]["TaskTemplate"]["Networks"].as_array().into_iter().flatten() {
            if let Some(id) = n["Target"].as_str() {
                attached.entry(id.to_owned()).or_default().insert(name.to_owned());
            }
        }
    }

    let mut out: Vec<SwarmNetwork> = nets
        .as_array()
        .into_iter()
        .flatten()
        .filter(|n| n["Scope"] == "swarm")
        .map(|n| {
            let id = n["Id"].as_str().unwrap_or_default().to_owned();
            let name = n["Name"].as_str().unwrap_or_default().to_owned();
            let labels = &n["Labels"];
            SwarmNetwork {
                services: attached.get(&id).or_else(|| attached.get(&name)).into_iter().flatten().cloned().collect(),
                driver: n["Driver"].as_str().unwrap_or_default().to_owned(),
                scope: n["Scope"].as_str().unwrap_or_default().to_owned(),
                subnets: n["IPAM"]["Config"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|c| c["Subnet"].as_str().map(str::to_owned))
                    .collect(),
                encrypted: n["Options"]["encrypted"].as_str().is_some(),
                attachable: n["Attachable"].as_bool().unwrap_or(false),
                ingress: n["Ingress"].as_bool().unwrap_or(false),
                managed: labels[MANAGED_LABEL] == "true" || labels[NAMESPACE_LABEL].as_str().is_some(),
                id,
                name,
            }
        })
        .collect();

    out.sort_by(|a, b| b.ingress.cmp(&a.ingress).then(a.name.cmp(&b.name)));

    out
}

/// Checks the stack again, writes its file and runs `docker stack deploy`.
pub async fn deploy(part: &SwarmStackPart) -> Result<Value, String> {
    let (tree, report) = part.prepare()?;
    let ns = part.namespace();
    let dir = part.policy.stack_dir.clone();
    let file = format!("{dir}/compose.yaml");
    let yaml = to_yaml(&tree);

    if !shared_services(&tree).is_empty() {
        ensure_shared_network().await?;
    }

    let write_dir = dir.clone();
    let write_file = file.clone();
    tokio::task::spawn_blocking(move || {
        crate::stacks::ensure_dir_in(SWARM_STACKS_DIR, &write_dir)?;
        crate::stacks::write_private(&write_file, &yaml)
    })
    .await
    .map_err(|e| e.to_string())??;

    let args = ["stack", "deploy", "--prune", "--with-registry-auth", "--resolve-image", "always", "-c", &file, &ns];
    crate::stacks::run("docker", &args, DEPLOY)
        .await
        .map_err(|e| e.replace("docker compose failed", "docker stack deploy failed"))?;

    Ok(json!({ "namespace": ns, "findings": report.findings }))
}

/// `docker stack rm`; removing a stack that is gone succeeds.
pub async fn remove(namespace: &str) -> Result<Value, String> {
    let services = list_services(namespace).await?;
    if services.as_array().is_some_and(|a| !a.is_empty()) {
        crate::stacks::run("docker", &["stack", "rm", namespace], DEPLOY)
            .await
            .map_err(|e| e.replace("docker compose failed", "docker stack rm failed"))?;
    }

    let dir = SWARM_STACKS_DIR.to_owned();
    let ns = namespace.to_owned();
    tokio::task::spawn_blocking(move || remove_files(&dir, &ns)).await.map_err(|e| e.to_string())?;

    Ok(json!({ "removed": namespace }))
}

/// Removes the stack's compose file. Best effort: the stack itself is gone,
/// and a leftover file is overwritten by the next deploy.
fn remove_files(base: &str, namespace: &str) {
    let Ok(envs) = std::fs::read_dir(base) else { return };

    for env in envs.flatten() {
        let Ok(stacks) = std::fs::read_dir(env.path()) else { continue };

        for stack in stacks.flatten() {
            let ns = format!("forge-{}-{}", env.file_name().to_string_lossy(), stack.file_name().to_string_lossy());

            if ns == namespace {
                let file = stack.path().join("compose.yaml");
                if let Err(e) = std::fs::remove_file(&file) {
                    tracing::debug!(error = %e, "swarm stack file already gone");
                }
            }
        }
    }
}

async fn list_services(namespace: &str) -> Result<Value, String> {
    let path = format!("/services?filters={}", label_filter(NAMESPACE_LABEL, namespace));
    Ok(api("GET", &path, None, QUICK).await?.unwrap_or(Value::Null))
}

/// The stack's services with their tasks (every state, newest first).
pub async fn status(namespace: &str) -> Result<Value, String> {
    let services = list_services(namespace).await?;
    let tasks = api("GET", &format!("/tasks?filters={}", label_filter(NAMESPACE_LABEL, namespace)), None, QUICK)
        .await?
        .unwrap_or(Value::Null);
    let shared = api("GET", &format!("/networks/{SHARED_NETWORK}"), None, QUICK).await?;
    let shared_id = shared.as_ref().and_then(|n| n["Id"].as_str()).unwrap_or_default().to_owned();

    to_value(&stack_status(namespace, &services, &tasks, &shared_id))
}

pub(crate) fn stack_status(namespace: &str, services: &Value, tasks: &Value, shared_id: &str) -> StackStatus {
    let prefix = format!("{namespace}_");
    let mut out = Vec::new();

    for svc in services.as_array().into_iter().flatten() {
        let id = svc["ID"].as_str().unwrap_or_default();
        let docker_name = svc["Spec"]["Name"].as_str().unwrap_or_default();
        let svc_tasks =
            Value::Array(tasks.as_array().into_iter().flatten().filter(|t| t["ServiceID"] == id).cloned().collect());
        let st = crate::swarm::status_from(svc, &svc_tasks);
        let mut task_list = st.tasks;
        task_list.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

        let ports = svc["Endpoint"]["Ports"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|p| StackPort {
                published: p["PublishedPort"].as_u64().and_then(|n| u16::try_from(n).ok()).unwrap_or(0),
                target: p["TargetPort"].as_u64().and_then(|n| u16::try_from(n).ok()).unwrap_or(0),
                protocol: p["Protocol"].as_str().unwrap_or("tcp").to_owned(),
                mode: p["PublishMode"].as_str().unwrap_or("ingress").to_owned(),
            })
            .collect();

        let shared = !shared_id.is_empty()
            && svc["Spec"]["TaskTemplate"]["Networks"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|n| n["Target"].as_str() == Some(shared_id));

        out.push(StackService {
            name: docker_name.strip_prefix(&prefix).unwrap_or(docker_name).to_owned(),
            docker_name: docker_name.to_owned(),
            id: id.to_owned(),
            image: st.image.split('@').next().unwrap_or_default().to_owned(),
            mode: st.mode,
            desired: st.desired,
            // Stack services carry no Forge fingerprint: every running task counts.
            running: st.running,
            ports,
            update_state: st.update_state,
            update_message: st.update_message,
            shared,
            tasks: task_list,
        });
    }

    out.sort_by(|a, b| a.name.cmp(&b.name));

    StackStatus { namespace: namespace.to_owned(), services: out }
}

/// The stack's service `service`, refusing one outside the namespace.
async fn stack_service(namespace: &str, service: &str) -> Result<Value, String> {
    let name = format!("{namespace}_{service}");
    let svc = api("GET", &format!("/services/{name}"), None, QUICK)
        .await?
        .ok_or_else(|| format!("service `{service}` is not in the stack"))?;

    if svc["Spec"]["Labels"][NAMESPACE_LABEL] != namespace {
        return Err(format!("`{name}` is not a service of this stack"));
    }

    Ok(svc)
}

/// Scales, restarts or rolls back one service of a stack.
pub async fn service_action(namespace: &str, service: &str, action: StackServiceAction) -> Result<Value, String> {
    let cur = stack_service(namespace, service).await?;
    let version = cur["Version"]["Index"].as_u64().ok_or("Docker did not report the service version")?;
    let id = cur["ID"].as_str().ok_or("Docker did not report the service id")?.to_owned();
    let mut spec = cur["Spec"].clone();
    let mut extra = "";

    match action {
        StackServiceAction::Scale { replicas } => {
            if !spec["Mode"]["Replicated"].is_object() {
                return Err("a global service runs one task per node; it cannot be scaled".into());
            }

            spec["Mode"]["Replicated"]["Replicas"] = json!(replicas);
        }
        StackServiceAction::Restart => {
            let n = spec["TaskTemplate"]["ForceUpdate"].as_u64().unwrap_or(0);
            spec["TaskTemplate"]["ForceUpdate"] = json!(n + 1);
        }
        StackServiceAction::Rollback => {
            if cur["PreviousSpec"].is_null() {
                return Err(format!("`{service}` has no previous version to roll back to"));
            }

            extra = "&rollback=previous";
        }
    }

    api("POST", &format!("/services/{id}/update?version={version}{extra}"), Some(&spec), QUICK).await?;

    Ok(json!({ "id": id }))
}

/// The last `tail` lines of a stack service's logs.
pub async fn service_logs(namespace: &str, service: &str, tail: u32) -> Result<Value, String> {
    let cur = stack_service(namespace, service).await?;
    let id = cur["ID"].as_str().ok_or("Docker did not report the service id")?;
    let path = format!("/services/{id}/logs?stdout=1&stderr=1&timestamps=1&tail={tail}");
    let (status, bytes) =
        crate::docker::call_at(std::path::Path::new(crate::docker::SOCKET), "GET", &path, None, QUICK).await?;

    if !(200..300).contains(&status) {
        return Err(crate::docker::docker_error(status, &bytes));
    }

    Ok(json!({ "logs": crate::docker::demux(&bytes) }))
}

/// Most task containers sampled at once.
const MAX_TASK_STATS: usize = 200;

/// CPU and memory of the Swarm task containers on this node. Docker's
/// one-off stats take about a second each, so they run side by side.
pub async fn task_stats() -> Result<Value, String> {
    let filter = "%7B%22label%22%3A%5B%22com.docker.swarm.task.id%22%5D%7D";
    let list = api("GET", &format!("/containers/json?filters={filter}"), None, QUICK).await?.unwrap_or(Value::Null);
    let mut set = tokio::task::JoinSet::new();

    for c in list.as_array().into_iter().flatten().take(MAX_TASK_STATS) {
        let Some(id) = c["Id"].as_str().filter(|i| crate::helper::valid_container_id(i)).map(str::to_owned) else {
            continue;
        };

        let task = c["Labels"]["com.docker.swarm.task.id"].as_str().unwrap_or_default().to_owned();
        let service = c["Labels"]["com.docker.swarm.service.name"].as_str().unwrap_or_default().to_owned();

        set.spawn(async move {
            let stats =
                api("GET", &format!("/containers/{id}/stats?stream=false"), None, QUICK).await.ok().flatten()?;
            Some(task_stat(task, service, &stats))
        });
    }

    let mut out = Vec::new();

    while let Some(r) = set.join_next().await {
        if let Ok(Some(t)) = r {
            out.push(t);
        }
    }

    to_value(&out)
}

pub(crate) fn task_stat(task_id: String, service: String, s: &Value) -> forge_catalog::swarm::TaskStat {
    let usage = s["memory_stats"]["usage"].as_u64().unwrap_or(0);

    // The page cache Docker's CLI leaves out (cgroup v2, then v1).
    let cache = s
        .pointer("/memory_stats/stats/inactive_file")
        .or_else(|| s.pointer("/memory_stats/stats/total_inactive_file"))
        .and_then(Value::as_u64)
        .unwrap_or(0);

    forge_catalog::swarm::TaskStat {
        task_id,
        service,
        cpu_percent: crate::docker::stats_cpu_percent(s),
        memory_bytes: usage.saturating_sub(cache),
        memory_limit: s["memory_stats"]["limit"].as_u64().unwrap_or(0),
    }
}

/// Secrets or configs of the swarm, with the services that use each.
pub async fn objects(kind: ObjectKind) -> Result<Value, String> {
    let list = api("GET", &format!("/{}", kind.path()), None, QUICK).await?.unwrap_or(Value::Null);
    let services = api("GET", "/services", None, QUICK).await?.unwrap_or(Value::Null);

    to_value(&object_views(kind, &list, &services))
}

pub(crate) fn object_views(kind: ObjectKind, list: &Value, services: &Value) -> Vec<SwarmObject> {
    let key = match kind {
        ObjectKind::Secret => "Secrets",
        ObjectKind::Config => "Configs",
    };

    let id_key = match kind {
        ObjectKind::Secret => "SecretID",
        ObjectKind::Config => "ConfigID",
    };

    let mut used: BTreeMap<String, Vec<String>> = BTreeMap::new();

    for svc in services.as_array().into_iter().flatten() {
        let name = svc["Spec"]["Name"].as_str().unwrap_or_default().to_owned();

        for r in svc["Spec"]["TaskTemplate"]["ContainerSpec"][key].as_array().into_iter().flatten() {
            if let Some(id) = r[id_key].as_str() {
                used.entry(id.to_owned()).or_default().push(name.clone());
            }
        }
    }

    let mut out: Vec<SwarmObject> = list
        .as_array()
        .into_iter()
        .flatten()
        .map(|o| {
            let id = o["ID"].as_str().unwrap_or_default().to_owned();
            let labels = &o["Spec"]["Labels"];
            let managed = labels[MANAGED_LABEL] == "true";
            SwarmObject {
                used_by: used.get(&id).cloned().unwrap_or_default(),
                kind,
                name: if managed {
                    labels[OBJECT_NAME_LABEL].as_str().unwrap_or_default().to_owned()
                } else {
                    String::new()
                },
                docker_name: o["Spec"]["Name"].as_str().unwrap_or_default().to_owned(),
                version: labels[OBJECT_VERSION_LABEL].as_str().and_then(|v| v.parse().ok()).unwrap_or(0),
                created_at: o["CreatedAt"].as_str().unwrap_or_default().to_owned(),
                managed,
                id,
            }
        })
        .collect();

    out.sort_by(|a, b| a.name.cmp(&b.name).then(b.version.cmp(&a.version)).then(a.docker_name.cmp(&b.docker_name)));

    out
}

/// Creates one version; an existing version of the same name is left alone
/// (a retried step), since Swarm objects are immutable.
pub async fn object_create(kind: ObjectKind, name: &str, version: u32, data: &ObjectData) -> Result<Value, String> {
    let docker_name = forge_catalog::swarm_stack::versioned(name, version);
    if api("GET", &format!("/{}/{docker_name}", kind.path()), None, QUICK).await?.is_some() {
        return Ok(json!({ "docker_name": docker_name, "created": false }));
    }

    let body = json!({
        "Name": docker_name,
        "Data": data.0,
        "Labels": { MANAGED_LABEL: "true", OBJECT_NAME_LABEL: name, OBJECT_VERSION_LABEL: version.to_string() },
    });

    // Docker may echo the request in an error; the value never goes back.
    api("POST", &format!("/{}/create", kind.path()), Some(&body), QUICK)
        .await
        .map_err(|e| e.replace(&data.0, "<value>"))?;

    Ok(json!({ "docker_name": docker_name, "created": true }))
}

/// Removes one Forge-made version; refused for objects Forge did not make.
pub async fn object_remove(kind: ObjectKind, docker_name: &str) -> Result<Value, String> {
    let Some(cur) = api("GET", &format!("/{}/{docker_name}", kind.path()), None, QUICK).await? else {
        return Ok(json!({ "removed": docker_name }));
    };

    if cur["Spec"]["Labels"][MANAGED_LABEL] != "true" {
        return Err(format!("{} `{docker_name}` was not made by Forge", kind.as_str()));
    }

    let id = cur["ID"].as_str().ok_or("Docker did not report the id")?;
    api("DELETE", &format!("/{}/{id}", kind.path()), None, QUICK).await?;

    Ok(json!({ "removed": docker_name }))
}

/// `filters={"label":{"<key>=<value>":true}}`, percent-encoded (keys and
/// values here are checked names).
fn label_filter(key: &str, value: &str) -> String {
    format!("%7B%22label%22%3A%7B%22{key}%3D{value}%22%3Atrue%7D%7D")
}

fn to_value<T: serde::Serialize>(v: &T) -> Result<Value, String> {
    serde_json::to_value(v).map_err(|e| e.to_string())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn networks_show_subnets_and_attached_services() {
        let nets = json!([
            {"Id": "n1", "Name": "ingress", "Driver": "overlay", "Scope": "swarm", "Ingress": true,
             "IPAM": {"Config": [{"Subnet": "10.0.0.0/24"}]}},
            {"Id": "n2", "Name": "forge-shared", "Driver": "overlay", "Scope": "swarm", "Attachable": true,
             "Options": {"encrypted": ""}, "Labels": {"forge.managed": "true"}},
            {"Id": "n3", "Name": "bridge", "Driver": "bridge", "Scope": "local"}
        ]);
        let services = json!([{"Spec": {"Name": "forge-prod-shop_api", "TaskTemplate": {"Networks": [{"Target": "n2"}]}},
            "Endpoint": {"VirtualIPs": [{"NetworkID": "n1"}]}}]);
        let v = network_views(&nets, &services);
        assert_eq!(v.len(), 2, "local networks are not the swarm's");
        assert!(v[0].ingress && v[0].subnets == vec!["10.0.0.0/24"]);
        assert!(v[1].encrypted && v[1].attachable && v[1].managed);
        assert_eq!(v[1].services, vec!["forge-prod-shop_api"]);
    }

    #[test]
    fn stack_status_names_services_and_ports() {
        let services = json!([{"ID": "s1", "Spec": {"Name": "forge-prod-shop_api", "Mode": {"Replicated": {"Replicas": 2}},
            "TaskTemplate": {"ContainerSpec": {"Image": "example/api:1@sha256:abc"}, "Networks": [{"Target": "sh"}]}},
            "Endpoint": {"Ports": [{"PublishedPort": 8080, "TargetPort": 80, "Protocol": "tcp", "PublishMode": "ingress"}]}}]);
        let tasks = json!([
            {"ID": "t1", "ServiceID": "s1", "DesiredState": "running", "Status": {"State": "running"}, "UpdatedAt": "2026-01-01T00:00:01Z"},
            {"ID": "t2", "ServiceID": "s1", "DesiredState": "shutdown", "Status": {"State": "failed", "Err": "exit 1"}, "UpdatedAt": "2026-01-01T00:00:02Z"},
            {"ID": "t3", "ServiceID": "other", "DesiredState": "running", "Status": {"State": "running"}}
        ]);
        let s = stack_status("forge-prod-shop", &services, &tasks, "sh");
        let api = &s.services[0];
        assert_eq!((api.name.as_str(), api.image.as_str()), ("api", "example/api:1"));
        assert_eq!((api.desired, api.running, api.tasks.len()), (2, 1, 2));
        assert_eq!(api.tasks[0].id, "t2", "newest first");
        assert_eq!(api.ports[0].published, 8080);
        assert!(api.shared);
        assert!(!s.converged());
    }

    #[test]
    fn objects_never_carry_values_and_show_use() {
        let list = json!([
            {"ID": "x1", "CreatedAt": "2026-01-01", "Spec": {"Name": "db.v2", "Data": "c2VjcmV0",
             "Labels": {"forge.managed": "true", "forge.object.name": "db", "forge.object.version": "2"}}},
            {"ID": "x2", "Spec": {"Name": "outside"}}
        ]);
        let services =
            json!([{"Spec": {"Name": "api", "TaskTemplate": {"ContainerSpec": {"Secrets": [{"SecretID": "x1"}]}}}}]);
        let v = object_views(ObjectKind::Secret, &list, &services);
        let text = serde_json::to_string(&v).unwrap();
        assert!(!text.contains("c2VjcmV0"));
        let db = v.iter().find(|o| o.docker_name == "db.v2").unwrap();
        assert_eq!((db.version, db.managed, db.used_by.clone()), (2, true, vec!["api".to_owned()]));
        assert!(!v.iter().find(|o| o.docker_name == "outside").unwrap().managed);
    }
}
