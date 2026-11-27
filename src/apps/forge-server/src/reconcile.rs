//! The service reconciler (D-177): keeps native services in their desired
//! state.
//!
//! * Agents push the state of each native service container
//!   ([`ServiceReconciler::observe_node`]); a container that runs another
//!   release than the last deploy marks its instance `drifted`.
//! * Every [`TICK`] the reconciler restarts exited containers (and
//!   redeploys missing ones) on online nodes, at most once per
//!   [`RETRY_GAP_MS`] and [`MAX_ATTEMPTS`] times per [`ATTEMPT_WINDOW_MS`];
//!   then it gives up and marks the instance `failed`.
//! * An instance whose node has been offline longer than the grace period
//!   ([`GRACE_KEY`]), or is drained, is `stranded`. A movable service is
//!   deployed on another eligible node first; a service with local data is
//!   only marked, which raises the `service-stranded` alert. When the node
//!   returns, a moved instance is removed and an unmoved one recovers.
//!
//! Every action is an `app.action` job submitted as the principal of the
//! instance's last job (the person who last deployed or ran it), so the job
//! runner authorizes it at every step as that person. The reconciler's own
//! decisions are audited as [`ACTOR`]. Services with an active deployment,
//! and instances that are busy, are left alone.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use forge_core::files::FileRepository;
use forge_core::model::{
    AgentStatus, AuditEvent, AuditOutcome, InstanceDesired, NodeFailurePolicy, Server, Service, ServiceInstance,
};
use forge_core::ports::{
    AuditRecorder, InstanceObservation, JobRepository, ServerRepository, ServiceRepository, SettingsRepository,
    StatusSwap, UserRepository,
};
use forge_core::tags::FactsRepository;
use forge_core::{AuthzContext, Clock, DomainResult, EntityKind, EntityRef, Principal, ServerId, ServiceId, Timestamp};
use forge_jobs::JobEngine;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::watch;

use crate::service_jobs::{AppActionRequest, app_action_job};

/// How often the reconciler looks at every instance.
pub const TICK: Duration = Duration::from_secs(15);
/// Setting: seconds a node may be offline before its services count as lost.
pub const GRACE_KEY: &str = "services.node_grace_secs";
/// Default of [`GRACE_KEY`].
pub const DEFAULT_GRACE_SECS: i64 = 300;
/// Shortest gap between two restarts of one instance.
pub const RETRY_GAP_MS: i64 = 2 * 60 * 1000;
/// Window in which [`MAX_ATTEMPTS`] restarts make the reconciler give up.
pub const ATTEMPT_WINDOW_MS: i64 = 30 * 60 * 1000;
/// Restarts in [`ATTEMPT_WINDOW_MS`] before the instance is marked `failed`.
pub const MAX_ATTEMPTS: usize = 5;
/// The audit actor of the reconciler's own decisions.
pub const ACTOR: &str = "system:reconciler";
/// Fact an agent reports when it pushes service container states.
pub const APP_STATES_FACT: &str = "app_states";

/// Audit: restarted (or redeployed) an instance.
pub const AUDIT_RESTART: &str = "service.reconcile.restart";
/// Audit: gave up restarting an instance.
pub const AUDIT_GAVE_UP: &str = "service.reconcile.gave_up";
/// Audit: moved a service off a lost node.
pub const AUDIT_MOVE: &str = "service.reconcile.move";
/// Audit: marked an instance stranded without moving it.
pub const AUDIT_STRANDED: &str = "service.reconcile.stranded";
/// Audit: removed a moved instance from the node that came back.
pub const AUDIT_REMOVE: &str = "service.reconcile.remove";
/// Audit: a stranded instance's node came back; it is deployed again.
pub const AUDIT_RECOVERED: &str = "service.reconcile.recovered";
/// Audit: an action was needed but there is no one to act as.
pub const AUDIT_REFUSED: &str = "service.reconcile.refused";

/// Instance status: deployed and as recorded.
pub const STATUS_DEPLOYED: &str = "deployed";
/// Instance status: its container runs another release than the last deploy.
pub const STATUS_DRIFTED: &str = "drifted";
/// Instance status: its node is lost (offline past the grace period or drained).
pub const STATUS_STRANDED: &str = "stranded";
/// Instance status: the last deploy failed, or the reconciler gave up.
pub const STATUS_FAILED: &str = "failed";
/// Instance state when the node reports no container for it.
pub const STATE_MISSING: &str = "missing";

/// One container row an agent pushes (`Heartbeat.app_states`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AppStateRow {
    /// `<environment>/<service>` (label `forge.app`).
    pub app: String,
    #[serde(default)]
    pub slot: String,
    #[serde(default)]
    pub release: String,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub exit_code: Option<i64>,
    #[serde(default)]
    pub container_id: String,
}

/// What a push says about one instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observed {
    pub state: String,
    pub exit_code: Option<i64>,
    /// `Some(true)` when no container runs the deployed release; `None`
    /// when unknown (no deployed release recorded, or no container).
    pub drifted: Option<bool>,
}

