//! Releases and deployments of services (#73).
//!
//! [`DeployService`] turns a `forge.deploy.yaml` (or an earlier release) into
//! an immutable [`Release`], plans its rollout with
//! [`forge_catalog::deploy::plan`] and starts the `deploy.rollout` job
//! ([`crate::jobs::deploy`]) under the service's deployment lock. Handlers in
//! `api/deployments.rs` authorize, call this and audit.
//!
//! What a rollout changes outside its job (endpoints on the edges, health
//! policies on the nodes) goes through [`RolloutEffects`], implemented next
//! to the API that owns those flows and wired in `state.rs`.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use forge_catalog::deploy::{self as cat, DeploymentDoc, Plan, PlanInput, PlanNode, RolloutPolicy, Slot};
use forge_catalog::ingress::Target;
use forge_core::deploy::{
    Deployment, DeploymentKind, DeploymentRepository, DeploymentState, Release, ReleaseRepository, SlotRecord,
};
use forge_core::model::{AgentStatus, EnvVar, NewJob, Service, ServiceInstance};
use forge_core::ports::{ServerRepository, ServiceRepository, SiteRepository, TenancyRepository};
use forge_core::tags::{TagKind, TagRepository};
use forge_core::{
    AuthzContext, Clock, DeploymentId, DomainError, DomainResult, EntityKind, EntityRef, Principal, ReleaseId,
    ServerId, Timestamp, WorkspaceId,
};
use serde::Serialize;
use serde_json::{Value, json};

/// The rollout job's kind.
pub const ROLLOUT_KIND: &str = "deploy.rollout";
/// Most releases and deployments one list returns.
pub const MAX_LISTED: u32 = 200;

/// Edge traffic of some upstreams over a window (canary gates).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct TrafficSample {
    pub requests: u64,
    /// Answers with a 5xx status.
    pub errors: u64,
    /// 95th percentile of upstream time, ms (`None`: no timings).
    pub p95_ms: Option<u64>,
}

/// What a rollout changes outside its job.
#[async_trait]
pub trait RolloutEffects: Send + Sync + std::fmt::Debug {
    /// Re-applies every endpoint that routes to `app`, as whoever applied
    /// each last (each edge tests the configuration before reloading).
    /// Returns the endpoints' names.
    async fn reapply_endpoints(&self, app: forge_core::ServiceId) -> DomainResult<Vec<String>>;
    /// Sends `app`'s health policy for the copy that serves on `server` now.
    async fn push_health(&self, app: forge_core::ServiceId, server: ServerId);
    /// Edge traffic (last minute, reports newer than `since`) to these
    /// copies of `app`.
    async fn traffic(
        &self,
        app: forge_core::ServiceId,
        copies: &[cat::Target],
        since: Timestamp,
    ) -> DomainResult<TrafficSample>;
}

/// Parses a deployment document's YAML (YAML 1.2, with limits on depth,
/// size and aliases, as compose files).
pub fn parse_yaml(text: &str) -> Result<Value, String> {
    if text.len() > cat::MAX_DOC_BYTES {
        return Err(format!("the document is larger than {} KiB", cat::MAX_DOC_BYTES / 1024));
    }
    if text.trim().is_empty() {
        return Err("the document is empty".into());
    }
    let mut budget = serde_saphyr::Budget::default();
    budget.max_depth = 32;
    budget.max_nodes = 20_000;
    budget.max_aliases = 200;
    budget.max_anchors = 200;
    budget.max_documents = 1;
    budget.max_events = 100_000;
    budget.max_total_scalar_bytes = cat::MAX_DOC_BYTES * 4;
    budget.max_recorded_anchor_bytes = cat::MAX_DOC_BYTES * 4;
    let mut options = serde_saphyr::Options::default();
    options.budget = Some(budget);
    options.strict_booleans = true;
    options.leading_zero_numbers_as_strings = true;
    serde_saphyr::from_str_with_options(text, options).map_err(|e| {
        let msg = e.to_string();
        format!("invalid YAML: {}", msg.lines().next().unwrap_or(&msg))
    })
}

/// Reads a document: YAML, then the typed `forge/v1` schema.
pub fn read_document(yaml: &str, environment: Option<&str>) -> DomainResult<DeploymentDoc> {
    let v = parse_yaml(yaml).map_err(DomainError::Invalid)?;
    cat::parse(v, environment).map_err(|e| DomainError::Invalid(e.to_string()))
}

