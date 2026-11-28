//! `deploy.rollout`: rolls one release of a service out to its nodes, step
//! by step, as the planner laid out (#73).
//!
//! Payload: `{"deployment_id", "privileged_mounts", "remove", "names"}`.
//! The plan lives on the deployment; `deployment.step` counts the steps
//! done, so a job resumed after a crash (or a lost lease) continues with the
//! step it was on. Every step is safe to repeat: a start deploys the copy
//! again, a gate probes again, a route re-applies the same weights, a stop
//! of a copy already gone does nothing.
//!
//! Between steps, and while it waits, the job reads the deployment's
//! request: `pause` holds it until `resume`, `abort` stops it. A failure (or
//! an abort) removes the new copies that never took traffic. When copies
//! that took traffic are left and the policy says so, a failure starts a
//! rollback to the release that served before.

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use forge_catalog::AppSpec;
use forge_catalog::deploy::{self as cat, ArtifactRef, Plan, RolloutPolicy, Slot, Step, Target, Weight};
use forge_core::deploy::{
    Deployment, DeploymentEvent, DeploymentKind, DeploymentRepository, DeploymentRequest, DeploymentState, Release,
    ReleaseRepository, SlotRecord,
};
use forge_core::model::{AuditEvent, AuditOutcome, NewJob, Service};
use forge_core::ports::{AuditRecorder, ServiceRepository};
use forge_core::{Action, Clock, DeploymentId, EntityKind, EntityRef, ServerId, ServiceId};
use forge_jobs::{JobEngine, JobHandler, StepContext, StepError};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::deploy::{DeployService, RolloutEffects, Starter, release_app};
use crate::jobs::AppJob;

/// Longest a deploy on one node may take (image pulls, runtime installs).
const DEPLOY_WAIT: Duration = Duration::from_secs(35 * 60);
const PROBE_WAIT: Duration = Duration::from_secs(60);
const RETIRE_WAIT: Duration = Duration::from_secs(120);
/// How often a canary's gates look at the traffic.
const ANALYSIS_EVERY: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Deserialize)]
pub(super) struct Payload {
    pub(super) deployment_id: DeploymentId,
    /// Container volumes (or, on Swarm, host binds) outside the approved
    /// roots that the submitter was authorized for (`stack.privileged`).
    #[serde(default)]
    pub(super) privileged_mounts: Vec<String>,
    #[serde(default)]
    remove: Vec<ServerId>,
    #[serde(default)]
    names: BTreeMap<String, String>,
    /// A rollout in this Docker Swarm cluster (D-156).
    #[serde(default)]
    pub(super) swarm_cluster: Option<forge_core::ClusterId>,
}

#[derive(Debug)]
pub struct DeployJob {
    pub apps: Arc<dyn ServiceRepository>,
    pub releases: Arc<dyn ReleaseRepository>,
    pub deploys: Arc<dyn DeploymentRepository>,
    /// Builds node specs and sends app actions (shared with `app.action`).
    pub runner: Arc<AppJob>,
    /// Starts automatic rollbacks.
    pub service: Arc<DeployService>,
    /// Set once the API exists (it implements the effects).
    pub effects: Arc<OnceLock<Arc<dyn RolloutEffects>>>,
    /// Set once the engine exists (it holds this handler).
    pub engine: Arc<OnceLock<Arc<JobEngine>>>,
    pub audit: Arc<dyn AuditRecorder>,
    pub clock: Arc<dyn Clock>,
    /// Pause between probes and between looks at a request.
    pub tick: Duration,
    /// Rollouts in Docker Swarm clusters (D-156).
    pub swarm: Arc<crate::swarm::SwarmService>,
    pub swarms: Arc<dyn forge_core::swarm::SwarmRepository>,
    pub ops: super::node_op::NodeOps,
    /// Credentials for private images, resolved per step (never stored).
    pub registries: Arc<crate::registries::RegistryService>,
}

/// Why a rollout stopped.
#[derive(Debug)]
pub(super) enum Stop {
    /// Try the step again later (a node did not answer).
    Retry(String),
    Failed(String),
    /// Someone aborted or rejected it.
    Aborted(String),
}

impl From<StepError> for Stop {
    fn from(e: StepError) -> Self {
        match e {
            StepError::Retryable(m) => Self::Retry(m),
            StepError::Fatal(m) => Self::Failed(m),
        }
    }
}

impl From<forge_core::DomainError> for Stop {
    fn from(e: forge_core::DomainError) -> Self {
        Self::Retry(e.to_string())
    }
}

pub(super) type Done<T = ()> = Result<T, Stop>;

