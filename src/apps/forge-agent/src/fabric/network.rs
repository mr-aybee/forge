//! The node's cluster container network (D-172): a
//! routed Docker bridge on the node's instance block, which every mesh peer
//! already routes, so containers get addresses other nodes reach directly.
//! No masquerade and no inbound filtering on it (routed gateway mode, Docker
//! 28 or newer). Reached over the Engine API, never the `docker` CLI.

use std::path::Path;
use std::time::Duration;

use forge_catalog::fabric::{Cidr, network_bridge, network_name};
use forge_proto::messages::RefusalCode;
use serde_json::{Value, json};

use super::run;
use crate::docker::SOCKET;
use crate::docker_ops::{call, enc, get};
use crate::executor::CommandError;

const QUICK: Duration = Duration::from_secs(30);
const MIN_DOCKER: (u64, u64, u64) = (28, 0, 0);
const MTU_OPT: &str = "com.docker.network.driver.mtu";
const GATEWAY_OPT: &str = "com.docker.network.bridge.gateway_mode_ipv4";
const BRIDGE_OPT: &str = "com.docker.network.bridge.name";
const FORWARD_CONF: &str = "/etc/sysctl.d/90-forge-fabric.conf";

/// What [`ensure`] must do with the network it found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EnsureStep {
    Keep,
    Create,
    /// Remove the network with this id, then create it.
    Recreate(String),
}

/// `major.minor.patch` of a Docker version string (`28.1.0`, `28.1.0-rc1`,
/// `28.0.4+dfsg1`); `None` when it does not start with three numbers.
pub(crate) fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let core = v.trim().split(['-', '+', '~']).next()?;
    let mut parts = core.split('.').map(str::parse::<u64>);

    match (parts.next(), parts.next(), parts.next()) {
        (Some(Ok(a)), Some(Ok(b)), Some(Ok(c))) => Some((a, b, c)),
        _ => None,
    }
}

/// Refuses Docker older than 28.0 (no routed gateway mode).
pub(crate) fn version_gate(version: &str) -> Result<(), CommandError> {
    match parse_version(version) {
        Some(v) if v >= MIN_DOCKER => Ok(()),
        _ => Err(CommandError::refused(
            RefusalCode::Unsupported,
            format!("docker 28.0 or newer is needed for the cluster container network (found {version})"),
        )),
    }
}

/// The subnets of a network's IPAM configuration.
fn subnets(n: &Value) -> Vec<Cidr> {
    let configs = n.pointer("/IPAM/Config").and_then(Value::as_array).cloned().unwrap_or_default();

    configs.iter().filter_map(|c| c["Subnet"].as_str()?.parse().ok()).collect()
}

/// Names of the containers attached to an inspected network.
fn attached(n: &Value) -> Vec<String> {
    let Some(map) = n["Containers"].as_object() else { return Vec::new() };

    map.values().map(|c| c["Name"].as_str().unwrap_or("?").to_owned()).collect()
}

/// Whether an inspected network carries the subnet and options we create.
fn matches(n: &Value, subnet: &Cidr, mtu: u16, iface: &str) -> bool {
    let opt = |k: &str| n["Options"][k].as_str();

    subnets(n) == [*subnet]
        && opt(MTU_OPT) == Some(&mtu.to_string())
        && opt(GATEWAY_OPT) == Some("routed")
        && opt(BRIDGE_OPT) == Some(&network_bridge(iface))
}

/// Decides what to do with the network named for `iface`. `current` is its
/// inspection (with `Containers`), `others` every other network. A network
/// that differs is recreated only while no container is attached: moving
/// containers is a redeploy, which the operator starts.
pub(crate) fn plan_ensure(
    iface: &str,
    subnet: &Cidr,
    mtu: u16,
    current: Option<&Value>,
    others: &[Value],
) -> Result<EnsureStep, String> {
    let name = network_name(iface);

    for n in others {
        if let Some(s) = subnets(n).iter().find(|s| s.overlaps(subnet)) {
            let other = n["Name"].as_str().unwrap_or("?");

            return Err(format!("Docker network {other} already uses {s}, which overlaps {subnet}"));
        }
    }

    let Some(n) = current else { return Ok(EnsureStep::Create) };

    if matches(n, subnet, mtu, iface) {
        return Ok(EnsureStep::Keep);
    }

    let users = attached(n);

    if !users.is_empty() {
        let old = subnets(n).iter().map(ToString::to_string).collect::<Vec<_>>().join(", ");

        return Err(format!(
            "network {name} has {} containers on subnet {old}; redeploy stacks to move to {subnet}",
            users.len()
        ));
    }

    Ok(EnsureStep::Recreate(n["Id"].as_str().unwrap_or(&name).to_owned()))
}