/// The copy that serves on `server`: the highest weight, then a ready one.
#[must_use]
pub fn serving(slots: &[SlotRecord], server: ServerId) -> Option<&SlotRecord> {
    slots
        .iter()
        .filter(|s| s.server_id == server)
        .max_by_key(|s| (s.weight, s.state == "ready", s.release_number, s.slot.clone()))
}

/// A copy's service port → its own port.
#[must_use]
pub fn port_map(s: &SlotRecord) -> BTreeMap<u16, u16> {
    s.ports
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(k, v)| Some((k.parse().ok()?, u16::try_from(v.as_u64()?).ok()?)))
        .collect()
}

/// Whether a site's typed targets include `app`.
#[must_use]
pub fn site_targets(config: &Value, app: forge_core::ServiceId) -> bool {
    let id = app.to_string();
    let routes = |v: &Value| {
        ["targets", "standby_targets"].iter().any(|k| {
            // A missing or foreign list routes nowhere.
            serde_json::from_value::<Vec<Target>>(v[*k].clone())
                .unwrap_or_default()
                .iter()
                .any(|t| matches!(t, Target::App { service_id, .. } if *service_id == id))
        })
    };
    // The default rule's targets, then each path rule's (D-169).
    routes(config) || config["spec"]["rules"].as_array().is_some_and(|rules| rules.iter().any(routes))
}

fn svc_ref(id: forge_core::ServiceId) -> EntityRef {
    EntityRef::new(EntityKind::Service, id)
}

/// A service with a release's spec and environment in place of its own.
#[must_use]
pub fn release_app(app: &Service, release: &Release) -> Service {
    let mut a = app.clone();
    a.spec = release.spec.clone();
    a.env = release.env.clone();
    if let Some(r) = release.spec.get("runtime").and_then(Value::as_str) {
        r.clone_into(&mut a.runtime);
    }
    a
}

/// A rollout ready to start (or to show, for a dry run).
#[derive(Debug, Clone)]
pub struct Prepared {
    pub app: Service,
    /// The service is created by this deployment.
    pub is_new: bool,
    pub release: Release,
    pub policy: RolloutPolicy,
    pub plan: Plan,
    /// Nodes the release runs on.
    pub nodes: Vec<ServerId>,
    /// Placements this deployment adds.
    pub added: Vec<ServerId>,
    /// Placements it removes once the rollout succeeded.
    pub removed: Vec<ServerId>,
    /// Node id → name, for the plan's descriptions.
    pub names: BTreeMap<String, String>,
    /// In a Docker Swarm cluster: the cluster and the Swarm plan, which
    /// replaces `plan`'s node steps.
    pub swarm: Option<SwarmRollout>,
}

/// A rollout in a Docker Swarm cluster (D-156).
#[derive(Debug, Clone)]
pub struct SwarmRollout {
    pub cluster: forge_core::ClusterId,
    pub plan: forge_catalog::swarm_rollout::SwarmPlan,
}

/// The plan as people read it.
#[derive(Debug, Clone, Serialize)]
pub struct PlanView {
    pub service: String,
    pub environment: String,
    pub creates_service: bool,
    pub release: i64,
    pub artifact: String,
    pub strategy: &'static str,
    pub slotted: bool,
    pub downtime: bool,
    pub nodes: Vec<String>,
    pub added_nodes: Vec<String>,
    pub removed_nodes: Vec<String>,
    pub steps: Vec<String>,
    pub notes: Vec<String>,
    pub plan: Plan,
}

impl Prepared {
    #[must_use]
    pub fn view(&self, environment: &str, next_number: i64) -> PlanView {
        let name = |s: &ServerId| self.names.get(&s.to_string()).cloned().unwrap_or_else(|| s.to_string());
        PlanView {
            service: self.app.name.clone(),
            environment: environment.to_owned(),
            creates_service: self.is_new,
            release: next_number,
            artifact: serde_json::from_value::<cat::ArtifactRef>(self.release.artifact.clone())
                .map(|a| a.label())
                .unwrap_or_default(),
            strategy: self.plan.strategy.as_str(),
            slotted: self.plan.slotted,
            downtime: self.plan.downtime,
            nodes: self.nodes.iter().map(name).collect(),
            added_nodes: self.added.iter().map(name).collect(),
            removed_nodes: self.removed.iter().map(name).collect(),
            steps: match &self.swarm {
                Some(sw) => sw.plan.steps.iter().map(forge_catalog::swarm_rollout::SwarmStep::describe).collect(),
                None => self.plan.steps.iter().map(|s| s.describe(&self.names)).collect(),
            },
            notes: self.plan.notes.clone(),
            plan: self.plan.clone(),
        }
    }
}

