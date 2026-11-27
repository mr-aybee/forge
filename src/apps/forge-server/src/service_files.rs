//! Services files (D-178): a Compose / Swarm format file run by Forge.
//!
//! [`ServiceFiles::plan`] reads the file
//! ([`forge_catalog::compose_native`]), compares it with the
//! environment's services and places every service with
//! [`forge_catalog::placement::select`]. It writes nothing. The result says
//! what would be created, changed, moved and removed, and what the caller
//! must be allowed to do: `server.manage` on every node a service is added
//! to and `stack.privileged` for privileged settings or mounts. The API
//! handler authorizes those, then [`ServiceFiles::apply`] carries the plan
//! out through the same paths as the services API: the stored spec from
//! [`crate::service_spec::apply_input`], optimistic `version` checks,
//! instance rows and `app.action` jobs submitted as the caller.
//!
//! Each service records the file it came from in `services.source` (the
//! file's top-level `name`, else [`DEFAULT_SOURCE`]). An apply removes the
//! services of that source the file no longer lists, and never touches
//! services made in the console or another file. A removed service with
//! deployed instances gets `remove` jobs and the source
//! [`REMOVING_SOURCE`]; the reconciler deletes it once its instances are
//! gone ([`ServiceFiles::finish_removals`]).
//!
//! Writes are not one transaction: each service is written, then its
//! instances, then its jobs, in dependency order. A failure stops the
//! apply; what was written stays, and applying the file again finishes the
//! rest (every step is idempotent: unchanged services are left alone and
//! job submissions carry idempotency keys).

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use forge_catalog::compose_native::{self, FileService, Orchestration, SPEC_KEY};
use forge_catalog::placement::{Candidate, PlaceError, select};
use forge_catalog::swarm_service::ServiceMode;
use forge_core::files::FileRepository;
use forge_core::model::{AgentStatus, Environment, Job, Server, Service, ServiceInstance};
use forge_core::ports::{ServerRepository, ServiceRepository, TenancyRepository};
use forge_core::tags::{FactsRepository, TagKind, TagRepository};
use forge_core::{AuthzContext, Clock, DomainError, DomainResult, EnvironmentId, Principal, ServerId, ServiceId};
use forge_jobs::JobEngine;
use forge_secrets::Envelope;
use serde::Serialize;
use serde_json::Value;

use crate::service_jobs::{AppActionRequest, app_action_job};
use crate::service_spec::{AppInput, EnvInput, apply_input};

/// The source of services from a file without a top-level `name`.
pub const DEFAULT_SOURCE: &str = "default";
/// The source of a removed service whose instances are still being removed.
pub const REMOVING_SOURCE: &str = "-removing";
/// Audit: a services file was planned.
pub const AUDIT_PLAN: &str = "services.plan";
/// Audit: a services file was applied.
pub const AUDIT_APPLY: &str = "services.apply";
/// Audit: a service's node count changed.
pub const AUDIT_SCALE: &str = "service.scale";
/// Audit: a removed service was deleted after its instances were gone.
pub const AUDIT_REMOVED: &str = "service.delete";

/// What happens to one service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Create,
    /// The spec (or the file that manages it) changes.
    Update,
    /// Only its nodes change.
    Move,
    Remove,
    Unchanged,
}

/// One service in a plan, as people read it.
#[derive(Debug, Clone, Serialize)]
pub struct ServiceChange {
    pub service: String,
    pub action: ChangeKind,
    /// Spec fields that change (`environment variables` for the variables).
    pub fields: Vec<String>,
    /// Nodes it runs on afterwards.
    pub nodes: Vec<String>,
    pub added_nodes: Vec<String>,
    pub removed_nodes: Vec<String>,
    /// Settings and mounts that need `stack.privileged`.
    pub privileged: Vec<String>,
    /// It is deployed (again) on these nodes.
    pub deploys: Vec<String>,
}

/// What [`ServiceFiles::plan`] found.
#[derive(Debug, Clone, Serialize)]
pub struct ServicesPlan {
    pub environment: String,
    pub source: String,
    pub changes: Vec<ServiceChange>,
    /// Nodes a service is added to: the caller needs `server.manage` there.
    #[serde(skip)]
    pub added_nodes: Vec<ServerId>,
    /// The caller needs `stack.privileged` on the environment.
    #[serde(skip)]
    pub needs_privileged: bool,
    /// Credential vault items the plan newly mounts as files: the caller
    /// needs the vault's use permission for each (D-179).
    #[serde(skip)]
    pub vault_items: Vec<String>,
    #[serde(skip)]
    pub environment_id: EnvironmentId,
    #[serde(skip)]
    pub workspace_id: forge_core::WorkspaceId,
    #[serde(skip)]
    work: Vec<Work>,
}

