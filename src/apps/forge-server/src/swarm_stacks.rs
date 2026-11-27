//! Compose stacks, secrets, configs, networks and node addresses of a
//! Swarm cluster (D-171). Handlers in `api/swarm_stacks.rs` parse,
//! authorize, call this and audit; deploys and removals are `swarm.stack`
//! jobs ([`crate::jobs::swarm_stack`]).

use std::collections::BTreeMap;
use std::sync::Arc;

use base64::Engine as _;
use forge_catalog::stacks::{Policy, Report};
use forge_catalog::swarm::{ObjectData, SwarmNetwork, SwarmOp, TaskStat};
use forge_catalog::swarm_stack::{
    ObjectKind, StackStatus, SwarmObject, SwarmStackPart, common_roots, namespace, object_name_ok, stack_dir,
};
use forge_core::model::{ClusterMode, Environment};
use forge_core::ports::{ClusterRepository, FabricRepository, ServerRepository, ServiceRepository, TenancyRepository};
use forge_core::stacks::{Stack, StackRepository};
use forge_core::swarm::SwarmRepository;
use forge_core::tags::FactsRepository;
use forge_core::{ClusterId, DomainError, DomainResult, EnvironmentId, ServerId};
use forge_secrets::Envelope;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::node_rpc::RpcError;
use crate::stacks::{ns_slug, open_env, parse_compose};
use crate::swarm::SwarmService;

fn invalid(msg: impl Into<String>) -> DomainError {
    DomainError::Invalid(msg.into())
}

/// Ports the swarm stacks need.
#[derive(Debug)]
pub struct SwarmStacks {
    pub swarm: Arc<SwarmService>,
    pub stacks: Arc<dyn StackRepository>,
    pub tenancy: Arc<dyn TenancyRepository>,
    pub swarms: Arc<dyn SwarmRepository>,
    pub servers: Arc<dyn ServerRepository>,
    pub clusters: Arc<dyn ClusterRepository>,
    pub services: Arc<dyn ServiceRepository>,
    pub files: Arc<dyn forge_core::files::FileRepository>,
    pub fabrics: Arc<dyn FabricRepository>,
    pub facts: Arc<dyn FactsRepository>,
    pub envelope: Envelope,
    /// Recent task stats per node (see [`Self::task_stats`]).
    pub stats_cache: std::sync::Mutex<std::collections::HashMap<ServerId, StatsEntry>>,
}

/// When a node's task stats were read, and what they were.
pub type StatsEntry = (std::time::Instant, Vec<TaskStat>);

/// How long a node's task stats are reused: views poll, and each read is a
/// helper command that samples every task container on the node.
const STATS_CACHE: std::time::Duration = std::time::Duration::from_secs(10);
/// Most nodes asked at once.
pub const MAX_STATS_NODES: usize = 32;

/// A stack's swarm part and what the policy found.
#[derive(Debug, Clone)]
pub struct Prepared {
    pub part: SwarmStackPart,
    pub report: Report,
}

/// A compose text to check for a swarm.
#[derive(Debug, Clone)]
pub struct Draft<'a> {
    pub environment: EnvironmentId,
    pub name: &'a str,
    pub content: &'a str,
    pub env: BTreeMap<String, String>,
}

/// One address a node could advertise to the swarm.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AddressChoice {
    pub address: String,
    /// `mesh` (the Forge fabric), `interface`, `primary` or `agent` (where
    /// the agent connects from).
    pub source: AddressSource,
    /// Interface name, when known.
    pub interface: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AddressSource {
    Mesh,
    Primary,
    Interface,
    Agent,
}

