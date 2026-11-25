//! Rollouts in Docker Swarm clusters (D-156): the `deploy.rollout` job's
//! executor for a [`SwarmPlan`]. It shares the deployment record, events,
//! pause, abort and approval handling with node rollouts
//! ([`super::deploy::DeployJob`]); only the steps differ.
//!
//! * `Deploy` builds the release's typed service definition (the slot's
//!   own ports for Blue-Green and Canary), re-checks host binds against
//!   `stack.privileged`, and creates or updates the slot's Swarm service.
//! * `Gate` waits until the slot runs all its tasks of the *new*
//!   definition (tasks carry its fingerprint), healthy.
//! * `Route` writes the slots' traffic weights for the swarm's nodes (the
//!   routing mesh answers on each; rows of nodes that left are removed) and
//!   re-applies the sites in front of the service, so the edge moves traffic.
//! * `Wait` watches the new slot; tasks that stop running fail the rollout.
//!
//! Failure and abort never rely on Swarm's "previous spec" (a re-sent update
//! can make it the new one):
//! * unslotted: Forge applies the release that served before again. An
//!   identical definition is a no-op on the node, so this is safe even when
//!   nothing was applied yet;
//! * slotted, before all traffic moved: the old slot is scaled back up and
//!   checked before traffic returns to it, then the new slot goes to zero;
//! * slotted, after all traffic moved: the new release serves; nothing is
//!   moved back (the old slot may already be at zero).
//!
//! Every step can run again: identical definitions are skipped on the node,
//! and scaling and weights are absolute.

use std::collections::BTreeMap;
use std::time::Duration;

use forge_catalog::deploy::{ArtifactRef, SLOT_PORTS};
use forge_catalog::swarm::{SwarmOp, SwarmServiceStatus};
use forge_catalog::swarm_rollout::{SwarmPlan, SwarmStep};
use forge_catalog::swarm_service::{ServiceDef, UpdatePolicy, service_def};
use forge_core::deploy::{Deployment, DeploymentKind, DeploymentState, Release, SlotRecord};
use forge_core::swarm::{MemberRole, MemberState};
use forge_core::{ClusterId, ServerId};
use forge_jobs::{StepContext, StepError};
use forge_proto::Command;
use forge_proto::messages::SwarmAction;
use serde_json::{Value, json};

use super::deploy::{DeployJob, Done, Run, Stop};
use crate::deploy::release_app;
use crate::jobs::swarm_service::{Progress, ServiceAction, progress};

const OP_WAIT: Duration = Duration::from_secs(60);
/// The state of a slot that serves.
const SLOT_READY: &str = "ready";
const POLL: Duration = Duration::from_secs(3);

/// Job-state and operation keys (one definition each).
const KEY_STATUS: &str = "status_op";
const KEY_RESTORE_APPLY: &str = "restore_apply_op";
const KEY_RESTORE_SCALE_OLD: &str = "restore_scale_old_op";
const KEY_RESTORE_SCALE_NEW: &str = "restore_scale_new_op";

fn ports_key(slot: &str) -> String {
    format!("ports_{slot}")
}

fn failed(e: impl std::fmt::Display) -> Stop {
    Stop::Failed(e.to_string())
}

fn slot_name(slot: &str) -> String {
    if slot.is_empty() { "the service".into() } else { format!("slot {slot}") }
}

/// What every step of one Swarm rollout needs.
struct Rollout<'a> {
    run: &'a Run<'a>,
    cluster: ClusterId,
    plan: &'a SwarmPlan,
}

/// How a definition is built for one apply.
struct DefFor<'a> {
    release: &'a Release,
    slot: &'a str,
    replicas: u32,
    update: Option<&'a UpdatePolicy>,
}