impl ServicesPlan {
    /// Whether applying it changes nothing.
    #[must_use]
    pub fn is_noop(&self) -> bool {
        self.changes.iter().all(|c| c.action == ChangeKind::Unchanged)
    }
}

/// One service's writes.
#[derive(Debug, Clone)]
enum Work {
    Upsert(Box<Upsert>),
    Remove { app: Box<Service>, instances: Vec<ServiceInstance> },
}

#[derive(Debug, Clone)]
struct Upsert {
    app: Service,
    is_new: bool,
    /// The stored version the plan was made from.
    version: i64,
    save: bool,
    added: Vec<ServerId>,
    removed: Vec<ServiceInstance>,
    deploy_on: Vec<ServerId>,
    /// Privileged mounts per node the service is deployed on.
    mounts: HashMap<ServerId, Vec<String>>,
}

/// A services file to plan or apply.
#[derive(Debug, Clone, Copy)]
pub struct FileRequest<'a> {
    pub environment: EnvironmentId,
    /// The file's YAML.
    pub text: &'a str,
    /// Remove services of this file's source the file no longer lists.
    pub prune: bool,
}

/// Who applies a plan.
#[derive(Debug, Clone)]
pub struct Applier {
    pub principal: Principal,
    pub context: AuthzContext,
}

/// The jobs an apply submitted.
#[derive(Debug, Clone, Serialize)]
pub struct Applied {
    #[serde(flatten)]
    pub plan: ServicesPlan,
    pub jobs: Vec<Job>,
}

/// Plans and applies services files (see the module docs).
#[derive(Debug)]
pub struct ServiceFiles {
    pub services: Arc<dyn ServiceRepository>,
    pub servers: Arc<dyn ServerRepository>,
    pub tenancy: Arc<dyn TenancyRepository>,
    pub tags: Arc<dyn TagRepository>,
    pub facts: Arc<dyn FactsRepository>,
    /// Approved file roots per node: which volumes need `stack.privileged`.
    pub files: Arc<dyn FileRepository>,
    /// Live node totals (memory, CPUs) for reservations.
    pub telemetry: Arc<crate::metrics::Telemetry>,
    /// Swarm clusters run their services through Swarm, not files.
    pub swarm: Arc<crate::swarm::SwarmService>,
    pub envelope: Envelope,
    pub clock: Arc<dyn Clock>,
    pub jobs: Arc<JobEngine>,
}

/// The environment and its nodes, as a plan sees them.
struct Scene {
    env: Environment,
    slug: String,
    servers: Vec<Server>,
    tags: HashMap<String, Vec<String>>,
    capacity: HashMap<ServerId, (Option<u64>, Option<u64>)>,
    services: Vec<Service>,
    instances: Vec<ServiceInstance>,
}

impl ServiceFiles {
    /// Plans a services file for `by` (who would create its new
    /// services). Writes nothing.
    pub async fn plan(&self, req: FileRequest<'_>, by: &Principal) -> DomainResult<ServicesPlan> {
        let tree = forge_catalog::yaml::parse_document(req.text).map_err(DomainError::Invalid)?;
        let file = compose_native::parse(&tree).map_err(|e| DomainError::Invalid(e.to_string()))?;
        let source = file.name.clone().unwrap_or_else(|| DEFAULT_SOURCE.to_owned());
        let scene = self.scene(req.environment).await?;
        let mut plan = empty_plan(&scene, &source);

        for fs in &file.services {
            let existing = scene.services.iter().find(|s| s.name == fs.name);

            if let Some(e) = existing {
                owned_by(e, &source)?;
            }

            let input = input_from(fs, existing);
            let mut app = existing.cloned().unwrap_or_else(|| new_service(&scene.env, &fs.name, by, self.clock.now()));
            apply_input(&self.envelope, self.clock.now(), &mut app, &scene.slug, input)
                .map_err(|e| prefixed(&format!("services.{}", fs.name), e))?;
            source.clone_into(&mut app.source);
            self.add(&mut plan, &scene, app, existing, &fs.orchestration).await?;
        }

        if req.prune {
            for gone in
                scene.services.iter().filter(|s| s.source == source && !file.services.iter().any(|f| f.name == s.name))
            {
                let instances: Vec<ServiceInstance> =
                    scene.instances.iter().filter(|i| i.service_id == gone.id).cloned().collect();
                plan.changes.push(ServiceChange {
                    service: gone.name.clone(),
                    action: ChangeKind::Remove,
                    fields: Vec::new(),
                    nodes: Vec::new(),
                    added_nodes: Vec::new(),
                    removed_nodes: instances.iter().map(|i| node_name(&scene, i.server_id)).collect(),
                    privileged: Vec::new(),
                    deploys: Vec::new(),
                });
                plan.work.push(Work::Remove { app: Box::new(gone.clone()), instances });
            }
        }

        Ok(plan)
    }

