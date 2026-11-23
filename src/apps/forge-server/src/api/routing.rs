//! How ingress endpoints reach services.
//!
//! * [`Resolver`] turns typed targets into Nginx upstreams (`ip:port`). A Forge
//!   service expands to every node it is placed on. A node (app or node
//!   target) is reached at its fabric (mesh) address only when its mesh is
//!   up and every edge of the endpoint is up in the same fabric; otherwise
//!   at the node's own (LAN) address ([`pick_address`]). Active placements
//!   become active upstreams, standby placements standby upstreams. A
//!   service rolled out by a deployment (#73) expands to the copies that
//!   take traffic on each node (slot `a`/`b` on their own ports), with
//!   their weights; a copy at weight 0 is left out (drained).
//! * `GET /api/v1/clusters/{id}/routing` — per endpoint: domain → edges →
//!   service (internal name) → upstream addresses, with placement state.

use std::collections::HashMap;

use axum::Json;
use axum::extract::{Path, State};
use forge_catalog::fabric::{FabricSpec, Lease, LeaseKind};
use forge_catalog::ingress::Target;
use forge_core::ports::{
    ClusterRepository, FabricRepository, ServerRepository, ServiceRepository, SiteRepository, TenancyRepository,
};
use forge_core::{Action, ClusterId, EntityKind, EntityRef, ServiceId};
use serde_json::{Value, json};

use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// One resolved hop to a backend.
#[derive(Debug, Clone)]
pub struct Upstream {
    pub address: String,
    pub standby: bool,
    /// Node the address belongs to, when known.
    pub server_id: Option<String>,
    /// Last state the node reported for the placement (`active`, `failed`, …).
    pub state: Option<String>,
    /// Relative weight (100 unless a rollout shifts traffic between copies).
    pub weight: u32,
}

/// Upstreams for Nginx: active and standby addresses, and the weights of
/// active ones that are not 100.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Resolved {
    pub active: Vec<String>,
    pub standby: Vec<String>,
    pub weights: std::collections::BTreeMap<String, u32>,
}

/// Address book for one resolution pass.
#[derive(Clone)]
pub struct Resolver {
    /// Node id → its own (LAN / primary) address.
    lan: HashMap<String, String>,
    /// Node id → (cluster, fabric address), only for nodes whose mesh is up.
    mesh: HashMap<String, (ClusterId, String)>,
    /// The edges that will connect to the upstreams, when known: the mesh
    /// address is used only when every edge is up in the target's fabric.
    edges: Option<Vec<String>>,
    /// Fabric domain per cluster, for display names.
    domains: HashMap<ClusterId, String>,
}

fn host_port(host: &str, port: u16) -> String {
    if host.contains(':') { format!("[{host}]:{port}") } else { format!("{host}:{port}") }
}

/// The address an edge set uses to reach a node (issue #18, the same rule as
/// the fleet's names, D-084): the node's fabric address only when the node's
/// mesh is up (`mesh`) and every edge is up in that same fabric
/// (`edge_fabrics`: each edge's cluster when its mesh is up, else `None`);
/// otherwise the node's own address. With no edges known, the fabric address
/// is used when the node has one.
#[must_use]
pub fn pick_address<'a>(
    lan: &'a str,
    mesh: Option<(ClusterId, &'a str)>,
    edge_fabrics: Option<&[Option<ClusterId>]>,
) -> &'a str {
    match (mesh, edge_fabrics) {
        (Some((_, m)), None) => m,
        (Some((c, m)), Some(edges)) if !edges.is_empty() && edges.iter().all(|e| *e == Some(c)) => m,
        _ => lan,
    }
}

impl Resolver {
    /// Node addresses: the fabric address of nodes whose mesh is up in their
    /// cluster's network, and every node's own address. Which one a target
    /// gets depends on the edges ([`Self::for_edges`]).
    pub async fn new(state: &AppState) -> ApiResult<Self> {
        let mut mesh: HashMap<String, (ClusterId, String)> = HashMap::new();
        let mut domains = HashMap::new();
        for f in state.store.list_fabrics(None).await? {
            if let Ok(spec) = serde_json::from_value::<FabricSpec>(f.spec.clone()) {
                domains.insert(f.cluster_id, spec.domain);
            }
            // Only nodes whose mesh is up: an address on a node where the
            // network failed to apply would be unreachable.
            let up = |id: &str| f.status.pointer(&format!("/nodes/{id}/state")).and_then(Value::as_str) == Some("ok");
            for l in serde_json::from_value::<Vec<Lease>>(f.leases.clone()).unwrap_or_default() {
                if l.kind == LeaseKind::Node && up(&l.owner) {
                    let ip = l.cidr.first().to_string();
                    mesh.insert(l.owner, (f.cluster_id, ip));
                }
            }
        }
        let mut lan = HashMap::new();
        for s in state.store.list_servers(None).await? {
            lan.insert(s.id.to_string(), super::ingress::node_address(state, &s).await);
        }
        Ok(Self { lan, mesh, edges: None, domains })
    }