/// What every step needs.
pub(super) struct Run<'a> {
    pub(super) principal: String,
    pub(super) app: Service,
    pub(super) release: Release,
    pub(super) policy: RolloutPolicy,
    pub(super) payload: &'a Payload,
}

impl Run<'_> {
    fn name(&self, server: &str) -> String {
        self.payload.names.get(server).cloned().unwrap_or_else(|| server.to_owned())
    }

    fn copy(&self, t: &Target) -> String {
        match t.slot {
            Some(s) => format!("{} (slot {})", self.name(&t.server_id), s.as_str()),
            None => self.name(&t.server_id),
        }
    }
}

fn server_of(t: &str) -> Done<ServerId> {
    t.parse().map_err(|_| Stop::Failed(format!("bad node id `{t}` in the plan")))
}

impl DeployJob {
    pub(super) fn effects(&self) -> Done<&Arc<dyn RolloutEffects>> {
        self.effects.get().ok_or_else(|| Stop::Retry("the control plane is still starting".into()))
    }

    pub(super) async fn event(&self, id: DeploymentId, level: &str, server: Option<ServerId>, message: String) {
        let e = DeploymentEvent {
            deployment_id: id,
            seq: 0,
            at: self.clock.now(),
            level: level.into(),
            server_id: server,
            message,
        };

        if let Err(err) = self.deploys.add_event(&e).await {
            tracing::warn!(deployment = %id, error = %err, "recording a deployment event failed");
        }
    }

    pub(super) async fn save(&self, d: &mut Deployment) -> Done {
        d.updated_at = self.clock.now();
        self.deploys.put_deployment(d).await?;

        Ok(())
    }

    pub(super) async fn audit(&self, principal: &str, d: &Deployment, action: &str, ok: bool, details: Value) {
        let ev = AuditEvent::new(
            principal,
            action,
            EntityRef::new(EntityKind::Service, d.service_id),
            if ok { AuditOutcome::Success } else { AuditOutcome::Failure },
        )
        .in_org(Some(d.workspace_id))
        .with_details(details);

        if let Err(e) = self.audit.record(ev).await {
            tracing::error!(error = %e, "audit write failed");
        }
    }

    /// Acts on a request between steps: holds while paused, stops on abort.
    pub(super) async fn checkpoint(&self, d: &mut Deployment) -> Done {
        loop {
            let cur = self.deploys.get_deployment(d.id).await?.ok_or_else(|| Stop::Failed("deployment gone".into()))?;
            match cur.request {
                Some(DeploymentRequest::Abort) => {
                    self.deploys.clear_request(d.id).await?;
                    return Err(Stop::Aborted("aborted on request".into()));
                }
                Some(DeploymentRequest::Pause) if d.state != DeploymentState::Paused => {
                    d.state = DeploymentState::Paused;
                    self.save(d).await?;
                    self.event(d.id, "info", None, "paused".into()).await;
                }
                Some(DeploymentRequest::Resume) => {
                    self.deploys.clear_request(d.id).await?;

                    if d.state == DeploymentState::Paused {
                        d.state = DeploymentState::Running;
                        self.save(d).await?;
                        self.event(d.id, "info", None, "resumed".into()).await;
                    }

                    return Ok(());
                }
                Some(DeploymentRequest::Pause) => {}
                // An approval outside an approval step means nothing.
                Some(DeploymentRequest::Approve | DeploymentRequest::Reject) => {
                    self.deploys.clear_request(d.id).await?;
                }
                None if d.state == DeploymentState::Paused => {}
                None => return Ok(()),
            }

            tokio::time::sleep(self.tick).await;
        }
    }

    /// Stops a wait early on abort; a pause during a wait holds after it.
    pub(super) async fn aborted(&self, id: DeploymentId) -> Done {
        let cur = self.deploys.get_deployment(id).await?;
        if cur.is_some_and(|c| c.request == Some(DeploymentRequest::Abort)) {
            self.deploys.clear_request(id).await?;
            return Err(Stop::Aborted("aborted on request".into()));
        }

        Ok(())
    }

    /// The release as stored now (the first pull may have pinned its digest).
    pub(super) async fn fresh_release(&self, r: &Run<'_>) -> Done<Release> {
        Ok(self.releases.get_release(r.release.id).await?.unwrap_or_else(|| r.release.clone()))
    }

