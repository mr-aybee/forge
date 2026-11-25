//! Forge network fabric: the per-cluster address plan.
//!
//! Forge itself owns cluster addressing (no Swarm / K3s IPAM). A cluster gets
//! one IPv4 subnet, carved into fixed regions:
//!
//! | Region | `/16` cluster (large) | `/24` cluster (small) |
//! | --- | --- | --- |
//! | reserved (gateway, ingress VIPs) | `.0.1 – .0.15` | `.1 – .15` |
//! | node mesh addresses | `.0.16 – .0.254` | `.16 – .31` |
//! | service VIPs | `.1.0/24` | `.32 – .95` |
//! | per-node instance blocks | `/24` each from `.10.0` | `/28` each from `.96` |
//!
//! Allocation is stable: existing leases are kept, new owners take the lowest
//! free slot, and pinned (manual) leases are validated and never moved.
//! Everything here is pure and unit-tested; storage and transport live elsewhere.

use std::fmt;
use std::net::Ipv4Addr;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// An IPv4 network in CIDR form. Always normalised (host bits cleared).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Cidr {
    base: u32,
    prefix: u8,
}

impl Cidr {
    pub fn new(addr: Ipv4Addr, prefix: u8) -> Result<Self, String> {
        if prefix > 32 {
            return Err(format!("prefix /{prefix} is out of range"));
        }

        Ok(Self { base: u32::from(addr) & mask(prefix), prefix })
    }
    pub fn prefix(&self) -> u8 {
        self.prefix
    }
    pub fn size(&self) -> u64 {
        1u64 << (32 - self.prefix)
    }
    pub fn first(&self) -> Ipv4Addr {
        Ipv4Addr::from(self.base)
    }
    /// The `n`-th address in the network (0 = network address).
    pub fn nth(&self, n: u64) -> Option<Ipv4Addr> {
        (n < self.size()).then(|| Ipv4Addr::from(self.base.wrapping_add(n as u32)))
    }
    pub fn contains(&self, ip: Ipv4Addr) -> bool {
        u32::from(ip) & mask(self.prefix) == self.base
    }
    pub fn contains_net(&self, other: &Cidr) -> bool {
        other.prefix >= self.prefix && self.contains(other.first())
    }
    pub fn overlaps(&self, other: &Cidr) -> bool {
        self.contains_net(other) || other.contains_net(self)
    }
    /// The `n`-th sub-network of the given prefix inside this one.
    pub fn subnet(&self, prefix: u8, n: u64) -> Option<Cidr> {
        if prefix < self.prefix || prefix > 32 {
            return None;
        }

        let count = 1u64 << (prefix - self.prefix);

        (n < count).then(|| Cidr { base: self.base.wrapping_add((n * (1u64 << (32 - prefix))) as u32), prefix })
    }
    fn is_private(&self) -> bool {
        ["10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16", "100.64.0.0/10"]
            .iter()
            .filter_map(|s| s.parse::<Cidr>().ok())
            .any(|r| r.contains_net(self))
    }
}

fn mask(prefix: u8) -> u32 {
    if prefix == 0 { 0 } else { u32::MAX << (32 - prefix) }
}

impl FromStr for Cidr {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        let (a, p) = s.trim().split_once('/').ok_or_else(|| format!("{s:?} is not in CIDR form (a.b.c.d/n)"))?;
        let addr: Ipv4Addr = a.parse().map_err(|_| format!("{a:?} is not an IPv4 address"))?;
        let prefix: u8 = p.parse().map_err(|_| format!("{p:?} is not a prefix length"))?;

        Cidr::new(addr, prefix)
    }
}

impl fmt::Display for Cidr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.first(), self.prefix)
    }
}

impl Serialize for Cidr {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Cidr {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d)?.parse().map_err(serde::de::Error::custom)
    }
}

/// How nodes reach each other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FabricMode {
    /// Encrypted WireGuard mesh (default): works across sites, clouds and NAT.
    #[default]
    Mesh,
    /// Legacy value only: routed mode was offered but never applied to
    /// nodes. New specs are refused (`validate`); a stored routed network is
    /// shown as a mesh and becomes one on its next **Apply network**.
    Routed,
}

/// The desired network of one cluster.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FabricSpec {
    pub subnet: Cidr,
    #[serde(default)]
    pub mode: FabricMode,
    /// Internal DNS domain, e.g. `lab.forge.internal`.
    pub domain: String,
    /// WireGuard listen port on every node.
    #[serde(default = "default_port")]
    pub port: u16,
    /// Manual public endpoints (`host:port`) per node id, e.g. a port
    /// forward on a home router. Nodes without one are reached on their
    /// address when it is public, else they dial out (behind NAT).
    #[serde(default)]
    pub endpoints: std::collections::BTreeMap<String, String>,
}