/// Reads the rows of the instance labelled `label` (a service may run in
/// two slots). The container of the deployed release speaks for it, then a
/// running one, then any.
#[must_use]
pub fn observe(rows: &[AppStateRow], label: &str, deployed_release: Option<&str>) -> Observed {
    let mine: Vec<&AppStateRow> = rows.iter().filter(|r| r.app == label).collect();

    if mine.is_empty() {
        return Observed { state: STATE_MISSING.into(), exit_code: None, drifted: None };
    }

    let deployed = |r: &&&AppStateRow| deployed_release.is_some_and(|d| r.release == d);
    let running = |r: &&&AppStateRow| r.state == "running";
    let pick = mine
        .iter()
        .find(|r| deployed(r) && running(r))
        .or_else(|| mine.iter().find(deployed))
        .or_else(|| mine.iter().find(running))
        .unwrap_or(&mine[0]);
    let drifted = deployed_release.map(|d| !mine.iter().any(|r| r.release == d));

    Observed { state: pick.state.clone(), exit_code: pick.exit_code, drifted }
}

/// What the reconciler does with one instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Nothing,
    /// Start the stopped container or unit.
    Start,
    /// Deploy again: the node has no container for it.
    Deploy,
    /// The node is lost: mark the instance stranded, moving it first when
    /// the policy says so.
    Strand(NodeFailurePolicy),
    /// A moved instance's node is back: remove it there.
    RemoveStranded,
    /// An unmoved instance's node is back: it counts as deployed again.
    Recover,
}

/// Everything [`decide`] looks at for one instance.
#[derive(Debug, Clone)]
pub struct Situation<'a> {
    pub instance: &'a ServiceInstance,
    pub node: &'a Server,
    /// A container service (else a systemd unit).
    pub container: bool,
    /// The node pushes container states ([`APP_STATES_FACT`]).
    pub reports_states: bool,
    pub policy: NodeFailurePolicy,
    /// Another instance of the service is not stranded and its node is
    /// online and not drained.
    pub other_active: bool,
    pub now: Timestamp,
    pub grace_ms: i64,
}

/// Whether `node` is lost: drained, revoked, or offline longer than the grace period.
#[must_use]
pub fn node_lost(node: &Server, now: Timestamp, grace_ms: i64) -> bool {
    match node.status {
        _ if node.drained => true,
        AgentStatus::Revoked => true,
        AgentStatus::Online => false,
        _ => node.last_seen_at.is_none_or(|t| now.0 - t.0 > grace_ms),
    }
}

/// Decides what to do with one instance. Pure, so every rule is tested
/// without a database.
#[must_use]
pub fn decide(s: &Situation<'_>) -> Decision {
    let i = s.instance;
    let up = s.node.status == AgentStatus::Online;
    let online = up && !s.node.drained;

    // A moved instance is removed once its node answers; an unmoved one
    // recovers when its node is back and not drained.
    if i.status == STATUS_STRANDED {
        return match (up, s.node.drained, s.policy) {
            (true, _, NodeFailurePolicy::Move) if s.other_active => Decision::RemoveStranded,
            (true, false, _) => Decision::Recover,
            _ => Decision::Nothing,
        };
    }

    // Only deployed instances are kept running; anything busy, pending or
    // failed waits for a person or the job that runs.
    if !matches!(i.status.as_str(), STATUS_DEPLOYED | STATUS_DRIFTED) {
        return Decision::Nothing;
    }

    if node_lost(s.node, s.now, s.grace_ms) {
        return Decision::Strand(s.policy);
    }

    if !online || i.desired != InstanceDesired::Running {
        return Decision::Nothing;
    }

    match (s.container, i.state.as_deref()) {
        (true, _) if !s.reports_states => Decision::Nothing,
        (true, Some(STATE_MISSING)) => Decision::Deploy,
        (true, Some("exited" | "dead")) | (false, Some("failed" | "inactive")) => Decision::Start,
        _ => Decision::Nothing,
    }
}

/// Whether a restart may go ahead given the earlier ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryVerdict {
    Go,
    Wait,
    GiveUp,
}

/// [`MAX_ATTEMPTS`] restarts in [`ATTEMPT_WINDOW_MS`] give up; otherwise
/// one per [`RETRY_GAP_MS`].
#[must_use]
pub fn retry_verdict(attempts: &[Timestamp], now: Timestamp) -> RetryVerdict {
    let recent: Vec<&Timestamp> = attempts.iter().filter(|t| now.0 - t.0 < ATTEMPT_WINDOW_MS).collect();

    if recent.len() >= MAX_ATTEMPTS {
        return RetryVerdict::GiveUp;
    }

    if recent.iter().any(|t| now.0 - t.0 < RETRY_GAP_MS) {
        return RetryVerdict::Wait;
    }

    RetryVerdict::Go
}

/// Whether a service can run on another node without losing data: a
/// container with no volumes, no devices and no App Store env files. A
/// systemd service is never movable.
#[must_use]
pub fn movable(spec: &Value, runtime: &str) -> bool {
    let empty = |k: &str| spec.get(k).and_then(Value::as_array).is_none_or(Vec::is_empty);

    runtime == "container" && empty("volumes") && empty("devices") && empty("env_files")
}

