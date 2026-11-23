//! Docker images, volumes and networks on this node (D-157), run by the root
//! helper over the local Engine API. The operations are the closed set in
//! [`forge_catalog::docker_ops::DockerOp`], validated before they get here.
//! Networks are inspected first, so a protected network cannot be reached
//! by its id. Registry credentials are passed as a header and never logged.

use std::path::Path;
use std::time::Duration;

use forge_catalog::docker_ops::{DockerOp, network_protected};
use serde_json::{Value, json};

use crate::docker::{SOCKET, call_with_registry_auth, docker_error};

const QUICK: Duration = Duration::from_secs(30);
/// Pulls, pushes and prunes move layers.
const SLOW: Duration = Duration::from_secs(15 * 60);
const MAX_EVENTS: usize = 1000;
/// Empty registry credentials: Docker wants the header on a push.
const NO_AUTH: &str = "e30=";

/// Percent-encodes a query value (RFC 3986 unreserved characters stay).
fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

async fn call(
    method: &str,
    path: &str,
    body: Option<&Value>,
    auth: Option<&str>,
    timeout: Duration,
) -> Result<Value, String> {
    let (status, bytes) = call_with_registry_auth(Path::new(SOCKET), method, path, body, auth, timeout).await?;
    if !(200..300).contains(&status) && status != 304 {
        return Err(docker_error(status, &bytes));
    }
    let text = String::from_utf8_lossy(&bytes);
    // Pulls and pushes stream JSON lines: an error can arrive mid-stream.
    if let Some(err) = text
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find_map(|v| v["error"].as_str().map(str::to_owned))
    {
        return Err(format!("Docker: {err}"));
    }
    // Some answers are empty or JSON lines; the summary below reads JSON only.
    Ok(serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn get(path: &str) -> Result<Value, String> {
    call("GET", path, None, None, QUICK).await
}

/// A network checked against the protected set by its *name*, whatever the
/// caller used to name it.
async fn unprotected_network(id: &str) -> Result<Value, String> {
    let n = get(&format!("/networks/{}", enc(id))).await?;
    let name = n["Name"].as_str().unwrap_or_default();
    if network_protected(name) || n["Labels"]["forge.managed"] == "true" {
        return Err(format!("network `{name}` belongs to Docker or Forge"));
    }
    Ok(n)
}

/// Whether a container belongs to Forge (an app, a stack service or a
/// Swarm task): Forge's own labels.
fn forge_container(c: &Value) -> bool {
    c["Labels"]
        .as_object()
        .or_else(|| c["Config"]["Labels"].as_object())
        .is_some_and(|l| l.keys().any(|k| k.starts_with("forge.")))
}

/// Refuses Forge's own containers for network changes.
async fn foreign_container(name: &str) -> Result<(), String> {
    let c = get(&format!("/containers/{}/json", enc(name))).await?;
    if forge_container(&c) {
        return Err(format!("container `{name}` is managed by Forge; change its networks through its service"));
    }
    Ok(())
}

/// Refuses Forge's volumes (labelled, or named in Forge's namespace).
async fn foreign_volume(name: &str) -> Result<(), String> {
    let v = get(&format!("/volumes/{}", enc(name))).await?;
    if v["Labels"]["forge.managed"] == "true" || name.starts_with("forge") {
        return Err(format!("volume `{name}` belongs to Forge"));
    }
    Ok(())
}

/// A forced remove of an image that Forge's containers use is refused: the
/// next restart or reschedule would fail.
async fn image_unused_by_forge(id: &str) -> Result<(), String> {
    let filter = enc(&json!({ "ancestor": [id] }).to_string());
    let users = get(&format!("/containers/json?all=1&filters={filter}")).await?;
    if users.as_array().is_some_and(|a| a.iter().any(forge_container)) {
        return Err(format!("image `{id}` is used by containers Forge manages"));
    }
    Ok(())
}

/// Streams a pull or push, keeping only the last error line.
async fn transfer(method: &str, path: &str, auth: Option<&str>) -> Result<(), String> {
    let mut last_error: Option<String> = None;
    crate::docker::call_lines(method, path, auth, SLOW, |line| {
        if let Some(e) = serde_json::from_str::<Value>(line).ok().and_then(|v| v["error"].as_str().map(str::to_owned)) {
            last_error = Some(e);
        }
    })
    .await?;
    match last_error {
        Some(e) => Err(format!("Docker: {e}")),
        None => Ok(()),
    }
}

/// The node's IPv4 routes (`/proc/net/route`) as network and prefix length,
/// without the default route.
fn ipv4_routes(text: &str) -> Vec<(std::net::IpAddr, u8)> {
    text.lines()
        .skip(1)
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            let dest = u32::from_str_radix(f.get(1)?, 16).ok()?;
            let mask = u32::from_str_radix(f.get(7)?, 16).ok()?;
            (mask != 0).then(|| {
                (
                    std::net::IpAddr::V4(std::net::Ipv4Addr::from(u32::from_be(dest))),
                    u8::try_from(u32::from_be(mask).count_ones()).unwrap_or(32),
                )
            })
        })
        .collect()
}

