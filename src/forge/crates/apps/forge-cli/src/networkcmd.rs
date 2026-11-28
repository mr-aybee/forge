//! `forge clusters network`: the cluster network (the encrypted WireGuard
//! mesh and each node's container network, D-172) through
//! `/api/v1/clusters/{id}/fabric`. `set` saves the plan; `apply` brings the
//! nodes to it; `resync` retries one node; `remove` takes it off them.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

use clap::Subcommand;
use forge_catalog::fabric::{Cidr, FabricMode, FabricSpec, Lease, LeaseKind};
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::admincmd::{cluster_id, enc, node_id};
use crate::api::Api;
use crate::cmds::confirm;
use crate::error::{CliError, Exit, Result};
use crate::out::{self, Col};
use crate::{Global, connect, target};

/// The WireGuard port when neither the flag nor a saved network gives one.
const DEFAULT_PORT: u16 = 51820;

#[derive(Subcommand, Debug)]
pub enum NetworkCmd {
    /// The cluster network: its plan and what each node last reported.
    #[command(alias = "show")]
    Get {
        /// The cluster (id, name or abbreviation).
        cluster: String,
    },
    /// The next free subnet and the default domain for this cluster.
    Suggest {
        /// The cluster (id, name or abbreviation).
        cluster: String,
        /// The prefix length of the suggested subnet (16 to 24).
        #[arg(long, default_value_t = 16)]
        prefix: u8,
    },
    /// Save the cluster network plan. Flags you leave out keep the saved
    /// values; a cluster without a network gets the suggested subnet and
    /// domain. Run `apply` next.
    Set {
        /// The cluster (id, name or abbreviation).
        cluster: String,
        /// The private subnet the mesh and the container networks use (/16 to /24).
        #[arg(long, value_name = "CIDR")]
        subnet: Option<String>,
        /// The internal DNS domain, e.g. `lab.forge.internal`.
        #[arg(long)]
        domain: Option<String>,
        /// The WireGuard listen port on every node.
        #[arg(long)]
        port: Option<u16>,
        /// A node's public endpoint: `<node>=<host>:<port>` (a port forward,
        /// for example). Repeat for more; the list replaces the saved one.
        #[arg(long = "endpoint", value_name = "NODE=HOST:PORT")]
        endpoints: Vec<String>,
    },
    /// Bring every member node to the saved plan. The server then keeps
    /// them there.
    Apply {
        /// The cluster (id, name or abbreviation).
        cluster: String,
        /// Wait until every node reports `ok` or `failed`.
        #[arg(long)]
        wait: bool,
        /// How long `--wait` waits.
        #[arg(long, default_value_t = 300)]
        wait_timeout_secs: u64,
        /// Do not ask first (required with --non-interactive).
        #[arg(long)]
        yes: bool,
    },
    /// Retry one node now, without waiting for its back-off.
    Resync {
        /// The cluster (id, name or abbreviation).
        cluster: String,
        /// The node (id, hostname or display name).
        node: String,
    },
    /// Remove the cluster network: its plan, and on every online member its
    /// container network and mesh interface.
    Remove {
        /// The cluster (id, name or abbreviation).
        cluster: String,
        /// Do not ask first (required with --non-interactive).
        #[arg(long)]
        yes: bool,
    },
}

/// A member node as `GET /fabric` lists it.
#[derive(Debug, Deserialize)]
struct Member {
    id: String,
    name: String,
    #[serde(default)]
    address: Option<String>,
    #[serde(default)]
    status: Option<String>,
}

/// One node's reconcile state inside `status.nodes`.
#[derive(Debug, Default, Deserialize)]
struct NodeStatus {
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    network: Option<String>,
    #[serde(default)]
    network_error: Option<String>,
}

/// `GET /api/v1/clusters/{id}/fabric`.
#[derive(Debug, Deserialize)]
struct FabricView {
    #[serde(default)]
    spec: Option<FabricSpec>,
    #[serde(default)]
    pins: Option<Value>,
    #[serde(default)]
    leases: Option<Vec<Lease>>,
    #[serde(default)]
    status: Option<Value>,
    #[serde(default)]
    members: Vec<Member>,
}