/// Who starts a deployment, and the authorizations the API already checked.
#[derive(Debug, Clone)]
pub struct Starter {
    pub principal: Principal,
    pub context: AuthzContext,
    /// Container volumes outside the app folder (need `stack.privileged`).
    pub privileged_mounts: Vec<String>,
}

#[derive(Debug)]
pub struct DeployService {
    pub apps: Arc<dyn ServiceRepository>,
    pub tenancy: Arc<dyn TenancyRepository>,
    pub servers: Arc<dyn ServerRepository>,
    pub tags: Arc<dyn TagRepository>,
    pub sites: Arc<dyn SiteRepository>,
    pub releases: Arc<dyn ReleaseRepository>,
    pub deploys: Arc<dyn DeploymentRepository>,
    /// Swarm clusters plan their rollouts differently (D-156).
    pub swarm: Arc<crate::swarm::SwarmService>,
    pub clock: Arc<dyn Clock>,
}

impl DeployService {
    /// Prepares a release of `doc` in workspace `ws`: the service (existing
    /// or new), the snapshot, the nodes and the plan. Writes nothing.
    pub async fn prepare(
        &self,
        ws: WorkspaceId,
        doc: &DeploymentDoc,
        source: &str,
        principal: &Principal,
    ) -> DomainResult<Prepared> {
        let env_slug = &doc.metadata.environment;
        let ns = self
            .tenancy
            .list_environments(ws, None)
            .await?
            .into_iter()
            .find(|n| crate::names::environment_slug(&n.name) == *env_slug)
            .ok_or_else(|| DomainError::NotFound(format!("environment `{env_slug}`")))?;
        let existing = self.apps.list_services(Some(ns.id)).await?.into_iter().find(|a| a.name == doc.metadata.name);
        let runtime = runtime_name(doc.spec.runtime);
        if let Some(a) = &existing
            && a.runtime != runtime
        {
            return Err(DomainError::Invalid(format!(
                "`{}` runs as a {}; a release cannot change it to a {runtime}",
                a.name, a.runtime
            )));
        }
        let now = self.clock.now();
        let app = existing.clone().unwrap_or_else(|| Service {
            id: forge_core::ServiceId::new(),
            workspace_id: ws,
            environment_id: ns.id,
            name: doc.metadata.name.clone(),
            runtime: runtime.to_owned(),
            spec: Value::Null,
            env: Vec::new(),
            created_by: principal.clone(),
            created_at: now,
            updated_at: now,
        });
        let artifact = doc.spec.artifact.resolve(doc.spec.runtime).map_err(|e| DomainError::Invalid(e.to_string()))?;
        let policy = doc.policy().map_err(|e| DomainError::Invalid(e.to_string()))?;
        let spec = release_spec(doc, &crate::names::environment_slug(&ns.name), &artifact, existing.as_ref())?;
        let env = release_env(doc, existing.as_ref())?;
        let release = Release {
            id: ReleaseId::new(),
            service_id: app.id,
            workspace_id: ws,
            number: 0,
            spec,
            env,
            artifact: serde_json::to_value(&artifact).map_err(DomainError::internal)?,
            policy: serde_json::to_value(&policy).map_err(DomainError::internal)?,
            source: Some(source.to_owned()),
            source_sha256: Some(forge_secrets::crypto::sha256_hex(source.as_bytes())),
            description: doc.metadata.description.clone(),
            created_by: principal.clone(),
            created_at: now,
        };
        crate::jobs::app::build_spec(&release_app(&app, &release), None).map_err(DomainError::Invalid)?;
        if let Some(prepared) = self.swarm_prepared(&app, existing.is_none(), &release, &policy).await? {
            return Ok(prepared);
        }
        let current: Vec<ServerId> = self.apps.list_instances(app.id).await?.iter().map(|i| i.server_id).collect();
        let nodes = match &doc.spec.placement {
            Some(p) => self.place(ws, p, &current).await?,
            None if current.is_empty() => {
                return Err(DomainError::Invalid(
                    "the service runs on no node yet: add spec.placement (nodes or nodeSelector)".into(),
                ));
            }
            None => current.clone(),
        };
        let added = nodes.iter().copied().filter(|n| !current.contains(n)).collect();
        let removed = current.iter().copied().filter(|n| !nodes.contains(n)).collect();
        let (plan, names) = self.plan(&app, &release, &policy, &nodes).await?;
        Ok(Prepared {
            app,
            is_new: existing.is_none(),
            release,
            policy,
            plan,
            nodes,
            added,
            removed,
            names,
            swarm: None,
        })
    }