fn default_port() -> u16 {
    51820
}

impl FabricSpec {
    pub fn validate(&self) -> Result<(), String> {
        if self.mode != FabricMode::Mesh {
            return Err("routed mode is not supported; use \"mesh\" (the encrypted WireGuard mesh)".into());
        }

        let p = self.subnet.prefix();
        if !(16..=24).contains(&p) {
            return Err("the cluster subnet must be between /16 and /24".into());
        }

        if !self.subnet.is_private() {
            return Err("the cluster subnet must be a private range (10/8, 172.16/12, 192.168/16 or 100.64/10)".into());
        }

        let d = &self.domain;
        let label_ok = |l: &str| {
            !l.is_empty()
                && l.len() <= 63
                && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                && !l.starts_with('-')
                && !l.ends_with('-')
        };

        if d.len() > 200 || !d.contains('.') || !d.split('.').all(label_ok) {
            return Err(format!("{d:?} is not a valid domain"));
        }

        if self.port < 1024 {
            return Err("the WireGuard port must be 1024 or higher".into());
        }

        for (node, ep) in &self.endpoints {
            valid_endpoint(ep).map_err(|e| format!("endpoint of {node}: {e}"))?;
        }

        Ok(())
    }

    pub fn layout(&self) -> Layout {
        Layout::of(self.subnet)
    }
}

/// The fixed regions of a cluster subnet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Layout {
    pub subnet: Cidr,
    /// Gateway / ingress VIPs: offsets `1..=15`.
    pub reserved: (u64, u64),
    /// Node mesh addresses (offsets, inclusive).
    pub nodes: (u64, u64),
    /// Service VIP addresses (offsets, inclusive).
    pub services: (u64, u64),
    /// Prefix of a per-node instance block.
    pub block_prefix: u8,
    /// Index of the first node block (in units of `block_prefix` networks).
    pub first_block: u64,
    /// Number of node blocks.
    pub blocks: u64,
}

impl Layout {
    pub fn of(subnet: Cidr) -> Self {
        let size = subnet.size();

        if subnet.prefix() <= 20 {
            // Large: first /24 = reserved + nodes, second /24 = services, /24 per node from the 10th.
            let blocks = size / 256;
            Layout {
                subnet,
                reserved: (1, 15),
                nodes: (16, 254),
                services: (256, 511),
                block_prefix: 24,
                first_block: 10,
                blocks: blocks.saturating_sub(10),
            }
        } else {
            // Small: everything in /28 units after the first 96 addresses.
            Layout {
                subnet,
                reserved: (1, 15),
                nodes: (16, 31),
                services: (32, 95),
                block_prefix: 28,
                first_block: 6,
                blocks: (size / 16).saturating_sub(6),
            }
        }
    }
    pub fn max_nodes(&self) -> u64 {
        self.blocks.min(self.nodes.1 - self.nodes.0 + 1)
    }
    fn block(&self, i: u64) -> Option<Cidr> {
        (i < self.blocks).then(|| self.subnet.subnet(self.block_prefix, self.first_block + i)).flatten()
    }
    fn offset(&self, ip: Ipv4Addr) -> Option<u64> {
        self.subnet.contains(ip).then(|| u64::from(u32::from(ip) - u32::from(self.subnet.first())))
    }
}

/// What a lease is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeaseKind {
    /// A node's address on the mesh (a `/32`).
    Node,
    /// A node's block for the instances it runs.
    Block,
    /// A service's stable virtual IP (a `/32`).
    Service,
}

impl LeaseKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            LeaseKind::Node => "node",
            LeaseKind::Block => "block",
            LeaseKind::Service => "service",
        }
    }
}

impl FromStr for LeaseKind {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "node" => Ok(LeaseKind::Node),
            "block" => Ok(LeaseKind::Block),
            "service" => Ok(LeaseKind::Service),
            _ => Err(format!("unknown lease kind {s:?}")),
        }
    }
}

/// One assignment of addresses to an owner (a node or a service id).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lease {
    pub kind: LeaseKind,
    pub owner: String,
    pub cidr: Cidr,
    /// Set by hand; never moved by the allocator.
    #[serde(default)]
    pub pinned: bool,
}