/// Two networks overlap when either contains the other's start.
fn overlaps(a: (std::net::IpAddr, u8), b: (std::net::IpAddr, u8)) -> bool {
    forge_catalog::docker_ops::contains(a.0, a.1, b.0) || forge_catalog::docker_ops::contains(b.0, b.1, a.0)
}

/// Refuses a subnet that overlaps a route of the node: the bridge would take
/// that traffic (the node's LAN, the fabric, the way to the control plane).
async fn subnet_free(cidr: &str) -> Result<(), String> {
    let Some(net) = forge_catalog::docker_ops::subnet(cidr) else { return Err("invalid subnet".into()) };
    let text =
        tokio::fs::read_to_string("/proc/net/route").await.map_err(|e| format!("reading the node's routes: {e}"))?;
    if let Some((ip, len)) = ipv4_routes(&text).into_iter().find(|r| overlaps(*r, net)) {
        return Err(format!("{cidr} overlaps the node's route {ip}/{len}"));
    }
    Ok(())
}

pub async fn run(op: &DockerOp) -> Result<Value, String> {
    match op {
        DockerOp::DiskUsage {} => Ok(disk_usage(&get("/system/df").await?)),
        DockerOp::Images {} => {
            Ok(json!(get("/images/json").await?.as_array().map(|a| a.iter().map(image_view).collect::<Vec<_>>())))
        }
        DockerOp::ImageInspect { id } => Ok(image_detail(&get(&format!("/images/{}/json", enc(id))).await?)),
        DockerOp::ImageHistory { id } => get(&format!("/images/{}/history", enc(id))).await,
        DockerOp::ImageRemove { id, force } => {
            if *force {
                image_unused_by_forge(id).await?;
            }
            call("DELETE", &format!("/images/{}?force={force}", enc(id)), None, None, QUICK).await
        }
        DockerOp::ImagePrune { all } => {
            let filter = enc(&json!({ "dangling": [if *all { "false" } else { "true" }] }).to_string());
            call("POST", &format!("/images/prune?filters={filter}"), None, None, SLOW).await
        }
        DockerOp::ImageTag { id, repo, tag } => {
            let path = format!("/images/{}/tag?repo={}&tag={}", enc(id), enc(repo), enc(tag));
            call("POST", &path, None, None, QUICK).await.map(|_| json!({ "tagged": format!("{repo}:{tag}") }))
        }
        DockerOp::ImagePull { image, registry_auth } => {
            let path = format!("/images/create?fromImage={}", enc(image));
            transfer("POST", &path, registry_auth.as_deref()).await?;
            Ok(json!({ "pulled": image }))
        }
        DockerOp::ImagePush { image, registry_auth } => {
            let (name, tag) = split_tag(image);
            let path = format!("/images/{}/push?tag={}", enc(name), enc(tag));
            transfer("POST", &path, Some(registry_auth.as_deref().unwrap_or(NO_AUTH))).await?;
            Ok(json!({ "pushed": image }))
        }
        DockerOp::Volumes {} => {
            let v = get("/volumes").await?;
            Ok(json!(v["Volumes"].as_array().map(|a| a.iter().map(volume_view).collect::<Vec<_>>())))
        }
        DockerOp::VolumeInspect { name } => Ok(volume_view(&get(&format!("/volumes/{}", enc(name))).await?)),
        DockerOp::VolumeCreate { name, labels } => {
            // Local driver, no options: options can bind host paths.
            let body = json!({ "Name": name, "Driver": "local", "Labels": labels });
            call("POST", "/volumes/create", Some(&body), None, QUICK).await.map(|v| volume_view(&v))
        }
        DockerOp::VolumeRemove { name, force } => {
            foreign_volume(name).await?;
            call("DELETE", &format!("/volumes/{}?force={force}", enc(name)), None, None, QUICK)
                .await
                .map(|_| json!({ "removed": name }))
        }
        DockerOp::VolumePrune {} => call("POST", "/volumes/prune", None, None, SLOW).await,
        DockerOp::Networks {} => {
            Ok(json!(get("/networks").await?.as_array().map(|a| a.iter().map(network_view).collect::<Vec<_>>())))
        }
        DockerOp::NetworkInspect { id } => Ok(network_view(&get(&format!("/networks/{}", enc(id))).await?)),
        DockerOp::NetworkCreate { name, driver, subnet, gateway, internal, attachable, labels } => {
            let mut body = json!({
                "Name": name, "Driver": driver.as_str(), "Internal": internal, "Attachable": attachable,
                "CheckDuplicate": true, "Labels": labels,
            });
            if let Some(subnet) = subnet {
                subnet_free(subnet).await?;
                let mut cfg = json!({ "Subnet": subnet });
                if let Some(g) = gateway {
                    cfg["Gateway"] = json!(g);
                }
                body["IPAM"] = json!({ "Driver": "default", "Config": [cfg] });
            }
            call("POST", "/networks/create", Some(&body), None, QUICK).await
        }
        DockerOp::NetworkRemove { id } => {
            let n = unprotected_network(id).await?;
            let real = n["Id"].as_str().unwrap_or(id);
            call("DELETE", &format!("/networks/{}", enc(real)), None, None, QUICK)
                .await
                .map(|_| json!({ "removed": id }))
        }
        DockerOp::NetworkConnect { network, container } => {
            let n = unprotected_network(network).await?;
            foreign_container(container).await?;
            let real = n["Id"].as_str().unwrap_or(network);
            let body = json!({ "Container": container });
            call("POST", &format!("/networks/{}/connect", enc(real)), Some(&body), None, QUICK)
                .await
                .map(|_| json!({ "connected": container }))
        }
        DockerOp::NetworkDisconnect { network, container, force } => {
            let n = unprotected_network(network).await?;
            foreign_container(container).await?;
            let real = n["Id"].as_str().unwrap_or(network);
            let body = json!({ "Container": container, "Force": force });
            call("POST", &format!("/networks/{}/disconnect", enc(real)), Some(&body), None, QUICK)
                .await
                .map(|_| json!({ "disconnected": container }))
        }
        DockerOp::NetworkPrune {} => {
            // Forge's own networks stay, even while unused between deploys.
            let filter = enc(&json!({ "label!": ["forge.managed=true"] }).to_string());
            call("POST", &format!("/networks/prune?filters={filter}"), None, None, SLOW).await
        }
        DockerOp::Events { since_secs } => events(*since_secs).await,
    }
}