    /// The spec of copy `t`, and its service port → own port map.
    async fn copy_spec(&self, r: &Run<'_>, t: &Target) -> Done<(AppSpec, BTreeMap<u16, u16>)> {
        let release = self.fresh_release(r).await?;
        let mut spec = self.runner.resolved_spec(&release_app(&r.app, &release)).await?;
        spec.release = release.number.to_string();

        match serde_json::from_value::<ArtifactRef>(release.artifact.clone()) {
            Ok(a) if a.staged() => {
                // Code fetched into `<home>/releases/<n>`, run from there
                // (each slot from its own release).
                let base = spec.release_dir();

                let sub = match &a {
                    ArtifactRef::Git { path, .. } if !path.is_empty() => format!("/{path}"),
                    _ => String::new(),
                };

                if spec.working_dir.is_empty() {
                    spec.working_dir = format!("{base}{sub}");
                }

                spec.artifact = Some(a);
            }
            Ok(a) => {
                if let Some(image) = a.image_ref() {
                    spec.image = image;
                }
            }
            Err(_) => {}
        }

        let map = match t.slot {
            Some(s) => self.runner.into_slot(&mut spec, r.app.id, s).await?,
            None => in_place_ports(&spec),
        };

        let server: ServerId =
            t.server_id.parse().map_err(|_| StepError::Fatal(format!("invalid node id `{}`", t.server_id)))?;
        self.runner.attach_mesh(&r.app, &mut spec, server).await?;

        Ok((spec, map))
    }

    async fn slot_row(&self, app: ServiceId, server: ServerId, slot: Option<Slot>) -> Done<Option<SlotRecord>> {
        let name = Slot::name(slot);
        Ok(self.deploys.list_slots(app).await?.into_iter().find(|s| s.server_id == server && s.slot == name))
    }

    async fn set_instance(
        &self,
        app: ServiceId,
        server: ServerId,
        f: impl FnOnce(&mut forge_core::model::ServiceInstance),
    ) -> Done {
        let Some(mut i) = self.apps.list_instances(app).await?.into_iter().find(|i| i.server_id == server) else {
            return Ok(());
        };

        f(&mut i);
        i.updated_at = self.clock.now();
        self.apps.put_instance(&i).await?;

        Ok(())
    }

    async fn start_one(&self, d: &Deployment, r: &Run<'_>, t: &Target) -> Done {
        let server = server_of(&t.server_id)?;
        let (spec, map) = self.copy_spec(r, t).await?;
        self.runner.runtimes_ready(&spec, server).await?;
        let roots = crate::stacks::bind_roots(self.runner.files.as_ref(), server).await;

        if let Some(v) =
            super::app::privileged_mounts(&spec, &roots).into_iter().find(|v| !r.payload.privileged_mounts.contains(v))
        {
            return Err(Stop::Failed(format!(
                "volume `{v}` is outside the app folder and the approved file roots; it needs stack.privileged \
                 (an administrator with MFA): deploy again as one"
            )));
        }

        // A new side-by-side slot takes no traffic until a route step moves
        // it there; a copy replaced in place keeps what it had, and one that
        // never ran (a first release) serves at once.
        let weight = match self.slot_row(r.app.id, server, t.slot).await? {
            Some(row) => row.weight,
            None if t.slot.is_none() => 100,
            None => 0,
        };

        let mut row = SlotRecord {
            service_id: r.app.id,
            server_id: server,
            slot: Slot::name(t.slot).to_owned(),
            release_number: r.release.number,
            weight,
            state: "starting".into(),
            ports: json!(map.iter().map(|(k, v)| (k.to_string(), json!(v))).collect::<serde_json::Map<_, _>>()),
            updated_at: self.clock.now(),
        };
        self.deploys.put_slot(&row).await?;
        self.set_instance(r.app.id, server, |i| {
            i.status = "deploying".into();
            i.last_error = None;
        })
        .await?;
        self.event(d.id, "info", Some(server), format!("starting release {} on {}", r.release.number, r.copy(t))).await;

        let out = match self.runner.send(&r.principal, r.app.workspace_id, server, "deploy", &spec, DEPLOY_WAIT).await {
            Ok(o) => o,
            Err(e) => {
                let msg = match &e {
                    StepError::Retryable(m) | StepError::Fatal(m) => m.clone(),
                };

                row.state = "failed".into();
                self.deploys.put_slot(&row).await?;
                self.set_instance(r.app.id, server, |i| {
                    i.status = "failed".into();
                    i.last_error = Some(msg.clone());
                })
                .await?;

                return Err(e.into());
            }
        };

        // A tag resolves to a digest on the first node; the rest pull that.
        if let Ok(ArtifactRef::Image { image, digest: None }) =
            serde_json::from_value::<ArtifactRef>(self.fresh_release(r).await?.artifact)
            && let Some(dg) = out.get("image_digest").and_then(Value::as_str).filter(|d| d.starts_with("sha256:"))
        {
            let pinned = ArtifactRef::Image { image, digest: Some(dg.to_owned()) };
            self.releases
                .set_release_artifact(
                    r.release.id,
                    &serde_json::to_value(&pinned).map_err(|e| Stop::Failed(e.to_string()))?,
                )
                .await?;
            self.event(d.id, "info", Some(server), format!("pinned the image to {dg}")).await;
        }

        row.state = "started".into();
        row.updated_at = self.clock.now();
        self.deploys.put_slot(&row).await?;
        let state = out.get("state").and_then(Value::as_str).map(str::to_owned);
        self.set_instance(r.app.id, server, |i| {
            i.status = "deployed".into();

            if state.is_some() {
                i.state = state;
            }
        })
        .await?;
        self.runner.ensure_app_root(&r.principal, &release_app(&r.app, &r.release), server).await?;

        Ok(())
    }