    /// Plans a new node count for service `id` (replicated services only).
    pub async fn plan_scale(&self, id: ServiceId, replicas: u32) -> DomainResult<ServicesPlan> {
        if !(1..=1000).contains(&replicas) {
            return Err(DomainError::Invalid("replicas: 1 to 1000 nodes".into()));
        }

        let app =
            self.services.get_service(id).await?.ok_or_else(|| DomainError::NotFound("service not found".into()))?;
        let scene = self.scene(app.environment_id).await?;
        let existing = scene
            .services
            .iter()
            .find(|s| s.id == id)
            .ok_or_else(|| DomainError::NotFound("service not found".into()))?;
        owned_by(existing, &existing.source)?;

        let mut orch: Orchestration = match existing.spec.get(SPEC_KEY).filter(|v| !v.is_null()) {
            Some(v) => serde_json::from_value(v.clone())
                .map_err(|e| DomainError::Invalid(format!("stored orchestration: {e}")))?,
            None => Orchestration::default(),
        };

        if orch.mode == ServiceMode::Global {
            return Err(DomainError::Invalid(
                "a global service runs on every eligible node; it has no node count".into(),
            ));
        }

        orch.replicas = replicas;
        let mut desired = existing.clone();
        desired.spec[SPEC_KEY] = serde_json::to_value(&orch).map_err(DomainError::internal)?;
        desired.updated_at = self.clock.now();
        let mut plan = empty_plan(&scene, &existing.source);
        self.add(&mut plan, &scene, desired, Some(existing), &orch).await?;

        Ok(plan)
    }

    /// Carries out `plan` as `who`. The caller has authorized `deploy` on
    /// the environment, `server.manage` on [`ServicesPlan::added_nodes`]
    /// and `stack.privileged` when [`ServicesPlan::needs_privileged`].
    pub async fn apply(&self, plan: ServicesPlan, who: &Applier) -> DomainResult<Applied> {
        let mut jobs = Vec::new();

        for w in &plan.work {
            match w {
                Work::Upsert(u) => self.upsert(u, who, &mut jobs).await?,
                Work::Remove { app, instances } => self.remove(app, instances, who, &mut jobs).await?,
            }
        }

        Ok(Applied { plan, jobs })
    }

    /// The services file of one service.
    pub async fn export_service(&self, id: ServiceId) -> DomainResult<(Service, String)> {
        let app =
            self.services.get_service(id).await?.ok_or_else(|| DomainError::NotFound("service not found".into()))?;
        let entry = file_entry(&app)?;
        let name = (!app.source.is_empty() && app.source != DEFAULT_SOURCE).then_some(app.source.as_str());

        Ok((app.clone(), compose_native::export(name, &[entry])))
    }

    /// The services file of every container and native service of an
    /// environment (stack and Swarm services are left out).
    pub async fn export_environment(&self, env: EnvironmentId) -> DomainResult<String> {
        let services = self.services.list_services(Some(env)).await?;
        let mut entries = Vec::new();

        for s in services.iter().filter(|s| matches!(s.runtime.as_str(), "container" | "systemd") && !swarm_managed(s))
        {
            entries.push(file_entry(s)?);
        }

        Ok(compose_native::export(None, &entries))
    }