impl DeployJob {
    pub(super) async fn swarm_rollout(
        &self,
        ctx: &mut StepContext,
        d: &mut Deployment,
        r: &Run<'_>,
        cluster: ClusterId,
        plan: Value,
    ) -> Result<(), StepError> {
        let plan: SwarmPlan = serde_json::from_value(plan).map_err(|e| StepError::Fatal(format!("bad plan: {e}")))?;

        if d.state == DeploymentState::Pending {
            d.state = DeploymentState::Running;
            self.save(d).await.map_err(stop_error)?;
            let line = format!(
                "rolling out release {} on Docker Swarm ({}, {} steps)",
                d.release_number,
                d.strategy,
                plan.steps.len()
            );
            self.event(d.id, "info", None, line).await;
        }

        let ro = Rollout { run: r, cluster, plan: &plan };
        let mut outcome = Ok(());

        for (i, step) in plan.steps.iter().enumerate().skip(usize::try_from(d.step).unwrap_or(0)) {
            if let Err(e) = self.checkpoint(d).await {
                outcome = Err(e);
                break;
            }

            if let Err(e) = self.swarm_step(ctx, d, &ro, i, step).await {
                outcome = Err(e);
                break;
            }

            d.step = i64::try_from(i + 1).unwrap_or(i64::MAX);

            if let Err(e) = self.save(d).await {
                outcome = Err(e);
                break;
            }
        }

        match outcome {
            Ok(()) => Ok(()),
            Err(Stop::Retry(m)) if ctx.job.attempts + 1 < ctx.job.max_attempts => {
                self.event(d.id, "warn", None, format!("{m}; trying again")).await;
                Err(StepError::Retryable(m))
            }
            Err(Stop::Retry(m) | Stop::Failed(m)) => {
                self.swarm_fail(ctx, d, &ro, &m, false).await;
                Err(StepError::Fatal(m))
            }
            Err(Stop::Aborted(m)) => {
                self.swarm_fail(ctx, d, &ro, &m, true).await;
                Err(StepError::Fatal(m))
            }
        }
    }

    async fn manager(&self, cluster: ClusterId) -> Done<ServerId> {
        self.swarms
            .list_members(cluster)
            .await?
            .into_iter()
            .find(|m| m.role == MemberRole::Manager && m.state == MemberState::Joined)
            .map(|m| m.server_id)
            .ok_or_else(|| Stop::Failed("the cluster has no swarm manager".into()))
    }

    async fn swarm_op(&self, ctx: &mut StepContext, cluster: ClusterId, key: &str, op: &SwarmOp) -> Done<Value> {
        op.validate().map_err(failed)?;
        let manager = self.manager(cluster).await?;
        let op_json = serde_json::to_string(op).map_err(failed)?;

        Ok(self.ops.run(ctx, manager, key, Command::SwarmAction(SwarmAction { op_json }), OP_WAIT).await?)
    }

    async fn slot_status(
        &self,
        ctx: &mut StepContext,
        cluster: ClusterId,
        sid: &str,
        slot: &str,
    ) -> Done<SwarmServiceStatus> {
        let op = SwarmOp::ServiceStatus { service_id: sid.to_owned(), slot: slot.to_owned() };
        serde_json::from_value(self.swarm_op(ctx, cluster, KEY_STATUS, &op).await?).map_err(failed)
    }

    /// Waits until `slot` runs all its tasks of its current definition.
    async fn gate(&self, ctx: &mut StepContext, d: &Deployment, ro: &Rollout<'_>, slot: &str) -> Done {
        let (r, cluster) = (ro.run, ro.cluster);
        let sid = r.app.id.to_string();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(r.policy.startup_timeout_secs.max(30));

        loop {
            self.aborted(d.id).await?;
            let s = self.slot_status(ctx, cluster, &sid, slot).await?;
            match progress(ServiceAction::Deploy, &s) {
                Progress::Done => {
                    let line = format!("{} runs {} of {} tasks", slot_name(slot), s.running_current, s.desired);
                    self.event(d.id, "info", None, line).await;

                    return Ok(());
                }
                Progress::Failed(why) => return Err(Stop::Failed(why)),
                Progress::Running if tokio::time::Instant::now() >= deadline => {
                    return Err(Stop::Failed(format!(
                        "{} did not become ready within {} s: {} of {} tasks running",
                        slot_name(slot),
                        r.policy.startup_timeout_secs,
                        s.running_current,
                        s.desired
                    )));
                }
                Progress::Running => tokio::time::sleep(POLL).await,
            }
        }
    }