impl FabricView {
    fn node_status(&self, id: &str) -> NodeStatus {
        let raw = self.status.as_ref().and_then(|s| s.get("nodes")).and_then(|n| n.get(id)).cloned();

        // A missing or unreadable entry reads as "no result yet".
        raw.and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default()
    }

    fn applied(&self) -> bool {
        self.status.as_ref().is_some_and(|s| s.get("applied_by").is_some())
    }

    fn node_ip(&self, id: &str) -> Option<String> {
        self.leases
            .as_ref()?
            .iter()
            .find(|l| l.kind == LeaseKind::Node && l.owner == id)
            .map(|l| l.cidr.first().to_string())
    }
}

/// `POST .../fabric/suggest`.
#[derive(Debug, Deserialize)]
struct Suggestion {
    subnet: String,
    domain: String,
}

/// `PUT .../fabric`.
#[derive(Debug, Serialize)]
struct FabricInput<'a> {
    spec: &'a FabricSpec,
    pins: Value,
}

/// The answer to `PUT .../fabric`.
#[derive(Debug, Deserialize)]
struct FabricSaved {
    #[serde(default)]
    restarts_containers: bool,
    #[serde(default)]
    redeploy_nodes: Vec<String>,
}

const NODE_COLS: &[Col] = &[
    ("NODE", "/node"),
    ("AGENT", "/agent"),
    ("ADDRESS", "/address"),
    ("MESH_IP", "/mesh_ip"),
    ("STATE", "/state"),
    ("NETWORK", "/network"),
    ("ERROR", "/error"),
];

/// Parses `<node>=<host>:<port>`.
fn parse_endpoint(spec: &str) -> Result<(String, String)> {
    let bad = |why: &str| CliError::usage(format!("--endpoint {spec}: {why}"));
    let (node, addr) = spec.split_once('=').ok_or_else(|| bad("write it as <node>=<host>:<port>"))?;
    let (host, port) = addr.rsplit_once(':').ok_or_else(|| bad("the endpoint needs a port"))?;

    if node.is_empty() || host.is_empty() {
        return Err(bad("name the node and the host"));
    }

    if port.parse::<u16>().ok().filter(|p| *p > 0).is_none() {
        return Err(bad("the port is a number from 1 to 65535"));
    }

    Ok((node.to_owned(), addr.to_owned()))
}

/// The flags of `set`, applied to the saved plan or a new one.
struct SetArgs {
    subnet: Option<Cidr>,
    domain: Option<String>,
    port: Option<u16>,
    endpoints: Option<BTreeMap<String, String>>,
}

/// The plan to save: `saved` (if any) with the flags laid over it; a new
/// plan takes the suggestion for what the flags leave out.
fn merged_spec(saved: Option<FabricSpec>, args: SetArgs, suggestion: Option<&Suggestion>) -> Result<FabricSpec> {
    let mut spec = match saved {
        Some(s) => s,
        None => {
            let s = suggestion.ok_or_else(|| CliError::usage("pass --subnet and --domain"))?;
            let subnet = s.subnet.parse::<Cidr>().map_err(CliError::usage)?;

            FabricSpec {
                subnet,
                mode: FabricMode::Mesh,
                domain: s.domain.clone(),
                port: DEFAULT_PORT,
                endpoints: BTreeMap::new(),
            }
        }
    };

    // The retired routed mode becomes the mesh when saved again.
    spec.mode = FabricMode::Mesh;

    if let Some(subnet) = args.subnet {
        spec.subnet = subnet;
    }

    if let Some(domain) = args.domain {
        spec.domain = domain;
    }

    if let Some(port) = args.port {
        spec.port = port;
    }

    if let Some(endpoints) = args.endpoints {
        spec.endpoints = endpoints;
    }

    spec.validate().map_err(CliError::usage)?;

    Ok(spec)
}