/// Compute the leases for the current owners. `previous` are the stored leases
/// (kept where still valid), `pins` are manual assignments (validated first).
pub fn allocate(
    spec: &FabricSpec,
    nodes: &[String],
    services: &[String],
    previous: &[Lease],
    pins: &[Lease],
) -> Result<Vec<Lease>, String> {
    let l = spec.layout();
    if nodes.len() as u64 > l.max_nodes() {
        return Err(format!("{} nodes do not fit in {} (at most {})", nodes.len(), spec.subnet, l.max_nodes()));
    }

    let wanted = |kind: LeaseKind, owner: &str| match kind {
        LeaseKind::Node | LeaseKind::Block => nodes.iter().any(|n| n == owner),
        LeaseKind::Service => services.iter().any(|s| s == owner),
    };

    let fits = |le: &Lease| -> bool {
        match le.kind {
            LeaseKind::Node => {
                le.cidr.prefix() == 32
                    && l.offset(le.cidr.first()).is_some_and(|o| (l.nodes.0..=l.nodes.1).contains(&o))
            }
            LeaseKind::Service => {
                le.cidr.prefix() == 32
                    && l.offset(le.cidr.first()).is_some_and(|o| (l.services.0..=l.services.1).contains(&o))
            }
            LeaseKind::Block => (0..l.blocks).any(|i| l.block(i) == Some(le.cidr)),
        }
    };

    let mut out: Vec<Lease> = Vec::new();
    let clash = |out: &[Lease], c: &Cidr| out.iter().any(|x| x.cidr.overlaps(c));

    // 1. Pins: must fit their region and not collide.
    for p in pins {
        if !wanted(p.kind, &p.owner) {
            continue;
        }

        if !fits(p) {
            return Err(format!(
                "{} for {} is outside the {} range of {}",
                p.cidr,
                p.owner,
                p.kind.as_str(),
                spec.subnet
            ));
        }

        if clash(&out, &p.cidr) {
            return Err(format!("{} is assigned twice", p.cidr));
        }

        if out.iter().any(|x| x.kind == p.kind && x.owner == p.owner) {
            return Err(format!("{} has two manual {} assignments", p.owner, p.kind.as_str()));
        }

        out.push(Lease { pinned: true, ..p.clone() });
    }

    let has = |out: &[Lease], k: LeaseKind, o: &str| out.iter().any(|x| x.kind == k && x.owner == o);

    // 2. Keep previous leases that are still wanted, valid and free.
    for p in previous {
        if wanted(p.kind, &p.owner) && fits(p) && !has(&out, p.kind, &p.owner) && !clash(&out, &p.cidr) {
            out.push(Lease { pinned: false, ..p.clone() });
        }
    }

    // 3. New owners take the lowest free slot.
    let host = |o: u64| l.subnet.nth(o).and_then(|ip| Cidr::new(ip, 32).ok());

    for n in nodes {
        if !has(&out, LeaseKind::Node, n) {
            let c = (l.nodes.0..=l.nodes.1)
                .filter_map(host)
                .find(|c| !clash(&out, c))
                .ok_or("no free node address left")?;
            out.push(Lease { kind: LeaseKind::Node, owner: n.clone(), cidr: c, pinned: false });
        }

        if !has(&out, LeaseKind::Block, n) {
            let c =
                (0..l.blocks).filter_map(|i| l.block(i)).find(|c| !clash(&out, c)).ok_or("no free node block left")?;
            out.push(Lease { kind: LeaseKind::Block, owner: n.clone(), cidr: c, pinned: false });
        }
    }

    for s in services {
        if !has(&out, LeaseKind::Service, s) {
            let c = (l.services.0..=l.services.1)
                .filter_map(host)
                .find(|c| !clash(&out, c))
                .ok_or("no free service address left")?;
            out.push(Lease { kind: LeaseKind::Service, owner: s.clone(), cidr: c, pinned: false });
        }
    }

    out.sort_by_key(|a| (a.kind, a.cidr));

    Ok(out)
}

/// The first `/prefix` network in `pool` that overlaps nothing in `taken`.
pub fn suggest_subnet(pool: Cidr, prefix: u8, taken: &[Cidr]) -> Option<Cidr> {
    if prefix < pool.prefix() {
        return None;
    }

    let count = 1u64 << (prefix - pool.prefix()).min(24);

    (0..count).filter_map(|i| pool.subnet(prefix, i)).find(|c| !taken.iter().any(|t| t.overlaps(c)))
}

/// Default pool clusters are carved from.
pub const DEFAULT_POOL: &str = "10.64.0.0/10";