    async fn swarm_step(
        &self,
        ctx: &mut StepContext,
        d: &mut Deployment,
        ro: &Rollout<'_>,
        index: usize,
        step: &SwarmStep,
    ) -> Done {
        let (r, cluster, plan) = (ro.run, ro.cluster, ro.plan);
        let sid = r.app.id.to_string();

        match step {
            SwarmStep::Deploy { slot, replicas } => {
                let release = self.fresh_release(r).await?;
                let f = DefFor { release: &release, slot, replicas: *replicas, update: Some(&plan.update) };
                let def = self.swarm_def(ctx, ro, &f).await?;
                let net = SwarmOp::EnsureNetwork { environment_id: r.app.environment_id.to_string() };
                self.swarm_op(ctx, cluster, &format!("network_op_{index}"), &net).await?;
                let registry_auth = self.registries.auth_for(r.app.workspace_id, &def.image).await?;
                let apply = SwarmOp::ServiceApply { def: Box::new(def), registry_auth };
                self.swarm_op(ctx, cluster, &format!("apply_op_{index}"), &apply).await?;
                let line = format!("release {} applied to {} ({replicas} tasks)", d.release_number, slot_name(slot));
                self.event(d.id, "info", None, line).await;
                Ok(())
            }
            SwarmStep::Gate { slot } => self.gate(ctx, d, ro, slot).await,
            SwarmStep::Scale { slot, replicas } => {
                let op = SwarmOp::ServiceScale { service_id: sid, slot: slot.clone(), replicas: *replicas };
                self.swarm_op(ctx, cluster, &format!("scale_op_{index}"), &op).await?;
                self.event(d.id, "info", None, format!("scaled {} to {replicas} tasks", slot_name(slot))).await;
                Ok(())
            }
            SwarmStep::Route { slot, weight, other } => {
                self.swarm_route(ctx, d, ro, slot, *weight, other).await?;
                self.event(d.id, "info", None, step.describe()).await;
                Ok(())
            }
            SwarmStep::Wait { secs } => {
                self.event(d.id, "info", None, step.describe()).await;
                let end = tokio::time::Instant::now() + Duration::from_secs(*secs);

                while tokio::time::Instant::now() < end {
                    self.aborted(d.id).await?;
                    let s = self.slot_status(ctx, cluster, &sid, &plan.slot).await?;
                    if !s.exists {
                        return Err(Stop::Failed(format!("{} is gone from the swarm", slot_name(&plan.slot))));
                    }

                    if s.running < s.desired {
                        return Err(Stop::Failed(format!(
                            "{} dropped to {} of {} running tasks",
                            slot_name(&plan.slot),
                            s.running,
                            s.desired
                        )));
                    }

                    let left = end.saturating_duration_since(tokio::time::Instant::now());
                    tokio::time::sleep(POLL.min(left).max(Duration::from_millis(10))).await;
                }

                Ok(())
            }
            SwarmStep::Approve { reason } => self.approve(d, r, reason).await,
        }
    }

    /// The typed definition of `f.release` for one slot, with host binds
    /// re-checked against `stack.privileged`.
    async fn swarm_def(&self, ctx: &mut StepContext, ro: &Rollout<'_>, f: &DefFor<'_>) -> Done<ServiceDef> {
        let (r, cluster) = (ro.run, ro.cluster);
        let mut spec = self.runner.resolved_spec(&release_app(&r.app, f.release)).await?;

        // A release without a recorded artifact keeps the spec's image, as
        // node rollouts do (`copy_spec`); a recorded one must parse.
        if !f.release.artifact.is_null() {
            let a: ArtifactRef = serde_json::from_value(f.release.artifact.clone())
                .map_err(|e| Stop::Failed(format!("the release's artifact is unreadable: {e}")))?;

            if let Some(image) = a.image_ref() {
                spec.image = image;
            }
        }

        let mut settings = spec.swarm.clone().unwrap_or_default();
        settings.replicas = f.replicas;

        if let Some(u) = f.update {
            settings.update = u.clone();
        }

        let sid = r.app.id.to_string();
        let mut def = service_def(&spec, &settings, &sid, &r.app.environment_id.to_string()).map_err(failed)?;
        let needed = self.swarm.privileged_binds(cluster, &def.bind_sources()).await?;
        if let Some(b) = needed.iter().find(|b| !r.payload.privileged_mounts.contains(b)) {
            return Err(Stop::Failed(format!(
                "host folder `{b}` is outside the approved file roots of the swarm's nodes; it needs \
                 stack.privileged (an administrator with MFA)"
            )));
        }

        if !f.slot.is_empty() {
            let count = u32::try_from(def.ports.len()).unwrap_or(u32::MAX);
            let range = (*SLOT_PORTS.start(), *SLOT_PORTS.end());
            let own = self.deploys.slot_ports(r.app.id, f.slot, count, range).await?;
            let by_target: BTreeMap<u16, u16> = def.ports.iter().zip(&own).map(|(p, o)| (p.target, *o)).collect();

            // Service port (published, else container port) → slot port, as
            // node rollouts record it for the edge.
            let service_ports: BTreeMap<String, u16> =
                def.ports.iter().zip(&own).map(|(p, o)| (p.published.unwrap_or(p.target).to_string(), *o)).collect();
            ctx.set_state(&ports_key(f.slot), json!(service_ports));
            def = def.into_slot(f.slot, &by_target).map_err(failed)?;
        }

        Ok(def)
    }