    /// In a Docker Swarm cluster, the swarm places the tasks: the plan is a
    /// [`SwarmPlan`](forge_catalog::swarm_rollout::SwarmPlan) and no node is
    /// added or removed (D-156). `None` outside Swarm clusters.
    async fn swarm_prepared(
        &self,
        app: &Service,
        is_new: bool,
        release: &Release,
        policy: &RolloutPolicy,
    ) -> DomainResult<Option<Prepared>> {
        let Some(cluster) = self.swarm.swarm_of_environment(app.environment_id).await? else { return Ok(None) };
        let runtime: forge_catalog::Runtime =
            serde_json::from_value(release.spec["runtime"].clone()).map_err(DomainError::internal)?;
        if runtime != forge_catalog::Runtime::Container {
            return Err(DomainError::Invalid("Docker Swarm clusters run container services only".into()));
        }
        // No Swarm settings in the release: Swarm's defaults (one replica).
        let settings: forge_catalog::swarm_service::SwarmSettings = match &release.spec["swarm"] {
            Value::Null => forge_catalog::swarm_service::SwarmSettings::default(),
            v => serde_json::from_value(v.clone()).map_err(|e| DomainError::Invalid(format!("swarm settings: {e}")))?,
        };
        // The slot serving now: a weighted slot, else the unslotted service
        // once a release went live, else nothing yet.
        let slots = self.deploys.list_slots(app.id).await?;
        let weighted = slots.iter().filter(|s| s.weight > 0).max_by_key(|s| s.weight).map(|s| s.slot.clone());
        let serving = match weighted {
            Some(s) => Some(s),
            None if self.deploys.current_release(app.id).await? > 0 => Some(String::new()),
            None => None,
        };
        let routed = self.sites.list_sites().await?.iter().any(|s| site_targets(&s.config, app.id));
        let input = forge_catalog::swarm_rollout::SwarmPlanInput {
            policy,
            replicas: settings.replicas,
            serving: serving.as_deref(),
            routed,
        };
        let swarm_plan = forge_catalog::swarm_rollout::plan(&input).map_err(DomainError::Invalid)?;
        let plan = Plan {
            strategy: swarm_plan.strategy,
            slotted: !swarm_plan.slot.is_empty(),
            steps: Vec::new(),
            downtime: swarm_plan.downtime,
            notes: vec!["Docker Swarm places the tasks on the swarm's nodes".into()],
            targets: Vec::new(),
        };
        Ok(Some(Prepared {
            app: app.clone(),
            is_new,
            release: release.clone(),
            policy: policy.clone(),
            plan,
            nodes: Vec::new(),
            added: Vec::new(),
            removed: Vec::new(),
            names: BTreeMap::new(),
            swarm: Some(SwarmRollout { cluster, plan: swarm_plan }),
        }))
    }

    /// Prepares a redeploy of release `number` (rollback, or again) on the
    /// service's current nodes.
    pub async fn prepare_release(&self, app: &Service, number: i64) -> DomainResult<Prepared> {
        let release = self
            .releases
            .release_by_number(app.id, number)
            .await?
            .ok_or_else(|| DomainError::NotFound(format!("release {number}")))?;
        let policy: RolloutPolicy = serde_json::from_value(release.policy.clone()).map_err(DomainError::internal)?;
        if let Some(prepared) = self.swarm_prepared(app, false, &release, &policy).await? {
            return Ok(prepared);
        }
        let nodes: Vec<ServerId> = self.apps.list_instances(app.id).await?.iter().map(|i| i.server_id).collect();
        if nodes.is_empty() {
            return Err(DomainError::Invalid("the service runs on no node".into()));
        }
        let (plan, names) = self.plan(app, &release, &policy, &nodes).await?;
        Ok(Prepared {
            app: app.clone(),
            is_new: false,
            release,
            policy,
            plan,
            nodes,
            added: Vec::new(),
            removed: Vec::new(),
            names,
            swarm: None,
        })
    }

