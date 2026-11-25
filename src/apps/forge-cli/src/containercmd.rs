//! Container platform commands (D-154 … D-160): Docker Swarm clusters and
//! services, a node's Docker images, volumes and networks, registries, deploy
//! webhooks and compose templates. Each command calls the same API as the
//! console; the server authorizes, asks for MFA and audits.

use std::collections::BTreeMap;
use std::time::Duration;

use clap::{Args, Subcommand, ValueEnum};
use forge_catalog::docker_ops::{DockerOp, NetworkDriver};
use reqwest::Method;
use serde::Serialize;
use serde_json::{Value, json};

use crate::Global;
use crate::admincmd::{cluster_id, enc, node_id};
use crate::api::Api;
use crate::cmds::{confirm, wait_jobs};
use crate::error::{CliError, Result};
use crate::out::{self, Col};
use crate::{connect, target};

fn show(g: &Global, v: &Value, cols: Option<&[Col]>) -> Result<()> {
    out::result(g.output, v, cols);
    Ok(())
}

/// How long `--wait` waits for a job before giving up (the job keeps running).
#[derive(Args, Debug, Clone, Copy)]
pub struct WaitFlags {
    /// Wait for the job to finish (exit 10 when it fails).
    #[arg(long)]
    pub wait: bool,
    /// Seconds `--wait` waits.
    #[arg(long, default_value_t = 900, value_name = "SECONDS")]
    pub timeout: u64,
}

/// Prints `v`, or waits for the job it names and prints the finished job.
async fn job_or_wait(g: &Global, api: &mut Api, v: Value, w: WaitFlags) -> Result<()> {
    let id = v["id"].as_str().filter(|_| v.get("kind").is_some() && v.get("status").is_some());
    match id {
        Some(id) if w.wait => {
            let jobs = wait_jobs(api, &[id.to_owned()], Duration::from_secs(w.timeout)).await?;
            show(g, &json!(jobs), None)
        }
        _ => show(g, &v, None),
    }
}

/// `key=value` pairs from repeated flags.
fn pairs(items: &[String], what: &str) -> Result<BTreeMap<String, String>> {
    items
        .iter()
        .map(|s| {
            s.split_once('=')
                .filter(|(k, _)| !k.trim().is_empty())
                .map(|(k, v)| (k.trim().to_owned(), v.to_owned()))
                .ok_or_else(|| CliError::usage(format!("{what}: `{s}` is not KEY=VALUE")))
        })
        .collect()
}

// ------------------------------------------------------------------ cluster swarm

#[derive(ValueEnum, Serialize, Debug, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Manager,
    Worker,
}

#[derive(ValueEnum, Serialize, Debug, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum Availability {
    /// Takes new tasks.
    Active,
    /// Keeps its tasks, takes no new ones.
    Pause,
    /// Moves its tasks to other nodes.
    Drain,
}

#[derive(Subcommand, Debug)]
pub enum ClusterSwarmCmd {
    /// Members, live Swarm nodes and manager quorum of a Swarm cluster.
    Status { cluster: String },
    /// Add a node to the cluster's swarm; the first node creates it.
    Join {
        cluster: String,
        /// Node id, host name or name.
        node: String,
        /// The first node is always a manager. Keep 1, 3 or 5 managers.
        #[arg(long, value_enum, default_value = "worker")]
        role: Role,
        /// Address or interface other nodes reach this one on.
        #[arg(long)]
        advertise_addr: Option<String>,
        /// Overlay address pool (CIDR), first node only; repeat for more.
        #[arg(long = "addr-pool", value_name = "CIDR")]
        addr_pools: Vec<String>,
        /// Lock manager keys at rest, first node only.
        #[arg(long)]
        autolock: bool,
        #[command(flatten)]
        wait: WaitFlags,
    },
    /// Remove a node from the swarm.
    Leave {
        cluster: String,
        node: String,
        /// Leave even when the node is unreachable or the last manager.
        #[arg(long)]
        force: bool,
        #[arg(long)]
        yes: bool,
        #[command(flatten)]
        wait: WaitFlags,
    },
    /// Change a member's role, availability or labels.
    Node {
        cluster: String,
        node: String,
        #[arg(long, value_enum)]
        role: Option<Role>,
        #[arg(long, value_enum)]
        availability: Option<Availability>,
        /// Replace the node's labels with these (`KEY=VALUE`, repeat).
        #[arg(long = "label", value_name = "KEY=VALUE")]
        labels: Vec<String>,
        /// Remove every label.
        #[arg(long, conflicts_with = "labels")]
        clear_labels: bool,
    },
    /// Rotate the swarm's join tokens.
    RotateTokens {
        cluster: String,
        #[arg(long)]
        worker: bool,
        #[arg(long)]
        manager: bool,
        #[arg(long)]
        yes: bool,
    },
}