    /// Writes `weight` for `slot` and the rest for `other` on every node of
    /// the swarm (and drops rows of nodes that are not in it), then
    /// re-applies the sites in front of the service.
    async fn swarm_route(
        &self,
        ctx: &StepContext,
        d: &Deployment,
        ro: &Rollout<'_>,
        slot: &str,
        weight: u32,
        other: &str,
    ) -> Done {
        let (r, cluster) = (ro.run, ro.cluster);
        let members: Vec<ServerId> = self
            .swarms
            .list_members(cluster)
            .await?
            .into_iter()
            .filter(|m| m.state == MemberState::Joined)
            .map(|m| m.server_id)
            .collect();
        let rows = self.deploys.list_slots(r.app.id).await?;
        let ports_of = |s: &str| -> Value {
            ctx.state_value(&ports_key(s))
                .cloned()
                .or_else(|| rows.iter().find(|x| x.slot == s).map(|x| x.ports.clone()))
                .unwrap_or_else(|| json!({}))
        };
        let other_release = rows.iter().find(|x| x.slot == other).map_or(d.from_release, |x| x.release_number);
        let now = self.clock.now();

        // The whole routing change at once: other slots of member nodes
        // stay, nodes that left the swarm lose their rows.
        let mut next: Vec<SlotRecord> = rows
            .iter()
            .filter(|x| members.contains(&x.server_id) && x.slot != slot && x.slot != other)
            .cloned()
            .collect();

        for server in &members {
            next.push(SlotRecord {
                service_id: r.app.id,
                server_id: *server,
                slot: slot.to_owned(),
                release_number: d.release_number,
                weight,
                state: SLOT_READY.into(),
                ports: ports_of(slot),
                updated_at: now,
            });

            if !other.is_empty() {
                next.push(SlotRecord {
                    service_id: r.app.id,
                    server_id: *server,
                    slot: other.to_owned(),
                    release_number: other_release,
                    weight: 100 - weight.min(100),
                    state: SLOT_READY.into(),
                    ports: ports_of(other),
                    updated_at: now,
                });
            }
        }

        self.deploys.replace_slots(r.app.id, &next).await?;
        self.effects()?.reapply_endpoints(r.app.id).await?;

        Ok(())
    }

    /// The index of the step that moved all traffic to the new slot.
    fn full_route_index(plan: &SwarmPlan) -> Option<usize> {
        plan.steps.iter().position(|s| matches!(s, SwarmStep::Route { slot, weight: 100, .. } if *slot == plan.slot))
    }