    /// Waits until copy `t` is ready, then stays ready for the
    /// stabilization period.
    async fn gate_one(&self, d: &Deployment, r: &Run<'_>, t: &Target) -> Done {
        let server = server_of(&t.server_id)?;
        let (spec, _) = self.copy_spec(r, t).await?;
        let start = tokio::time::Instant::now();
        let deadline = start + Duration::from_secs(r.policy.startup_timeout_secs);
        let stable_for = Duration::from_secs(r.policy.stabilization_secs);
        let mut ready_since: Option<tokio::time::Instant> = None;
        let mut why = String::from("no answer yet");

        loop {
            self.aborted(d.id).await?;

            match self.runner.send(&r.principal, r.app.workspace_id, server, "probe", &spec, PROBE_WAIT).await {
                Ok(out) if out["ready"] == true => {
                    let since = *ready_since.get_or_insert_with(tokio::time::Instant::now);
                    if since.elapsed() >= stable_for {
                        break;
                    }
                }
                Ok(out) => {
                    ready_since = None;
                    why = failing_checks(&out);
                }
                // A node too old for `probe`: say so rather than wait.
                Err(StepError::Fatal(m)) if m.contains("unknown action") => {
                    return Err(Stop::Failed(format!(
                        "{} cannot probe copies: update its agent ({m})",
                        r.name(&t.server_id)
                    )));
                }
                Err(StepError::Fatal(m) | StepError::Retryable(m)) => {
                    ready_since = None;
                    why = m;
                }
            }

            if tokio::time::Instant::now() >= deadline {
                if let Some(mut row) = self.slot_row(r.app.id, server, t.slot).await? {
                    row.state = "failed".into();
                    row.updated_at = self.clock.now();
                    self.deploys.put_slot(&row).await?;
                }

                return Err(Stop::Failed(format!(
                    "{} did not become ready and stable within {}s: {why}",
                    r.copy(t),
                    r.policy.startup_timeout_secs
                )));
            }

            tokio::time::sleep(self.tick).await;
        }

        if let Some(mut row) = self.slot_row(r.app.id, server, t.slot).await? {
            row.state = "ready".into();
            row.updated_at = self.clock.now();
            self.deploys.put_slot(&row).await?;
        }

        self.event(d.id, "info", Some(server), format!("{} is ready", r.copy(t))).await;

        Ok(())
    }

    async fn route(&self, d: &Deployment, r: &Run<'_>, weights: &[Weight]) -> Done {
        let before = self.deploys.list_slots(r.app.id).await?;

        for w in weights {
            let server = server_of(&w.server_id)?;
            let name = Slot::name(w.slot);
            let mut row = match before.iter().find(|s| s.server_id == server && s.slot == name) {
                Some(s) => s.clone(),
                // The single instance a service had before its first
                // rollout: it serves on the service's own ports.
                None if w.slot.is_none() => SlotRecord {
                    service_id: r.app.id,
                    server_id: server,
                    slot: String::new(),
                    release_number: d.from_release,
                    weight: 100,
                    state: "ready".into(),
                    ports: json!({}),
                    updated_at: self.clock.now(),
                },
                None => continue,
            };

            row.weight = w.weight;
            row.updated_at = self.clock.now();
            self.deploys.put_slot(&row).await?;
        }

        let applied = self.effects()?.reapply_endpoints(r.app.id).await.map_err(|e| Stop::Failed(e.to_string()))?;
        let after = self.deploys.list_slots(r.app.id).await?;
        let mut nodes: Vec<ServerId> = weights.iter().filter_map(|w| w.server_id.parse().ok()).collect();
        nodes.dedup();

        for n in nodes {
            let was = crate::deploy::serving(&before, n).map(|s| s.slot.clone());
            let now = crate::deploy::serving(&after, n).map(|s| s.slot.clone());
            if was != now {
                self.effects()?.push_health(r.app.id, n).await;
            }
        }

        let names: BTreeMap<String, String> = r.payload.names.clone();
        let line = Step::Route { weights: weights.to_vec() }.describe(&names);
        let via = if applied.is_empty() { String::new() } else { format!(" (endpoints: {})", applied.join(", ")) };
        self.event(d.id, "info", None, format!("{line}{via}")).await;

        Ok(())
    }