pub async fn cluster_swarm(g: &Global, cmd: ClusterSwarmCmd) -> Result<()> {
    let t = target(g)?;
    let mut api = connect(g, &t).await?;
    let base = |id: &str| format!("/api/v1/clusters/{}/swarm", enc(id));

    match cmd {
        ClusterSwarmCmd::Status { cluster } => {
            let id = cluster_id(&mut api, &cluster).await?;
            let v = api.get(&base(&id)).await?;
            show(g, &v, None)
        }
        ClusterSwarmCmd::Join { cluster, node, role, advertise_addr, addr_pools, autolock, wait } => {
            let id = cluster_id(&mut api, &cluster).await?;
            let server = node_id(&mut api, &node).await?;
            let body = json!({
                "server_id": server, "role": role, "advertise_addr": advertise_addr,
                "default_addr_pool": addr_pools, "autolock": autolock,
            });
            let v = api.post(&format!("{}/members", base(&id)), &body).await?;

            // The join job, plus Docker installs it waits for.
            let mut jobs = vec![v["job"]["id"].as_str().unwrap_or_default().to_owned()];
            jobs.retain(|j| !j.is_empty());

            if wait.wait && !jobs.is_empty() {
                let done = wait_jobs(&mut api, &jobs, Duration::from_secs(wait.timeout)).await?;
                return show(g, &json!(done), None);
            }

            show(g, &v, None)
        }
        ClusterSwarmCmd::Leave { cluster, node, force, yes, wait } => {
            let id = cluster_id(&mut api, &cluster).await?;
            let server = node_id(&mut api, &node).await?;
            confirm(g, yes, &format!("Remove {node} from the swarm of {cluster}?"))?;
            let path = format!("{}/members/{}?force={force}", base(&id), enc(&server));
            let v = api.call(Method::DELETE, &path, None).await?;
            job_or_wait(g, &mut api, v, wait).await
        }
        ClusterSwarmCmd::Node { cluster, node, role, availability, labels, clear_labels } => {
            let id = cluster_id(&mut api, &cluster).await?;
            let server = node_id(&mut api, &node).await?;

            let labels = if clear_labels {
                Some(BTreeMap::new())
            } else if labels.is_empty() {
                None
            } else {
                Some(pairs(&labels, "--label")?)
            };

            if role.is_none() && availability.is_none() && labels.is_none() {
                return Err(CliError::usage(
                    "nothing to change: give --role, --availability, --label or --clear-labels",
                ));
            }

            let body = json!({ "role": role, "availability": availability, "labels": labels });
            let v = api.call(Method::PATCH, &format!("{}/members/{}", base(&id), enc(&server)), Some(&body)).await?;
            show(g, &v, None)
        }
        ClusterSwarmCmd::RotateTokens { cluster, worker, manager, yes } => {
            if !worker && !manager {
                return Err(CliError::usage("give --worker, --manager or both"));
            }

            let id = cluster_id(&mut api, &cluster).await?;
            confirm(g, yes, &format!("Rotate the join tokens of {cluster}? Old tokens stop working."))?;
            let v = api
                .post(&format!("{}/tokens/rotate", base(&id)), &json!({"worker": worker, "manager": manager}))
                .await?;
            show(g, &v, None)
        }
    }
}

// ------------------------------------------------------------------ service swarm

