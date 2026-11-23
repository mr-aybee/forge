//! Rollouts in Docker Swarm clusters (D-156): the `deploy.rollout` job's
//! executor for a [`SwarmPlan`]. It shares the deployment record, events,
//! pause, abort and approval handling with node rollouts
//! ([`super::deploy::DeployJob`]); only the steps differ.
//!
//! * `Deploy` builds the release's typed service definition (the slot's
//!   own ports for Blue-Green and Canary), re-checks host binds against
//!   `stack.privileged`, and creates or updates the slot's Swarm service.
//! * `Gate` waits until the slot runs all its tasks, healthy.
//! * `Route` writes the slots' traffic weights for every node of the swarm
//!   (the routing mesh answers on each) and re-applies the sites in front
//!   of the service, so the edge moves traffic.
//! * `Wait` watches the new slot; tasks that stop running fail the rollout.
//!
//! On failure, traffic goes back to the slot that served before and the new
//! slot is scaled to zero; an unslotted service is rolled back by Swarm
//! (unless Swarm already did). Every step can run again: applying the same
//! definition, scaling to the same count and writing the same weights are
//! no-ops.

use std::collections::BTreeMap;
use std::time::Duration;

use forge_catalog::deploy::{ArtifactRef, SLOT_PORTS};
use forge_catalog::swarm::{SwarmOp, SwarmServiceStatus};
use forge_catalog::swarm_rollout::{SwarmPlan, SwarmStep};
use forge_catalog::swarm_service::service_def;
use forge_core::deploy::{Deployment, DeploymentKind, DeploymentState, SlotRecord};
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
const POLL: Duration = Duration::from_secs(3);

fn failed(e: impl std::fmt::Display) -> Stop {
    Stop::Failed(e.to_string())
}

/// What every step of one Swarm rollout needs.
struct Rollout<'a> {
    run: &'a Run<'a>,
    cluster: ClusterId,
    plan: &'a SwarmPlan,
}