    async fn wait(
        &self,
        d: &Deployment,
        r: &Run<'_>,
        secs: u64,
        why: &str,
        analysis: Option<&cat::Analysis>,
        new: &[Target],
    ) -> Done {
        self.event(d.id, "info", None, format!("waiting {secs}s: {why}")).await;
        let since = self.clock.now();
        let end = tokio::time::Instant::now() + Duration::from_secs(secs);
        let mut next_look = tokio::time::Instant::now() + ANALYSIS_EVERY;

        loop {
            self.aborted(d.id).await?;
            let now = tokio::time::Instant::now();
            let last = now >= end;

            if let Some(a) = analysis
                && (now >= next_look || last)
            {
                next_look = now + ANALYSIS_EVERY;
                let sample =
                    self.effects()?.traffic(r.app.id, new, since).await.map_err(|e| Stop::Retry(e.to_string()))?;

                match judge(a, &sample) {
                    Verdict::Fail(m) => return Err(Stop::Failed(format!("{why}: {m}"))),
                    Verdict::TooLittle if last => {
                        self.event(d.id, "warn", None, format!("{why}: {} requests reached the new release, fewer than the {} the gates need; going on", sample.requests, a.min_requests)).await;
                    }
                    Verdict::Pass if last => {
                        self.event(
                            d.id,
                            "info",
                            None,
                            format!(
                                "{why}: {} requests, {} errors, p95 {} ms",
                                sample.requests,
                                sample.errors,
                                sample.p95_ms.map_or("–".into(), |p| p.to_string())
                            ),
                        )
                        .await;
                    }
                    _ => {}
                }
            }

            if last {
                return Ok(());
            }

            tokio::time::sleep(self.tick.min(end.saturating_duration_since(now)).max(Duration::from_millis(10))).await;
        }
    }

    pub(super) async fn approve(&self, d: &mut Deployment, r: &Run<'_>, reason: &str) -> Done {
        d.state = DeploymentState::AwaitingApproval;
        self.save(d).await?;
        self.event(d.id, "info", None, format!("waiting for approval: {reason}")).await;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(r.policy.approval_timeout_secs);

        loop {
            let cur = self.deploys.get_deployment(d.id).await?.ok_or_else(|| Stop::Failed("deployment gone".into()))?;
            match cur.request {
                Some(DeploymentRequest::Approve) => {
                    self.deploys.clear_request(d.id).await?;
                    d.state = DeploymentState::Running;
                    self.save(d).await?;

                    return Ok(());
                }
                Some(DeploymentRequest::Reject) => {
                    self.deploys.clear_request(d.id).await?;
                    return Err(Stop::Aborted(format!("rejected ({reason})")));
                }
                Some(DeploymentRequest::Abort) => {
                    self.deploys.clear_request(d.id).await?;
                    return Err(Stop::Aborted("aborted on request".into()));
                }
                _ => {}
            }

            if tokio::time::Instant::now() >= deadline {
                return Err(Stop::Failed(format!(
                    "nobody approved within {}s ({reason})",
                    r.policy.approval_timeout_secs
                )));
            }

            tokio::time::sleep(self.tick).await;
        }
    }

    async fn stop_one(&self, d: &Deployment, r: &Run<'_>, t: &Target) -> Done {
        let server = server_of(&t.server_id)?;
        let (spec, _) = self.copy_spec(r, t).await?;
        self.runner.send(&r.principal, r.app.workspace_id, server, "retire", &spec, RETIRE_WAIT).await?;
        self.deploys.delete_slot(r.app.id, server, Slot::name(t.slot)).await?;
        self.event(d.id, "info", Some(server), format!("stopped {}", r.copy(t))).await;

        Ok(())
    }

    async fn step(&self, d: &mut Deployment, r: &Run<'_>, plan: &Plan, step: &Step) -> Done {
        match step {
            Step::Start { targets } => {
                futures::future::try_join_all(targets.iter().map(|t| self.start_one(d, r, t))).await?;
            }
            Step::Gate { targets } => {
                futures::future::try_join_all(targets.iter().map(|t| self.gate_one(d, r, t))).await?;
            }
            Step::Route { weights } => self.route(d, r, weights).await?,
            Step::Wait { secs, why, analysis } => {
                let new: Vec<Target> = plan.targets.clone();
                self.wait(d, r, *secs, why, analysis.as_ref(), &new).await?;
            }
            Step::Approve { reason } => self.approve(d, r, reason).await?,
            Step::Stop { targets } => {
                futures::future::try_join_all(targets.iter().map(|t| self.stop_one(d, r, t))).await?;
            }
        }

        Ok(())
    }