#[derive(Args, Debug, Clone)]
pub struct ServiceRef {
    /// Service id or name.
    pub service: String,
    /// The service's environment (id or name), when the name is ambiguous.
    #[arg(long)]
    pub environment: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum ServiceSwarmCmd {
    /// The Swarm service: settings, tasks, HA report.
    Status {
        #[command(flatten)]
        svc: ServiceRef,
    },
    /// Apply the current spec to the swarm (skipped when unchanged).
    Deploy {
        #[command(flatten)]
        svc: ServiceRef,
        #[command(flatten)]
        wait: WaitFlags,
    },
    /// Set the replica count (0 to 1000).
    Scale {
        #[command(flatten)]
        svc: ServiceRef,
        replicas: u32,
        #[command(flatten)]
        wait: WaitFlags,
    },
    /// Restart every task, one batch at a time.
    Restart {
        #[command(flatten)]
        svc: ServiceRef,
        #[command(flatten)]
        wait: WaitFlags,
    },
    /// Go back to the previous release.
    Rollback {
        #[command(flatten)]
        svc: ServiceRef,
        #[arg(long)]
        yes: bool,
        #[command(flatten)]
        wait: WaitFlags,
    },
    /// Remove the Swarm service (the Forge service stays).
    Remove {
        #[command(flatten)]
        svc: ServiceRef,
        #[arg(long)]
        yes: bool,
        #[command(flatten)]
        wait: WaitFlags,
    },
    /// The last lines of the service's logs.
    Logs {
        #[command(flatten)]
        svc: ServiceRef,
        #[arg(long, default_value_t = 200)]
        tail: u32,
    },
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ServiceAction {
    Deploy,
    Scale { replicas: u32 },
    Restart,
    Rollback,
    Remove,
}

pub async fn service_swarm(g: &Global, cmd: ServiceSwarmCmd) -> Result<()> {
    let t = target(g)?;
    let mut api = connect(g, &t).await?;

    let svc = match &cmd {
        ServiceSwarmCmd::Status { svc }
        | ServiceSwarmCmd::Deploy { svc, .. }
        | ServiceSwarmCmd::Scale { svc, .. }
        | ServiceSwarmCmd::Restart { svc, .. }
        | ServiceSwarmCmd::Rollback { svc, .. }
        | ServiceSwarmCmd::Remove { svc, .. }
        | ServiceSwarmCmd::Logs { svc, .. } => svc.clone(),
    };

    let app = crate::rescmd::resolve_app(&mut api, &svc.service, svc.environment.as_deref()).await?;
    let path = format!("/api/v1/services/{}/swarm", enc(app["id"].as_str().unwrap_or_default()));

    let (action, wait) = match cmd {
        ServiceSwarmCmd::Status { .. } => return show(g, &api.get(&path).await?, None),
        ServiceSwarmCmd::Logs { tail, .. } => {
            let v = api.get(&format!("{path}/logs?tail={tail}")).await?;

            if g.output.is_machine() {
                return show(g, &v, None);
            }

            match v["logs"].as_str() {
                Some(text) => print!("{text}"),
                None => return show(g, &v, None),
            }

            return Ok(());
        }
        ServiceSwarmCmd::Deploy { wait, .. } => (ServiceAction::Deploy, wait),
        ServiceSwarmCmd::Scale { replicas, wait, .. } => (ServiceAction::Scale { replicas }, wait),
        ServiceSwarmCmd::Restart { wait, .. } => (ServiceAction::Restart, wait),
        ServiceSwarmCmd::Rollback { yes, wait, .. } => {
            confirm(g, yes, &format!("Roll {} back to its previous release?", svc.service))?;
            (ServiceAction::Rollback, wait)
        }
        ServiceSwarmCmd::Remove { yes, wait, .. } => {
            confirm(g, yes, &format!("Remove the Swarm service of {}?", svc.service))?;
            (ServiceAction::Remove, wait)
        }
    };

    let body = serde_json::to_value(&action).map_err(|e| CliError::usage(e.to_string()))?;
    let v = api.post(&path, &body).await?;

    job_or_wait(g, &mut api, v, wait).await
}

// ------------------------------------------------------------------ docker resources

#[derive(ValueEnum, Debug, Clone, Copy)]
pub enum Driver {
    Bridge,
    Overlay,
}

#[derive(Subcommand, Debug)]
pub enum DockerResCmd {
    /// A node's images.
    Images { node: String },
    /// A node's volumes.
    Volumes { node: String },
    /// A node's networks.
    Networks { node: String },
    /// Space used by images, containers, volumes and build cache.
    DiskUsage { node: String },
    /// Recent Docker events on a node.
    Events {
        node: String,
        /// How far back (seconds, at most one day).
        #[arg(long, default_value_t = 300)]
        since_secs: u32,
    },
    /// Inspect, pull, push, tag, remove or prune images.
    #[command(subcommand)]
    Image(ImageCmd),
    /// Inspect, create, remove or prune volumes.
    #[command(subcommand)]
    Volume(VolumeCmd),
    /// Inspect, create, remove, connect or prune networks.
    #[command(subcommand)]
    Network(NetworkCmd),
}

#[derive(Subcommand, Debug)]
pub enum ImageCmd {
    /// An image's configuration and layers.
    Inspect { node: String, image: String },
    /// The layers an image was built from.
    History { node: String, image: String },
    /// Pull an image; private registries use the credentials added in Forge.
    Pull {
        node: String,
        image: String,
        #[command(flatten)]
        wait: WaitFlags,
    },
    /// Push an image to a registry added in Forge with pushes allowed.
    Push {
        node: String,
        image: String,
        #[command(flatten)]
        wait: WaitFlags,
    },
    /// Tag an image as `repo:tag`.
    Tag { node: String, image: String, repo: String, tag: String },
    /// Remove an image.
    Remove {
        node: String,
        image: String,
        #[arg(long)]
        force: bool,
        #[arg(long)]
        yes: bool,
    },
    /// Remove dangling images (`--all`: every unused image).
    Prune {
        node: String,
        #[arg(long)]
        all: bool,
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum VolumeCmd {
    /// A volume's driver, mount point and labels.
    Inspect { node: String, name: String },
    /// Create a named volume.
    Create {
        node: String,
        name: String,
        #[arg(long = "label", value_name = "KEY=VALUE")]
        labels: Vec<String>,
    },
    /// Remove a volume and its data.
    Remove {
        node: String,
        name: String,
        #[arg(long)]
        force: bool,
        #[arg(long)]
        yes: bool,
    },
    /// Remove unused volumes and their data.
    Prune {
        node: String,
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum NetworkCmd {
    /// A network's configuration and attached containers.
    Inspect { node: String, network: String },
    /// Create a bridge or overlay network.
    Create {
        node: String,
        name: String,
        #[arg(long, value_enum, default_value = "bridge")]
        driver: Driver,
        #[arg(long, value_name = "CIDR")]
        subnet: Option<String>,
        #[arg(long)]
        gateway: Option<String>,
        /// No outside access.
        #[arg(long)]
        internal: bool,
        /// Standalone containers may join (overlay).
        #[arg(long)]
        attachable: bool,
        #[arg(long = "label", value_name = "KEY=VALUE")]
        labels: Vec<String>,
    },
    /// Remove a network.
    Remove {
        node: String,
        network: String,
        #[arg(long)]
        yes: bool,
    },
    /// Attach a container to a network.
    Connect { node: String, network: String, container: String },
    /// Detach a container from a network.
    Disconnect {
        node: String,
        network: String,
        container: String,
        #[arg(long)]
        force: bool,
    },
    /// Remove unused networks.
    Prune {
        node: String,
        #[arg(long)]
        yes: bool,
    },
}

const IMAGE_COLS: &[Col] = &[("ID", "/id"), ("TAGS", "/tags"), ("SIZE", "/size"), ("CREATED", "/created")];
const VOLUME_COLS: &[Col] =
    &[("NAME", "/name"), ("DRIVER", "/driver"), ("SCOPE", "/scope"), ("MANAGED", "/managed"), ("CREATED", "/created")];
const NETWORK_COLS: &[Col] = &[
    ("ID", "/id"),
    ("NAME", "/name"),
    ("DRIVER", "/driver"),
    ("SCOPE", "/scope"),
    ("SUBNETS", "/subnets"),
    ("CONTAINERS", "/containers"),
    ("PROTECTED", "/protected"),
];

/// Runs one op on `node` and prints the answer (or the job).
async fn op(g: &Global, api: &mut Api, node: &str, op: DockerOp, wait: Option<WaitFlags>) -> Result<()> {
    op.validate().map_err(CliError::usage)?;
    let id = node_id(api, node).await?;
    let body = serde_json::to_value(&op).map_err(|e| CliError::usage(e.to_string()))?;
    let v = api.post(&format!("/api/v1/servers/{}/docker/ops", enc(&id)), &body).await?;

    job_or_wait(g, api, v, wait.unwrap_or(WaitFlags { wait: false, timeout: 0 })).await
}

async fn list(g: &Global, api: &mut Api, node: &str, what: &str, cols: Option<&[Col]>) -> Result<()> {
    let id = node_id(api, node).await?;
    let v = api.get(&format!("/api/v1/servers/{}/docker/{what}", enc(&id))).await?;

    show(g, &v, cols)
}

pub async fn docker_resources(g: &Global, cmd: DockerResCmd) -> Result<()> {
    let t = target(g)?;
    let mut api = connect(g, &t).await?;
    let api = &mut api;

    match cmd {
        DockerResCmd::Images { node } => list(g, api, &node, "images", Some(IMAGE_COLS)).await,
        DockerResCmd::Volumes { node } => list(g, api, &node, "volumes", Some(VOLUME_COLS)).await,
        DockerResCmd::Networks { node } => list(g, api, &node, "networks", Some(NETWORK_COLS)).await,
        DockerResCmd::DiskUsage { node } => list(g, api, &node, "disk-usage", None).await,
        DockerResCmd::Events { node, since_secs } => {
            list(g, api, &node, &format!("events?since_secs={since_secs}"), None).await
        }
        DockerResCmd::Image(c) => image(g, api, c).await,
        DockerResCmd::Volume(c) => volume(g, api, c).await,
        DockerResCmd::Network(c) => network(g, api, c).await,
    }
}

async fn image(g: &Global, api: &mut Api, cmd: ImageCmd) -> Result<()> {
    match cmd {
        ImageCmd::Inspect { node, image } => op(g, api, &node, DockerOp::ImageInspect { id: image }, None).await,
        ImageCmd::History { node, image } => op(g, api, &node, DockerOp::ImageHistory { id: image }, None).await,
        ImageCmd::Pull { node, image, wait } => {
            op(g, api, &node, DockerOp::ImagePull { image, registry_auth: None }, Some(wait)).await
        }
        ImageCmd::Push { node, image, wait } => {
            op(g, api, &node, DockerOp::ImagePush { image, registry_auth: None }, Some(wait)).await
        }
        ImageCmd::Tag { node, image, repo, tag } => {
            op(g, api, &node, DockerOp::ImageTag { id: image, repo, tag }, None).await
        }
        ImageCmd::Remove { node, image, force, yes } => {
            confirm(g, yes, &format!("Remove image {image} from {node}?"))?;
            op(g, api, &node, DockerOp::ImageRemove { id: image, force }, None).await
        }
        ImageCmd::Prune { node, all, yes } => {
            let what = if all { "every unused image" } else { "dangling images" };
            confirm(g, yes, &format!("Remove {what} on {node}?"))?;
            op(g, api, &node, DockerOp::ImagePrune { all }, None).await
        }
    }
}

async fn volume(g: &Global, api: &mut Api, cmd: VolumeCmd) -> Result<()> {
    match cmd {
        VolumeCmd::Inspect { node, name } => op(g, api, &node, DockerOp::VolumeInspect { name }, None).await,
        VolumeCmd::Create { node, name, labels } => {
            let labels = pairs(&labels, "--label")?;
            op(g, api, &node, DockerOp::VolumeCreate { name, labels }, None).await
        }
        VolumeCmd::Remove { node, name, force, yes } => {
            confirm(g, yes, &format!("Remove volume {name} and its data on {node}?"))?;
            op(g, api, &node, DockerOp::VolumeRemove { name, force }, None).await
        }
        VolumeCmd::Prune { node, yes } => {
            confirm(g, yes, &format!("Remove every unused volume and its data on {node}?"))?;
            op(g, api, &node, DockerOp::VolumePrune {}, None).await
        }
    }
}

async fn network(g: &Global, api: &mut Api, cmd: NetworkCmd) -> Result<()> {
    match cmd {
        NetworkCmd::Inspect { node, network } => {
            op(g, api, &node, DockerOp::NetworkInspect { id: network }, None).await
        }
        NetworkCmd::Create { node, name, driver, subnet, gateway, internal, attachable, labels } => {
            let driver = match driver {
                Driver::Bridge => NetworkDriver::Bridge,
                Driver::Overlay => NetworkDriver::Overlay,
            };

            let labels = pairs(&labels, "--label")?;
            let o = DockerOp::NetworkCreate { name, driver, subnet, gateway, internal, attachable, labels };
            op(g, api, &node, o, None).await
        }
        NetworkCmd::Remove { node, network, yes } => {
            confirm(g, yes, &format!("Remove network {network} on {node}?"))?;
            op(g, api, &node, DockerOp::NetworkRemove { id: network }, None).await
        }
        NetworkCmd::Connect { node, network, container } => {
            op(g, api, &node, DockerOp::NetworkConnect { network, container }, None).await
        }
        NetworkCmd::Disconnect { node, network, container, force } => {
            op(g, api, &node, DockerOp::NetworkDisconnect { network, container, force }, None).await
        }
        NetworkCmd::Prune { node, yes } => {
            confirm(g, yes, &format!("Remove every unused network on {node}?"))?;
            op(g, api, &node, DockerOp::NetworkPrune {}, None).await
        }
    }
}

// ------------------------------------------------------------------ registries

#[derive(ValueEnum, Serialize, Debug, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum RegistryKind {
    DockerHub,
    Ghcr,
    Gitlab,
    Ecr,
    Custom,
}

#[derive(Args, Debug)]
pub struct RegistryFlags {
    #[arg(long, value_enum)]
    pub kind: RegistryKind,
    /// `host[:port]`, e.g. `ghcr.io`.
    #[arg(long)]
    pub host: String,
    #[arg(long)]
    pub username: String,
    /// Read the password or token from stdin (never from a flag).
    #[arg(long)]
    pub secret_stdin: bool,
    /// Images may be pushed to it.
    #[arg(long)]
    pub allow_push: bool,
}

#[derive(Subcommand, Debug)]
pub enum RegistriesCmd {
    /// Registries of the workspace (credentials are never shown).
    List,
    /// Add a registry: `printf %s "$TOKEN" | forge registries add ghcr --kind ghcr --host ghcr.io --username me --secret-stdin`.
    Add {
        name: String,
        #[command(flatten)]
        flags: RegistryFlags,
    },
    /// Replace a registry's settings; without `--secret-stdin` the stored
    /// secret stays (only while host, kind and user name stay the same).
    Update {
        registry: String,
        name: String,
        #[command(flatten)]
        flags: RegistryFlags,
    },
    /// Remove a registry and its stored credentials.
    Remove {
        registry: String,
        #[arg(long)]
        yes: bool,
    },
}

fn read_secret(stdin: bool) -> Result<Option<String>> {
    if !stdin {
        return Ok(None);
    }

    let mut s = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut s)?;
    let s = s.trim_end_matches(['\n', '\r']).to_owned();
    if s.is_empty() {
        return Err(CliError::usage("--secret-stdin: stdin was empty"));
    }

    Ok(Some(s))
}

async fn by_name(api: &mut Api, path: &str, what: &str, key: &str) -> Result<String> {
    if crate::admincmd::looks_like_id(key) {
        return Ok(key.to_owned());
    }

    let all = api.get(path).await?;
    let hits: Vec<&Value> = all.as_array().into_iter().flatten().filter(|r| r["name"] == key).collect();
    match hits.as_slice() {
        [one] => Ok(one["id"].as_str().unwrap_or_default().to_owned()),
        [] => Err(CliError::usage(format!("no {what} named `{key}`")).with_code("not_found")),
        _ => Err(CliError::usage(format!("several {what}s are named `{key}`: use the id")).with_code("ambiguous")),
    }
}

const REGISTRY_COLS: &[Col] = &[
    ("ID", "/id"),
    ("NAME", "/name"),
    ("KIND", "/kind"),
    ("HOST", "/host"),
    ("USER", "/username"),
    ("PUSH", "/allow_push"),
];

pub async fn registries(g: &Global, cmd: RegistriesCmd) -> Result<()> {
    const PATH: &str = "/api/v1/registries";
    let t = target(g)?;
    let mut api = connect(g, &t).await?;
    let body = |name: String, f: RegistryFlags| -> Result<Value> {
        Ok(json!({
            "name": name, "kind": f.kind, "host": f.host, "username": f.username,
            "secret": read_secret(f.secret_stdin)?, "allow_push": f.allow_push,
        }))
    };

    match cmd {
        RegistriesCmd::List => show(g, &api.get(PATH).await?, Some(REGISTRY_COLS)),
        RegistriesCmd::Add { name, flags } => {
            let b = body(name, flags)?;
            show(g, &api.post(PATH, &b).await?, None)
        }
        RegistriesCmd::Update { registry, name, flags } => {
            let id = by_name(&mut api, PATH, "registry", &registry).await?;
            let b = body(name, flags)?;
            show(g, &api.call(Method::PUT, &format!("{PATH}/{}", enc(&id)), Some(&b)).await?, None)
        }
        RegistriesCmd::Remove { registry, yes } => {
            let id = by_name(&mut api, PATH, "registry", &registry).await?;
            confirm(g, yes, &format!("Remove registry {registry} and its stored credentials?"))?;
            api.call(Method::DELETE, &format!("{PATH}/{}", enc(&id)), None).await?;
            Ok(())
        }
    }
}

// ------------------------------------------------------------------ webhooks

#[derive(ValueEnum, Serialize, Debug, Clone, Copy, Default)]
#[serde(rename_all = "snake_case")]
pub enum Signature {
    /// The secret URL alone (the tag cannot be changed).
    #[default]
    None,
    /// `X-Hub-Signature-256`.
    Github,
    /// `X-Gitlab-Token`.
    Gitlab,
    /// Forge's timestamped HMAC (`X-Forge-Signature`).
    Forge,
}

#[derive(Args, Debug)]
pub struct WebhookFlags {
    /// Redeploy this service (id or name) …
    #[arg(long, group = "target", required = true)]
    pub service: Option<String>,
    /// … or this compose stack (id).
    #[arg(long, group = "target")]
    pub stack: Option<String>,
    #[arg(long, value_enum, default_value = "none")]
    pub signature: Signature,
    /// Tags a delivery may deploy (`v*`); needs a signature.
    #[arg(long)]
    pub tag_pattern: Option<String>,
    /// Create or keep it disabled.
    #[arg(long)]
    pub disabled: bool,
}

#[derive(Subcommand, Debug)]
pub enum WebhooksCmd {
    /// Deploy webhooks of the workspace.
    List,
    /// Create one; its URL and signing secret are printed once.
    Create {
        name: String,
        #[command(flatten)]
        flags: WebhookFlags,
    },
    /// Change a webhook's name, target, signature or tag pattern.
    Update {
        webhook: String,
        name: String,
        #[command(flatten)]
        flags: WebhookFlags,
    },
    /// Delete a webhook; its URL stops working.
    Delete {
        webhook: String,
        #[arg(long)]
        yes: bool,
    },
    /// New URL and secret; the old ones stop working at once.
    Rotate {
        webhook: String,
        #[arg(long)]
        yes: bool,
    },
    /// Recent deliveries and their outcome.
    Deliveries { webhook: String },
}

const WEBHOOK_COLS: &[Col] = &[
    ("ID", "/id"),
    ("NAME", "/name"),
    ("TARGET", "/target/kind"),
    ("SIGNATURE", "/signature"),
    ("TAGS", "/tag_pattern"),
    ("ENABLED", "/enabled"),
    ("URL", "/token_hint"),
];
const DELIVERY_COLS: &[Col] = &[
    ("AT", "/at"),
    ("OUTCOME", "/outcome"),
    ("TAG", "/tag"),
    ("SOURCE", "/source_ip"),
    ("REASON", "/reason"),
    ("JOB", "/job_id"),
];

async fn webhook_body(api: &mut Api, name: String, f: WebhookFlags) -> Result<Value> {
    let target = match (f.service, f.stack) {
        (Some(s), _) => {
            let app = crate::rescmd::resolve_app(api, &s, None).await?;
            json!({"kind": "service", "id": app["id"]})
        }
        (None, Some(s)) => json!({"kind": "stack", "id": s}),
        (None, None) => return Err(CliError::usage("give --service or --stack")),
    };

    Ok(json!({
        "name": name, "target": target, "signature": f.signature,
        "tag_pattern": f.tag_pattern, "enabled": !f.disabled,
    }))
}

pub async fn webhooks(g: &Global, cmd: WebhooksCmd) -> Result<()> {
    const PATH: &str = "/api/v1/webhooks";
    let t = target(g)?;
    let mut api = connect(g, &t).await?;
    let api = &mut api;

    match cmd {
        WebhooksCmd::List => show(g, &api.get(PATH).await?, Some(WEBHOOK_COLS)),
        WebhooksCmd::Create { name, flags } => {
            let b = webhook_body(api, name, flags).await?;
            let v = api.post(PATH, &b).await?;

            if !g.output.is_machine() {
                eprintln!("The URL and secret are shown only now: store them in your CI's secrets.");
            }

            show(g, &v, None)
        }
        WebhooksCmd::Update { webhook, name, flags } => {
            let id = by_name(api, PATH, "webhook", &webhook).await?;
            let b = webhook_body(api, name, flags).await?;
            show(g, &api.call(Method::PUT, &format!("{PATH}/{}", enc(&id)), Some(&b)).await?, None)
        }
        WebhooksCmd::Delete { webhook, yes } => {
            let id = by_name(api, PATH, "webhook", &webhook).await?;
            confirm(g, yes, &format!("Delete webhook {webhook}? Its URL stops working."))?;
            api.call(Method::DELETE, &format!("{PATH}/{}", enc(&id)), None).await?;
            Ok(())
        }
        WebhooksCmd::Rotate { webhook, yes } => {
            let id = by_name(api, PATH, "webhook", &webhook).await?;
            confirm(g, yes, &format!("Rotate webhook {webhook}? The old URL and secret stop working."))?;
            show(g, &api.post(&format!("{PATH}/{}/rotate", enc(&id)), &json!({})).await?, None)
        }
        WebhooksCmd::Deliveries { webhook } => {
            let id = by_name(api, PATH, "webhook", &webhook).await?;
            show(g, &api.get(&format!("{PATH}/{}/deliveries", enc(&id))).await?, Some(DELIVERY_COLS))
        }
    }
}

// ------------------------------------------------------------------ stack templates

#[derive(Subcommand, Debug)]
pub enum TemplatesCmd {
    /// Compose app templates and their variables.
    Templates,
    /// Create a compose stack from a template.
    FromTemplate {
        /// Template id (see `forge stacks templates`).
        template: String,
        /// Name of the new stack.
        name: String,
        /// Environment (id or name).
        #[arg(long)]
        environment: String,
        /// Node it runs on (id, host name or name).
        #[arg(long)]
        node: String,
        /// A variable (`KEY=VALUE`, repeat). Empty secrets are generated.
        #[arg(long = "set", value_name = "KEY=VALUE")]
        values: Vec<String>,
    },
}

const TEMPLATE_COLS: &[Col] =
    &[("ID", "/id"), ("TITLE", "/title"), ("CATEGORY", "/category"), ("DESCRIPTION", "/description")];

pub async fn templates(g: &Global, cmd: TemplatesCmd) -> Result<()> {
    let t = target(g)?;
    let mut api = connect(g, &t).await?;

    match cmd {
        TemplatesCmd::Templates => {
            let v = api.get("/api/v1/stack-templates").await?;
            show(g, &v, Some(TEMPLATE_COLS))
        }
        TemplatesCmd::FromTemplate { template, name, environment, node, values } => {
            let env = crate::rescmd::resolve_environment(&mut api, &environment).await?;
            let server = node_id(&mut api, &node).await?;
            let body = json!({
                "template": template, "name": name, "environment_id": env,
                "server_id": server, "values": pairs(&values, "--set")?,
            });
            show(g, &api.post("/api/v1/stacks/from-template", &body).await?, None)
        }
    }
}