/// The human summary of `get`.
fn summary(cluster: &str, v: &FabricView) -> Value {
    let mut s = Map::new();
    s.insert("cluster".into(), json!(cluster));

    let Some(spec) = &v.spec else {
        s.insert("network".into(), json!("none (save one with `forge clusters network set`)"));

        return Value::Object(s);
    };

    s.insert("subnet".into(), json!(spec.subnet.to_string()));
    s.insert("domain".into(), json!(spec.domain));
    s.insert("port".into(), json!(spec.port));

    if !spec.endpoints.is_empty() {
        let names: HashMap<&str, &str> = v.members.iter().map(|m| (m.id.as_str(), m.name.as_str())).collect();
        let list: Vec<String> = spec
            .endpoints
            .iter()
            .map(|(id, ep)| format!("{}={ep}", names.get(id.as_str()).copied().unwrap_or(id)))
            .collect();
        s.insert("endpoints".into(), json!(list.join(", ")));
    }

    s.insert("applied".into(), json!(if v.applied() { "yes" } else { "no (run `forge clusters network apply`)" }));
    s.insert("nodes".into(), Value::Array(node_rows(v)));

    Value::Object(s)
}

fn node_rows(v: &FabricView) -> Vec<Value> {
    v.members
        .iter()
        .map(|m| {
            let st = v.node_status(&m.id);
            let error = st.error.or(st.network_error).unwrap_or_else(|| "-".into());

            json!({
                "node": m.name,
                "agent": m.status.clone().unwrap_or_else(|| "-".into()),
                "address": m.address.clone().unwrap_or_else(|| "-".into()),
                "mesh_ip": v.node_ip(&m.id).unwrap_or_else(|| "-".into()),
                "state": st.state.unwrap_or_else(|| "not applied".into()),
                "network": st.network.unwrap_or_else(|| "-".into()),
                "error": error,
            })
        })
        .collect()
}

async fn view(api: &mut Api, id: &str) -> Result<(Value, FabricView)> {
    let raw = api.get(&format!("/api/v1/clusters/{}/fabric", enc(id))).await?;
    let parsed = serde_json::from_value(raw.clone())
        .map_err(|e| CliError::new(Exit::General, format!("the server's cluster network answer: {e}")))?;

    Ok((raw, parsed))
}

async fn suggestion(api: &mut Api, id: &str, prefix: u8) -> Result<(Value, Suggestion)> {
    let raw = api.post(&format!("/api/v1/clusters/{}/fabric/suggest", enc(id)), &json!({ "prefix": prefix })).await?;
    let parsed = serde_json::from_value(raw.clone())
        .map_err(|e| CliError::new(Exit::General, format!("the server's suggestion: {e}")))?;

    Ok((raw, parsed))
}