    async fn load(&self, p: &Payload) -> Result<(Deployment, Service, Release, Value, RolloutPolicy), StepError> {
        let retry = |e: forge_core::DomainError| StepError::Retryable(e.to_string());
        let d = self
            .deploys
            .get_deployment(p.deployment_id)
            .await
            .map_err(retry)?
            .ok_or_else(|| StepError::Fatal("deployment not found".into()))?;
        let app = self
            .apps
            .get_service(d.service_id)
            .await
            .map_err(retry)?
            .ok_or_else(|| StepError::Fatal("service not found".into()))?;
        let release = self
            .releases
            .get_release(d.release_id)
            .await
            .map_err(retry)?
            .ok_or_else(|| StepError::Fatal("release not found".into()))?;
        let plan = d.plan.clone();
        let policy: RolloutPolicy =
            serde_json::from_value(release.policy.clone()).map_err(|e| StepError::Fatal(format!("bad policy: {e}")))?;

        Ok((d, app, release, plan, policy))
    }

    async fn rollout(&self, ctx: &mut StepContext) -> Result<(), StepError> {
        let p: Payload = serde_json::from_value(ctx.job.payload.clone())
            .map_err(|e| StepError::Fatal(format!("bad payload: {e}")))?;
        let (mut d, app, release, plan, policy) = self.load(&p).await?;
        if !d.state.active() {
            return Ok(());
        }

        let r = Run { principal: ctx.job.principal.to_string(), app, release, policy, payload: &p };

        if let Some(cluster) = p.swarm_cluster {
            return self.swarm_rollout(ctx, &mut d, &r, cluster, plan).await;
        }

        let plan: Plan = serde_json::from_value(plan).map_err(|e| StepError::Fatal(format!("bad plan: {e}")))?;

        if d.state == DeploymentState::Pending {
            d.state = DeploymentState::Running;
            self.save(&mut d).await.map_err(stop_error)?;
            self.event(
                d.id,
                "info",
                None,
                format!("rolling out release {} ({}, {} steps)", d.release_number, d.strategy, plan.steps.len()),
            )
            .await;
        }

        let mut outcome = Ok(());

        for (i, step) in plan.steps.iter().enumerate().skip(usize::try_from(d.step).unwrap_or(0)) {
            if let Err(e) = self.checkpoint(&mut d).await {
                outcome = Err(e);
                break;
            }

            if let Err(e) = self.step(&mut d, &r, &plan, step).await {
                outcome = Err(e);
                break;
            }

            d.step = i64::try_from(i + 1).unwrap_or(i64::MAX);

            if let Err(e) = self.save(&mut d).await {
                outcome = Err(e);
                break;
            }
        }

        match outcome {
            Ok(()) => Ok(()),
            // Again later, unless this was the last attempt.
            Err(Stop::Retry(m)) if ctx.job.attempts + 1 < ctx.job.max_attempts => {
                self.event(d.id, "warn", None, format!("{m}; trying again")).await;
                Err(StepError::Retryable(m))
            }
            Err(Stop::Retry(m) | Stop::Failed(m)) => {
                self.fail(&mut d, &r, &plan, &m, false, ctx).await;
                Err(StepError::Fatal(m))
            }
            Err(Stop::Aborted(m)) => {
                self.fail(&mut d, &r, &plan, &m, true, ctx).await;
                Err(StepError::Fatal(m))
            }
        }
    }