    /// The release a rollback goes to: `to`, or the one that served before
    /// the current one.
    pub async fn rollback_target(&self, app: &Service, to: Option<i64>) -> DomainResult<i64> {
        let current = self.deploys.current_release(app.id).await?;
        if let Some(n) = to {
            if n == current {
                return Err(DomainError::Invalid(format!("release {n} is already the current one")));
            }
            return Ok(n);
        }
        let older = self
            .deploys
            .list_deployments(app.id, MAX_LISTED)
            .await?
            .into_iter()
            .filter(|d| d.state == DeploymentState::Succeeded && d.release_number != current)
            .map(|d| d.release_number)
            .next();
        older.ok_or_else(|| DomainError::Invalid("there is no earlier release that ran".into()))
    }

    async fn plan(
        &self,
        app: &Service,
        release: &Release,
        policy: &RolloutPolicy,
        nodes: &[ServerId],
    ) -> DomainResult<(Plan, BTreeMap<String, String>)> {
        let slots = self.deploys.list_slots(app.id).await?;
        let instances = self.apps.list_instances(app.id).await?;
        let mut plan_nodes = Vec::with_capacity(nodes.len());
        let mut names = BTreeMap::new();
        for id in nodes {
            let server = self.servers.get_server(*id).await?;
            let name = server.display_name.clone().unwrap_or_else(|| server.hostname.clone());
            names.insert(id.to_string(), name.clone());
            let serving_now = match serving(&slots, *id) {
                Some(s) => Some(Slot::parse(&s.slot)),
                None => instances.iter().find(|i| i.server_id == *id && i.status != "pending").map(|_| None),
            };
            plan_nodes.push(PlanNode {
                server_id: id.to_string(),
                name,
                online: server.status == AgentStatus::Online,
                serving: serving_now,
            });
        }
        let spec = &release.spec;
        let runtime: forge_catalog::Runtime =
            serde_json::from_value(spec["runtime"].clone()).unwrap_or(forge_catalog::Runtime::Systemd);
        let has_ports = match runtime {
            forge_catalog::Runtime::Container => spec["ports"].as_array().is_some_and(|p| !p.is_empty()),
            _ => spec["listen_port"].as_u64().is_some(),
        };
        let routed = self.sites.list_sites().await?.iter().any(|s| site_targets(&s.config, app.id));
        let input = PlanInput { nodes: plan_nodes, policy: policy.clone(), runtime, routed, has_ports };
        let plan = cat::plan(&input).map_err(|e| DomainError::Invalid(e.0))?;
        Ok((plan, names))
    }

    /// Nodes for a `placement`: named nodes or every node of the
    /// workspace, filtered by tags, cut to `replicas` (nodes the service
    /// already runs on first, then online ones, then by name).
    async fn place(
        &self,
        ws: WorkspaceId,
        p: &cat::PlacementSpec,
        current: &[ServerId],
    ) -> DomainResult<Vec<ServerId>> {
        let servers: Vec<_> = self
            .servers
            .list_servers(Some(ws))
            .await?
            .into_iter()
            .filter(|s| s.status != AgentStatus::Revoked)
            .collect();
        let mut picked = Vec::new();
        if p.nodes.is_empty() {
            picked.extend(servers.iter());
        } else {
            for want in &p.nodes {
                let w = want.to_lowercase();
                let s = servers
                    .iter()
                    .find(|s| {
                        s.id.to_string() == w
                            || s.hostname.to_lowercase() == w
                            || s.display_name.as_deref().is_some_and(|d| d.to_lowercase() == w)
                    })
                    .ok_or_else(|| DomainError::Invalid(format!("spec.placement.nodes: no node `{want}`")))?;
                if !picked.iter().any(|x: &&forge_core::model::Server| x.id == s.id) {
                    picked.push(s);
                }
            }
        }
        if !p.node_selector.is_empty() {
            let tags = self.tags.tags_of_kind(TagKind::Server).await?;
            let wanted: Vec<String> = p.node_selector.iter().map(|(k, v)| format!("{k}:{v}").to_lowercase()).collect();
            picked.retain(|s| {
                let have: HashSet<String> =
                    tags.get(&s.id.to_string()).into_iter().flatten().map(|t| t.to_lowercase()).collect();
                wanted.iter().all(|w| have.contains(w))
            });
        }
        picked.sort_by_key(|s| {
            (
                !current.contains(&s.id),
                s.status != AgentStatus::Online,
                s.display_name.clone().unwrap_or_default(),
                s.hostname.clone(),
            )
        });
        if let Some(n) = p.replicas {
            picked.truncate(n as usize);
        }
        if picked.is_empty() {
            return Err(DomainError::Invalid("no node matches spec.placement".into()));
        }
        Ok(picked.into_iter().map(|s| s.id).collect())
    }