/// `name[:tag]` → (`name`, `tag`), `latest` when absent. A `:` in a
/// registry host (before the last `/`) is not a tag.
fn split_tag(image: &str) -> (&str, &str) {
    match image.rsplit_once(':') {
        Some((n, t)) if !t.contains('/') => (n, t),
        _ => (image, "latest"),
    }
}

async fn events(since_secs: u32) -> Result<Value, String> {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|e| e.to_string())?.as_secs();
    let since = now.saturating_sub(u64::from(since_secs));
    let path = format!("/events?since={since}&until={now}");
    // Streamed, keeping only the newest MAX_EVENTS lines.
    let mut newest: std::collections::VecDeque<String> = std::collections::VecDeque::new();
    crate::docker::call_lines("GET", &path, None, QUICK, |line| {
        if newest.len() == MAX_EVENTS {
            newest.pop_front();
        }
        newest.push_back(line.to_owned());
    })
    .await?;
    Ok(json!(event_lines(&newest.into_iter().collect::<Vec<_>>().join("\n"))))
}

/// The newest events, compact, newest first.
pub(crate) fn event_lines(text: &str) -> Vec<Value> {
    let mut out: Vec<Value> = text
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .map(|e| {
            json!({
                "time": e["time"], "type": e["Type"], "action": e["Action"],
                "id": e["Actor"]["ID"].as_str().map(|s| s.chars().take(12).collect::<String>()),
                "name": e["Actor"]["Attributes"]["name"],
            })
        })
        .collect();
    out.reverse();
    out.truncate(MAX_EVENTS);
    out
}