/// The addresses a node can advertise, best first: the mesh address (works
/// across NAT and sites), then the node's primary address, its other
/// interfaces and where the agent connects from. Loopback, link-local and
/// container bridges are left out.
#[must_use]
pub fn address_choices(mesh: Option<&str>, facts: Option<&Value>, live: Option<&str>) -> Vec<AddressChoice> {
    let mut out: Vec<AddressChoice> = Vec::new();
    let mut add = |address: &str, source: AddressSource, interface: &str| {
        let Ok(ip) = address.parse::<std::net::IpAddr>() else { return };
        let link_local = match ip {
            std::net::IpAddr::V4(v4) => v4.is_link_local(),
            std::net::IpAddr::V6(v6) => v6.segments()[0] & 0xffc0 == 0xfe80,
        };

        let bridge = interface.starts_with("docker") || interface.starts_with("br-") || interface.starts_with("veth");

        if ip.is_loopback() || ip.is_unspecified() || link_local || bridge || out.iter().any(|c| c.address == address) {
            return;
        }

        out.push(AddressChoice { address: address.to_owned(), source, interface: interface.to_owned() });
    };

    if let Some(m) = mesh {
        add(m, AddressSource::Mesh, "");
    }

    if let Some(f) = facts {
        if let Some(a) = f["address"].as_str() {
            add(a, AddressSource::Primary, "");
        }

        for i in f["interfaces"].as_array().into_iter().flatten() {
            if let Some(a) = i["address"].as_str() {
                add(a, AddressSource::Interface, i["name"].as_str().unwrap_or_default());
            }
        }
    }

    if let Some(l) = live {
        add(l, AddressSource::Agent, "");
    }

    out
}

/// The Forge name of the Swarm secret that carries secret variable `key`
/// of service `service` (`env-<first 8 of the id>-<key>`).
#[must_use]
pub fn env_object_name(service: &str, key: &str) -> String {
    let id: String =
        service.chars().filter(char::is_ascii_alphanumeric).take(8).collect::<String>().to_ascii_lowercase();
    let key: String =
        key.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' }).take(38).collect();

    format!("env-{id}-{key}")
}

/// A version for a secret variable that changes exactly when its sealed
/// value does (the sealed text, not the value, is hashed: nothing about
/// the value can be learned from it).
#[must_use]
pub fn env_object_version(sealed: &str) -> u32 {
    let d = ring::digest::digest(&ring::digest::SHA256, sealed.as_bytes());
    let mut b = [0u8; 4];
    b.copy_from_slice(&d.as_ref()[..4]);

    u32::from_be_bytes(b) % 999_999_999 + 1
}

/// Resolves a Swarm service's secrets and configs to their newest Docker
/// versions, and, when the service delivers secret variables as files,
/// moves each such variable into a Swarm secret (`/run/secrets/<KEY>`, with
/// `<KEY>_FILE` naming it) so its value is not in the service spec (D-171).
pub async fn resolve_objects(
    swarm: &SwarmService,
    cluster: ClusterId,
    principal: &str,
    app: &forge_core::model::Service,
    def: &mut forge_catalog::swarm_service::ServiceDef,
) -> DomainResult<()> {
    use forge_catalog::swarm_service::{ObjectMount, SecretEnv};

    let wanted_env = def.settings.secret_env == SecretEnv::Files && app.env.iter().any(|e| e.secret);
    if def.settings.secrets.is_empty() && def.settings.configs.is_empty() && !wanted_env {
        return Ok(());
    }

    let list = |kind| async move {
        let v = swarm.run(cluster, principal, &SwarmOp::Objects { kind }).await?;
        serde_json::from_value::<Vec<SwarmObject>>(v).map_err(DomainError::internal)
    };
    let mut objects = Vec::new();

    for (kind, refs) in
        [(ObjectKind::Secret, def.settings.secrets.clone()), (ObjectKind::Config, def.settings.configs.clone())]
    {
        if refs.is_empty() {
            continue;
        }

        let latest = newest(&list(kind).await?);

        for r in refs {
            let version = latest.get(&r.name).ok_or_else(|| {
                invalid(format!(
                    "{} `{}` does not exist in this swarm; create it under Swarm → Secrets & configs",
                    kind.as_str(),
                    r.name
                ))
            })?;
            objects.push(ObjectMount {
                kind,
                docker_name: forge_catalog::swarm_stack::versioned(&r.name, *version),
                file: r.file(kind),
                uid: r.uid.clone(),
                gid: r.gid.clone(),
                mode: r.mode,
            });
        }
    }

    if wanted_env {
        for e in app.env.iter().filter(|e| e.secret) {
            let Some(pos) = def.env.iter().position(|(k, _)| *k == e.key) else { continue };
            let (key, value) = def.env.remove(pos);
            let name = env_object_name(&def.service_id, &key);
            let version = env_object_version(e.sealed.as_deref().unwrap_or_default());
            let data = ObjectData(base64::engine::general_purpose::STANDARD.encode(value.as_bytes()));
            let op = SwarmOp::ObjectCreate { kind: ObjectKind::Secret, name: name.clone(), version, data };
            swarm.run(cluster, principal, &op).await?;
            def.env.push((format!("{key}_FILE"), format!("/run/secrets/{key}")));
            objects.push(ObjectMount {
                kind: ObjectKind::Secret,
                docker_name: forge_catalog::swarm_stack::versioned(&name, version),
                file: key,
                uid: "0".into(),
                gid: "0".into(),
                mode: 0o444,
            });
        }
    }

    def.objects = objects;

    def.validate().map_err(invalid)
}

