//! `forge swarm`: everything Docker Swarm in one place (D-154, D-171):
//! membership and nodes, stacks, secrets and configs, networks, volumes,
//! services and converting a cluster. Each command calls the same API as the
//! console; the server authorizes, asks for MFA and audits.

use std::collections::BTreeMap;
use std::time::Duration;

use clap::{Args, Subcommand, ValueEnum};
use reqwest::Method;
use serde::Serialize;
use serde_json::{Value, json};

use crate::Global;
use crate::admincmd::{cluster_id, enc, node_id};
use crate::api::Api;
use crate::cmds::{confirm, wait_jobs};
use crate::containercmd::{ClusterSwarmCmd, Role, ServiceSwarmCmd, WaitFlags};
use crate::error::{CliError, Result};
use crate::out::{self, Col};
use crate::rescmd::{read_text, resolve_stack};
use crate::{connect, target};

fn show(g: &Global, v: &Value, cols: Option<&[Col]>) -> Result<()> {
    out::result(g.output, v, cols);
    Ok(())
}

async fn job_or_wait(g: &Global, api: &mut Api, v: Value, w: WaitFlags) -> Result<()> {
    match v["id"].as_str() {
        Some(id) if w.wait => {
            let jobs = wait_jobs(api, &[id.to_owned()], Duration::from_secs(w.timeout)).await?;
            show(g, &json!(jobs), None)
        }
        _ => show(g, &v, Some(&[("JOB", "/id"), ("KIND", "/kind"), ("STATUS", "/status")])),
    }
}

#[derive(Subcommand, Debug)]
pub enum SwarmCmd {
    /// Members, live nodes and manager quorum.
    Status { cluster: String },
    /// Create the swarm: the node becomes its first manager.
    Init {
        cluster: String,
        node: String,
        /// Address other nodes reach this one on (default: the mesh address
        /// when the node has one; see `forge swarm addresses`).
        #[arg(long)]
        advertise_addr: Option<String>,
        /// Overlay address pool (CIDR); repeat for more.
        #[arg(long = "addr-pool", value_name = "CIDR")]
        addr_pools: Vec<String>,
        #[arg(long)]
        autolock: bool,
        #[command(flatten)]
        wait: WaitFlags,
    },
    /// Add a node to the swarm.
    Join {
        cluster: String,
        node: String,
        #[arg(long, value_enum, default_value = "worker")]
        role: Role,
        #[arg(long)]
        advertise_addr: Option<String>,
        #[command(flatten)]
        wait: WaitFlags,
    },
    /// Remove a node from the swarm.
    Leave {
        cluster: String,
        node: String,
        #[arg(long)]
        force: bool,
        #[arg(long)]
        yes: bool,
        #[command(flatten)]
        wait: WaitFlags,
    },
    /// The swarm's nodes as a manager sees them.
    Nodes { cluster: String },
    /// Make a worker a manager.
    Promote { cluster: String, node: String },
    /// Make a manager a worker.
    Demote { cluster: String, node: String },
    /// Move the node's tasks away and schedule nothing new on it.
    Drain { cluster: String, node: String },
    /// Schedule tasks on the node again.
    Activate { cluster: String, node: String },
    /// Keep the node's tasks, schedule nothing new on it.
    Pause { cluster: String, node: String },
    /// Replace a node's labels (placement rules use them).
    Labels {
        cluster: String,
        node: String,
        /// `KEY=VALUE`; repeat.
        #[arg(long = "label", value_name = "KEY=VALUE")]
        labels: Vec<String>,
        /// Remove every label.
        #[arg(long, conflicts_with = "labels")]
        clear: bool,
    },
    /// Rotate the join tokens.
    Tokens {
        cluster: String,
        #[arg(long)]
        worker: bool,
        #[arg(long)]
        manager: bool,
        #[arg(long)]
        yes: bool,
    },
    /// Addresses a node could advertise, the mesh address first.
    Addresses { cluster: String, node: String },
    /// Switch a cluster between Default and Swarm mode (checked first).
    Convert {
        cluster: String,
        #[arg(long, value_enum)]
        to: Mode,
        /// Only run the checks.
        #[arg(long)]
        check: bool,
        #[arg(long)]
        yes: bool,
    },
    /// Compose stacks deployed to the swarm.
    #[command(subcommand)]
    Stacks(StackCmd),
    /// Swarm secrets (values never leave Forge's request).
    #[command(subcommand)]
    Secrets(ObjectCmd),
    /// Swarm configs.
    #[command(subcommand)]
    Configs(ObjectCmd),
    /// Overlay networks, the routing mesh and the shared network.
    #[command(subcommand)]
    Networks(NetworkCmd),
    /// Docker volumes on every node of the swarm.
    Volumes { cluster: String },
    /// CPU and memory of a stack's running tasks, sampled now.
    Stats {
        #[command(flatten)]
        stack: StackRef,
    },
    /// A Forge service running as a Swarm service.
    #[command(subcommand)]
    Services(ServiceSwarmCmd),
}