    async fn scene(&self, environment: EnvironmentId) -> DomainResult<Scene> {
        let env = self
            .tenancy
            .get_environment(environment)
            .await
            .map_err(|_| DomainError::NotFound("environment not found".into()))?;

        if self.swarm.swarm_of_environment(env.id).await?.is_some() {
            return Err(DomainError::Invalid(
                "this environment runs on a Docker Swarm cluster: use a swarm stack instead of a services file".into(),
            ));
        }

        let servers: Vec<Server> = self
            .servers
            .list_servers(Some(env.workspace_id))
            .await?
            .into_iter()
            .filter(|s| {
                s.status != AgentStatus::Revoked && (env.cluster_id.is_none() || s.cluster_id == env.cluster_id)
            })
            .collect();
        let tags = self.tags.tags_of_kind(TagKind::Server).await?;
        let facts = self.facts.all_facts().await?;
        let mut capacity = HashMap::new();

        for s in &servers {
            let sample = self.telemetry.latest(s.id);
            let reported = facts.get(&s.id).and_then(|(f, _)| f.get("cpus")).and_then(Value::as_u64);
            let cpus = sample.map(|x| u64::from(x.cpus)).filter(|n| *n > 0).or(reported);
            let memory = sample.map(|x| x.mem_total / (1024 * 1024)).filter(|n| *n > 0);
            capacity.insert(s.id, (cpus.map(|c| c * 1000), memory));
        }

        Ok(Scene {
            slug: crate::names::environment_slug(&env.name),
            services: self.services.list_services(Some(env.id)).await?,
            instances: self.services.list_all_instances(Some(env.id)).await?,
            env,
            servers,
            tags,
            capacity,
        })
    }

    /// Places `app` (its desired state) and adds it to `plan`.
    async fn add(
        &self,
        plan: &mut ServicesPlan,
        scene: &Scene,
        app: Service,
        existing: Option<&Service>,
        orch: &Orchestration,
    ) -> DomainResult<()> {
        let current: Vec<&ServiceInstance> =
            scene.instances.iter().filter(|i| existing.is_some_and(|e| e.id == i.service_id)).collect();
        let current_ids: Vec<ServerId> = current.iter().map(|i| i.server_id).collect();
        let nodes = place(scene, &app.name, orch, &current_ids)?;
        let added: Vec<ServerId> = nodes.iter().copied().filter(|n| !current_ids.contains(n)).collect();
        let removed: Vec<ServiceInstance> =
            current.iter().filter(|i| !nodes.contains(&i.server_id)).map(|i| (*i).clone()).collect();

        let fields = existing.map(|e| changed_fields(e, &app)).unwrap_or_default();
        let runtime_changed = fields.iter().any(|f| f != SPEC_KEY && f != "source");
        let save = existing.is_none() || !fields.is_empty();
        let deploy_on: Vec<ServerId> =
            if existing.is_none() || runtime_changed { nodes.clone() } else { added.clone() };

        let mut mounts = HashMap::new();

        if !deploy_on.is_empty() {
            let spec = crate::jobs::app::build_spec(&app, None).map_err(DomainError::Invalid)?;

            for n in &deploy_on {
                let roots = crate::stacks::bind_roots(self.files.as_ref(), *n).await;
                mounts.insert(*n, crate::jobs::app::privileged_mounts(&spec, &roots));
            }
        }

        let mut privileged: Vec<String> = mounts.values().flatten().cloned().collect();
        privileged.sort();
        privileged.dedup();

        let action = match () {
            () if existing.is_none() => ChangeKind::Create,
            () if save => ChangeKind::Update,
            () if !added.is_empty() || !removed.is_empty() => ChangeKind::Move,
            () => ChangeKind::Unchanged,
        };

        let refs = crate::service_spec::stored_refs(Some(&app.spec));
        plan.vault_items.extend(crate::service_spec::new_vault_items(existing.map(|e| &e.spec), &refs));
        plan.needs_privileged |= !privileged.is_empty();
        plan.added_nodes.extend(added.iter().copied());
        plan.changes.push(ServiceChange {
            service: app.name.clone(),
            action,
            fields,
            nodes: nodes.iter().map(|n| node_name(scene, *n)).collect(),
            added_nodes: added.iter().map(|n| node_name(scene, *n)).collect(),
            removed_nodes: removed.iter().map(|i| node_name(scene, i.server_id)).collect(),
            privileged,
            deploys: deploy_on.iter().map(|n| node_name(scene, *n)).collect(),
        });
        plan.work.push(Work::Upsert(Box::new(Upsert {
            version: existing.map_or(0, |e| e.version),
            is_new: existing.is_none(),
            app,
            save,
            added,
            removed,
            deploy_on,
            mounts,
        })));

        Ok(())
    }