fn short(id: &str) -> String {
    id.trim_start_matches("sha256:").chars().take(12).collect()
}

pub(crate) fn image_view(i: &Value) -> Value {
    json!({
        "id": short(i["Id"].as_str().unwrap_or_default()),
        "full_id": i["Id"],
        "tags": i["RepoTags"],
        "digests": i["RepoDigests"],
        "size": i["Size"],
        "created": i["Created"],
        "containers": i["Containers"],
        "dangling": i["RepoTags"].as_array().is_none_or(|t| t.is_empty() || t.iter().all(|x| x == "<none>:<none>")),
    })
}

/// An image's details: no environment *values* (build-time secrets end up
/// there); the control plane adds them for callers allowed to read them.
pub(crate) fn image_detail(i: &Value) -> Value {
    let c = &i["Config"];
    let env: Vec<Value> = c["Env"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| e.as_str())
        .map(|e| {
            let (k, v) = e.split_once('=').unwrap_or((e, ""));
            json!({ "key": k, "value": v })
        })
        .collect();
    json!({
        "id": short(i["Id"].as_str().unwrap_or_default()),
        "full_id": i["Id"], "tags": i["RepoTags"], "digests": i["RepoDigests"],
        "created": i["Created"], "size": i["Size"], "architecture": i["Architecture"], "os": i["Os"],
        "author": i["Author"], "entrypoint": c["Entrypoint"], "cmd": c["Cmd"], "working_dir": c["WorkingDir"],
        "user": c["User"], "exposed_ports": c["ExposedPorts"], "volumes": c["Volumes"], "labels": c["Labels"],
        "env": env,
        "layers": i["RootFS"]["Layers"].as_array().map_or(0, Vec::len),
    })
}

pub(crate) fn volume_view(v: &Value) -> Value {
    json!({
        "name": v["Name"], "driver": v["Driver"], "mountpoint": v["Mountpoint"], "scope": v["Scope"],
        "created": v["CreatedAt"], "labels": v["Labels"],
        "managed": v["Labels"]["forge.managed"] == "true",
    })
}

pub(crate) fn network_view(n: &Value) -> Value {
    let name = n["Name"].as_str().unwrap_or_default();
    json!({
        "id": short(n["Id"].as_str().unwrap_or_default()),
        "name": name, "driver": n["Driver"], "scope": n["Scope"],
        "internal": n["Internal"], "attachable": n["Attachable"],
        "subnets": n["IPAM"]["Config"].as_array().map(|c| c.iter().filter_map(|x| x["Subnet"].as_str()).collect::<Vec<_>>()),
        "containers": n["Containers"].as_object().map_or(0, serde_json::Map::len),
        "protected": network_protected(name) || n["Labels"]["forge.managed"] == "true",
    })
}