    /// Puts the service back to what served before (see the module docs).
    /// `Ok(true)` when something was restored.
    async fn swarm_restore(&self, ctx: &mut StepContext, d: &Deployment, ro: &Rollout<'_>) -> Done<bool> {
        let (r, cluster, plan) = (ro.run, ro.cluster, ro.plan);
        let sid = r.app.id.to_string();

        if plan.slot.is_empty() {
            if d.from_release == 0 {
                return Ok(false);
            }

            let previous = self
                .releases
                .release_by_number(r.app.id, d.from_release)
                .await?
                .ok_or_else(|| Stop::Failed(format!("release {} is gone", d.from_release)))?;
            let replicas = previous.spec["swarm"]["replicas"].as_u64().map_or(1, |n| u32::try_from(n).unwrap_or(1));
            let f = DefFor { release: &previous, slot: "", replicas, update: None };
            let def = self.swarm_def(ctx, ro, &f).await?;
            let registry_auth = self.registries.auth_for(r.app.workspace_id, &def.image).await?;
            let apply = SwarmOp::ServiceApply { def: Box::new(def), registry_auth };
            self.swarm_op(ctx, cluster, KEY_RESTORE_APPLY, &apply).await?;

            return Ok(true);
        }

        let done = usize::try_from(d.step).unwrap_or(0);
        if Self::full_route_index(plan).is_some_and(|i| i < done) {
            // All traffic is on the new release already: leave it there.
            return Ok(false);
        }

        let Some(old) = plan.from_slot.as_deref().filter(|o| !o.is_empty()) else {
            let s = self.slot_status(ctx, cluster, &sid, &plan.slot).await?;
            if s.exists {
                let op = SwarmOp::ServiceScale { service_id: sid, slot: plan.slot.clone(), replicas: 0 };
                self.swarm_op(ctx, cluster, KEY_RESTORE_SCALE_NEW, &op).await?;
            }

            return Ok(false);
        };

        let full = plan
            .steps
            .iter()
            .filter_map(|s| match s {
                SwarmStep::Deploy { replicas, .. } | SwarmStep::Scale { replicas, .. } => Some(*replicas),
                _ => None,
            })
            .max()
            .unwrap_or(1);
        let op = SwarmOp::ServiceScale { service_id: sid.clone(), slot: old.to_owned(), replicas: full };
        self.swarm_op(ctx, cluster, KEY_RESTORE_SCALE_OLD, &op).await?;
        self.gate(ctx, d, ro, old).await?;
        self.swarm_route(ctx, d, ro, old, 100, &plan.slot).await?;
        let op = SwarmOp::ServiceScale { service_id: sid, slot: plan.slot.clone(), replicas: 0 };
        self.swarm_op(ctx, cluster, KEY_RESTORE_SCALE_NEW, &op).await?;

        Ok(true)
    }

    /// Ends a failed or aborted Swarm rollout.
    async fn swarm_fail(&self, ctx: &mut StepContext, d: &mut Deployment, ro: &Rollout<'_>, why: &str, aborted: bool) {
        let r = ro.run;
        self.event(d.id, "error", None, why.to_owned()).await;

        let restored = if aborted || r.policy.auto_rollback {
            match self.swarm_restore(ctx, d, ro).await {
                Ok(true) => {
                    self.event(d.id, "info", None, format!("release {} serves again", d.from_release)).await;
                    true
                }
                Ok(false) => {
                    self.event(d.id, "info", None, "nothing to restore".into()).await;
                    false
                }
                Err(Stop::Retry(m) | Stop::Failed(m) | Stop::Aborted(m)) => {
                    self.event(d.id, "warn", None, format!("could not restore the previous release: {m}")).await;
                    false
                }
            }
        } else {
            false
        };

        d.state = if aborted {
            DeploymentState::Aborted
        } else if restored && d.kind == DeploymentKind::Apply {
            DeploymentState::RolledBack
        } else {
            DeploymentState::Failed
        };

        d.error = Some(why.to_owned());
        d.finished_at = Some(self.clock.now());

        if let Err(e) = self.save(d).await {
            tracing::error!(deployment = %d.id, error = ?e, "could not record the failed deployment");
            return;
        }

        let action = if aborted { "deployment.aborted" } else { "deployment.failed" };
        let details = json!({ "service": r.app.name, "release": d.release_number, "error": why, "restored": restored });
        self.audit(&r.principal, d, action, false, details).await;
    }
}

fn stop_error(s: Stop) -> StepError {
    match s {
        Stop::Retry(m) => StepError::Retryable(m),
        Stop::Failed(m) | Stop::Aborted(m) => StepError::Fatal(m),
    }
}