    /// Ends a failed or aborted rollout: removes new copies that never took
    /// traffic, frees the service and, when copies that took traffic remain
    /// and the policy allows, starts a rollback to the previous release.
    async fn fail(&self, d: &mut Deployment, r: &Run<'_>, plan: &Plan, why: &str, aborted: bool, ctx: &StepContext) {
        self.event(d.id, "error", None, why.to_owned()).await;
        let slots = self.deploys.list_slots(r.app.id).await.unwrap_or_default();
        let started: Vec<&Target> =
            plan.steps.iter().take(usize::try_from(d.step + 1).unwrap_or(0)).flat_map(Step::started).collect();
        let mut switched = false;

        for t in started {
            let Ok(server) = server_of(&t.server_id) else { continue };
            let name = Slot::name(t.slot);
            let row = slots.iter().find(|s| s.server_id == server && s.slot == name);
            let took_traffic = row.is_some_and(|s| s.weight > 0) || t.slot.is_none() || !plan.slotted;
            if took_traffic {
                switched |= row.is_none_or(|s| s.release_number == d.release_number);
                continue;
            }

            match self.stop_one(d, r, t).await {
                Ok(()) => {}
                Err(Stop::Retry(m) | Stop::Failed(m) | Stop::Aborted(m)) => {
                    self.event(d.id, "warn", Some(server), format!("could not remove {}: {m}", r.copy(t))).await;
                }
            }
        }

        let rollback =
            !aborted && r.policy.auto_rollback && d.kind == DeploymentKind::Apply && d.from_release > 0 && switched;
        d.state = if aborted { DeploymentState::Aborted } else { DeploymentState::Failed };
        d.error = Some(why.to_owned());
        d.finished_at = Some(self.clock.now());

        if let Err(e) = self.save(d).await {
            tracing::error!(deployment = %d.id, error = ?e, "could not record the failed deployment");
            return;
        }

        let action = if aborted { "deployment.aborted" } else { "deployment.failed" };
        self.audit(
            &r.principal,
            d,
            action,
            false,
            json!({ "service": r.app.name, "release": d.release_number, "error": why }),
        )
        .await;

        if !rollback {
            return;
        }

        let Some(engine) = self.engine.get() else { return };
        let started = async {
            let prepared = self.service.prepare_release(&r.app, d.from_release).await?;
            let who = Starter {
                principal: ctx.job.principal.clone(),
                context: ctx.job.context,
                privileged_mounts: r.payload.privileged_mounts.clone(),
            };
            self.service.start(prepared, DeploymentKind::AutoRollback, &who, engine).await
        };

        match started.await {
            Ok(back) => {
                d.state = DeploymentState::RolledBack;
                let _ = self.save(d).await;
                self.event(
                    d.id,
                    "info",
                    None,
                    format!("rolling back to release {} (deployment {})", d.from_release, back.id),
                )
                .await;
            }
            Err(e) => {
                self.event(
                    d.id,
                    "error",
                    None,
                    format!("the rollback to release {} could not start: {e}", d.from_release),
                )
                .await
            }
        }
    }

    async fn finalize(&self, ctx: &mut StepContext) -> Result<(), StepError> {
        let p: Payload = serde_json::from_value(ctx.job.payload.clone())
            .map_err(|e| StepError::Fatal(format!("bad payload: {e}")))?;
        let (mut d, mut app, release, plan, policy) = self.load(&p).await?;
        if !d.state.active() {
            return Ok(());
        }

        let retry = |e: forge_core::DomainError| StepError::Retryable(e.to_string());

        // The service now is the release.
        let live = release_app(&app, &release);
        app.spec = live.spec;
        app.env = live.env;
        app.runtime = live.runtime;
        app.updated_at = self.clock.now();
        self.apps.put_service(&app).await.map_err(retry)?;

        if let Some(engine) = self.engine.get() {
            for server in &p.remove {
                let job = engine
                    .submit(NewJob {
                        kind: "app.action".into(),
                        workspace_id: Some(app.workspace_id),
                        resource: EntityRef::new(EntityKind::Service, app.id),
                        principal: ctx.job.principal.clone(),
                        context: ctx.job.context,
                        idempotency_key: None,
                        payload: json!({ "service_id": app.id, "server_id": server, "action": "remove", "tail": 0, "cursor": "", "privileged_mounts": [] }),
                        max_attempts: 2,
                    })
                    .await;

                let line = match job {
                    Ok(j) => format!(
                        "removing the service from {} (job {})",
                        p.names.get(&server.to_string()).cloned().unwrap_or_else(|| server.to_string()),
                        j.id
                    ),
                    Err(e) => format!("could not remove the service from {server}: {e}"),
                };

                self.event(d.id, "info", Some(*server), line).await;
            }
        }

        let pruned = self
            .releases
            .prune_releases(app.id, policy.retain_releases, &[release.number, d.from_release])
            .await
            .map_err(retry)?;

        if pruned > 0 {
            self.event(
                d.id,
                "info",
                None,
                format!("deleted {pruned} old releases (keeping {})", policy.retain_releases),
            )
            .await;
        }

        d.state = DeploymentState::Succeeded;
        d.finished_at = Some(self.clock.now());
        self.save(&mut d).await.map_err(stop_error)?;

        // Every copy now runs the vault values as they were when it started (D-179).
        self.apps.clear_secrets_changed(app.id, ctx.job.created_at).await.map_err(retry)?;
        self.event(d.id, "info", None, format!("release {} is live", d.release_number)).await;
        let principal = ctx.job.principal.to_string();
        self.audit(
            &principal,
            &d,
            "deployment.succeeded",
            true,
            json!({ "service": app.name, "release": d.release_number, "from": d.from_release }),
        )
        .await;

        if let (Some(fx), Ok(plan)) = (self.effects.get(), serde_json::from_value::<Plan>(plan)) {
            for t in &plan.targets {
                if let Ok(server) = t.server_id.parse() {
                    fx.push_health(app.id, server).await;
                }
            }
        }

        Ok(())
    }
}