fn slot_name(slot: &str) -> String {
    if slot.is_empty() { "the service".into() } else { format!("slot {slot}") }
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
            self.event(
                d.id,
                "info",
                None,
                format!(
                    "rolling out release {} on Docker Swarm ({}, {} steps)",
                    d.release_number,
                    d.strategy,
                    plan.steps.len()
                ),
            )
            .await;
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
        serde_json::from_value(self.swarm_op(ctx, cluster, "status_op", &op).await?).map_err(failed)
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
            SwarmStep::Deploy { slot, replicas } => self.swarm_deploy(ctx, d, ro, index, slot, *replicas).await,
            SwarmStep::Gate { slot } => {
                let deadline = tokio::time::Instant::now() + Duration::from_secs(r.policy.startup_timeout_secs.max(30));
                loop {
                    self.aborted(d.id).await?;
                    let s = self.slot_status(ctx, cluster, &sid, slot).await?;
                    match progress(ServiceAction::Deploy, &s) {
                        Progress::Done => {
                            self.event(
                                d.id,
                                "info",
                                None,
                                format!("{} runs {} of {} tasks", slot_name(slot), s.running, s.desired),
                            )
                            .await;
                            return Ok(());
                        }
                        Progress::Failed(why) => return Err(Stop::Failed(why)),
                        Progress::Running if tokio::time::Instant::now() >= deadline => {
                            return Err(Stop::Failed(format!(
                                "{} did not become ready within {} s: {} of {} tasks running",
                                slot_name(slot),
                                r.policy.startup_timeout_secs,
                                s.running,
                                s.desired
                            )));
                        }
                        Progress::Running => tokio::time::sleep(POLL).await,
                    }
                }
            }
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
                    if s.running < s.desired {
                        return Err(Stop::Failed(format!(
                            "{} dropped to {} of {} running tasks",
                            slot_name(&plan.slot),
                            s.running,
                            s.desired
                        )));
                    }
                    tokio::time::sleep(
                        POLL.min(end.saturating_duration_since(tokio::time::Instant::now()))
                            .max(Duration::from_millis(10)),
                    )
                    .await;
                }
                Ok(())
            }
            SwarmStep::Approve { reason } => self.approve(d, r, reason).await,
        }
    }

    async fn swarm_deploy(
        &self,
        ctx: &mut StepContext,
        d: &Deployment,
        ro: &Rollout<'_>,
        index: usize,
        slot: &str,
        replicas: u32,
    ) -> Done {
        let (r, cluster, plan) = (ro.run, ro.cluster, ro.plan);
        let release = self.fresh_release(r).await?;
        let mut spec = self.runner.resolved_spec(&release_app(&r.app, &release)).await?;
        // Releases without a recorded artifact keep the spec's image, as node
        // rollouts do (`copy_spec`).
        if let Some(image) =
            serde_json::from_value::<ArtifactRef>(release.artifact.clone()).ok().and_then(|a| a.image_ref())
        {
            spec.image = image;
        }
        let mut settings = spec.swarm.clone().unwrap_or_default();
        settings.replicas = replicas;
        settings.update = plan.update.clone();
        let sid = r.app.id.to_string();
        let mut def = service_def(&spec, &settings, &sid, &r.app.environment_id.to_string()).map_err(failed)?;
        // Host binds outside the swarm's approved roots need stack.privileged.
        let needed = self.swarm.privileged_binds(cluster, &def.bind_sources()).await?;
        if let Some(b) = needed.iter().find(|b| !r.payload.privileged_mounts.contains(b)) {
            return Err(Stop::Failed(format!(
                "host folder `{b}` is outside the approved file roots of the swarm's nodes; it needs stack.privileged (an administrator with MFA)"
            )));
        }
        if !slot.is_empty() {
            let count = u32::try_from(def.ports.len()).unwrap_or(u32::MAX);
            let own = self.deploys.slot_ports(r.app.id, slot, count, (*SLOT_PORTS.start(), *SLOT_PORTS.end())).await?;
            let by_target: BTreeMap<u16, u16> = def.ports.iter().zip(&own).map(|(p, o)| (p.target, *o)).collect();
            // Service port (published, else container port) → slot port, as
            // node rollouts record it for the edge.
            let service_ports: BTreeMap<String, u16> =
                def.ports.iter().zip(&own).map(|(p, o)| (p.published.unwrap_or(p.target).to_string(), *o)).collect();
            ctx.set_state(&format!("ports_{slot}"), json!(service_ports));
            def = def.into_slot(slot, &by_target).map_err(failed)?;
        }
        let net = SwarmOp::EnsureNetwork { environment_id: r.app.environment_id.to_string() };
        self.swarm_op(ctx, cluster, &format!("network_op_{index}"), &net).await?;
        let apply = SwarmOp::ServiceApply { def: Box::new(def) };
        self.swarm_op(ctx, cluster, &format!("apply_op_{index}"), &apply).await?;
        self.event(
            d.id,
            "info",
            None,
            format!("release {} applied to {} ({replicas} tasks)", d.release_number, slot_name(slot)),
        )
        .await;
        Ok(())
    }

    /// Writes `weight` for `slot` and the rest for `other` on every node of
    /// the swarm, then re-applies the sites in front of the service.
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
            ctx.state_value(&format!("ports_{s}"))
                .cloned()
                .or_else(|| rows.iter().find(|x| x.slot == s).map(|x| x.ports.clone()))
                .unwrap_or_else(|| json!({}))
        };
        let now = self.clock.now();
        for server in &members {
            self.deploys
                .put_slot(&SlotRecord {
                    service_id: r.app.id,
                    server_id: *server,
                    slot: slot.to_owned(),
                    release_number: d.release_number,
                    weight,
                    state: "ready".into(),
                    ports: ports_of(slot),
                    updated_at: now,
                })
                .await?;
            if !other.is_empty() {
                let release = rows.iter().find(|x| x.slot == other).map_or(d.from_release, |x| x.release_number);
                self.deploys
                    .put_slot(&SlotRecord {
                        service_id: r.app.id,
                        server_id: *server,
                        slot: other.to_owned(),
                        release_number: release,
                        weight: 100 - weight.min(100),
                        state: "ready".into(),
                        ports: ports_of(other),
                        updated_at: now,
                    })
                    .await?;
            }
        }
        self.effects()?.reapply_endpoints(r.app.id).await?;
        Ok(())
    }

    /// Ends a failed or aborted Swarm rollout (see the module docs).
    async fn swarm_fail(&self, ctx: &mut StepContext, d: &mut Deployment, ro: &Rollout<'_>, why: &str, aborted: bool) {
        let (r, cluster, plan) = (ro.run, ro.cluster, ro.plan);
        self.event(d.id, "error", None, why.to_owned()).await;
        let sid = r.app.id.to_string();
        let mut rolled_back = false;
        let cleanup: Done = async {
            if plan.slot.is_empty() {
                // Swarm may have rolled back on its own (failure action).
                let s = self.slot_status(ctx, cluster, &sid, "").await?;
                let swarm_did = s.update_state.starts_with("rollback");
                if !swarm_did && !aborted && r.policy.auto_rollback && d.from_release > 0 && s.exists {
                    let op = SwarmOp::ServiceRollback { service_id: sid.clone(), slot: String::new() };
                    self.swarm_op(ctx, cluster, "fail_rollback_op", &op).await?;
                }
                rolled_back = swarm_did || (!aborted && r.policy.auto_rollback && d.from_release > 0);
                return Ok(());
            }
            if let Some(old) = plan.from_slot.as_deref().filter(|o| !o.is_empty()) {
                self.swarm_route(ctx, d, ro, old, 100, &plan.slot).await?;
                rolled_back = true;
            }
            let s = self.slot_status(ctx, cluster, &sid, &plan.slot).await?;
            if s.exists {
                let op = SwarmOp::ServiceScale { service_id: sid.clone(), slot: plan.slot.clone(), replicas: 0 };
                self.swarm_op(ctx, cluster, "fail_scale_op", &op).await?;
            }
            Ok(())
        }
        .await;
        if let Err(Stop::Retry(m) | Stop::Failed(m) | Stop::Aborted(m)) = cleanup {
            self.event(d.id, "warn", None, format!("could not restore the previous state: {m}")).await;
        } else if rolled_back {
            self.event(d.id, "info", None, "traffic is back on the previous release".into()).await;
        }
        d.state = if aborted {
            DeploymentState::Aborted
        } else if rolled_back && d.kind == DeploymentKind::Apply {
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
        self.audit(
            &r.principal,
            d,
            action,
            false,
            json!({ "service": r.app.name, "release": d.release_number, "error": why }),
        )
        .await;
    }
}

fn stop_error(s: Stop) -> StepError {
    match s {
        Stop::Retry(m) => StepError::Retryable(m),
        Stop::Failed(m) | Stop::Aborted(m) => StepError::Fatal(m),
    }
}