/// The newest version of every Forge-made object, by Forge name.
#[must_use]
pub fn newest(objects: &[SwarmObject]) -> BTreeMap<String, u32> {
    let mut out: BTreeMap<String, u32> = BTreeMap::new();

    for o in objects.iter().filter(|o| o.managed && !o.name.is_empty()) {
        let v = out.entry(o.name.clone()).or_insert(0);
        *v = (*v).max(o.version);
    }

    out
}

/// Forge-made versions that no service uses and that are not the newest of
/// their name: what a prune removes.
#[must_use]
pub fn prunable(objects: &[SwarmObject]) -> Vec<String> {
    let latest = newest(objects);
    objects
        .iter()
        .filter(|o| o.managed && o.used_by.is_empty() && latest.get(&o.name).is_some_and(|v| *v > o.version))
        .map(|o| o.docker_name.clone())
        .collect()
}

/// Why a cluster cannot change mode, if anything stands in the way.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ConvertCheck {
    pub from: String,
    pub to: String,
    /// Every check passed.
    pub ok: bool,
    pub checks: Vec<ConvertItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ConvertItem {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

impl SwarmStacks {
    /// The Swarm cluster of the stack's environment, or `Invalid`.
    pub async fn cluster_of(&self, stack: &Stack) -> DomainResult<ClusterId> {
        self.swarm
            .swarm_of_environment(stack.environment_id)
            .await?
            .ok_or_else(|| invalid("this stack's environment is not in a Docker Swarm cluster"))
    }

    /// The stacks of the cluster's environments, with their environment.
    pub async fn cluster_stacks(&self, cluster: ClusterId) -> DomainResult<Vec<(Environment, Stack)>> {
        let c = self.swarm.swarm_cluster(cluster).await?;
        let mut out = Vec::new();

        for e in self.tenancy.list_environments(c.workspace_id, Some(cluster)).await? {
            for s in self.stacks.list_stacks(Some(e.id)).await? {
                out.push((e.clone(), s));
            }
        }

        Ok(out)
    }

    /// The stack's namespace in the swarm (`forge-<environment>-<stack>`).
    pub async fn namespace(&self, stack: &Stack) -> DomainResult<String> {
        Ok(namespace(&self.env_slug(stack).await?, &stack.name))
    }

    async fn env_slug(&self, stack: &Stack) -> DomainResult<String> {
        self.env_slug_of(stack.environment_id).await
    }

    async fn env_slug_of(&self, environment: EnvironmentId) -> DomainResult<String> {
        let env = self.tenancy.get_environment(environment).await?;
        let slug = ns_slug(&env.name);
        if !forge_catalog::stacks::valid_stack_name(&slug) {
            return Err(invalid(format!("environment `{}` has no usable short name", env.name)));
        }

        Ok(slug)
    }

    /// Builds and checks revision `revision` for the swarm. Secret and config
    /// versions are read live from a manager. `.env` values are opened here
    /// and never stored in a job.
    pub async fn prepare(
        &self,
        stack: &Stack,
        revision: i64,
        allow_privileged: bool,
        principal: &str,
    ) -> DomainResult<Prepared> {
        let rev = self
            .stacks
            .get_revision(stack.id, revision)
            .await?
            .ok_or_else(|| DomainError::NotFound("revision not found".into()))?;
        let env = open_env(&self.envelope, stack.id, stack.env_sealed.as_deref()).map_err(DomainError::internal)?;
        let input = Draft { environment: stack.environment_id, name: &stack.name, content: &rev.content, env };

        self.prepare_draft(input, allow_privileged, principal).await
    }

    /// [`Self::prepare`] for a compose text that is not saved yet (create,
    /// edit, preview).
    pub async fn prepare_draft(&self, d: Draft<'_>, allow_privileged: bool, principal: &str) -> DomainResult<Prepared> {
        let cluster = self
            .swarm
            .swarm_of_environment(d.environment)
            .await?
            .ok_or_else(|| invalid("this environment is not in a Docker Swarm cluster"))?;
        let compose = parse_compose(d.content).map_err(invalid)?;
        let env_slug = self.env_slug_of(d.environment).await?;
        let mut per_node = Vec::new();

        for m in self.swarms.list_members(cluster).await? {
            per_node.push(crate::stacks::bind_roots(self.files.as_ref(), m.server_id).await);
        }

        let secrets = newest(&self.objects(cluster, ObjectKind::Secret, principal).await?);
        let configs = newest(&self.objects(cluster, ObjectKind::Config, principal).await?);
        let part = SwarmStackPart {
            policy: Policy {
                allow_privileged,
                bind_roots: common_roots(&per_node),
                stack_dir: stack_dir(&env_slug, d.name),
            },
            environment: env_slug,
            stack: d.name.to_owned(),
            compose,
            env: d.env,
            secrets,
            configs,
        };

        // Checked with privileged settings allowed, so the report lists them
        // and the caller decides whether `stack.privileged` is needed.
        let mut check = part.clone();
        check.policy.allow_privileged = true;
        let (_, report) = check.prepare().map_err(invalid)?;

        Ok(Prepared { part, report })
    }

    /// The stack's services and tasks, live from a manager.
    pub async fn status(&self, stack: &Stack, principal: &str) -> DomainResult<StackStatus> {
        let cluster = self.cluster_of(stack).await?;
        let op = SwarmOp::StackStatus { namespace: self.namespace(stack).await? };

        serde_json::from_value(self.swarm.run(cluster, principal, &op).await?).map_err(DomainError::internal)
    }

    /// Runs a quick operation on one of the stack's services.
    pub async fn service_op(&self, stack: &Stack, principal: &str, op: SwarmOp) -> DomainResult<Value> {
        let cluster = self.cluster_of(stack).await?;
        self.swarm.run(cluster, principal, &op).await
    }

    pub async fn objects(
        &self,
        cluster: ClusterId,
        kind: ObjectKind,
        principal: &str,
    ) -> DomainResult<Vec<SwarmObject>> {
        let v = self.swarm.run(cluster, principal, &SwarmOp::Objects { kind }).await?;
        serde_json::from_value(v).map_err(DomainError::internal)
    }

    /// Creates the first version of `name`, or the next one (a rotation).
    /// The value goes only into the signed command. Returns the Docker name.
    pub async fn put_object(
        &self,
        cluster: ClusterId,
        kind: ObjectKind,
        name: &str,
        value: &[u8],
        principal: &str,
    ) -> DomainResult<(String, u32)> {
        if !object_name_ok(name) {
            return Err(invalid("names use a-z, 0-9, `_` and `-` (up to 51)"));
        }

        if value.is_empty() || value.len() > forge_catalog::swarm_stack::MAX_OBJECT_BYTES {
            return Err(invalid("the value must be 1 byte to 500 KiB"));
        }

        let version = newest(&self.objects(cluster, kind, principal).await?).get(name).copied().unwrap_or(0) + 1;
        let data = ObjectData(base64::engine::general_purpose::STANDARD.encode(value));
        let op = SwarmOp::ObjectCreate { kind, name: name.to_owned(), version, data };
        self.swarm.run(cluster, principal, &op).await?;

        Ok((forge_catalog::swarm_stack::versioned(name, version), version))
    }

    pub async fn remove_object(
        &self,
        cluster: ClusterId,
        kind: ObjectKind,
        docker_name: &str,
        principal: &str,
    ) -> DomainResult<()> {
        let all = self.objects(cluster, kind, principal).await?;
        if let Some(o) = all.iter().find(|o| o.docker_name == docker_name)
            && !o.used_by.is_empty()
        {
            return Err(DomainError::Conflict(format!(
                "`{docker_name}` is used by {}; deploy them without it first",
                o.used_by.join(", ")
            )));
        }

        let op = SwarmOp::ObjectRemove { kind, docker_name: docker_name.to_owned() };

        self.swarm.run(cluster, principal, &op).await.map(|_| ())
    }

    /// Removes old, unused versions; returns their Docker names.
    pub async fn prune_objects(
        &self,
        cluster: ClusterId,
        kind: ObjectKind,
        principal: &str,
    ) -> DomainResult<Vec<String>> {
        let gone = prunable(&self.objects(cluster, kind, principal).await?);

        for d in &gone {
            let op = SwarmOp::ObjectRemove { kind, docker_name: d.clone() };
            self.swarm.run(cluster, principal, &op).await?;
        }

        Ok(gone)
    }

    /// CPU and memory of the Swarm tasks on the swarm nodes `node_ids`
    /// (Docker node ids, as tasks report them): one helper command per node,
    /// side by side, reused for ten seconds. A node that does not answer is
    /// left out.
    pub async fn task_stats(
        &self,
        cluster: ClusterId,
        node_ids: &[String],
        principal: &str,
    ) -> DomainResult<Vec<TaskStat>> {
        let members = self.swarms.list_members(cluster).await?;
        let servers: Vec<ServerId> = members
            .iter()
            .filter(|m| node_ids.contains(&m.node_id))
            .map(|m| m.server_id)
            .take(MAX_STATS_NODES)
            .collect();
        let cached = |s: &ServerId| -> Option<Vec<TaskStat>> {
            let c = self.stats_cache.lock().ok()?;
            c.get(s).filter(|(at, _)| at.elapsed() < STATS_CACHE).map(|(_, v)| v.clone())
        };
        let mut out = Vec::new();
        let mut ask = Vec::new();

        for s in servers {
            match cached(&s) {
                Some(v) => out.extend(v),
                None => ask.push(s),
            }
        }

        let calls = ask.iter().map(|s| self.swarm.run_on(*s, principal, &SwarmOp::TaskStats {}));
        let answers = futures::future::join_all(calls).await;

        for (server, answer) in ask.into_iter().zip(answers) {
            let stats: Vec<TaskStat> = match answer.map(serde_json::from_value) {
                Ok(Ok(v)) => v,
                Ok(Err(e)) => {
                    tracing::debug!(%server, error = %e, "task stats: bad answer");
                    continue;
                }
                Err(e) => {
                    tracing::debug!(%server, error = %e, "task stats: node did not answer");
                    continue;
                }
            };

            if let Ok(mut c) = self.stats_cache.lock() {
                c.insert(server, (std::time::Instant::now(), stats.clone()));
            }

            out.extend(stats);
        }

        Ok(out)
    }

    pub async fn networks(&self, cluster: ClusterId, principal: &str) -> DomainResult<Vec<SwarmNetwork>> {
        let v = self.swarm.run(cluster, principal, &SwarmOp::Networks {}).await?;
        serde_json::from_value(v).map_err(DomainError::internal)
    }

    pub async fn ensure_shared(&self, cluster: ClusterId, principal: &str) -> DomainResult<()> {
        self.swarm.run(cluster, principal, &SwarmOp::EnsureSharedNetwork {}).await.map(|_| ())
    }

    /// The addresses `server` could advertise, mesh first.
    pub async fn addresses(&self, cluster: ClusterId, server: ServerId) -> DomainResult<Vec<AddressChoice>> {
        let s = self.servers.get_server(server).await?;
        if s.cluster_id != Some(cluster) {
            return Err(invalid("the node is not in this cluster"));
        }

        let mesh = crate::stacks::fabric_ips(self.fabrics.as_ref(), s.workspace_id).await;
        let facts = self.facts.facts_of(server).await?.map(|(f, _)| f);
        let live = crate::facts::live_address(server);

        Ok(address_choices(mesh.get(&server.to_string()).map(String::as_str), facts.as_ref(), live.as_deref()))
    }

    /// Removes the cluster container network from every Linux member before
    /// the cluster becomes a Swarm (D-172). The checks
    /// above leave nothing Forge manages on it, so a node that still has
    /// containers attached holds something outside Forge: the conversion is
    /// refused naming it. The fabric stays (Swarm's data path uses it). An
    /// offline node keeps its empty network, which nothing uses.
    async fn remove_container_networks(&self, cluster: ClusterId, principal: &str) -> DomainResult<()> {
        let c = self.clusters.get_cluster(cluster).await?;

        if self.fabrics.get_fabric(cluster).await?.is_none() {
            return Ok(());
        }

        let iface = forge_catalog::fabric::interface_name(&cluster.to_string());
        let op = forge_catalog::fabric::FabricOp::NetworkRemove { iface };
        let nodes = self.servers.list_servers(Some(c.workspace_id)).await?;
        let mut held = Vec::new();

        for s in nodes.iter().filter(|s| s.cluster_id == Some(cluster) && s.os.eq_ignore_ascii_case("linux")) {
            let name = s.display_name.clone().unwrap_or_else(|| s.hostname.clone());

            match self.swarm.fabric_on(s.id, principal, &op).await {
                Ok(_) => {}
                Err(RpcError::Unreachable(e)) => {
                    tracing::info!(node = %s.id, error = %e, "node offline; its container network stays");
                }
                Err(e) => held.push(format!("{name}: {e}")),
            }
        }

        if held.is_empty() {
            return Ok(());
        }

        Err(DomainError::Conflict(format!(
            "remove these containers from the cluster network first: {}",
            held.join("; ")
        )))
    }

    /// What stands in the way of switching `cluster` to `to`.
    pub async fn convert_check(&self, cluster: ClusterId, to: ClusterMode) -> DomainResult<ConvertCheck> {
        let c = self.clusters.get_cluster(cluster).await?;
        let nodes: Vec<_> = self
            .servers
            .list_servers(Some(c.workspace_id))
            .await?
            .into_iter()
            .filter(|s| s.cluster_id == Some(cluster))
            .collect();
        let envs = self.tenancy.list_environments(c.workspace_id, Some(cluster)).await?;
        let mut checks = Vec::new();
        let mut item = |name: &str, ok: bool, detail: String| {
            checks.push(ConvertItem { name: name.to_owned(), ok, detail });
        };

        item(
            "mode",
            c.mode != to,
            if c.mode == to { "the cluster is already in this mode".into() } else { String::new() },
        );

        match to {
            ClusterMode::Swarm => {
                let not_linux: Vec<_> = nodes
                    .iter()
                    .filter(|s| !s.os.to_ascii_lowercase().contains("linux"))
                    .map(|s| s.hostname.clone())
                    .collect();
                item(
                    "linux",
                    not_linux.is_empty(),
                    if not_linux.is_empty() {
                        format!("{} node(s), all Linux", nodes.len())
                    } else {
                        format!("not Linux: {}", not_linux.join(", "))
                    },
                );

                let mut pinned = Vec::new();
                let mut stacks_on_nodes = Vec::new();

                for e in &envs {
                    for svc in self.services.list_services(Some(e.id)).await? {
                        pinned.push(format!("{}/{} ({})", e.name, svc.name, svc.runtime));
                    }

                    for st in self.stacks.list_stacks(Some(e.id)).await? {
                        if !self.stacks.list_nodes(st.id).await?.is_empty() {
                            stacks_on_nodes.push(format!("{}/{}", e.name, st.name));
                        }
                    }
                }

                item(
                    "services",
                    pinned.is_empty(),
                    if pinned.is_empty() {
                        "no services run on single nodes".into()
                    } else {
                        format!(
                            "these services run on chosen nodes and would lose their scheduler; move or delete them \
                             first: {}",
                            pinned.join(", ")
                        )
                    },
                );
                item(
                    "stacks",
                    stacks_on_nodes.is_empty(),
                    if stacks_on_nodes.is_empty() {
                        "no compose stacks are deployed per node".into()
                    } else {
                        format!("take these stacks down first: {}", stacks_on_nodes.join(", "))
                    },
                );
            }
            ClusterMode::Default => {
                let swarm = self.swarms.get_swarm(cluster).await?;
                item(
                    "swarm",
                    swarm.is_none(),
                    if swarm.is_none() {
                        "no swarm exists".into()
                    } else {
                        "make every node leave the swarm first (Swarm → Nodes → Leave)".into()
                    },
                );

                let mut services = Vec::new();

                for e in &envs {
                    for svc in self.services.list_services(Some(e.id)).await? {
                        services.push(format!("{}/{}", e.name, svc.name));
                    }
                }

                item(
                    "services",
                    services.is_empty(),
                    if services.is_empty() {
                        "no Swarm services are defined".into()
                    } else {
                        format!("delete or move these Swarm services first: {}", services.join(", "))
                    },
                );
            }
        }

        let ok = checks.iter().all(|c| c.ok);

        Ok(ConvertCheck { from: c.mode.as_str().to_owned(), to: to.as_str().to_owned(), ok, checks })
    }

    /// Switches the cluster's mode after [`Self::convert_check`] passes.
    pub async fn convert(&self, cluster: ClusterId, to: ClusterMode, principal: &str) -> DomainResult<ConvertCheck> {
        let check = self.convert_check(cluster, to).await?;
        if !check.ok {
            let why: Vec<_> = check.checks.iter().filter(|c| !c.ok).map(|c| c.detail.clone()).collect();
            return Err(DomainError::Conflict(why.join("; ")));
        }

        if to == ClusterMode::Swarm {
            self.remove_container_networks(cluster, principal).await?;
        }

        self.clusters.set_cluster_mode(cluster, to).await?;

        Ok(check)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn mesh_address_comes_first_and_noise_is_dropped() {
        let facts = json!({"address": "192.0.2.10", "interfaces": [
            {"name": "eth0", "address": "192.0.2.10"},
            {"name": "docker0", "address": "172.17.0.1"},
            {"name": "lo", "address": "127.0.0.1"},
            {"name": "eth1", "address": "198.51.100.4"},
            {"name": "eth0", "address": "fe80::1"}
        ]});
        let c = address_choices(Some("100.64.0.3"), Some(&facts), Some("203.0.113.9"));
        let addrs: Vec<_> = c.iter().map(|c| c.address.as_str()).collect();
        assert_eq!(addrs, vec!["100.64.0.3", "192.0.2.10", "198.51.100.4", "203.0.113.9"]);
        assert_eq!(c[0].source, AddressSource::Mesh);
        assert_eq!(c[2].interface, "eth1");
        assert!(address_choices(None, None, None).is_empty());
    }

    fn obj(name: &str, version: u32, used: bool) -> SwarmObject {
        SwarmObject {
            id: format!("{name}{version}"),
            kind: ObjectKind::Secret,
            name: name.into(),
            docker_name: forge_catalog::swarm_stack::versioned(name, version),
            version,
            created_at: String::new(),
            used_by: if used { vec!["api".into()] } else { vec![] },
            managed: true,
        }
    }

    #[test]
    fn secret_variables_get_stable_names_and_versions() {
        let n = env_object_name("01900000-0000-7000-8000-000000000001", "DB_PASSWORD");
        assert_eq!(n, "env-01900000-db_password");
        assert!(object_name_ok(&n));
        assert!(object_name_ok(&env_object_name("01900000", &"K".repeat(200))));
        let v = env_object_version("sealed-a");
        assert_eq!(v, env_object_version("sealed-a"));
        assert_ne!(v, env_object_version("sealed-b"));
        assert!(v > 0);
    }

    #[test]
    fn newest_versions_and_prune_keep_what_is_used() {
        let all = vec![obj("db", 1, false), obj("db", 2, true), obj("db", 3, false), obj("tls", 1, false)];
        assert_eq!(newest(&all), BTreeMap::from([("db".into(), 3), ("tls".into(), 1)]));
        assert_eq!(prunable(&all), vec!["db.v1"], "v2 is in use and v3 is the newest");
    }
}