fn stop_error(s: Stop) -> StepError {
    match s {
        Stop::Retry(m) => StepError::Retryable(m),
        Stop::Failed(m) | Stop::Aborted(m) => StepError::Fatal(m),
    }
}

/// The service's own published ports (or listen port), mapped to themselves.
fn in_place_ports(spec: &AppSpec) -> BTreeMap<u16, u16> {
    let ports = match spec.runtime {
        forge_catalog::Runtime::Container => cat::published_host_ports(&spec.ports),
        _ => spec.listen_port.into_iter().collect(),
    };

    ports.into_iter().map(|p| (p, p)).collect()
}

/// The failing checks of a probe answer, for people.
fn failing_checks(out: &Value) -> String {
    let failing: Vec<String> = out["checks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| c["ok"] == false)
        .map(|c| {
            let detail = c["detail"].as_str().unwrap_or_default();
            if detail.is_empty() {
                c["name"].as_str().unwrap_or("?").to_owned()
            } else {
                format!("{}: {detail}", c["name"].as_str().unwrap_or("?"))
            }
        })
        .collect();
    if failing.is_empty() { "not ready".into() } else { failing.join("; ") }
}

#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    Pass,
    TooLittle,
    Fail(String),
}

/// A canary's gates against the new release's traffic.
fn judge(a: &cat::Analysis, s: &crate::deploy::TrafficSample) -> Verdict {
    if s.requests < u64::from(a.min_requests) {
        return Verdict::TooLittle;
    }

    if let Some(max) = a.max_error_rate {
        let pct = s.errors * 100 / s.requests.max(1);
        if pct > u64::from(max) {
            return Verdict::Fail(format!("{pct}% of {} requests failed (at most {max}%)", s.requests));
        }
    }

    if let (Some(max), Some(p95)) = (a.max_p95_ms, s.p95_ms)
        && p95 > u64::from(max)
    {
        return Verdict::Fail(format!("p95 {p95} ms (at most {max} ms)"));
    }

    Verdict::Pass
}

#[async_trait]
impl JobHandler for DeployJob {
    fn kind(&self) -> &'static str {
        crate::deploy::ROLLOUT_KIND
    }

    fn steps(&self) -> &'static [&'static str] {
        &["rollout", "finalize"]
    }

    fn required_action(&self, payload: &Value) -> Action {
        if payload.get("privileged_mounts").and_then(Value::as_array).is_some_and(|m| !m.is_empty()) {
            return Action::StackPrivileged;
        }

        Action::Deploy
    }

    async fn run_step(&self, index: usize, ctx: &mut StepContext) -> Result<(), StepError> {
        match index {
            0 => self.rollout(ctx).await,
            _ => self.finalize(ctx).await,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::deploy::TrafficSample;

    #[test]
    fn canary_gates_judge_errors_latency_and_volume() {
        let a = cat::Analysis { max_error_rate: Some(1), max_p95_ms: Some(300), min_requests: 20 };
        let s = |requests, errors, p95| TrafficSample { requests, errors, p95_ms: p95 };
        assert_eq!(judge(&a, &s(10, 9, None)), Verdict::TooLittle);
        assert_eq!(judge(&a, &s(200, 2, Some(120))), Verdict::Pass, "1% is allowed");
        assert!(matches!(judge(&a, &s(200, 4, Some(120))), Verdict::Fail(m) if m.contains("2%")));
        assert!(matches!(judge(&a, &s(200, 0, Some(900))), Verdict::Fail(m) if m.contains("p95 900")));
    }

    #[test]
    fn probe_answers_name_the_failing_checks() {
        let out = json!({ "ready": false, "checks": [
            { "name": "default", "ok": true, "detail": "" },
            { "name": "http", "ok": false, "detail": "status 503" },
            { "name": "cmd", "ok": null, "detail": "checked by the node's health runner" },
        ] });
        assert_eq!(failing_checks(&out), "http: status 503");
        assert_eq!(failing_checks(&json!({ "ready": false })), "not ready");
    }

    #[test]
    fn in_place_copies_keep_the_service_ports() {
        let spec: AppSpec = serde_json::from_value(json!({
            "environment": "prod", "name": "web", "runtime": "container", "image": "nginx:1", "ports": ["8080:80", "9090:90/udp"],
        }))
        .unwrap();
        assert_eq!(in_place_ports(&spec), BTreeMap::from([(8080, 8080), (9090, 9090)]));
    }
}