// ------------------------------------------------------------------ mesh

/// Interface name of a cluster's mesh on every node (`forge` + 6 id chars,
/// within the 15-character Linux limit).
pub fn interface_name(cluster_id: &str) -> String {
    let tail: String = cluster_id.chars().filter(char::is_ascii_alphanumeric).take(6).collect();
    format!("forge{}", tail.to_lowercase())
}

/// MTU of the mesh: 1500 minus the WireGuard/UDP/IPv4 overhead.
pub const MESH_MTU: u16 = 1420;
/// Keep-alive for nodes behind NAT, so their mapping stays open.
pub const NAT_KEEPALIVE: u16 = 25;

fn valid_iface(s: &str) -> Result<(), String> {
    if s.is_empty() || s.len() > 15 || !s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
        return Err(format!("{s:?} is not a valid interface name"));
    }

    Ok(())
}

/// A fabric interface: only Forge's own `forge…` names, so fabric commands
/// (which need only `server.manage` for `status`) cannot read or change
/// other WireGuard interfaces such as remote-access gateways (`fgacc…`).
fn valid_fabric_iface(s: &str) -> Result<(), String> {
    valid_iface(s)?;

    if !s.starts_with("forge") {
        return Err(format!("{s:?} is not a Forge network interface"));
    }

    Ok(())
}

/// `host:port` with a plain host name or IPv4 address.
pub fn valid_endpoint(s: &str) -> Result<(), String> {
    let (h, p) = s.rsplit_once(':').ok_or("expected host:port")?;
    if p.parse::<u16>().map_err(|_| format!("{p:?} is not a port"))? == 0 {
        return Err("port 0 is not a usable port".into());
    }

    if h.is_empty() || h.len() > 253 || !h.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-') {
        return Err(format!("{h:?} is not a host name or IPv4 address"));
    }

    Ok(())
}

/// A base64 WireGuard public key (32 bytes → 44 characters).
pub fn valid_key(s: &str) -> Result<(), String> {
    if s.len() != 44
        || !s.ends_with('=')
        || !s[..43].bytes().all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/')
    {
        return Err("not a WireGuard public key".into());
    }

    Ok(())
}

/// One node as the mesh planner sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeshNode {
    pub id: String,
    /// Real address (IPv4 literal or host name) the agent connected from / reports.
    pub address: String,
    /// Manual public endpoint, overriding `address`.
    pub endpoint: Option<String>,
    pub public_key: Option<String>,
    pub mesh_ip: Cidr,
    pub block: Cidr,
    /// Can forward between peers (Linux).
    pub can_relay: bool,
}

impl MeshNode {
    /// Reachable from anywhere: a manual endpoint, a public IPv4 address, or a
    /// host name (assumed to resolve publicly).
    pub fn is_public(&self) -> bool {
        self.endpoint.is_some()
            || match self.address.parse::<Ipv4Addr>() {
                Ok(ip) => Cidr::new(ip, 32).is_ok_and(|c| !c.is_private()) && !ip.is_loopback(),
                Err(_) => !self.address.is_empty(),
            }
    }
    fn lan(&self) -> Option<Cidr> {
        self.address.parse::<Ipv4Addr>().ok().and_then(|ip| Cidr::new(ip, 24).ok())
    }
    fn endpoint_or(&self, port: u16) -> String {
        self.endpoint.clone().unwrap_or_else(|| format!("{}:{port}", self.address))
    }
}

/// A WireGuard peer of one node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Peer {
    pub public_key: String,
    #[serde(default)]
    pub endpoint: Option<String>,
    pub allowed_ips: Vec<Cidr>,
    #[serde(default)]
    pub keepalive: Option<u16>,
}

/// The desired WireGuard state of one node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeshConfig {
    pub iface: String,
    /// The node's mesh address, with the cluster prefix (e.g. `10.64.0.16/16`).
    pub address: String,
    pub port: u16,
    pub mtu: u16,
    pub peers: Vec<Peer>,
    /// Forward packets between peers (this node relays for NATed sites).
    #[serde(default)]
    pub forward: bool,
    /// Names under the cluster's internal domain and their mesh addresses
    /// (`node-1.lab.forge.internal`, `api.payments.lab.forge.internal`),
    /// written to a Forge-managed block of the node's hosts file.
    #[serde(default)]
    pub hosts: Vec<HostEntry>,
}

/// One hosts-file line managed by Forge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostEntry {
    pub name: String,
    pub ip: Ipv4Addr,
}