/// The service's node failure policy: its own `on_node_failure`, else move
/// when movable and alert otherwise. A systemd service is never moved.
#[must_use]
pub fn policy(app: &Service) -> NodeFailurePolicy {
    let chosen = app.spec.get("on_node_failure").and_then(|v| serde_json::from_value(v.clone()).ok());

    match chosen {
        _ if app.runtime != "container" => NodeFailurePolicy::Alert,
        Some(p) => p,
        None if movable(&app.spec, &app.runtime) => NodeFailurePolicy::Move,
        None => NodeFailurePolicy::Alert,
    }
}

/// The node a service moves to from `old`: online, not drained, Linux, in
/// the same workspace and cluster, not hosting the service yet; the one
/// with the fewest instances first.
#[must_use]
pub fn pick_target(
    old: &Server,
    servers: &[Server],
    hosting: &HashSet<ServerId>,
    load: &HashMap<ServerId, usize>,
) -> Option<ServerId> {
    servers
        .iter()
        .filter(|s| {
            s.id != old.id
                && s.workspace_id == old.workspace_id
                && s.status == AgentStatus::Online
                && !s.drained
                && crate::app_roots::supported(s)
                && (old.cluster_id.is_none() || s.cluster_id == old.cluster_id)
                && !hosting.contains(&s.id)
        })
        .min_by_key(|s| (load.get(&s.id).copied().unwrap_or(0), s.hostname.clone()))
        .map(|s| s.id)
}

/// The fleet as one tick sees it.
struct Fleet {
    servers: HashMap<ServerId, Server>,
    reports: HashSet<ServerId>,
    instances: Vec<ServiceInstance>,
    load: HashMap<ServerId, usize>,
    grace_ms: i64,
    now: Timestamp,
}

/// Keeps native services in their desired state (see the module docs).
#[derive(Debug)]
pub struct ServiceReconciler {
    pub services: Arc<dyn ServiceRepository>,
    pub servers: Arc<dyn ServerRepository>,
    pub deploys: Arc<dyn forge_core::deploy::DeploymentRepository>,
    pub job_repo: Arc<dyn JobRepository>,
    pub users: Arc<dyn UserRepository>,
    pub settings: Arc<dyn SettingsRepository>,
    /// Approved file roots per node: which volumes need `stack.privileged`.
    pub files: Arc<dyn FileRepository>,
    pub facts: Arc<dyn FactsRepository>,
    pub audit: Arc<dyn AuditRecorder>,
    pub clock: Arc<dyn Clock>,
    pub jobs: Arc<JobEngine>,
    /// Restarts per instance, for [`retry_verdict`].
    pub attempts: Mutex<HashMap<(ServiceId, ServerId), Vec<Timestamp>>>,
    /// Instances whose refusal is already audited (once until it changes).
    pub refused: Mutex<HashSet<(ServiceId, ServerId)>>,
}

impl ServiceReconciler {
    /// A reconciler over these ports with no history.
    #[must_use]
    pub fn new(ports: ReconcilerPorts) -> Self {
        Self {
            services: ports.services,
            servers: ports.servers,
            deploys: ports.deploys,
            job_repo: ports.job_repo,
            users: ports.users,
            settings: ports.settings,
            files: ports.files,
            facts: ports.facts,
            audit: ports.audit,
            clock: ports.clock,
            jobs: ports.jobs,
            attempts: Mutex::new(HashMap::new()),
            refused: Mutex::new(HashSet::new()),
        }
    }

    /// Drains (or undrains) a node and returns it. Idempotent.
    pub async fn set_drained(&self, id: ServerId, drained: bool) -> DomainResult<Server> {
        self.servers.get_server(id).await?;
        self.servers.set_drained(id, drained).await?;

        self.servers.get_server(id).await
    }

    /// Runs [`Self::tick`] every [`TICK`] until shutdown. A failed tick is
    /// logged and skipped.
    pub async fn run(self: Arc<Self>, mut shutdown: watch::Receiver<bool>) {
        let mut tick = tokio::time::interval(TICK);

        loop {
            tokio::select! {
                _ = tick.tick() => {}
                _ = shutdown.changed() => break,
            }

            if let Err(e) = self.tick().await {
                tracing::warn!(error = %e, "service reconciler tick failed");
            }
        }
    }

    /// Records one node's push of its service containers: each instance's
    /// state and exit code, and drift. Never fails the heartbeat.
    pub async fn observe_node(&self, server: ServerId, states_json: &str) {
        let rows: Vec<AppStateRow> = match serde_json::from_str(states_json) {
            Ok(r) => r,
            Err(e) => return tracing::debug!(%server, error = %e, "malformed service container states"),
        };

        if let Err(e) = self.apply_observation(server, &rows).await {
            tracing::warn!(%server, error = %e, "recording service container states failed");
        }
    }