/// The create body (`POST /networks/create`).
pub(crate) fn create_body(iface: &str, subnet: &Cidr, mtu: u16) -> Value {
    json!({
        "Name": network_name(iface),
        "Driver": "bridge",
        "CheckDuplicate": true,
        "Attachable": true,
        "IPAM": { "Config": [ { "Subnet": subnet.to_string() } ] },
        "Options": {
            MTU_OPT: mtu.to_string(),
            GATEWAY_OPT: "routed",
            BRIDGE_OPT: network_bridge(iface),
        },
        "Labels": { "forge.managed": "true", "forge.fabric": iface },
    })
}

/// Refuses when Docker is not installed: like an old Docker, only the
/// operator changes that, so the control plane must not retry.
fn docker_present() -> Result<(), CommandError> {
    if Path::new(SOCKET).exists() {
        return Ok(());
    }

    Err(CommandError::refused(RefusalCode::Unsupported, "Docker is not installed on this node"))
}

/// The inspected network named for `iface`, if it exists. The name filter
/// matches substrings, so the exact name is picked from the list.
async fn find(iface: &str) -> Result<(Option<Value>, Vec<Value>), String> {
    let name = network_name(iface);
    let all = get("/networks").await?;
    let list = all.as_array().cloned().unwrap_or_default();
    let (mine, others): (Vec<Value>, Vec<Value>) = list.into_iter().partition(|n| n["Name"].as_str() == Some(&name));

    let Some(id) = mine.first().and_then(|n| n["Id"].as_str()) else { return Ok((None, others)) };
    let current = get(&format!("/networks/{}", enc(id))).await?;

    Ok((Some(current), others))
}

/// Checks that `subnet` lies inside the address prefix of `iface`, so the
/// mesh routes it to this node. Returns the node's mesh address.
async fn inside_interface(iface: &str, subnet: &Cidr) -> Result<std::net::Ipv4Addr, String> {
    let text = run("ip", &["-j", "addr", "show", iface]).await?;
    let links: Value = serde_json::from_str(&text).map_err(|e| format!("ip addr {iface}: {e}"))?;
    let addrs = links.pointer("/0/addr_info").and_then(Value::as_array).cloned().unwrap_or_default();

    let inside = addrs.iter().find_map(|a| {
        let (Some(ip), Some(len)) = (a["local"].as_str(), a["prefixlen"].as_u64()) else { return None };
        let (Ok(ip), Ok(len)) = (ip.parse::<std::net::Ipv4Addr>(), u8::try_from(len)) else { return None };

        Cidr::new(ip, len).is_ok_and(|net| net.contains_net(subnet)).then_some(ip)
    });

    if let Some(ip) = inside {
        return Ok(ip);
    }

    Err(format!("{subnet} is not inside the addresses of {iface}"))
}

/// Turns on IPv4 forwarding when it is off and keeps it on across reboots,
/// as relays do: containers on the routed bridge are reached through it.
async fn ensure_forwarding() -> Result<(), String> {
    let now =
        tokio::fs::read_to_string("/proc/sys/net/ipv4/ip_forward").await.map_err(|e| format!("ip_forward: {e}"))?;

    if now.trim() == "1" {
        return Ok(());
    }

    run("sysctl", &["-w", "net.ipv4.ip_forward=1"]).await?;

    tokio::task::spawn_blocking(|| super::write_private(Path::new(FORWARD_CONF), "net.ipv4.ip_forward = 1\n"))
        .await
        .map_err(|e| e.to_string())?
}

/// [`forge_catalog::fabric::FabricOp::NetworkEnsure`].
pub async fn ensure(iface: &str, subnet: &Cidr, mtu: u16) -> Result<Value, CommandError> {
    docker_present()?;

    let version = get("/version").await?;
    let version = version["Version"].as_str().unwrap_or_default().to_owned();
    version_gate(&version)?;

    let mesh_ip = inside_interface(iface, subnet).await?;

    let (current, others) = find(iface).await?;
    let step = plan_ensure(iface, subnet, mtu, current.as_ref(), &others)?;

    if let EnsureStep::Recreate(id) = &step {
        call("DELETE", &format!("/networks/{}", enc(id)), None, None, QUICK).await?;
    }

    if step != EnsureStep::Keep {
        call("POST", "/networks/create", Some(&create_body(iface, subnet, mtu)), None, QUICK).await?;
    }

    ensure_forwarding().await?;

    // Name service on the mesh address; never fails the network.
    let dns = super::dns::configure(iface, mesh_ip).await;

    Ok(json!({
        "forge_dns": dns,
        "name": network_name(iface),
        "subnet": subnet.to_string(),
        "created": step != EnsureStep::Keep,
        "recreated": matches!(step, EnsureStep::Recreate(_)),
        "docker_version": version,
    }))
}