    async fn upsert(&self, u: &Upsert, who: &Applier, jobs: &mut Vec<Job>) -> DomainResult<()> {
        let app = &u.app;

        if u.is_new {
            if self.services.list_services(Some(app.environment_id)).await?.iter().any(|s| s.name == app.name) {
                return Err(DomainError::Conflict(format!(
                    "`{}` was created while the file was applied; plan again",
                    app.name
                )));
            }

            self.services.put_service(app).await?;
        } else if u.save && !self.services.update_service_if(app, u.version).await? {
            return Err(DomainError::Conflict(format!("`{}` changed since the plan was made; plan again", app.name)));
        }

        let now = self.clock.now();

        for server in &u.added {
            self.services.put_instance(&pending_instance(app.id, *server, now)).await?;
        }

        // Jobs are keyed by the service's state and node, so a retried
        // apply does not submit the same deploy twice.
        let key = |action: &str, server: ServerId| {
            format!("services-apply:{}:{}:{action}:{server}", app.id, app.updated_at.0)
        };

        for server in &u.deploy_on {
            let mounts = u.mounts.get(server).cloned().unwrap_or_default();
            let job = app_action_job(AppActionRequest {
                app,
                server: *server,
                action: "deploy",
                tail: 0,
                cursor: "",
                privileged_mounts: &mounts,
                principal: who.principal.clone(),
                context: who.context,
                idempotency_key: Some(key("deploy", *server)),
            });
            jobs.push(self.jobs.submit(job).await?);
        }

        for i in &u.removed {
            self.take_off(app, i, who, jobs, &key("remove", i.server_id)).await?;
        }

        Ok(())
    }

    async fn remove(
        &self,
        app: &Service,
        instances: &[ServiceInstance],
        who: &Applier,
        jobs: &mut Vec<Job>,
    ) -> DomainResult<()> {
        let mut busy = false;

        for i in instances {
            busy |= i.status != "pending";
            self.take_off(app, i, who, jobs, &format!("services-apply:{}:remove:{}", app.id, i.server_id)).await?;
        }

        if !busy {
            return self.services.delete_service(app.id).await;
        }

        // Deleted by the reconciler once the remove jobs took the
        // instances off their nodes.
        let mut marked = app.clone();
        REMOVING_SOURCE.clone_into(&mut marked.source);
        marked.updated_at = self.clock.now();

        if !self.services.update_service_if(&marked, app.version).await? {
            return Err(DomainError::Conflict(format!("`{}` changed since the plan was made; plan again", app.name)));
        }

        Ok(())
    }

    /// Takes `app` off one node: a never-deployed placement is dropped, a
    /// deployed one gets a `remove` job (which deletes the row when done).
    async fn take_off(
        &self,
        app: &Service,
        i: &ServiceInstance,
        who: &Applier,
        jobs: &mut Vec<Job>,
        key: &str,
    ) -> DomainResult<()> {
        if i.status == "pending" {
            return self.services.delete_instance(app.id, i.server_id).await;
        }

        let job = app_action_job(AppActionRequest {
            app,
            server: i.server_id,
            action: "remove",
            tail: 0,
            cursor: "",
            privileged_mounts: &[],
            principal: who.principal.clone(),
            context: who.context,
            idempotency_key: Some(key.to_owned()),
        });
        jobs.push(self.jobs.submit(job).await?);

        Ok(())
    }
}

fn empty_plan(scene: &Scene, source: &str) -> ServicesPlan {
    ServicesPlan {
        environment: scene.env.name.clone(),
        source: source.to_owned(),
        changes: Vec::new(),
        added_nodes: Vec::new(),
        needs_privileged: false,
        vault_items: Vec::new(),
        environment_id: scene.env.id,
        workspace_id: scene.env.workspace_id,
        work: Vec::new(),
    }
}