    /// Resolves targets for these edges: a target's fabric address only when
    /// every edge is in the same fabric, else its own address.
    #[must_use]
    pub fn for_edges<I: IntoIterator<Item = S>, S: ToString>(mut self, edges: I) -> Self {
        self.edges = Some(edges.into_iter().map(|e| e.to_string()).collect());
        self
    }

    /// The address the edges use for node `id`.
    fn addr(&self, id: &str) -> Option<String> {
        let lan = self.lan.get(id)?;
        let mesh = self.mesh.get(id).map(|(c, m)| (*c, m.as_str()));
        let edges: Option<Vec<Option<ClusterId>>> =
            self.edges.as_ref().map(|es| es.iter().map(|e| self.mesh.get(e).map(|(c, _)| *c)).collect());
        Some(pick_address(lan, mesh, edges.as_deref()).to_owned())
    }

    /// Expands one target. `standby` marks every resulting upstream standby
    /// (a target listed under standby targets).
    pub async fn expand(&self, state: &AppState, t: &Target, standby: bool) -> ApiResult<Vec<Upstream>> {
        match t {
            Target::App { service_id, port } => {
                if *port == 0 {
                    return Err(ApiError::bad_request("port must be 1–65535"));
                }
                let id: ServiceId = service_id.parse().map_err(|_| ApiError::bad_request("invalid service id"))?;
                let slots = forge_core::deploy::DeploymentRepository::list_slots(&state.store, id).await?;
                let mut out = Vec::new();
                for i in state.store.list_instances(id).await? {
                    let sid = i.server_id.to_string();
                    let Some(host) = self.addr(&sid) else { continue };
                    let copies: Vec<&forge_core::deploy::SlotRecord> =
                        slots.iter().filter(|s| s.server_id == i.server_id).collect();
                    if copies.is_empty() {
                        out.push(Upstream {
                            address: host_port(&host, *port),
                            standby: standby || i.role == "standby",
                            server_id: Some(sid),
                            state: i.state.clone(),
                            weight: 100,
                        });
                        continue;
                    }
                    for c in copies.into_iter().filter(|c| c.weight > 0) {
                        // The unslotted copy listens on the service port itself.
                        let p =
                            if c.slot.is_empty() { Some(*port) } else { crate::deploy::port_map(c).get(port).copied() };
                        let Some(p) = p else { continue };
                        out.push(Upstream {
                            address: host_port(&host, p),
                            standby: standby || i.role == "standby",
                            server_id: Some(sid.clone()),
                            state: i.state.clone(),
                            weight: c.weight,
                        });
                    }
                }
                Ok(out)
            }
            other => {
                let a = other.resolve(|id| self.addr(id)).map_err(ApiError::bad_request)?;
                let server_id = match other {
                    Target::Node { server_id, .. } => Some(server_id.clone()),
                    _ => None,
                };
                Ok(vec![Upstream { address: a, standby, server_id, state: None, weight: 100 }])
            }
        }
    }

    /// `(active, standby)` upstream lists for Nginx.
    pub async fn upstreams(
        &self,
        state: &AppState,
        targets: &[Target],
        standby: &[Target],
    ) -> ApiResult<(Vec<String>, Vec<String>)> {
        let r = self.resolve(state, targets, standby).await?;
        Ok((r.active, r.standby))
    }