    /// Writes the release (and a new service and its placements) and starts
    /// the rollout. `Conflict` when the service has a deployment running.
    pub async fn start(
        &self,
        mut prepared: Prepared,
        kind: DeploymentKind,
        who: &Starter,
        jobs: &forge_jobs::JobEngine,
    ) -> DomainResult<Deployment> {
        let app = &prepared.app;
        if self.deploys.active_deployment(app.id).await?.is_some() {
            return Err(DomainError::Conflict("the service already has a deployment running".into()));
        }
        let now = self.clock.now();
        if prepared.is_new {
            let mut a = release_app(app, &prepared.release);
            a.updated_at = now;
            self.apps.put_service(&a).await?;
        }
        let swarm = prepared.swarm.clone();
        for server in &prepared.added {
            self.apps
                .put_instance(&ServiceInstance {
                    service_id: app.id,
                    server_id: *server,
                    role: "active".into(),
                    status: "pending".into(),
                    state: None,
                    last_job_id: None,
                    last_error: None,
                    updated_at: now,
                })
                .await?;
        }
        if prepared.release.number == 0 {
            prepared.release.number = match self.releases.create_release(&prepared.release).await {
                // Another release of the service took the number: once more.
                Err(DomainError::Conflict(_)) => self.releases.create_release(&prepared.release).await?,
                other => other?,
            };
        }
        let mut d = Deployment {
            id: DeploymentId::new(),
            service_id: app.id,
            workspace_id: app.workspace_id,
            release_id: prepared.release.id,
            release_number: prepared.release.number,
            from_release: self.deploys.current_release(app.id).await?,
            kind,
            strategy: prepared.plan.strategy.as_str().to_owned(),
            state: DeploymentState::Pending,
            request: None,
            job_id: None,
            plan: match &swarm {
                Some(sw) => serde_json::to_value(&sw.plan),
                None => serde_json::to_value(&prepared.plan),
            }
            .map_err(DomainError::internal)?,
            step: 0,
            error: None,
            created_by: who.principal.clone(),
            created_at: now,
            updated_at: now,
            finished_at: None,
        };
        self.deploys.create_deployment(&d).await?;
        let privileged = !who.privileged_mounts.is_empty();
        let job = jobs
            .submit(NewJob {
                kind: ROLLOUT_KIND.into(),
                workspace_id: Some(app.workspace_id),
                // Privileged mounts need `stack.privileged`, held per environment.
                resource: if privileged {
                    EntityRef::new(EntityKind::Environment, app.environment_id)
                } else {
                    svc_ref(app.id)
                },
                principal: who.principal.clone(),
                context: who.context,
                idempotency_key: None,
                payload: json!({
                    "deployment_id": d.id,
                    "privileged_mounts": who.privileged_mounts,
                    "remove": prepared.removed,
                    "names": prepared.names,
                    "swarm_cluster": swarm.as_ref().map(|sw| sw.cluster),
                }),
                max_attempts: 3,
            })
            .await;
        match job {
            Ok(j) => {
                d.job_id = Some(j.id);
                d.updated_at = self.clock.now();
                self.deploys.put_deployment(&d).await?;
                Ok(d)
            }
            Err(e) => {
                // Not started: free the service again.
                d.state = DeploymentState::Failed;
                d.error = Some(e.to_string());
                d.finished_at = Some(self.clock.now());
                self.deploys.put_deployment(&d).await?;
                Err(e)
            }
        }
    }
}