/// Refuses a service another file, a stack or Swarm manages.
fn owned_by(existing: &Service, source: &str) -> DomainResult<()> {
    if existing.runtime == forge_catalog::stacks::STACK_RUNTIME {
        return Err(DomainError::Conflict(format!(
            "`{}` belongs to a compose stack; change it from the stack",
            existing.name
        )));
    }

    if swarm_managed(existing) {
        return Err(DomainError::Conflict(format!("`{}` runs on Docker Swarm", existing.name)));
    }

    if existing.source == REMOVING_SOURCE {
        return Err(DomainError::Conflict(format!(
            "`{}` is being removed; apply again once it is gone",
            existing.name
        )));
    }

    if !existing.source.is_empty() && existing.source != source {
        return Err(DomainError::Conflict(format!(
            "`{}` is managed by the services file `{}`; change it there",
            existing.name, existing.source
        )));
    }

    Ok(())
}

fn swarm_managed(app: &Service) -> bool {
    app.spec.get("swarm").is_some_and(|v| !v.is_null())
}

fn prefixed(path: &str, e: DomainError) -> DomainError {
    match e {
        DomainError::Invalid(m) => DomainError::Invalid(format!("{path}: {m}")),
        other => other,
    }
}

fn new_service(env: &Environment, name: &str, by: &Principal, now: forge_core::Timestamp) -> Service {
    Service {
        id: ServiceId::new(),
        workspace_id: env.workspace_id,
        environment_id: env.id,
        name: name.to_owned(),
        runtime: String::new(),
        spec: Value::Null,
        env: Vec::new(),
        created_by: by.clone(),
        created_at: now,
        updated_at: now,
        version: 0,
        source: String::new(),
        secrets_changed_at: None,
    }
}

fn pending_instance(service: ServiceId, server: ServerId, now: forge_core::Timestamp) -> ServiceInstance {
    ServiceInstance {
        service_id: service,
        server_id: server,
        role: "active".into(),
        status: "pending".into(),
        state: None,
        last_job_id: None,
        last_error: None,
        updated_at: now,
        desired: forge_core::model::InstanceDesired::Running,
        deployed_release: None,
        exit_code: None,
    }
}

/// The service's editable fields from a file entry. Secret variables the
/// service already has stay (a file never carries secret values, only
/// vault item names for secret files); everything
/// a file does not set (runtimes, package env files, log size) keeps the
/// stored value.
fn input_from(fs: &FileService, existing: Option<&Service>) -> AppInput {
    let s = &fs.spec;
    let mut env: Vec<EnvInput> =
        fs.env.iter().map(|(k, v)| EnvInput { key: k.clone(), value: Some(v.clone()), secret: false }).collect();

    for e in existing.map(|a| a.env.as_slice()).unwrap_or_default() {
        if e.secret && !fs.env.iter().any(|(k, _)| *k == e.key) {
            env.push(EnvInput { key: e.key.clone(), value: None, secret: true });
        }
    }

    let stored = |k: &str| existing.and_then(|a| a.spec.get(k)).cloned().unwrap_or(Value::Null);

    AppInput {
        environment_id: None,
        name: Some(fs.name.clone()),
        runtime: match fs.runtime {
            forge_catalog::Runtime::Container => "container".into(),
            forge_catalog::Runtime::Systemd => "systemd".into(),
        },
        command: s.command.clone(),
        working_dir: s.working_dir.clone(),
        listen_port: s.listen_port,
        image: s.image.clone(),
        ports: s.ports.clone(),
        volumes: s.volumes.clone(),
        // A stored value that does not read back is dropped: the file is
        // the source of truth for everything else.
        env_files: serde_json::from_value(stored("env_files")).unwrap_or_default(),
        restart: s.restart,
        env,
        dependencies: serde_json::from_value(stored("dependencies")).unwrap_or_default(),
        cpu_millis: s.cpu_millis,
        memory_mb: s.memory_mb,
        entrypoint: s.entrypoint.clone(),
        user: s.user.clone(),
        hostname: s.hostname.clone(),
        labels: s.labels.clone(),
        extra_hosts: s.extra_hosts.clone(),
        healthcheck: s.healthcheck.clone(),
        stop_timeout_secs: s.stop_timeout_secs,
        read_only: s.read_only,
        log_max_mb: stored("log_max_mb").as_u64().and_then(|n| u32::try_from(n).ok()).unwrap_or(0),
        swarm: None,
        mesh: fs.mesh,
        on_node_failure: fs.on_node_failure,
        options: s.options.clone(),
        orchestration: Some(fs.orchestration.clone()),
        secret_files: s.secret_files.clone(),
        version: None,
    }
}