/// A host name under an internal domain: lowercase DNS labels.
#[must_use]
pub fn valid_hostname(n: &str) -> bool {
    n.len() <= 253
        && n.split('.').all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
}

impl MeshConfig {
    pub fn validate(&self) -> Result<(), String> {
        valid_fabric_iface(&self.iface)?;
        let (ip, prefix) = self.address.split_once('/').ok_or("address needs a prefix")?;
        ip.parse::<Ipv4Addr>().map_err(|_| "address is not IPv4")?;
        prefix.parse::<u8>().ok().filter(|p| (16..=32).contains(p)).ok_or("bad address prefix")?;

        if self.port < 1024 || !(1280..=1500).contains(&self.mtu) {
            return Err("port or MTU out of range".into());
        }

        for p in &self.peers {
            valid_key(&p.public_key)?;

            if let Some(e) = &p.endpoint {
                valid_endpoint(e)?;
            }
        }

        if self.hosts.len() > 4096 {
            return Err("too many host names".into());
        }

        if let Some(h) = self.hosts.iter().find(|h| !valid_hostname(&h.name)) {
            return Err(format!("invalid host name `{}`", h.name));
        }

        Ok(())
    }

    /// The wg-quick / WireGuard-for-Windows configuration. The private key is
    /// added by the node itself and never leaves it.
    pub fn render(&self, private_key: &str) -> String {
        let mut s = format!(
            "# Managed by Orbyn Forge. Changes are overwritten.\n[Interface]\nPrivateKey = {private_key}\nAddress = {}\nListenPort = {}\nMTU = {}\n",
            self.address, self.port, self.mtu
        );

        for p in &self.peers {
            s.push_str(&format!("\n[Peer]\nPublicKey = {}\n", p.public_key));

            if let Some(e) = &p.endpoint {
                s.push_str(&format!("Endpoint = {e}\n"));
            }

            let ips: Vec<String> = p.allowed_ips.iter().map(ToString::to_string).collect();
            s.push_str(&format!("AllowedIPs = {}\n", ips.join(", ")));

            if let Some(k) = p.keepalive {
                s.push_str(&format!("PersistentKeepalive = {k}\n"));
            }
        }

        s
    }
}

/// Plan one node's peers.
///
/// * Public nodes (and nodes on the same private /24) connect directly.
/// * A node behind NAT dials the public nodes and keeps the mapping open.
/// * Two NATed nodes on different networks cannot reach each other: their
///   traffic goes through the relay (the first public Linux node), whose peer
///   entry carries the other node's addresses, and the relay forwards.
///
/// Returns the config and the ids of peers that are unreachable (no key yet,
/// or no relay available).
pub fn plan_mesh(
    spec: &FabricSpec,
    iface: &str,
    nodes: &[MeshNode],
    me: &str,
) -> Result<(MeshConfig, Vec<String>), String> {
    let me_node = nodes.iter().find(|n| n.id == me).ok_or("node is not in the cluster")?;
    let relay = nodes.iter().find(|n| n.is_public() && n.can_relay && n.public_key.is_some());
    let same_lan =
        |a: &MeshNode, b: &MeshNode| !a.is_public() && !b.is_public() && a.lan().is_some() && a.lan() == b.lan();
    let mut peers: Vec<(String, Peer)> = Vec::new();
    let mut unreachable = Vec::new();
    let mut via_relay: Vec<Cidr> = Vec::new();
    let mut forward = false;

    for p in nodes.iter().filter(|n| n.id != me) {
        let Some(key) = &p.public_key else {
            unreachable.push(p.id.clone());
            continue;
        };

        let allowed = vec![p.mesh_ip, p.block];
        let direct = p.is_public() || me_node.is_public() || same_lan(me_node, p);
        if direct {
            let endpoint = (p.is_public() || same_lan(me_node, p)).then(|| p.endpoint_or(spec.port));
            let keepalive = (!me_node.is_public()).then_some(NAT_KEEPALIVE);
            peers.push((p.id.clone(), Peer { public_key: key.clone(), endpoint, allowed_ips: allowed, keepalive }));

            // The relay forwards for NATed nodes that cannot reach each other.
            if relay.is_some_and(|r| r.id == me) && !p.is_public() {
                forward = true;
            }
        } else if relay.is_some() {
            via_relay.extend(allowed);
        } else {
            unreachable.push(p.id.clone());
        }
    }

    if !via_relay.is_empty() {
        let r = relay.ok_or("no relay")?;
        match peers.iter_mut().find(|(id, _)| *id == r.id) {
            Some((_, peer)) => peer.allowed_ips.extend(via_relay),
            None => unreachable.push(r.id.clone()),
        }
    }

    let config = MeshConfig {
        iface: iface.to_owned(),
        address: format!("{}/{}", me_node.mesh_ip.first(), spec.subnet.prefix()),
        port: spec.port,
        mtu: MESH_MTU,
        peers: peers.into_iter().map(|(_, p)| p).collect(),
        forward,
        hosts: Vec::new(),
    };

    Ok((config, unreachable))
}