fn runtime_name(r: forge_catalog::Runtime) -> &'static str {
    match r {
        forge_catalog::Runtime::Systemd => "systemd",
        forge_catalog::Runtime::Container => "container",
    }
}

/// The service spec a release carries: the template's fields, the runtime,
/// the environment's slug, the artifact's image and the health checks (the
/// document's, else the service's own).
fn release_spec(
    doc: &DeploymentDoc,
    ns_slug: &str,
    artifact: &cat::ArtifactRef,
    existing: Option<&Service>,
) -> DomainResult<Value> {
    let mut m = match cat::snake_keys(Value::Object(doc.spec.template.clone())) {
        Value::Object(m) => m,
        _ => serde_json::Map::new(),
    };
    m.insert("environment".into(), json!(ns_slug));
    m.insert("runtime".into(), json!(runtime_name(doc.spec.runtime)));
    if let Some(image) = artifact.image_ref() {
        m.insert("image".into(), json!(image));
    }
    let health = match &doc.spec.health.checks {
        Some(c) => Some(cat::snake_keys(c.clone())),
        None => existing.and_then(|a| a.spec.get("health").cloned()).filter(|h| !h.is_null()),
    };
    if let Some(h) = health {
        let policy: forge_catalog::health::HealthPolicy =
            serde_json::from_value(h.clone()).map_err(|e| DomainError::Invalid(format!("spec.health.checks: {e}")))?;
        if let Some(e) = policy.validate().into_iter().next() {
            return Err(DomainError::Invalid(format!("spec.health.checks.{}: {}", e.field, e.message)));
        }
        m.insert("health".into(), h);
    }
    Ok(Value::Object(m))
}

/// The environment a release carries: the document's plain variables and
/// the service's sealed values of the secrets it names.
fn release_env(doc: &DeploymentDoc, existing: Option<&Service>) -> DomainResult<Vec<EnvVar>> {
    let mut env: Vec<EnvVar> = doc
        .spec
        .env
        .iter()
        .map(|(k, v)| EnvVar { key: k.clone(), value: Some(v.clone()), secret: false, sealed: None })
        .collect();
    for key in &doc.spec.secrets {
        let found = existing
            .and_then(|a| a.env.iter().find(|e| e.key == *key && e.secret && e.sealed.is_some()))
            .ok_or_else(|| {
                DomainError::Invalid(format!(
                    "secret `{key}` is not set on the service: set it in the console (Service → Environment) first"
                ))
            })?;
        env.push(found.clone());
    }
    Ok(env)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn the_serving_copy_has_the_most_traffic() {
        let app = forge_core::ServiceId::new();
        let n = ServerId::new();
        let row = |slot: &str, weight: u32, state: &str| SlotRecord {
            service_id: app,
            server_id: n,
            slot: slot.into(),
            release_number: 1,
            weight,
            state: state.into(),
            ports: json!({ "8080": 21000 }),
            updated_at: Timestamp(0),
        };
        let slots = vec![row("a", 0, "ready"), row("b", 100, "ready")];
        assert_eq!(serving(&slots, n).unwrap().slot, "b");
        assert!(serving(&slots, ServerId::new()).is_none());
        assert_eq!(port_map(&slots[0]), BTreeMap::from([(8080, 21000)]));
    }

    #[test]
    fn yaml_documents_are_bounded() {
        assert!(parse_yaml("").is_err());
        assert!(parse_yaml(&"a: 1\n".repeat(cat::MAX_DOC_BYTES)).is_err());
        let v = parse_yaml("apiVersion: forge/v1\nkind: Deployment\nflag: yes\n").unwrap();
        assert_eq!(v["flag"], "yes", "YAML 1.2: yes is a string");
    }

    #[test]
    fn sites_route_to_a_service_through_typed_targets() {
        let app = forge_core::ServiceId::new();
        let cfg = json!({ "targets": [{ "kind": "app", "service_id": app.to_string(), "port": 8080 }] });
        assert!(site_targets(&cfg, app));
        assert!(!site_targets(&cfg, forge_core::ServiceId::new()));
        assert!(!site_targets(&json!({}), app));
    }
}