    /// Upstream lists for Nginx, with the weights of active copies.
    pub async fn resolve(&self, state: &AppState, targets: &[Target], standby: &[Target]) -> ApiResult<Resolved> {
        let mut act = Vec::new();
        let mut sby = Vec::new();
        let mut weights = std::collections::BTreeMap::new();
        for (list, is_standby) in [(targets, false), (standby, true)] {
            for t in list {
                let ups = self.expand(state, t, is_standby).await?;
                if ups.is_empty()
                    && let Target::App { service_id, .. } = t
                {
                    return Err(ApiError::bad_request(format!(
                        "service {service_id} is not placed on any node yet; deploy it first"
                    )));
                }
                for u in ups {
                    let dst = if u.standby { &mut sby } else { &mut act };
                    if u.weight != 100 && !u.standby {
                        weights.insert(u.address.clone(), u.weight);
                    }
                    if !dst.contains(&u.address) {
                        dst.push(u.address);
                    }
                }
            }
        }
        // A standby that is also active is just active.
        sby.retain(|a| !act.contains(a));
        if act.is_empty() && !sby.is_empty() {
            act = std::mem::take(&mut sby);
            weights.clear();
        }
        weights.retain(|a, _| act.contains(a));
        Ok(Resolved { active: act, standby: sby, weights })
    }

    /// The address the edges (when set) use for a node.
    #[must_use]
    pub fn address_of(&self, server_id: &str) -> Option<String> {
        self.addr(server_id)
    }

    /// Every address a node may be reached at: its own and its fabric address.
    #[must_use]
    pub fn addresses_of(&self, server_id: &str) -> Vec<String> {
        self.lan.get(server_id).into_iter().chain(self.mesh.get(server_id).map(|(_, m)| m)).cloned().collect()
    }
}

/// `<app>.<environment>.<cluster domain>` when the cluster has a network.
async fn internal_name(state: &AppState, r: &Resolver, app: ServiceId) -> Option<(String, String)> {
    let a = state.store.get_service(app).await.ok()??;
    let ns = state.store.get_environment(a.environment_id).await.ok()?;
    let name = match ns.cluster_id.and_then(|c| r.domains.get(&c)) {
        Some(d) => format!("{}.{}.{d}", a.name, ns.name),
        None => format!("{}.{}", a.name, ns.name),
    };
    Some((a.name, name))
}