#[derive(ValueEnum, Serialize, Debug, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Default,
    Swarm,
}

#[derive(Args, Debug, Clone)]
pub struct StackRef {
    /// Stack id or name.
    pub stack: String,
    /// The stack's environment, when the name is ambiguous.
    #[arg(long)]
    pub environment: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum StackCmd {
    /// Stacks in the cluster's environments, and the revision deployed.
    Ls { cluster: String },
    /// Services, replicas, ports and recent tasks of a stack.
    Ps {
        #[command(flatten)]
        stack: StackRef,
        /// Every task, not only the newest per service.
        #[arg(long)]
        all: bool,
    },
    /// Deploy a revision (default: the latest; an older one rolls back).
    Deploy {
        #[command(flatten)]
        stack: StackRef,
        #[arg(long)]
        revision: Option<i64>,
        #[arg(long)]
        yes: bool,
        #[command(flatten)]
        wait: WaitFlags,
    },
    /// Remove the stack from the swarm (volumes stay).
    Rm {
        #[command(flatten)]
        stack: StackRef,
        #[arg(long)]
        yes: bool,
        #[command(flatten)]
        wait: WaitFlags,
    },
    /// Line differences between two revisions (default: deployed → latest).
    Diff {
        #[command(flatten)]
        stack: StackRef,
        #[arg(long)]
        from: Option<i64>,
        #[arg(long)]
        to: Option<i64>,
    },
    /// Set a stack service's replica count.
    Scale {
        #[command(flatten)]
        stack: StackRef,
        service: String,
        replicas: u32,
    },
    /// Replace every task of a stack service.
    Restart {
        #[command(flatten)]
        stack: StackRef,
        service: String,
    },
    /// Return a stack service to its previous spec.
    Rollback {
        #[command(flatten)]
        stack: StackRef,
        service: String,
        #[arg(long)]
        yes: bool,
    },
    /// The last lines of a stack service's logs.
    Logs {
        #[command(flatten)]
        stack: StackRef,
        service: String,
        #[arg(long, default_value_t = 200)]
        tail: u32,
    },
}

#[derive(Subcommand, Debug)]
pub enum ObjectCmd {
    /// Every version, the services using it.
    Ls { cluster: String },
    /// Create the first version.
    Create {
        cluster: String,
        name: String,
        /// File with the value (`-`: standard input). Never pass a secret as an argument.
        #[arg(long, value_name = "PATH", conflicts_with = "from_vault")]
        value_file: Option<String>,
        /// A credential vault item id (administrators).
        #[arg(long, value_name = "ITEM")]
        from_vault: Option<String>,
    },
    /// Create the next version; redeploy the stacks that use it.
    Rotate {
        cluster: String,
        name: String,
        #[arg(long, value_name = "PATH", conflicts_with = "from_vault")]
        value_file: Option<String>,
        #[arg(long, value_name = "ITEM")]
        from_vault: Option<String>,
    },
    /// Remove one version (`<name>.v<N>`) no service uses.
    Rm {
        cluster: String,
        docker_name: String,
        #[arg(long)]
        yes: bool,
    },
    /// Remove old versions no service uses.
    Prune {
        cluster: String,
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum NetworkCmd {
    /// Swarm networks: subnets, encryption, attached services.
    Ls { cluster: String },
    /// Create the shared network `forge-shared` if it is missing.
    Shared { cluster: String },
}

const STACK_COLS: &[Col] = &[
    ("STACK", "/name"),
    ("ENVIRONMENT", "/environment"),
    ("REVISION", "/revision"),
    ("DEPLOYED", "/deployed_revision"),
    ("ID", "/id"),
];
const NODE_COLS: &[Col] = &[
    ("HOSTNAME", "/hostname"),
    ("ROLE", "/role"),
    ("AVAILABILITY", "/availability"),
    ("STATE", "/state"),
    ("MANAGER", "/manager_status"),
    ("LEADER", "/leader"),
    ("ADDRESS", "/addr"),
    ("ENGINE", "/engine_version"),
];
const OBJECT_COLS: &[Col] = &[
    ("NAME", "/name"),
    ("VERSION", "/version"),
    ("DOCKER NAME", "/docker_name"),
    ("USED BY", "/used_by"),
    ("MANAGED", "/managed"),
    ("CREATED", "/created_at"),
];
const NETWORK_COLS: &[Col] = &[
    ("NAME", "/name"),
    ("DRIVER", "/driver"),
    ("SUBNETS", "/subnets"),
    ("ENCRYPTED", "/encrypted"),
    ("ATTACHABLE", "/attachable"),
    ("INGRESS", "/ingress"),
    ("SERVICES", "/services"),
];

pub async fn run(g: &Global, cmd: SwarmCmd) -> Result<()> {
    // Membership commands are the `forge clusters swarm` ones.
    let member = match cmd {
        SwarmCmd::Status { cluster } => Some(ClusterSwarmCmd::Status { cluster }),
        SwarmCmd::Init { cluster, node, advertise_addr, addr_pools, autolock, wait } => Some(ClusterSwarmCmd::Join {
            cluster,
            node,
            role: Role::Manager,
            advertise_addr,
            addr_pools,
            autolock,
            wait,
        }),
        SwarmCmd::Join { cluster, node, role, advertise_addr, wait } => Some(ClusterSwarmCmd::Join {
            cluster,
            node,
            role,
            advertise_addr,
            addr_pools: Vec::new(),
            autolock: false,
            wait,
        }),
        SwarmCmd::Leave { cluster, node, force, yes, wait } => {
            Some(ClusterSwarmCmd::Leave { cluster, node, force, yes, wait })
        }
        SwarmCmd::Tokens { cluster, worker, manager, yes } => {
            Some(ClusterSwarmCmd::RotateTokens { cluster, worker, manager, yes })
        }
        SwarmCmd::Services(s) => return crate::containercmd::service_swarm(g, s).await,
        other => return run_rest(g, other).await,
    };

    match member {
        Some(m) => crate::containercmd::cluster_swarm(g, m).await,
        None => Ok(()),
    }
}

async fn node_change(api: &mut Api, cluster: &str, node: &str, body: Value) -> Result<Value> {
    let id = cluster_id(api, cluster).await?;
    let server = node_id(api, node).await?;

    api.call(Method::PATCH, &format!("/api/v1/clusters/{}/swarm/members/{}", enc(&id), enc(&server)), Some(&body)).await
}

async fn run_rest(g: &Global, cmd: SwarmCmd) -> Result<()> {
    let t = target(g)?;
    let mut api = connect(g, &t).await?;
    let api = &mut api;

    match cmd {
        SwarmCmd::Nodes { cluster } => {
            let id = cluster_id(api, &cluster).await?;
            let v = api.get(&format!("/api/v1/clusters/{}/swarm", enc(&id))).await?;
            if let Some(e) = v["nodes_error"].as_str() {
                return Err(CliError::new(crate::error::Exit::General, e.to_owned()));
            }

            show(g, &v["nodes"], Some(NODE_COLS))
        }
        SwarmCmd::Promote { cluster, node } => {
            show(g, &node_change(api, &cluster, &node, json!({"role": "manager"})).await?, Some(NODE_COLS))
        }
        SwarmCmd::Demote { cluster, node } => {
            show(g, &node_change(api, &cluster, &node, json!({"role": "worker"})).await?, Some(NODE_COLS))
        }
        SwarmCmd::Drain { cluster, node } => {
            show(g, &node_change(api, &cluster, &node, json!({"availability": "drain"})).await?, Some(NODE_COLS))
        }
        SwarmCmd::Activate { cluster, node } => {
            show(g, &node_change(api, &cluster, &node, json!({"availability": "active"})).await?, Some(NODE_COLS))
        }
        SwarmCmd::Pause { cluster, node } => {
            show(g, &node_change(api, &cluster, &node, json!({"availability": "pause"})).await?, Some(NODE_COLS))
        }
        SwarmCmd::Labels { cluster, node, labels, clear } => {
            if labels.is_empty() && !clear {
                return Err(CliError::usage("give --label KEY=VALUE (repeat) or --clear"));
            }

            let map = pairs(&labels)?;
            show(g, &node_change(api, &cluster, &node, json!({"labels": map})).await?, Some(NODE_COLS))
        }
        SwarmCmd::Addresses { cluster, node } => {
            let id = cluster_id(api, &cluster).await?;
            let server = node_id(api, &node).await?;
            let v =
                api.get(&format!("/api/v1/clusters/{}/swarm/addresses?server_id={}", enc(&id), enc(&server))).await?;
            show(g, &v, Some(&[("ADDRESS", "/address"), ("SOURCE", "/source"), ("INTERFACE", "/interface")]))
        }
        SwarmCmd::Convert { cluster, to, check, yes } => {
            let id = cluster_id(api, &cluster).await?;
            let to_s = serde_json::to_value(to).map_err(|e| CliError::usage(e.to_string()))?;
            let path = format!("/api/v1/clusters/{}/convert", enc(&id));
            let cols: &[Col] = &[("CHECK", "/name"), ("OK", "/ok"), ("DETAIL", "/detail")];

            if check {
                let v = api.get(&format!("{path}?to={}", to_s.as_str().unwrap_or_default())).await?;
                return show(g, &v["checks"], Some(cols));
            }

            confirm(g, yes, &format!("Switch cluster {cluster} to {} mode?", to_s.as_str().unwrap_or_default()))?;
            let v = api.post(&path, &json!({ "to": to_s })).await?;
            show(g, &v["checks"], Some(cols))
        }
        SwarmCmd::Stacks(s) => stacks(g, api, s).await,
        SwarmCmd::Secrets(o) => objects(g, api, "secret", o).await,
        SwarmCmd::Configs(o) => objects(g, api, "config", o).await,
        SwarmCmd::Networks(NetworkCmd::Ls { cluster }) => {
            let id = cluster_id(api, &cluster).await?;
            show(g, &api.get(&format!("/api/v1/clusters/{}/swarm/networks", enc(&id))).await?, Some(NETWORK_COLS))
        }
        SwarmCmd::Networks(NetworkCmd::Shared { cluster }) => {
            let id = cluster_id(api, &cluster).await?;
            api.post(&format!("/api/v1/clusters/{}/swarm/networks/shared", enc(&id)), &json!({})).await?;
            show(g, &json!({ "network": "forge-shared", "ready": true }), None)
        }
        SwarmCmd::Volumes { cluster } => volumes(g, api, &cluster).await,
        SwarmCmd::Stats { stack } => stats(g, api, &stack).await,
        _ => Ok(()),
    }
}

fn pairs(items: &[String]) -> Result<BTreeMap<String, String>> {
    items
        .iter()
        .map(|s| {
            s.split_once('=')
                .filter(|(k, _)| !k.trim().is_empty())
                .map(|(k, v)| (k.trim().to_owned(), v.to_owned()))
                .ok_or_else(|| CliError::usage(format!("--label: `{s}` is not KEY=VALUE")))
        })
        .collect()
}

async fn stack_id(api: &mut Api, r: &StackRef) -> Result<(String, Value)> {
    let s = resolve_stack(api, &r.stack, r.environment.as_deref()).await?;
    Ok((s["stack"]["id"].as_str().unwrap_or_default().to_owned(), s["stack"].clone()))
}

async fn stacks(g: &Global, api: &mut Api, cmd: StackCmd) -> Result<()> {
    match cmd {
        StackCmd::Ls { cluster } => {
            let id = cluster_id(api, &cluster).await?;
            show(g, &api.get(&format!("/api/v1/clusters/{}/swarm/stacks", enc(&id))).await?, Some(STACK_COLS))
        }
        StackCmd::Ps { stack, all } => {
            let (id, _) = stack_id(api, &stack).await?;
            let v = api.get(&format!("/api/v1/stacks/{id}/swarm")).await?;

            if let Some(e) = v["status_error"].as_str() {
                return Err(CliError::new(crate::error::Exit::General, e.to_owned()));
            }

            show(g, &json!(ps_rows(&v["status"], all)), Some(PS_COLS))
        }
        StackCmd::Deploy { stack, revision, yes, wait } => {
            let (id, s) = stack_id(api, &stack).await?;
            let rev = revision.unwrap_or_else(|| s["revision"].as_i64().unwrap_or(1));
            confirm(g, yes, &format!("Deploy revision {rev} of stack {} to the swarm?", s["name"]))?;
            let v = api.post(&format!("/api/v1/stacks/{id}/swarm/deploy"), &json!({ "revision": revision })).await?;
            job_or_wait(g, api, v, wait).await
        }
        StackCmd::Rm { stack, yes, wait } => {
            let (id, s) = stack_id(api, &stack).await?;
            confirm(g, yes, &format!("Remove stack {} from the swarm? Its volumes stay.", s["name"]))?;
            let v = api.post(&format!("/api/v1/stacks/{id}/swarm/remove"), &json!({})).await?;
            job_or_wait(g, api, v, wait).await
        }
        StackCmd::Diff { stack, from, to } => {
            let (id, s) = stack_id(api, &stack).await?;
            let to = to.unwrap_or_else(|| s["revision"].as_i64().unwrap_or(1));
            let from = from.unwrap_or_else(|| s["deployed_revision"].as_i64().unwrap_or(0));
            let text = |v: &Value| v["content"].as_str().unwrap_or_default().to_owned();

            let a = if from > 0 {
                text(&api.get(&format!("/api/v1/stacks/{id}/revisions/{from}")).await?)
            } else {
                String::new()
            };

            let b = text(&api.get(&format!("/api/v1/stacks/{id}/revisions/{to}")).await?);
            let d = diff_lines(&a, &b);

            match g.output {
                out::Format::Table => {
                    println!("--- revision {from}\n+++ revision {to}");
                    d.iter().for_each(|l| println!("{l}"));
                    Ok(())
                }
                _ => show(g, &json!({ "from": from, "to": to, "lines": d }), None),
            }
        }
        StackCmd::Scale { stack, service, replicas } => {
            stack_service(g, api, &stack, &service, json!({ "kind": "scale", "replicas": replicas })).await
        }
        StackCmd::Restart { stack, service } => {
            stack_service(g, api, &stack, &service, json!({ "kind": "restart" })).await
        }
        StackCmd::Rollback { stack, service, yes } => {
            confirm(g, yes, &format!("Roll {service} back to its previous spec?"))?;
            stack_service(g, api, &stack, &service, json!({ "kind": "rollback" })).await
        }
        StackCmd::Logs { stack, service, tail } => {
            let (id, _) = stack_id(api, &stack).await?;
            let v = api.get(&format!("/api/v1/stacks/{id}/swarm/services/{}/logs?tail={tail}", enc(&service))).await?;
            match (g.output, v["logs"].as_str()) {
                (out::Format::Table, Some(text)) => {
                    print!("{text}");
                    Ok(())
                }
                _ => show(g, &v, None),
            }
        }
    }
}

async fn stack_service(g: &Global, api: &mut Api, stack: &StackRef, service: &str, body: Value) -> Result<()> {
    let (id, _) = stack_id(api, stack).await?;
    api.post(&format!("/api/v1/stacks/{id}/swarm/services/{}", enc(service)), &body).await?;

    show(g, &json!({ "service": service, "done": body }), None)
}

const PS_COLS: &[Col] = &[
    ("SERVICE", "/service"),
    ("TASK", "/task"),
    ("NODE", "/node"),
    ("STATE", "/state"),
    ("DESIRED", "/desired"),
    ("REPLICAS", "/replicas"),
    ("PORTS", "/ports"),
    ("ERROR", "/error"),
];

/// One row per task (newest per replica unless `all`), with the service's
/// replica count and ports on each.
fn ps_rows(status: &Value, all: bool) -> Vec<Value> {
    let mut rows = Vec::new();

    for s in status["services"].as_array().into_iter().flatten() {
        let replicas = format!("{}/{}", s["running"], s["desired"]);
        let ports: Vec<String> = s["ports"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|p| format!("{}:{}/{}", p["published"], p["target"], p["protocol"].as_str().unwrap_or("tcp")))
            .collect();
        let mut seen = std::collections::BTreeSet::new();

        for t in s["tasks"].as_array().into_iter().flatten() {
            // Tasks come newest first: the first one per slot is current.
            let slot = format!("{}{}", t["slot"], t["node_id"].as_str().filter(|_| t["slot"] == 0).unwrap_or(""));
            if !all && !seen.insert(slot) {
                continue;
            }

            rows.push(json!({
                "service": s["name"], "task": t["id"], "node": t["node_id"], "state": t["state"],
                "desired": t["desired_state"], "replicas": replicas, "ports": ports.join(","), "error": t["error"],
            }));
        }

        if s["tasks"].as_array().is_none_or(Vec::is_empty) {
            rows.push(json!({ "service": s["name"], "replicas": replicas, "ports": ports.join(",") }));
        }
    }

    rows
}

/// A plain line diff (longest common subsequence).
fn diff_lines(a: &str, b: &str) -> Vec<String> {
    let (a, b): (Vec<&str>, Vec<&str>) = (a.lines().collect(), b.lines().collect());
    let mut lcs = vec![vec![0usize; b.len() + 1]; a.len() + 1];

    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            lcs[i][j] = if a[i] == b[j] { lcs[i + 1][j + 1] + 1 } else { lcs[i + 1][j].max(lcs[i][j + 1]) };
        }
    }