    async fn apply_observation(&self, server: ServerId, rows: &[AppStateRow]) -> DomainResult<()> {
        let now = self.clock.now();

        for app in self.services.list_server_services(server).await? {
            if app.runtime != "container" || swarm_managed(&app) {
                continue;
            }

            let instances = self.services.list_instances(app.id).await?;
            let Some(i) = instances.into_iter().find(|i| i.server_id == server) else { continue };
            let env = app.spec.get("environment").and_then(Value::as_str).unwrap_or_default();
            let seen = observe(rows, &format!("{env}/{}", app.name), i.deployed_release.as_deref());
            let report = InstanceObservation { state: Some(seen.state), exit_code: seen.exit_code, at: now };
            self.services.observe_instance(app.id, server, &report).await?;

            let swap = match (seen.drifted, i.status.as_str()) {
                (Some(true), STATUS_DEPLOYED) => Some((STATUS_DEPLOYED, STATUS_DRIFTED)),
                (Some(false), STATUS_DRIFTED) => Some((STATUS_DRIFTED, STATUS_DEPLOYED)),
                _ => None,
            };

            if let Some((from, to)) = swap {
                let change = StatusSwap { from: from.into(), to: to.into(), error: None, at: now };
                self.services.swap_instance_status(app.id, server, &change).await?;
            }
        }

        Ok(())
    }

    /// One pass over every instance.
    pub async fn tick(&self) -> DomainResult<()> {
        let fleet = self.fleet().await?;
        let mut by_service: HashMap<ServiceId, Vec<&ServiceInstance>> = HashMap::new();

        for i in &fleet.instances {
            by_service.entry(i.service_id).or_default().push(i);
        }

        let ids: Vec<ServiceId> = by_service.keys().copied().collect();

        for app in self.services.get_services(&ids).await? {
            // A service removed from its services file is only taken off.
            if swarm_managed(&app)
                || app.source == crate::service_files::REMOVING_SOURCE
                || self.deploys.active_deployment(app.id).await?.is_some()
            {
                continue;
            }

            let Some(instances) = by_service.get(&app.id) else { continue };

            for i in instances {
                // One instance's failure never stops the others.
                if let Err(e) = self.reconcile(&fleet, &app, instances, i).await {
                    tracing::warn!(service = %app.id, server = %i.server_id, error = %e, "reconciling an instance failed");
                }
            }
        }

        // Services removed from their file whose instances are gone (D-178).
        for app in crate::service_files::finish_removals(self.services.as_ref()).await? {
            let details = json!({ "service": app.name, "reason": "removed from its services file" });
            self.record(&app, crate::service_files::AUDIT_REMOVED, AuditOutcome::Success, details).await;
        }

        Ok(())
    }

    async fn fleet(&self) -> DomainResult<Fleet> {
        let grace_secs = self
            .settings
            .all_settings()
            .await?
            .into_iter()
            .find(|(k, _)| k == GRACE_KEY)
            .and_then(|(_, v)| v.trim().parse::<i64>().ok())
            .filter(|s| *s >= 0)
            .unwrap_or(DEFAULT_GRACE_SECS);
        let servers: HashMap<ServerId, Server> =
            self.servers.list_servers(None).await?.into_iter().map(|s| (s.id, s)).collect();
        let reports = self
            .facts
            .all_facts()
            .await?
            .into_iter()
            .filter(|(_, (f, _))| f.get(APP_STATES_FACT).and_then(Value::as_bool) == Some(true))
            .map(|(id, _)| id)
            .collect();
        let instances = self.services.list_all_instances(None).await?;
        let mut load: HashMap<ServerId, usize> = HashMap::new();

        for i in &instances {
            *load.entry(i.server_id).or_default() += 1;
        }

        Ok(Fleet { servers, reports, instances, load, grace_ms: grace_secs * 1000, now: self.clock.now() })
    }

    async fn reconcile(
        &self,
        fleet: &Fleet,
        app: &Service,
        siblings: &[&ServiceInstance],
        i: &ServiceInstance,
    ) -> DomainResult<()> {
        let Some(node) = fleet.servers.get(&i.server_id) else { return Ok(()) };
        let other_active = siblings.iter().any(|o| {
            o.server_id != i.server_id
                && o.status != STATUS_STRANDED
                && fleet.servers.get(&o.server_id).is_some_and(|s| s.status == AgentStatus::Online && !s.drained)
        });
        let situation = Situation {
            instance: i,
            node,
            container: app.runtime == "container",
            reports_states: fleet.reports.contains(&i.server_id),
            policy: policy(app),
            other_active,
            now: fleet.now,
            grace_ms: fleet.grace_ms,
        };

        match decide(&situation) {
            Decision::Nothing => Ok(()),
            Decision::Start => self.restart(app, i, "start").await,
            Decision::Deploy => self.restart(app, i, "deploy").await,
            Decision::Strand(NodeFailurePolicy::Move) => self.move_away(fleet, app, siblings, i, node).await,
            Decision::Strand(NodeFailurePolicy::Alert) => self.strand_only(app, i, node, None).await,
            Decision::RemoveStranded => self.remove_stranded(app, i).await,
            Decision::Recover => self.recover(app, i).await,
        }
    }