pub async fn get(State(state): State<AppState>, caller: Caller, Path(id): Path<ClusterId>) -> ApiResult<Json<Value>> {
    caller.authorize(Action::View, EntityRef::new(EntityKind::Cluster, id)).await?;
    let cluster = state.store.get_cluster(id).await?;
    let r = Resolver::new(&state).await?;
    let all_servers = state.store.list_servers(None).await?;
    let members: Vec<String> =
        all_servers.iter().filter(|s| s.cluster_id == Some(id)).map(|s| s.id.to_string()).collect();
    let servers: HashMap<String, String> =
        all_servers.into_iter().map(|s| (s.id.to_string(), s.display_name.clone().unwrap_or(s.hostname))).collect();
    let profile = super::ingress::load_profile(&state, id).await.ok().flatten().and_then(|(p, _)| p.ingress).is_some();
    let cid = id.to_string();
    let mut endpoints = Vec::new();
    for site in state.store.list_sites().await? {
        let Some(source) = endpoint_source(&site.config, &cid, &members) else { continue };
        if source == "edges" && site.workspace_id != cluster.workspace_id {
            continue;
        }
        let edge_ids: Vec<String> = serde_json::from_value(site.config["edges"].clone()).unwrap_or_default();
        let r = r.clone().for_edges(edge_ids);
        let edges: Vec<Value> = site.config["edges"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(|e| {
                json!({
                    "server_id": e,
                    "name": servers.get(e),
                    "address": r.address_of(e),
                    "status": site.edge_status[e]["status"],
                })
            })
            .collect();
        let mut targets = Vec::new();
        for (key, standby) in [("targets", false), ("standby_targets", true)] {
            let list: Vec<Target> = serde_json::from_value(site.config[key].clone()).unwrap_or_default();
            for t in list {
                let (label, internal) = match &t {
                    Target::App { service_id, .. } => match service_id.parse().ok() {
                        Some(a) => {
                            internal_name(&state, &r, a).await.map_or((service_id.clone(), None), |(n, i)| (n, Some(i)))
                        }
                        None => (service_id.clone(), None),
                    },
                    Target::Node { server_id, port } => {
                        (format!("{}:{port}", servers.get(server_id).cloned().unwrap_or_default()), None)
                    }
                    Target::External { address } => (address.clone(), None),
                };
                let ups = r.expand(&state, &t, standby).await.unwrap_or_default();
                // Nginx logs the address it connected to (`$upstream_addr`,
                // an IP): resolve names so the console can match traffic.
                let mut resolved = Vec::with_capacity(ups.len());
                for u in &ups {
                    let ips: Vec<String> = match tokio::time::timeout(
                        std::time::Duration::from_secs(2),
                        tokio::net::lookup_host(u.address.clone()),
                    )
                    .await
                    {
                        Ok(Ok(it)) => it.map(|a| a.to_string()).collect(),
                        _ => Vec::new(),
                    };
                    resolved.push(ips);
                }
                targets.push(json!({
                    "target": t,
                    "label": label,
                    "internal_name": internal,
                    "standby": standby,
                    "upstreams": ups.iter().zip(resolved).map(|(u, ips)| json!({
                        "address": u.address,
                        "resolved": ips,
                        "standby": u.standby,
                        "server_id": u.server_id,
                        "node": u.server_id.as_ref().and_then(|s| servers.get(s)),
                        "state": u.state,
                    })).collect::<Vec<_>>(),
                }));
            }
        }
        endpoints.push(json!({
            "id": site.id,
            "name": site.name,
            "domains": site.config["spec"]["domains"],
            "tls": site.config["spec"]["tls"],
            "edges": edges,
            "targets": targets,
            "source": source,
            "linked": source == "profile",
        }));
    }
    Ok(Json(json!({ "endpoints": endpoints, "profile": profile })))
}

/// Why an endpoint shows in a cluster's routing table: `profile` when it is
/// linked to the cluster (`cluster_id`), `edges` when it is not linked to any
/// cluster but one of its explicit edges is a member of this one (F-11).
/// `None` when it belongs elsewhere.
fn endpoint_source(config: &Value, cluster: &str, members: &[String]) -> Option<&'static str> {
    match config["cluster_id"].as_str() {
        Some(c) if c == cluster => Some("profile"),
        Some(_) => None,
        None => config["edges"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .any(|e| members.iter().any(|m| m == e))
            .then_some("edges"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// F-11: a site with explicit edges on this cluster's nodes shows up as
    /// `edges`; one linked to another cluster or on other nodes does not.
    #[test]
    fn edge_based_sites_belong_to_the_cluster_of_their_edges() {
        let members = vec!["lab-a".to_owned()];
        let linked = json!({ "cluster_id": "c1", "edges": ["lab-a"] });
        assert_eq!(endpoint_source(&linked, "c1", &members), Some("profile"));
        assert_eq!(endpoint_source(&linked, "c2", &members), None);
        let explicit = json!({ "cluster_id": null, "edges": ["deb-x", "lab-a"] });
        assert_eq!(endpoint_source(&explicit, "c1", &members), Some("edges"));
        let elsewhere = json!({ "edges": ["deb-x"] });
        assert_eq!(endpoint_source(&elsewhere, "c1", &members), None);
        assert_eq!(endpoint_source(&json!({}), "c1", &members), None);
    }

    /// Issue #18: an edge outside the targets' fabric gets their LAN address.
    #[test]
    fn mesh_addresses_only_when_every_edge_shares_the_fabric() {
        let (control, lab) = (ClusterId::new(), ClusterId::new());
        let mesh = Some((lab, "10.64.0.16"));
        let lan = "192.168.1.20";
        // Edge in another cluster (or with no mesh up): LAN.
        assert_eq!(pick_address(lan, mesh, Some(&[Some(control)])), lan);
        assert_eq!(pick_address(lan, mesh, Some(&[None])), lan);
        // Edge in the same fabric: mesh.
        assert_eq!(pick_address(lan, mesh, Some(&[Some(lab)])), "10.64.0.16");
        // One edge outside is enough to need the LAN address (one upstream
        // list serves every edge).
        assert_eq!(pick_address(lan, mesh, Some(&[Some(lab), Some(control)])), lan);
        // Target without a mesh up: LAN, whatever the edges.
        assert_eq!(pick_address(lan, None, Some(&[Some(lab)])), lan);
        // Edges unknown (display only): the mesh address when there is one.
        assert_eq!(pick_address(lan, mesh, None), "10.64.0.16");
        assert_eq!(pick_address(lan, mesh, Some(&[])), lan);
    }
}