/// The spec keys (and `environment variables`, `runtime`, `source`) that
/// differ between the stored service and its desired state.
fn changed_fields(old: &Service, new: &Service) -> Vec<String> {
    let mut out = Vec::new();

    if old.runtime != new.runtime {
        out.push("runtime".to_owned());
    }

    let empty = serde_json::Map::new();
    let a = old.spec.as_object().unwrap_or(&empty);
    let b = new.spec.as_object().unwrap_or(&empty);
    let keys: std::collections::BTreeSet<&String> = a.keys().chain(b.keys()).collect();

    for k in keys {
        let x = a.get(k).filter(|v| !is_blank(v));
        let y = b.get(k).filter(|v| !is_blank(v));

        if x != y {
            out.push(k.clone());
        }
    }

    let vars = |s: &Service| {
        s.env.iter().map(|e| (e.key.clone(), e.value.clone(), e.secret, e.sealed.clone())).collect::<Vec<_>>()
    };

    if vars(old) != vars(new) {
        out.push("environment variables".to_owned());
    }

    if old.source != new.source {
        out.push("source".to_owned());
    }

    out
}

/// Absent and empty compare equal (an older stored spec may lack a key).
fn is_blank(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
        Value::String(s) => s.is_empty(),
        Value::Bool(b) => !b,
        Value::Number(n) => n.as_u64() == Some(0),
    }
}

/// The nodes for one service, or `Invalid` naming why there are none.
fn place(scene: &Scene, name: &str, orch: &Orchestration, current: &[ServerId]) -> DomainResult<Vec<ServerId>> {
    let candidates: Vec<Candidate> = scene
        .servers
        .iter()
        .filter(|s| crate::app_roots::supported(s))
        .map(|s| crate::deploy::candidate(s, &scene.tags, current, scene.capacity.get(&s.id).copied()))
        .collect();
    let rules = orch.rules();

    let picked = select(&candidates, &rules).map_err(|e| match e {
        PlaceError::UnknownNode(n) => DomainError::Invalid(format!(
            "services.{name}.deploy.placement.constraints: no node `{n}` in the environment's cluster"
        )),
        PlaceError::NoneMatch => DomainError::Invalid(format!(
            "services.{name}: no eligible node (in the environment's cluster, not drained, matching the constraints and reservations)"
        )),
    })?;

    if orch.mode == ServiceMode::Replicated && picked.len() < orch.replicas as usize {
        return Err(DomainError::Invalid(format!(
            "services.{name}.deploy.replicas: {} nodes wanted, {} eligible (one copy runs per node)",
            orch.replicas,
            picked.len()
        )));
    }

    picked.iter().map(|id| id.parse().map_err(|_| DomainError::internal("placement returned an unknown id"))).collect()
}

fn node_name(scene: &Scene, id: ServerId) -> String {
    scene
        .servers
        .iter()
        .find(|s| s.id == id)
        .map_or_else(|| id.to_string(), |s| s.display_name.clone().unwrap_or_else(|| s.hostname.clone()))
}

/// `app` as a file entry: its plain variables, never its secrets.
fn file_entry(app: &Service) -> DomainResult<FileService> {
    let env: Vec<(String, String)> =
        app.env.iter().filter(|e| !e.secret).map(|e| (e.key.clone(), e.value.clone().unwrap_or_default())).collect();

    compose_native::from_stored(&app.name, &app.runtime, &app.spec, env).map_err(DomainError::Invalid)
}

/// Deletes removed services whose instances are gone (the reconciler's
/// tick calls it). Returns the deleted services.
pub async fn finish_removals(services: &dyn ServiceRepository) -> DomainResult<Vec<Service>> {
    let mut done = Vec::new();

    for s in services.list_services(None).await?.into_iter().filter(|s| s.source == REMOVING_SOURCE) {
        if services.list_instances(s.id).await?.is_empty() {
            services.delete_service(s.id).await?;
            done.push(s);
        }
    }

    Ok(done)
}

/// How many services each kind of change touches (audit details).
#[must_use]
pub fn counts(plan: &ServicesPlan) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();

    for c in &plan.changes {
        *counts.entry(format!("{:?}", c.action).to_lowercase()).or_insert(0) += 1;
    }

    counts
}