    let (mut i, mut j, mut out) = (0, 0, Vec::new());

    while i < a.len() || j < b.len() {
        if i < a.len() && j < b.len() && a[i] == b[j] {
            out.push(format!(" {}", a[i]));
            i += 1;
            j += 1;
        } else if j < b.len() && (i == a.len() || lcs[i][j + 1] >= lcs[i + 1][j]) {
            out.push(format!("+{}", b[j]));
            j += 1;
        } else {
            out.push(format!("-{}", a[i]));
            i += 1;
        }
    }

    out
}

fn object_value(value_file: Option<&str>, vault: Option<&str>) -> Result<Value> {
    match (value_file, vault) {
        (Some(f), None) => Ok(json!({ "value": read_text(f)? })),
        (None, Some(v)) => Ok(json!({ "vault_item_id": v })),
        _ => Err(CliError::usage("give --value-file PATH (or -) or --from-vault ITEM")),
    }
}

async fn objects(g: &Global, api: &mut Api, kind: &str, cmd: ObjectCmd) -> Result<()> {
    match cmd {
        ObjectCmd::Ls { cluster } => {
            let id = cluster_id(api, &cluster).await?;
            show(
                g,
                &api.get(&format!("/api/v1/clusters/{}/swarm/objects?kind={kind}", enc(&id))).await?,
                Some(OBJECT_COLS),
            )
        }
        ObjectCmd::Create { cluster, name, value_file, from_vault }
        | ObjectCmd::Rotate { cluster, name, value_file, from_vault } => {
            let id = cluster_id(api, &cluster).await?;
            let mut body = object_value(value_file.as_deref(), from_vault.as_deref())?;
            body["kind"] = json!(kind);
            body["name"] = json!(name);
            show(g, &api.post(&format!("/api/v1/clusters/{}/swarm/objects", enc(&id)), &body).await?, None)
        }
        ObjectCmd::Rm { cluster, docker_name, yes } => {
            let id = cluster_id(api, &cluster).await?;
            confirm(g, yes, &format!("Remove {kind} {docker_name}?"))?;
            let path = format!("/api/v1/clusters/{}/swarm/objects/{kind}/{}", enc(&id), enc(&docker_name));
            api.call(Method::DELETE, &path, None).await?;
            show(g, &json!({ "removed": docker_name }), None)
        }
        ObjectCmd::Prune { cluster, yes } => {
            let id = cluster_id(api, &cluster).await?;
            confirm(g, yes, &format!("Remove every old {kind} version no service uses?"))?;
            show(
                g,
                &api.post(&format!("/api/v1/clusters/{}/swarm/objects/prune", enc(&id)), &json!({ "kind": kind }))
                    .await?,
                None,
            )
        }
    }
}

/// Live CPU and memory of a stack's running tasks.
async fn stats(g: &Global, api: &mut Api, stack: &StackRef) -> Result<()> {
    let (id, _) = stack_id(api, stack).await?;
    let v = api.get(&format!("/api/v1/stacks/{id}/swarm")).await?;
    let cluster = v["cluster_id"].as_str().unwrap_or_default().to_owned();
    let mut nodes: Vec<String> = Vec::new();
    let mut names = BTreeMap::new();

    for s in v["status"]["services"].as_array().into_iter().flatten() {
        for t in s["tasks"].as_array().into_iter().flatten().filter(|t| t["state"] == "running") {
            let node = t["node_id"].as_str().unwrap_or_default().to_owned();
            if !node.is_empty() && !nodes.contains(&node) {
                nodes.push(node);
            }

            names.insert(t["id"].as_str().unwrap_or_default().to_owned(), s["name"].clone());
        }
    }

    if nodes.is_empty() {
        return show(g, &json!([]), None);
    }

    let path = format!("/api/v1/clusters/{}/swarm/task-stats?nodes={}", enc(&cluster), enc(&nodes.join(",")));
    let all = api.get(&path).await?;
    let rows: Vec<Value> = all
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| {
            let name = names.get(t["task_id"].as_str().unwrap_or_default())?;
            let mib = t["memory_bytes"].as_u64().unwrap_or(0) / (1024 * 1024);
            Some(json!({
                "service": name, "task": t["task_id"],
                "cpu": format!("{:.1}%", t["cpu_percent"].as_f64().unwrap_or(0.0)), "memory": format!("{mib} MiB"),
            }))
        })
        .collect();