/// Operations the agent's fabric command runs (as root, in the helper).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum FabricOp {
    /// Install WireGuard if it is missing.
    Install,
    /// Create the node's key pair for `iface` if missing; returns the public key only.
    Key { iface: String },
    /// Bring the interface to `config` (addresses, peers, routes, forwarding).
    Apply { config: Box<MeshConfig> },
    /// Peers with their last handshake and traffic.
    Status { iface: String },
    /// Take the interface down and delete its configuration and key.
    Remove { iface: String },
}

impl FabricOp {
    pub fn validate(&self) -> Result<(), String> {
        match self {
            FabricOp::Install => Ok(()),
            FabricOp::Key { iface } | FabricOp::Status { iface } | FabricOp::Remove { iface } => {
                valid_fabric_iface(iface)
            }
            FabricOp::Apply { config } => config.validate(),
        }
    }
}

/// Interfaces whose addresses a node never offers as reachable from
/// outside: loopback, Forge's own tunnels and container bridges (only
/// reachable from the node itself). Used for the `interfaces` fact.
#[must_use]
pub fn offerable_interface(name: &str) -> bool {
    const SKIP: &[&str] =
        &["lo", "forge", "fgacc", "docker", "br-", "veth", "cni", "flannel", "cali", "vxlan", "kube", "virbr", "wg"];
    !name.is_empty() && !SKIP.iter().any(|p| name.starts_with(p))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// Security review: `status` on any interface (`server.manage` only)
    /// showed remote-access devices' keys and home endpoints.
    #[test]
    fn fabric_ops_touch_only_forge_interfaces() {
        let op = |iface: &str| FabricOp::Status { iface: iface.into() }.validate();
        assert!(op(&interface_name("01a1d3c6-2bb3")).is_ok());

        for other in ["fgacc01a1d3", "wg0", "eth0", "tailscale0"] {
            assert!(op(other).is_err(), "{other}");
            assert!(FabricOp::Remove { iface: other.into() }.validate().is_err(), "{other}");
        }
    }

    fn spec(s: &str) -> FabricSpec {
        FabricSpec {
            subnet: s.parse().unwrap(),
            mode: FabricMode::Mesh,
            domain: "lab.forge.internal".into(),
            port: 51820,
            endpoints: Default::default(),
        }
    }
    fn ids(n: usize, p: &str) -> Vec<String> {
        (0..n).map(|i| format!("{p}{i}")).collect()
    }

    #[test]
    fn cidr_parses_normalises_and_overlaps() {
        let c: Cidr = "10.42.7.9/24".parse().unwrap();
        assert_eq!(c.to_string(), "10.42.7.0/24");
        assert!(c.overlaps(&"10.42.0.0/16".parse().unwrap()));
        assert!(!c.overlaps(&"10.42.8.0/24".parse().unwrap()));
        assert!("10.42.7.0".parse::<Cidr>().is_err());
        assert!("10.42.7.0/33".parse::<Cidr>().is_err());
    }

    #[test]
    fn spec_validation() {
        assert!(spec("10.64.0.0/16").validate().is_ok());
        assert!(spec("10.64.0.0/8").validate().is_err());
        assert!(spec("8.8.0.0/16").validate().is_err());
        let mut s = spec("10.64.0.0/16");
        s.domain = "bad domain".into();
        assert!(s.validate().is_err());
        let mut s = spec("10.64.0.0/16");
        s.mode = FabricMode::Routed;
        assert!(s.validate().unwrap_err().contains("routed mode is not supported"));
    }

    #[test]
    fn large_layout_allocates_nodes_blocks_and_services() {
        let s = spec("10.64.0.0/16");
        let out = allocate(&s, &ids(2, "n"), &ids(1, "svc"), &[], &[]).unwrap();
        let get = |k, o: &str| out.iter().find(|l| l.kind == k && l.owner == o).unwrap().cidr.to_string();
        assert_eq!(get(LeaseKind::Node, "n0"), "10.64.0.16/32");
        assert_eq!(get(LeaseKind::Node, "n1"), "10.64.0.17/32");
        assert_eq!(get(LeaseKind::Block, "n0"), "10.64.10.0/24");
        assert_eq!(get(LeaseKind::Block, "n1"), "10.64.11.0/24");
        assert_eq!(get(LeaseKind::Service, "svc0"), "10.64.1.0/32");
    }

    #[test]
    fn small_layout_and_capacity() {
        let s = spec("10.64.5.0/24");
        let out = allocate(&s, &ids(1, "n"), &ids(1, "s"), &[], &[]).unwrap();
        assert!(out.iter().any(|l| l.kind == LeaseKind::Block && l.cidr.to_string() == "10.64.5.96/28"));
        assert!(out.iter().any(|l| l.kind == LeaseKind::Service && l.cidr.to_string() == "10.64.5.32/32"));
        assert_eq!(s.layout().max_nodes(), 10);
        assert!(allocate(&s, &ids(11, "n"), &[], &[], &[]).is_err());
    }

    #[test]
    fn allocation_is_stable_and_respects_pins() {
        let s = spec("10.64.0.0/16");
        let first = allocate(&s, &ids(3, "n"), &[], &[], &[]).unwrap();

        // n0 leaves: n1/n2 keep their addresses, a new node takes the freed slot.
        let again = allocate(&s, &["n1".into(), "n2".into(), "n9".into()], &[], &first, &[]).unwrap();
        let addr = |v: &[Lease], o: &str| v.iter().find(|l| l.kind == LeaseKind::Node && l.owner == o).unwrap().cidr;
        assert_eq!(addr(&first, "n1"), addr(&again, "n1"));
        assert_eq!(addr(&again, "n9").to_string(), "10.64.0.16/32");

        // A pin wins over a previous lease and is kept.
        let pin =
            Lease { kind: LeaseKind::Node, owner: "n1".into(), cidr: "10.64.0.100/32".parse().unwrap(), pinned: false };
        let pinned = allocate(&s, &["n1".into()], &[], &first, &[pin]).unwrap();
        assert_eq!(addr(&pinned, "n1").to_string(), "10.64.0.100/32");
        assert!(pinned.iter().any(|l| l.pinned));

        // Pins outside their range, or twice the same address, are refused.
        let bad =
            Lease { kind: LeaseKind::Node, owner: "n1".into(), cidr: "10.64.1.5/32".parse().unwrap(), pinned: true };
        assert!(allocate(&s, &["n1".into()], &[], &[], &[bad]).is_err());
        let a =
            Lease { kind: LeaseKind::Node, owner: "n1".into(), cidr: "10.64.0.50/32".parse().unwrap(), pinned: true };
        let b = Lease { owner: "n2".into(), ..a.clone() };
        assert!(allocate(&s, &["n1".into(), "n2".into()], &[], &[], &[a, b]).is_err());
    }

    #[test]
    fn suggests_the_first_free_subnet() {
        let pool: Cidr = DEFAULT_POOL.parse().unwrap();
        let taken: Vec<Cidr> = vec!["10.64.0.0/16".parse().unwrap(), "10.65.3.0/24".parse().unwrap()];
        assert_eq!(suggest_subnet(pool, 16, &taken).unwrap().to_string(), "10.66.0.0/16");
        assert_eq!(suggest_subnet(pool, 24, &taken).unwrap().to_string(), "10.65.0.0/24");
        assert!(suggest_subnet("10.0.0.0/24".parse().unwrap(), 16, &[]).is_none());
    }
    fn key(c: char) -> String {
        format!("{}=", c.to_string().repeat(43))
    }
    fn node(id: &str, addr: &str, k: char, i: u8) -> MeshNode {
        MeshNode {
            id: id.into(),
            address: addr.into(),
            endpoint: None,
            public_key: Some(key(k)),
            mesh_ip: format!("10.64.0.{}/32", 16 + i).parse().unwrap(),
            block: format!("10.64.{}.0/24", 10 + i).parse().unwrap(),
            can_relay: true,
        }
    }

    #[test]
    fn mesh_direct_nat_and_relay() {
        let s = spec("10.64.0.0/16");
        let cloud = node("cloud", "203.0.113.10", 'A', 0);
        let home = node("home", "192.168.1.20", 'B', 1);
        let lab = node("lab", "10.50.0.5", 'C', 2);
        let lab2 = node("lab2", "10.50.0.6", 'D', 3);
        let all = vec![cloud.clone(), home.clone(), lab.clone(), lab2.clone()];

        // The cloud node talks to everyone directly and relays.
        let (c, un) = plan_mesh(&s, "forgeabc", &all, "cloud").unwrap();
        assert!(un.is_empty());
        assert_eq!(c.peers.len(), 3);
        assert!(c.forward);
        assert!(c.peers.iter().all(|p| p.endpoint.is_none()), "NATed peers dial in");
        assert_eq!(c.address, "10.64.0.16/16");

        // A home node dials the cloud with keep-alive; lab traffic goes via the cloud relay.
        let (h, _) = plan_mesh(&s, "forgeabc", &all, "home").unwrap();
        assert_eq!(h.peers.len(), 1);
        let p = &h.peers[0];
        assert_eq!(p.endpoint.as_deref(), Some("203.0.113.10:51820"));
        assert_eq!(p.keepalive, Some(NAT_KEEPALIVE));
        assert!(p.allowed_ips.iter().any(|c| c.to_string() == "10.64.12.0/24"), "lab block via relay");
        assert!(!h.forward);

        // Nodes on the same private LAN connect directly.
        let (l, _) = plan_mesh(&s, "forgeabc", &all, "lab").unwrap();
        assert!(l.peers.iter().any(|p| p.public_key == key('D') && p.endpoint.as_deref() == Some("10.50.0.6:51820")));

        // A manual endpoint makes a home node public.
        let mut fwd = home.clone();
        fwd.endpoint = Some("home.example.net:51900".into());
        let (l2, _) = plan_mesh(&s, "forgeabc", &[cloud, fwd, lab.clone()], "lab").unwrap();
        assert!(l2.peers.iter().any(|p| p.endpoint.as_deref() == Some("home.example.net:51900")));

        // Without a public node, two NATed sites are unreachable for each other.
        let (_, un) = plan_mesh(&s, "forgeabc", &[home, lab], "home").unwrap();
        assert_eq!(un, vec!["lab".to_string()]);
    }

    #[test]
    fn mesh_config_renders_and_validates() {
        let s = spec("10.64.0.0/16");
        let all = vec![node("a", "203.0.113.10", 'A', 0), node("b", "203.0.113.11", 'B', 1)];
        let (c, _) = plan_mesh(&s, "forgeabc", &all, "a").unwrap();
        c.validate().unwrap();
        let text = c.render("PRIVATE");
        assert!(text.contains("PrivateKey = PRIVATE"));
        assert!(text.contains("Endpoint = 203.0.113.11:51820"));
        assert!(text.contains("AllowedIPs = 10.64.0.17/32, 10.64.11.0/24"));
        let mut bad = c.clone();
        bad.peers[0].public_key = "x\nPostUp = rm -rf /".into();
        assert!(bad.validate().is_err(), "keys cannot inject config lines");
        let mut bad = c;
        bad.peers[0].endpoint = Some("a b:1".into());
        assert!(bad.validate().is_err());
        assert!(FabricOp::Key { iface: "../../etc".into() }.validate().is_err());
        assert_eq!(interface_name("3F2a-91bc-0000"), "forge3f2a91");
    }

    #[test]
    fn hosts_block_is_replaced_and_removed() {
        let h = |n: &str, ip: [u8; 4]| crate::names::NameEntry { name: n.into(), ip: Ipv4Addr::from(ip).into() };
        let base = "127.0.0.1\tlocalhost\n::1\tlocalhost\n";
        let one =
            crate::names::render_block(base, "forge0", &[h("node-1.lab.forge.internal", [10, 64, 0, 16])]).unwrap();
        assert!(one.starts_with(base));
        assert!(one.contains("10.64.0.16\tnode-1.lab.forge.internal"));
        let two = crate::names::render_block(&one, "forge0", &[h("api.payments.lab.forge.internal", [10, 64, 1, 5])])
            .unwrap();
        assert!(!two.contains("node-1"), "the old block is replaced");
        assert_eq!(two.matches("# BEGIN orbyn-forge forge0").count(), 1);
        assert_eq!(crate::names::render_block(&two, "forge0", &[]).unwrap(), base, "an empty list removes the block");
        let crlf = crate::names::render_block("127.0.0.1 localhost\r\n", "forge0", &[h("a.b", [10, 0, 0, 1])]).unwrap();
        assert!(crlf.contains("\r\n# BEGIN") && crlf.ends_with("\r\n"));
        assert!(valid_hostname("api.payments.lab.forge.internal"));
        assert!(!valid_hostname("Bad_Name.forge.internal"));
        assert!(!valid_hostname("a..b"));
    }
}