/// [`forge_catalog::fabric::FabricOp::NetworkRemove`]. A node without Docker
/// has nothing to remove.
pub async fn remove(iface: &str) -> Result<Value, String> {
    let name = network_name(iface);

    if !Path::new(SOCKET).exists() {
        return Ok(json!({ "name": name, "removed": false }));
    }

    let (current, _) = find(iface).await?;
    let Some(n) = current else { return Ok(json!({ "name": name, "removed": false })) };
    let users = attached(&n);

    if !users.is_empty() {
        return Err(format!("network {name} still has containers attached: {}", users.join(", ")));
    }

    let id = n["Id"].as_str().unwrap_or(&name);
    call("DELETE", &format!("/networks/{}", enc(id)), None, None, QUICK).await?;

    Ok(json!({ "name": name, "removed": true }))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // Reason: test assertions.
mod tests {
    use super::*;

    const IFACE: &str = "forgea1b2c3";

    fn block() -> Cidr {
        "10.64.12.0/24".parse().unwrap()
    }

    fn net(subnet: &str, containers: &[&str]) -> Value {
        let users: serde_json::Map<String, Value> =
            containers.iter().enumerate().map(|(i, c)| (format!("id{i}"), json!({ "Name": c }))).collect();

        json!({
            "Id": "n1",
            "Name": network_name(IFACE),
            "IPAM": { "Config": [ { "Subnet": subnet } ] },
            "Options": { MTU_OPT: "1420", GATEWAY_OPT: "routed", BRIDGE_OPT: network_bridge(IFACE) },
            "Containers": users,
        })
    }

    #[test]
    fn docker_version_gate() {
        assert!(version_gate("27.5.1").is_err());
        assert!(version_gate("28.0.0").is_ok());
        assert!(version_gate("28.1.0-rc1").is_ok());
        assert!(version_gate("28.0.4+dfsg1").is_ok());
        assert!(version_gate("").is_err());

        let e = version_gate("27.5.1").unwrap_err();
        assert_eq!(e.code, Some(RefusalCode::Unsupported));
        assert_eq!(e.message, "docker 28.0 or newer is needed for the cluster container network (found 27.5.1)");
    }

    #[test]
    fn ensure_creates_keeps_and_recreates() {
        let plan = |current: Option<&Value>| plan_ensure(IFACE, &block(), 1420, current, &[]);

        assert_eq!(plan(None).unwrap(), EnsureStep::Create);
        assert_eq!(plan(Some(&net("10.64.12.0/24", &[]))).unwrap(), EnsureStep::Keep);
        assert_eq!(plan(Some(&net("10.64.12.0/24", &["web-1"]))).unwrap(), EnsureStep::Keep);
        assert_eq!(plan(Some(&net("10.64.9.0/24", &[]))).unwrap(), EnsureStep::Recreate("n1".into()));

        let e = plan(Some(&net("10.64.9.0/24", &["a", "b"]))).unwrap_err();
        assert_eq!(
            e,
            "network forge-forgea1b2c3 has 2 containers on subnet 10.64.9.0/24; redeploy stacks to move to 10.64.12.0/24"
        );
    }

    #[test]
    fn ensure_refuses_overlap_with_other_networks() {
        let other = json!({ "Name": "shop_default", "IPAM": { "Config": [ { "Subnet": "10.64.0.0/16" } ] } });

        assert!(plan_ensure(IFACE, &block(), 1420, None, &[other]).unwrap_err().contains("shop_default"));
    }

    #[test]
    fn create_body_is_routed_with_mesh_mtu() {
        let b = create_body(IFACE, &block(), 1420);

        assert_eq!(b["Name"], "forge-forgea1b2c3");
        assert_eq!(b.pointer("/IPAM/Config/0/Subnet").unwrap(), "10.64.12.0/24");
        assert_eq!(b["Options"][GATEWAY_OPT], "routed");
        assert_eq!(b["Options"][MTU_OPT], "1420");
        assert_eq!(b["Options"][BRIDGE_OPT], "fgforgea1b");
        assert_eq!(b["Labels"]["forge.fabric"], IFACE);
    }
}