    /// Starts (or redeploys) a stopped instance, or gives up after too many tries.
    async fn restart(&self, app: &Service, i: &ServiceInstance, action: &str) -> DomainResult<()> {
        let now = self.clock.now();
        let key = (app.id, i.server_id);

        let verdict = {
            let attempts = self.attempts.lock().map_err(|_| poisoned())?;
            retry_verdict(attempts.get(&key).map_or(&[][..], Vec::as_slice), now)
        };

        match verdict {
            RetryVerdict::Wait => return Ok(()),
            RetryVerdict::GiveUp => return self.give_up(app, i).await,
            RetryVerdict::Go => {}
        }

        let Some((principal, context)) = self.acting_as(app, i, action).await? else { return Ok(()) };
        let mounts = if action == "deploy" { self.privileged_mounts(app, i.server_id).await } else { Vec::new() };
        let job = self
            .jobs
            .submit(app_action_job(AppActionRequest {
                app,
                server: i.server_id,
                action,
                tail: 0,
                cursor: "",
                privileged_mounts: &mounts,
                principal: principal.clone(),
                context,
                idempotency_key: Some(format!(
                    "reconcile:{}:{}:{action}:{}",
                    app.id,
                    i.server_id,
                    now.0 / RETRY_GAP_MS
                )),
            }))
            .await?;

        {
            let mut attempts = self.attempts.lock().map_err(|_| poisoned())?;
            let list = attempts.entry(key).or_default();
            list.retain(|t| now.0 - t.0 < ATTEMPT_WINDOW_MS);
            list.push(now);
        }

        let details = json!({
            "service": app.name, "server_id": i.server_id, "action": action, "job_id": job.id,
            "state": i.state, "exit_code": i.exit_code, "on_behalf_of": principal.to_string(),
        });
        self.record(app, AUDIT_RESTART, AuditOutcome::Success, details).await;

        Ok(())
    }

    /// Marks an instance that keeps stopping `failed` and forgets its tries.
    async fn give_up(&self, app: &Service, i: &ServiceInstance) -> DomainResult<()> {
        let error = format!(
            "stopped {MAX_ATTEMPTS} times in {} minutes; Forge no longer restarts it: check its logs, then start or deploy it",
            ATTEMPT_WINDOW_MS / 60_000
        );
        let change =
            StatusSwap { from: i.status.clone(), to: STATUS_FAILED.into(), error: Some(error), at: self.clock.now() };

        if self.services.swap_instance_status(app.id, i.server_id, &change).await? {
            self.attempts.lock().map_err(|_| poisoned())?.remove(&(app.id, i.server_id));
            let details = json!({ "service": app.name, "server_id": i.server_id, "exit_code": i.exit_code });
            self.record(app, AUDIT_GAVE_UP, AuditOutcome::Failure, details).await;
        }

        Ok(())
    }

    /// Deploys the service on another node, then marks the old instance
    /// stranded. Not atomic: the new placement and its job come first, so a
    /// crash in between leaves a pending placement (shown, deployable by
    /// hand) and the old instance unmarked, which the next tick moves again.
    /// A failed submit removes the new placement (compensation).
    async fn move_away(
        &self,
        fleet: &Fleet,
        app: &Service,
        siblings: &[&ServiceInstance],
        i: &ServiceInstance,
        node: &Server,
    ) -> DomainResult<()> {
        let servers: Vec<Server> = fleet.servers.values().cloned().collect();
        let hosting: HashSet<ServerId> = siblings.iter().map(|s| s.server_id).collect();

        let Some(target) = pick_target(node, &servers, &hosting, &fleet.load) else {
            return self.strand_only(app, i, node, Some("no other eligible node")).await;
        };

        let Some((principal, context)) = self.acting_as(app, i, "deploy").await? else { return Ok(()) };
        let now = self.clock.now();
        let placed = ServiceInstance {
            service_id: app.id,
            server_id: target,
            role: i.role.clone(),
            status: "pending".into(),
            state: None,
            last_job_id: None,
            last_error: None,
            updated_at: now,
            desired: InstanceDesired::Running,
            deployed_release: None,
            exit_code: None,
        };
        self.services.put_instance(&placed).await?;

        let mounts = self.privileged_mounts(app, target).await;
        let submitted = self
            .jobs
            .submit(app_action_job(AppActionRequest {
                app,
                server: target,
                action: "deploy",
                tail: 0,
                cursor: "",
                privileged_mounts: &mounts,
                principal: principal.clone(),
                context,
                idempotency_key: Some(format!("reconcile:{}:{}:move:{target}", app.id, i.server_id)),
            }))
            .await;

        let job = match submitted {
            Ok(j) => j,
            Err(e) => {
                self.services.delete_instance(app.id, target).await?;

                return Err(e);
            }
        };

        if self.strand(app, i, node, None).await? {
            let details = json!({
                "service": app.name, "from": i.server_id, "to": target, "job_id": job.id,
                "on_behalf_of": principal.to_string(),
            });
            self.record(app, AUDIT_MOVE, AuditOutcome::Success, details).await;
        }

        Ok(())
    }

    /// Marks an instance stranded; `true` when it was not already.
    async fn strand(&self, app: &Service, i: &ServiceInstance, node: &Server, why: Option<&str>) -> DomainResult<bool> {
        let lost = if node.drained { "its node is drained" } else { "its node is offline" };

        let error = match why {
            Some(w) => format!("{lost} and it could not move ({w})"),
            None => lost.to_owned(),
        };

        let change =
            StatusSwap { from: i.status.clone(), to: STATUS_STRANDED.into(), error: Some(error), at: self.clock.now() };

        self.services.swap_instance_status(app.id, i.server_id, &change).await
    }