pub(crate) fn disk_usage(df: &Value) -> Value {
    let sum = |key: &str, field: &str| {
        df[key].as_array().map_or(0, |a| a.iter().filter_map(|x| x[field].as_i64()).sum::<i64>())
    };
    json!({
        "images": { "count": df["Images"].as_array().map_or(0, Vec::len), "bytes": sum("Images", "Size") },
        "containers": { "count": df["Containers"].as_array().map_or(0, Vec::len), "bytes": sum("Containers", "SizeRw") },
        "volumes": { "count": df["Volumes"].as_array().map_or(0, Vec::len), "bytes": df["Volumes"].as_array().map_or(0, |a| a.iter().filter_map(|v| v["UsageData"]["Size"].as_i64()).filter(|n| *n > 0).sum::<i64>()) },
        "build_cache": { "count": df["BuildCache"].as_array().map_or(0, Vec::len), "bytes": sum("BuildCache", "Size") },
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn queries_are_encoded() {
        assert_eq!(enc("ghcr.io/a/b"), "ghcr.io%2Fa%2Fb");
        assert_eq!(enc("{\"x\":1}"), "%7B%22x%22%3A1%7D");
    }

    #[test]
    fn tags_split_after_the_registry_host() {
        assert_eq!(split_tag("nginx:1.27"), ("nginx", "1.27"));
        assert_eq!(split_tag("registry:5000/team/app"), ("registry:5000/team/app", "latest"));
        assert_eq!(split_tag("registry:5000/team/app:v2"), ("registry:5000/team/app", "v2"));
    }

    #[test]
    fn views_are_compact_and_flag_protected_networks() {
        let img = image_view(&json!({"Id": "sha256:abcdef0123456789", "RepoTags": ["<none>:<none>"], "Size": 10}));
        assert_eq!(img["id"], "abcdef012345");
        assert_eq!(img["dangling"], true);
        let net = network_view(&json!({"Id": "n1", "Name": "forge-net-x", "Containers": {"a": {}}}));
        assert_eq!(net["protected"], true);
        assert_eq!(net["containers"], 1);
        let df = disk_usage(&json!({"Images": [{"Size": 5}, {"Size": 7}], "Volumes": [{"UsageData": {"Size": -1}}]}));
        assert_eq!(df["images"]["bytes"], 12);
        assert_eq!(df["volumes"]["bytes"], 0);
    }

    #[test]
    fn events_are_newest_first_and_capped() {
        let text = (0..1200)
            .map(|i| format!("{{\"time\":{i},\"Type\":\"container\",\"Action\":\"start\"}}"))
            .collect::<Vec<_>>()
            .join("\n");
        let ev = event_lines(&text);
        assert_eq!(ev.len(), MAX_EVENTS);
        assert_eq!(ev[0]["time"], 1199);
    }

    #[test]
    fn routes_are_read_and_overlaps_found() {
        let text = "Iface\tDestination\tGateway\tFlags\tRefCnt\tUse\tMetric\tMask\n\
eth0\t00000000\t0102000A\t0003\t0\t0\t0\t00000000\n\
eth0\t0002000A\t00000000\t0001\t0\t0\t0\t00FFFFFF\n";
        let routes = ipv4_routes(text);
        assert_eq!(routes.len(), 1, "the default route is left out");
        assert_eq!(routes[0], ("10.0.2.0".parse().unwrap(), 24));
        let net = forge_catalog::docker_ops::subnet("10.0.0.0/16").unwrap();
        assert!(overlaps(routes[0], net));
        let other = forge_catalog::docker_ops::subnet("10.30.0.0/24").unwrap();
        assert!(!overlaps(routes[0], other));
    }

    #[test]
    fn forge_containers_are_recognised() {
        assert!(forge_container(&json!({"Labels": {"forge.app": "prod/web"}})));
        assert!(forge_container(&json!({"Config": {"Labels": {"forge.spec": "x"}}})));
        assert!(!forge_container(&json!({"Labels": {"com.example": "x"}})));
    }

    #[test]
    fn image_details_keep_env_for_the_control_plane_to_filter() {
        let d = image_detail(
            &json!({"Id": "sha256:0123456789abcdef", "Config": {"Env": ["TOKEN=s3cret", "PATH=/bin"]}, "RootFS": {"Layers": ["a", "b"]}}),
        );
        assert_eq!(d["env"][0]["key"], "TOKEN");
        assert_eq!(d["layers"], 2);
    }
}