/// Polls until every member reports `ok` or `failed`; a failed node makes
/// the command fail (exit 10), the deadline exit 8.
async fn wait_settled(g: &Global, api: &mut Api, cluster: &str, id: &str, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;

    loop {
        let (raw, v) = view(api, id).await?;
        let states: Vec<(String, NodeStatus)> =
            v.members.iter().map(|m| (m.name.clone(), v.node_status(&m.id))).collect();
        let settled = states.iter().all(|(_, s)| matches!(s.state.as_deref(), Some("ok" | "failed")));

        if settled {
            let failed: Vec<String> = states
                .iter()
                .filter(|(_, s)| s.state.as_deref() == Some("failed"))
                .map(|(n, s)| format!("{n}: {}", s.error.as_deref().unwrap_or("failed")))
                .collect();

            if g.output.is_machine() {
                out::result(g.output, &raw, None);
            } else {
                out::result(g.output, &Value::Array(node_rows(&v)), Some(NODE_COLS));
            }

            if failed.is_empty() {
                return Ok(());
            }

            return Err(CliError::new(
                Exit::JobFailed,
                format!("the network of cluster {cluster} failed on {}", failed.join("; ")),
            )
            .with_code("network_failed"));
        }

        if Instant::now() >= deadline {
            return Err(CliError::new(
                Exit::Timeout,
                format!(
                    "some nodes of cluster {cluster} have not settled; the server keeps trying (check with `forge clusters network get {cluster}`)"
                ),
            ));
        }

        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

pub async fn run(g: &Global, cmd: NetworkCmd) -> Result<()> {
    let t = target(g)?;
    let mut api = connect(g, &t).await?;

    match cmd {
        NetworkCmd::Get { cluster } => {
            let id = cluster_id(&mut api, &cluster).await?;
            let (raw, v) = view(&mut api, &id).await?;

            if g.output.is_machine() {
                out::result(g.output, &raw, None);
            } else {
                out::result(g.output, &summary(&cluster, &v), None);
            }

            Ok(())
        }
        NetworkCmd::Suggest { cluster, prefix } => {
            let id = cluster_id(&mut api, &cluster).await?;
            let (raw, _) = suggestion(&mut api, &id, prefix).await?;
            out::result(g.output, &raw, None);

            Ok(())
        }
        NetworkCmd::Set { cluster, subnet, domain, port, endpoints } => {
            let subnet = subnet.map(|s| s.parse::<Cidr>().map_err(CliError::usage)).transpose()?;
            let parsed: Vec<(String, String)> = endpoints.iter().map(|e| parse_endpoint(e)).collect::<Result<_>>()?;
            let id = cluster_id(&mut api, &cluster).await?;
            let (_, current) = view(&mut api, &id).await?;

            let mut resolved = BTreeMap::new();

            for (node, addr) in parsed {
                resolved.insert(node_id(&mut api, &node).await?, addr);
            }

            let endpoints = if endpoints.is_empty() { None } else { Some(resolved) };

            // A new network takes the suggestion for what the flags leave out.
            let suggested = match (&current.spec, &subnet, &domain) {
                (None, None, _) | (None, _, None) => Some(suggestion(&mut api, &id, 16).await?.1),
                _ => None,
            };

            let args = SetArgs { subnet, domain, port, endpoints };
            let spec = merged_spec(current.spec.clone(), args, suggested.as_ref())?;
            let body = FabricInput { spec: &spec, pins: current.pins.clone().unwrap_or_else(|| json!([])) };
            let body = serde_json::to_value(&body)
                .map_err(|e| CliError::new(Exit::General, format!("cannot encode the network: {e}")))?;
            let raw = api.call(Method::PUT, &format!("/api/v1/clusters/{}/fabric", enc(&id)), Some(&body)).await?;

            if !g.output.is_machine() {
                eprintln!(
                    "Saved {} ({}). Run `forge clusters network apply {cluster}` to bring the nodes to it.",
                    spec.subnet, spec.domain
                );

                // The answer is informative only; an unreadable one changes nothing.
                if let Ok(saved) = serde_json::from_value::<FabricSaved>(raw.clone())
                    && saved.restarts_containers
                {
                    eprintln!(
                        "The subnet moved: the container networks on {} are recreated on apply; redeploy their stacks afterwards.",
                        saved.redeploy_nodes.join(", ")
                    );
                }
            }

            out::result(g.output, &raw, None);

            Ok(())
        }
        NetworkCmd::Apply { cluster, wait, wait_timeout_secs, yes } => {
            let id = cluster_id(&mut api, &cluster).await?;
            let (_, current) = view(&mut api, &id).await?;

            let Some(spec) = &current.spec else {
                return Err(CliError::new(
                    Exit::NotFound,
                    format!("cluster {cluster} has no network yet; save one with `forge clusters network set`"),
                ));
            };

            confirm(
                g,
                yes,
                &format!(
                    "Apply the network {} of cluster {cluster} to its {} node(s) ({})? Host networking changes there.",
                    spec.subnet,
                    current.members.len(),
                    t.server
                ),
            )?;
            let raw = api.post(&format!("/api/v1/clusters/{}/fabric/apply", enc(&id)), &json!({})).await?;

            if wait {
                return wait_settled(g, &mut api, &cluster, &id, Duration::from_secs(wait_timeout_secs)).await;
            }

            if !g.output.is_machine() {
                eprintln!("Applying. Follow it with `forge clusters network get {cluster}`.");
            }

            out::result(g.output, &raw, None);

            Ok(())
        }
        NetworkCmd::Resync { cluster, node } => {
            let id = cluster_id(&mut api, &cluster).await?;
            let server = node_id(&mut api, &node).await?;
            let path = format!("/api/v1/clusters/{}/fabric/nodes/{}/resync", enc(&id), enc(&server));
            api.post(&path, &json!({})).await?;

            if g.output.is_machine() {
                out::result(g.output, &json!({ "cluster_id": id, "server_id": server, "resync": "started" }), None);
            } else {
                eprintln!("Retrying {node} now. Follow it with `forge clusters network get {cluster}`.");
            }

            Ok(())
        }
        NetworkCmd::Remove { cluster, yes } => {
            let id = cluster_id(&mut api, &cluster).await?;
            confirm(
                g,
                yes,
                &format!(
                    "Remove the network of cluster {cluster} ({})? Containers on it lose their cluster addresses.",
                    t.server
                ),
            )?;
            let raw = api.call(Method::DELETE, &format!("/api/v1/clusters/{}/fabric", enc(&id)), None).await?;
            out::result(g.output, &raw, None);

            Ok(())
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn args() -> SetArgs {
        SetArgs { subnet: None, domain: None, port: None, endpoints: None }
    }

    #[test]
    fn endpoints_parse_as_node_host_port() {
        assert_eq!(parse_endpoint("lab-b=192.0.2.10:51820").unwrap(), ("lab-b".into(), "192.0.2.10:51820".into()));
        assert_eq!(parse_endpoint("lab-c=edge.example:4000").unwrap().1, "edge.example:4000");
        assert!(parse_endpoint("lab-b").is_err());
        assert!(parse_endpoint("lab-b=192.0.2.10").is_err());
        assert!(parse_endpoint("lab-b=192.0.2.10:0").is_err());
        assert!(parse_endpoint("=192.0.2.10:1").is_err());
    }

    #[test]
    fn a_new_network_takes_the_suggestion_and_flags_win() {
        let s = Suggestion { subnet: "10.64.0.0/16".into(), domain: "lab.forge.internal".into() };
        let spec = merged_spec(None, args(), Some(&s)).unwrap();
        assert_eq!(
            (spec.subnet.to_string(), spec.domain.as_str(), spec.port),
            ("10.64.0.0/16".into(), "lab.forge.internal", 51820)
        );

        let mut a = args();
        a.subnet = Some("10.80.0.0/20".parse().unwrap());
        a.port = Some(51900);
        let spec = merged_spec(None, a, Some(&s)).unwrap();
        assert_eq!((spec.subnet.to_string(), spec.port), ("10.80.0.0/20".into(), 51900));

        assert!(merged_spec(None, args(), None).is_err(), "nothing to start from");
    }

    #[test]
    fn a_saved_network_keeps_what_the_flags_leave_out() {
        let saved = FabricSpec {
            subnet: "10.64.0.0/16".parse().unwrap(),
            mode: FabricMode::Mesh,
            domain: "lab.forge.internal".into(),
            port: 51821,
            endpoints: BTreeMap::from([("n1".into(), "192.0.2.1:51821".into())]),
        };
        let mut a = args();
        a.domain = Some("prod.forge.internal".into());
        let spec = merged_spec(Some(saved.clone()), a, None).unwrap();
        assert_eq!((spec.domain.as_str(), spec.port, spec.endpoints.len()), ("prod.forge.internal", 51821, 1));

        let mut a = args();
        a.subnet = Some("8.8.0.0/16".parse().unwrap());
        assert!(merged_spec(Some(saved), a, None).is_err(), "a public subnet is refused before sending");
    }
}