    /// Marks an instance stranded without moving it, and audits that.
    async fn strand_only(
        &self,
        app: &Service,
        i: &ServiceInstance,
        node: &Server,
        why: Option<&str>,
    ) -> DomainResult<()> {
        if self.strand(app, i, node, why).await? {
            let details =
                json!({ "service": app.name, "server_id": i.server_id, "drained": node.drained, "reason": why });
            self.record(app, AUDIT_STRANDED, AuditOutcome::Success, details).await;
        }

        Ok(())
    }

    /// Removes a moved instance from the node that came back. The `remove`
    /// job deletes the placement when it succeeds.
    async fn remove_stranded(&self, app: &Service, i: &ServiceInstance) -> DomainResult<()> {
        let Some((principal, context)) = self.acting_as(app, i, "remove").await? else { return Ok(()) };
        let now = self.clock.now();
        let job = self
            .jobs
            .submit(app_action_job(AppActionRequest {
                app,
                server: i.server_id,
                action: "remove",
                tail: 0,
                cursor: "",
                privileged_mounts: &[],
                principal: principal.clone(),
                context,
                idempotency_key: Some(format!("reconcile:{}:{}:remove:{}", app.id, i.server_id, now.0 / RETRY_GAP_MS)),
            }))
            .await?;

        let details = json!({
            "service": app.name, "server_id": i.server_id, "job_id": job.id, "on_behalf_of": principal.to_string(),
        });
        self.record(app, AUDIT_REMOVE, AuditOutcome::Success, details).await;

        Ok(())
    }

    async fn recover(&self, app: &Service, i: &ServiceInstance) -> DomainResult<()> {
        let change = StatusSwap {
            from: STATUS_STRANDED.into(),
            to: STATUS_DEPLOYED.into(),
            error: Some(String::new()),
            at: self.clock.now(),
        };

        if self.services.swap_instance_status(app.id, i.server_id, &change).await? {
            let details = json!({ "service": app.name, "server_id": i.server_id });
            self.record(app, AUDIT_RECOVERED, AuditOutcome::Success, details).await;
        }

        Ok(())
    }

    /// The principal and context of the instance's last job, when that
    /// person still exists and is enabled. Otherwise audits a refusal (once
    /// per instance until it can act again) and returns `None`.
    async fn acting_as(
        &self,
        app: &Service,
        i: &ServiceInstance,
        action: &str,
    ) -> DomainResult<Option<(Principal, AuthzContext)>> {
        let key = (app.id, i.server_id);

        let found = match i.last_job_id {
            Some(id) => self.job_repo.get_job(id).await.ok().map(|j| (j.principal, j.context)),
            None => None,
        };

        let why = match &found {
            None => Some("no earlier job records who runs it"),
            Some((Principal::User(uid), _)) => match self.users.get_user(*uid).await {
                Ok(u) if !u.disabled => None,
                Ok(_) => Some("the person who last ran it is disabled"),
                Err(_) => Some("the person who last ran it was removed"),
            },
        };

        let Some(why) = why else {
            self.refused.lock().map_err(|_| poisoned())?.remove(&key);

            return Ok(found);
        };

        if self.refused.lock().map_err(|_| poisoned())?.insert(key) {
            let details = json!({ "service": app.name, "server_id": i.server_id, "action": action, "reason": why });
            self.record(app, AUDIT_REFUSED, AuditOutcome::Denied, details).await;
        }

        Ok(None)
    }

    /// Volumes of `app` on `server` that need `stack.privileged`.
    async fn privileged_mounts(&self, app: &Service, server: ServerId) -> Vec<String> {
        let Ok(spec) = crate::jobs::app::build_spec(app, None) else { return Vec::new() };
        let roots = crate::stacks::bind_roots(self.files.as_ref(), server).await;

        crate::jobs::app::privileged_mounts(&spec, &roots)
    }

    async fn record(&self, app: &Service, action: &str, outcome: AuditOutcome, details: Value) {
        let ev = AuditEvent::new(ACTOR, action, EntityRef::new(EntityKind::Service, app.id), outcome)
            .in_org(Some(app.workspace_id))
            .with_details(details);

        if let Err(e) = self.audit.record(ev).await {
            tracing::error!(error = %e, "audit write failed");
        }
    }
}

/// The ports a [`ServiceReconciler`] uses.
#[derive(Debug)]
pub struct ReconcilerPorts {
    pub services: Arc<dyn ServiceRepository>,
    pub servers: Arc<dyn ServerRepository>,
    pub deploys: Arc<dyn forge_core::deploy::DeploymentRepository>,
    pub job_repo: Arc<dyn JobRepository>,
    pub users: Arc<dyn UserRepository>,
    pub settings: Arc<dyn SettingsRepository>,
    pub files: Arc<dyn FileRepository>,
    pub facts: Arc<dyn FactsRepository>,
    pub audit: Arc<dyn AuditRecorder>,
    pub clock: Arc<dyn Clock>,
    pub jobs: Arc<JobEngine>,
}

/// A Swarm service: the swarm keeps it running, not Forge.
fn swarm_managed(app: &Service) -> bool {
    app.spec.get("swarm").is_some_and(|v| !v.is_null())
}