    show(g, &json!(rows), Some(&[("SERVICE", "/service"), ("TASK", "/task"), ("CPU", "/cpu"), ("MEMORY", "/memory")]))
}

/// Volumes of every swarm member, with the node they are on.
async fn volumes(g: &Global, api: &mut Api, cluster: &str) -> Result<()> {
    let id = cluster_id(api, cluster).await?;
    let view = api.get(&format!("/api/v1/clusters/{}/swarm", enc(&id))).await?;
    let mut rows = Vec::new();

    for m in view["members"].as_array().into_iter().flatten() {
        let server = m["server_id"].as_str().unwrap_or_default().to_owned();
        let host = m["hostname"].clone();

        match api.get(&format!("/api/v1/servers/{}/docker/volumes", enc(&server))).await {
            Ok(v) => {
                let list = v["volumes"].as_array().or_else(|| v.as_array()).cloned().unwrap_or_default();

                for vol in list {
                    rows.push(json!({
                        "node": host, "name": vol["name"], "driver": vol["driver"],
                        "used_by": vol["used_by"], "size": vol["size"],
                    }));
                }
            }
            Err(e) => rows.push(json!({ "node": host, "name": format!("(error: {e})") })),
        }
    }

    show(
        g,
        &json!(rows),
        Some(&[("NODE", "/node"), ("VOLUME", "/name"), ("DRIVER", "/driver"), ("USED BY", "/used_by")]),
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn diff_marks_added_and_removed_lines() {
        let d = diff_lines("a\nb\nc\n", "a\nc\nd\n");
        assert_eq!(d, vec![" a", "-b", " c", "+d"]);
        assert_eq!(diff_lines("", "x"), vec!["+x"]);
    }

    #[test]
    fn ps_keeps_the_newest_task_per_replica() {
        let st = json!({"services": [{"name": "api", "running": 1, "desired": 1,
            "ports": [{"published": 8080, "target": 80, "protocol": "tcp"}],
            "tasks": [{"id": "t2", "slot": 1, "state": "running"}, {"id": "t1", "slot": 1, "state": "shutdown"}]}]});
        let rows = ps_rows(&st, false);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["task"], "t2");
        assert_eq!(rows[0]["ports"], "8080:80/tcp");
        assert_eq!(ps_rows(&st, true).len(), 2);
    }
}