fn poisoned() -> forge_core::DomainError {
    forge_core::DomainError::Internal("reconciler state lock poisoned".into())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use forge_core::model::NodeRole;

    use super::*;

    const GRACE: i64 = 300_000;

    fn server(id: ServerId, status: AgentStatus, seen: i64) -> Server {
        Server {
            id,
            workspace_id: forge_core::WorkspaceId::new(),
            environment_id: None,
            cluster_id: None,
            role: NodeRole::default(),
            display_name: None,
            hostname: format!("lab-{id}"),
            os: "linux".into(),
            arch: "x86_64".into(),
            agent_version: "1".into(),
            status,
            last_seen_at: Some(Timestamp(seen)),
            cert_serial: None,
            cert_not_after: None,
            created_at: Timestamp(0),
            drained: false,
        }
    }

    fn instance(status: &str, state: Option<&str>) -> ServiceInstance {
        ServiceInstance {
            service_id: ServiceId::new(),
            server_id: ServerId::new(),
            role: "active".into(),
            status: status.into(),
            state: state.map(str::to_owned),
            last_job_id: None,
            last_error: None,
            updated_at: Timestamp(0),
            desired: InstanceDesired::Running,
            deployed_release: None,
            exit_code: None,
        }
    }

    fn situation<'a>(i: &'a ServiceInstance, node: &'a Server) -> Situation<'a> {
        Situation {
            instance: i,
            node,
            container: true,
            reports_states: true,
            policy: NodeFailurePolicy::Move,
            other_active: false,
            now: Timestamp(1_000_000),
            grace_ms: GRACE,
        }
    }

    #[test]
    fn an_exited_container_on_an_online_node_is_started() {
        let node = server(ServerId::new(), AgentStatus::Online, 1_000_000);
        let i = instance("deployed", Some("exited"));

        assert_eq!(decide(&situation(&i, &node)), Decision::Start);

        let missing = instance("deployed", Some(STATE_MISSING));
        assert_eq!(decide(&situation(&missing, &node)), Decision::Deploy);

        let running = instance("deployed", Some("running"));
        assert_eq!(decide(&situation(&running, &node)), Decision::Nothing);
    }

    #[test]
    fn stopped_busy_or_unreported_instances_are_left_alone() {
        let node = server(ServerId::new(), AgentStatus::Online, 1_000_000);
        let mut stopped = instance("deployed", Some("exited"));
        stopped.desired = InstanceDesired::Stopped;
        assert_eq!(decide(&situation(&stopped, &node)), Decision::Nothing);

        for status in ["deploying", "starting", "pending", "failed"] {
            let i = instance(status, Some("exited"));
            assert_eq!(decide(&situation(&i, &node)), Decision::Nothing, "{status}");
        }

        let i = instance("deployed", Some("exited"));
        let s = Situation { reports_states: false, ..situation(&i, &node) };
        assert_eq!(decide(&s), Decision::Nothing, "an older agent reports nothing");

        let unit = instance("deployed", Some("failed"));
        let s = Situation { container: false, reports_states: false, ..situation(&unit, &node) };
        assert_eq!(decide(&s), Decision::Start, "a failed systemd unit is started");
    }

    #[test]
    fn a_lost_node_strands_its_instances_after_the_grace_period() {
        let i = instance("deployed", Some("running"));
        let recent = server(ServerId::new(), AgentStatus::Offline, 1_000_000 - GRACE + 1);
        assert_eq!(decide(&situation(&i, &recent)), Decision::Nothing, "within the grace period");

        let gone = server(ServerId::new(), AgentStatus::Offline, 1_000_000 - GRACE - 1);
        assert_eq!(decide(&situation(&i, &gone)), Decision::Strand(NodeFailurePolicy::Move));

        let s = Situation { policy: NodeFailurePolicy::Alert, ..situation(&i, &gone) };
        assert_eq!(decide(&s), Decision::Strand(NodeFailurePolicy::Alert));

        let mut drained = server(ServerId::new(), AgentStatus::Online, 1_000_000);
        drained.drained = true;
        assert_eq!(decide(&situation(&i, &drained)), Decision::Strand(NodeFailurePolicy::Move));
    }

    #[test]
    fn a_stranded_instance_is_removed_or_recovers_when_its_node_returns() {
        let i = instance(STATUS_STRANDED, Some("running"));
        let back = server(ServerId::new(), AgentStatus::Online, 1_000_000);
        let gone = server(ServerId::new(), AgentStatus::Offline, 0);

        assert_eq!(decide(&situation(&i, &gone)), Decision::Nothing);

        let moved = Situation { other_active: true, ..situation(&i, &back) };
        assert_eq!(decide(&moved), Decision::RemoveStranded);
        assert_eq!(decide(&situation(&i, &back)), Decision::Recover, "the move found no node");

        let kept = Situation { policy: NodeFailurePolicy::Alert, other_active: true, ..situation(&i, &back) };
        assert_eq!(decide(&kept), Decision::Recover, "data stays where it is");

        let mut drained = back.clone();
        drained.drained = true;
        assert_eq!(decide(&Situation { other_active: true, ..situation(&i, &drained) }), Decision::RemoveStranded);
        let held = Situation { policy: NodeFailurePolicy::Alert, other_active: true, ..situation(&i, &drained) };
        assert_eq!(decide(&held), Decision::Nothing, "a drained node keeps an unmoved instance");
    }

    #[test]
    fn restarts_wait_two_minutes_and_stop_after_five_in_half_an_hour() {
        let now = Timestamp(10_000_000);
        assert_eq!(retry_verdict(&[], now), RetryVerdict::Go);
        assert_eq!(retry_verdict(&[Timestamp(now.0 - 60_000)], now), RetryVerdict::Wait);
        assert_eq!(retry_verdict(&[Timestamp(now.0 - RETRY_GAP_MS)], now), RetryVerdict::Go);

        let five: Vec<Timestamp> = (1..=5).map(|n| Timestamp(now.0 - n * RETRY_GAP_MS - 1)).collect();
        assert_eq!(retry_verdict(&five, now), RetryVerdict::GiveUp);

        let old: Vec<Timestamp> = (0..5).map(|n| Timestamp(now.0 - ATTEMPT_WINDOW_MS - n)).collect();
        assert_eq!(retry_verdict(&old, now), RetryVerdict::Go, "old attempts are forgotten");
    }

    #[test]
    fn only_containers_without_local_data_are_movable() {
        assert!(movable(&json!({ "image": "nginx" }), "container"));
        assert!(!movable(&json!({ "volumes": ["/srv/data:/data"] }), "container"));
        assert!(!movable(&json!({ "devices": [{ "host": "/dev/ttyUSB0" }] }), "container"));
        assert!(!movable(&json!({ "env_files": ["postgres"] }), "container"));
        assert!(!movable(&json!({}), "systemd"));
    }

    #[test]
    fn the_policy_defaults_by_movability_and_systemd_never_moves() {
        let mut app = Service {
            id: ServiceId::new(),
            workspace_id: forge_core::WorkspaceId::new(),
            environment_id: forge_core::EnvironmentId::new(),
            name: "web".into(),
            runtime: "container".into(),
            spec: json!({ "image": "nginx" }),
            env: Vec::new(),
            created_by: Principal::User(forge_core::UserId::new()),
            created_at: Timestamp(0),
            updated_at: Timestamp(0),
            version: 0,
            source: String::new(),
            secrets_changed_at: None,
        };
        assert_eq!(policy(&app), NodeFailurePolicy::Move);

        app.spec["on_node_failure"] = json!("alert");
        assert_eq!(policy(&app), NodeFailurePolicy::Alert);

        app.spec = json!({ "volumes": ["/srv:/srv"] });
        assert_eq!(policy(&app), NodeFailurePolicy::Alert);
        app.spec["on_node_failure"] = json!("move");
        assert_eq!(policy(&app), NodeFailurePolicy::Move, "an explicit choice wins");

        app.runtime = "systemd".into();
        assert_eq!(policy(&app), NodeFailurePolicy::Alert);
    }

    #[test]
    fn the_target_is_an_eligible_node_with_the_fewest_instances() {
        let old = server(ServerId::new(), AgentStatus::Offline, 0);
        let same_ws = |status| {
            let mut s = server(ServerId::new(), status, 0);
            s.workspace_id = old.workspace_id;
            s
        };
        let busy = same_ws(AgentStatus::Online);
        let idle = same_ws(AgentStatus::Online);
        let offline = same_ws(AgentStatus::Offline);
        let mut drained = same_ws(AgentStatus::Online);
        drained.drained = true;
        let mut windows = same_ws(AgentStatus::Online);
        windows.os = "windows".into();
        let foreign = server(ServerId::new(), AgentStatus::Online, 0);

        let all = vec![old.clone(), busy.clone(), idle.clone(), offline, drained, windows, foreign];
        let load = HashMap::from([(busy.id, 3), (idle.id, 1)]);

        assert_eq!(pick_target(&old, &all, &HashSet::new(), &load), Some(idle.id));
        assert_eq!(pick_target(&old, &all, &HashSet::from([idle.id]), &load), Some(busy.id));
        assert_eq!(pick_target(&old, &all, &HashSet::from([idle.id, busy.id]), &load), None);
    }

    #[test]
    fn the_deployed_release_speaks_for_an_instance_and_others_are_drift() {
        let row = |release: &str, state: &str| AppStateRow {
            app: "prod/web".into(),
            slot: String::new(),
            release: release.into(),
            state: state.into(),
            exit_code: None,
            container_id: "c".into(),
        };

        let seen = observe(&[row("3", "exited"), row("4", "running")], "prod/web", Some("3"));
        assert_eq!((seen.state.as_str(), seen.drifted), ("exited", Some(false)));

        let seen = observe(&[row("5", "running")], "prod/web", Some("4"));
        assert_eq!(seen.drifted, Some(true));

        let seen = observe(&[row("5", "running")], "prod/web", None);
        assert_eq!(seen.drifted, None, "nothing recorded, nothing to compare");

        let seen = observe(&[row("1", "running")], "prod/api", Some("1"));
        assert_eq!(seen.state, STATE_MISSING);
    }
}
