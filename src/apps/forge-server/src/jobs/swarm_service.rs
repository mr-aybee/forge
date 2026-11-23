//! `swarm.service`: a container service in a Docker Swarm cluster (D-154).
//!
//! Payload: `{"service_id", "cluster_id", "action", "privileged_binds"}`
//! where `action` is `{"kind": "deploy"}`, `{"kind": "scale", "replicas"}`,
//! `{"kind": "restart"}`, `{"kind": "rollback"}` or `{"kind": "remove"}`.
//! Steps:
//! 1. `prepare`: picks a manager; a deploy also makes sure the
//!    environment's encrypted overlay network exists.
//! 2. `apply`: one operation on the manager. A deploy turns the service into
//!    a typed [`ServiceDef`](forge_catalog::swarm_service::ServiceDef) (secrets
//!    resolved here, never stored in the job), re-checks host binds against
//!    `stack.privileged`, and creates or updates the service.
//! 3. `converge`: waits until Swarm runs the tasks asked for. An update that
//!    Swarm rolled back or paused fails the job with Swarm's message.
//!
//! Every step can run again: applying the same definition is an update to
//! the same state, scaling to the same count is a no-op, and removing a
//! service that is gone succeeds.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use forge_catalog::swarm::{SwarmOp, SwarmServiceStatus};
use forge_catalog::swarm_service::service_def;
use forge_core::model::Service;
use forge_core::ports::ServiceRepository;
use forge_core::swarm::{MemberRole, MemberState, SwarmRepository};
use forge_core::{Action, ClusterId, ServerId, ServiceId};
use forge_jobs::{JobHandler, StepContext, StepError};
use forge_proto::Command;
use forge_proto::messages::SwarmAction;
use forge_secrets::Envelope;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::node_op::NodeOps;

pub const KIND: &str = "swarm.service";
const QUICK_WAIT: Duration = Duration::from_secs(60);
/// How long one `converge` attempt watches the rollout before the step is
/// retried (the job's attempts bound the total).
const CONVERGE_FOR: Duration = Duration::from_secs(5 * 60);
const POLL: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ServiceAction {
    Deploy,
    Scale { replicas: u32 },
    Restart,
    Rollback,
    Remove,
}

impl ServiceAction {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Deploy => "deploy",
            Self::Scale { .. } => "scale",
            Self::Restart => "restart",
            Self::Rollback => "rollback",
            Self::Remove => "remove",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Payload {
    pub service_id: ServiceId,
    pub cluster_id: ClusterId,
    pub action: ServiceAction,
    /// Host folders outside the swarm's approved roots that the submitter
    /// was authorized for (`stack.privileged`). A deploy refuses any other.
    #[serde(default)]
    pub privileged_binds: Vec<String>,
    /// The slot acted on (`""`: the unslotted service).
    #[serde(default)]
    pub slot: String,
}

#[derive(Debug)]
pub struct SwarmServiceJob {
    pub ops: NodeOps,
    pub swarm: Arc<crate::swarm::SwarmService>,
    pub swarms: Arc<dyn SwarmRepository>,
    pub services: Arc<dyn ServiceRepository>,
    pub deploys: Arc<dyn forge_core::deploy::DeploymentRepository>,
    /// Credentials for private images, resolved per step (never stored).
    pub registries: Arc<crate::registries::RegistryService>,
    pub envelope: Envelope,
}

fn retry(e: impl std::fmt::Display) -> StepError {
    StepError::Retryable(e.to_string())
}

fn fatal(e: impl std::fmt::Display) -> StepError {
    StepError::Fatal(e.to_string())
}

/// Whether a rollout is done, still running, or failed.
#[derive(Debug, PartialEq, Eq)]
pub enum Progress {
    Done,
    Running,
    Failed(String),
}

/// Reads a service's status after `action`.
#[must_use]
pub fn progress(action: ServiceAction, s: &SwarmServiceStatus) -> Progress {
    if action == ServiceAction::Remove {
        return if s.exists { Progress::Running } else { Progress::Done };
    }
    if !s.exists {
        return Progress::Failed("the service is gone from the swarm".into());
    }
    match s.update_state.as_str() {
        "paused" | "rollback_paused" => Progress::Failed(format!("Swarm paused the update: {}", s.update_message)),
        "rollback_completed" if action != ServiceAction::Rollback => {
            Progress::Failed(format!("Swarm rolled the update back: {}", s.update_message))
        }
        "updating" | "rollback_started" => Progress::Running,
        // Tasks of the current definition only (older ones still run while
        // Swarm picks the update up); services from before fingerprints
        // count every running task.
        _ if (if s.spec.is_empty() { s.running } else { s.running_current }) >= s.desired => Progress::Done,
        _ => Progress::Running,
    }
}

/// The host binds of `app`'s Swarm definition.
pub fn binds_of(app: &Service) -> Result<Vec<String>, String> {
    let spec = crate::jobs::app::build_spec(app, None)?;
    let settings = spec.swarm.clone().unwrap_or_default();
    Ok(service_def(&spec, &settings, &app.id.to_string(), &app.environment_id.to_string())?.bind_sources())
}

impl SwarmServiceJob {
    async fn op(
        &self,
        ctx: &mut StepContext,
        manager: ServerId,
        key: &str,
        op: &SwarmOp,
        wait: Duration,
    ) -> Result<Value, StepError> {
        op.validate().map_err(fatal)?;
        let op_json = serde_json::to_string(op).map_err(fatal)?;
        self.ops.run(ctx, manager, key, Command::SwarmAction(SwarmAction { op_json }), wait).await
    }

    async fn manager(&self, ctx: &mut StepContext, cluster: ClusterId) -> Result<ServerId, StepError> {
        if let Some(id) = ctx.state_value("manager").and_then(Value::as_str).and_then(|s| s.parse().ok()) {
            return Ok(id);
        }
        let m = self
            .swarms
            .list_members(cluster)
            .await
            .map_err(retry)?
            .into_iter()
            .find(|m| m.role == MemberRole::Manager && m.state == MemberState::Joined)
            .ok_or_else(|| fatal("the cluster has no swarm manager; add a node on the Swarm tab"))?;
        ctx.set_state("manager", Value::String(m.server_id.to_string()));
        Ok(m.server_id)
    }

    async fn apply_op(&self, p: &Payload, app: &Service) -> Result<SwarmOp, StepError> {
        let sid = app.id.to_string();
        Ok(match p.action {
            ServiceAction::Deploy => {
                let spec = crate::jobs::app::build_spec(app, Some(&self.envelope)).map_err(fatal)?;
                let settings = spec.swarm.clone().unwrap_or_default();
                let def = service_def(&spec, &settings, &sid, &app.environment_id.to_string()).map_err(fatal)?;
                // Binds outside the swarm's approved roots need
                // stack.privileged; checked again at every attempt.
                let needed = self.swarm.privileged_binds(p.cluster_id, &def.bind_sources()).await.map_err(retry)?;
                if let Some(b) = needed.iter().find(|b| !p.privileged_binds.contains(b)) {
                    return Err(fatal(format!(
                        "host folder `{b}` is outside the approved file roots of the swarm's nodes; it needs \
                         stack.privileged (an administrator with MFA): deploy again as one"
                    )));
                }
                let registry_auth = self.registries.auth_for(app.workspace_id, &def.image).await.map_err(retry)?;
                SwarmOp::ServiceApply { def: Box::new(def), registry_auth }
            }
            ServiceAction::Scale { replicas } => {
                SwarmOp::ServiceScale { service_id: sid, slot: p.slot.clone(), replicas }
            }
            ServiceAction::Restart => SwarmOp::ServiceRestart { service_id: sid, slot: p.slot.clone() },
            ServiceAction::Rollback => SwarmOp::ServiceRollback { service_id: sid, slot: p.slot.clone() },
            ServiceAction::Remove => SwarmOp::ServiceRemove { service_id: sid, slot: p.slot.clone() },
        })
    }

    async fn converge(&self, ctx: &mut StepContext, p: &Payload, manager: ServerId) -> Result<(), StepError> {
        let deadline = tokio::time::Instant::now() + CONVERGE_FOR;
        let op = SwarmOp::ServiceStatus { service_id: p.service_id.to_string(), slot: p.slot.clone() };
        loop {
            // One key: a finished operation is no longer tracked, so each
            // round sends a fresh status request.
            let status: SwarmServiceStatus =
                serde_json::from_value(self.op(ctx, manager, "status_op", &op, QUICK_WAIT).await?).map_err(fatal)?;
            ctx.set_state("status", serde_json::to_value(&status).map_err(fatal)?);
            match progress(p.action, &status) {
                Progress::Done => return Ok(()),
                Progress::Failed(why) => return Err(fatal(why)),
                Progress::Running if tokio::time::Instant::now() >= deadline => {
                    return Err(retry(format!(
                        "still converging: {} of {} tasks running",
                        status.running, status.desired
                    )));
                }
                Progress::Running => tokio::time::sleep(POLL).await,
            }
        }
    }

    async fn step(&self, index: usize, ctx: &mut StepContext, p: &Payload) -> Result<(), StepError> {
        let manager = self.manager(ctx, p.cluster_id).await?;
        match index {
            0 if p.action == ServiceAction::Deploy => {
                let app = self
                    .services
                    .get_service(p.service_id)
                    .await
                    .map_err(retry)?
                    .ok_or_else(|| fatal("the service is gone"))?;
                let op = SwarmOp::EnsureNetwork { environment_id: app.environment_id.to_string() };
                self.op(ctx, manager, "network_op", &op, QUICK_WAIT).await.map(|_| ())
            }
            // Remove takes every slot's service away, and the slots'
            // traffic rows, so nothing is left behind.
            1 if p.action == ServiceAction::Remove => {
                for slot in ["", "a", "b"] {
                    let op = SwarmOp::ServiceRemove { service_id: p.service_id.to_string(), slot: slot.into() };
                    self.op(ctx, manager, &format!("remove_op_{slot}"), &op, QUICK_WAIT).await?;
                }
                self.deploys.replace_slots(p.service_id, &[]).await.map_err(retry)?;
                Ok(())
            }
            1 => {
                let app = self
                    .services
                    .get_service(p.service_id)
                    .await
                    .map_err(retry)?
                    .ok_or_else(|| fatal("the service is gone"))?;
                let op = self.apply_op(p, &app).await?;
                self.op(ctx, manager, "apply_op", &op, QUICK_WAIT).await.map(|_| ())
            }
            2 => self.converge(ctx, p, manager).await,
            _ => Ok(()),
        }
    }
}

#[async_trait]
impl JobHandler for SwarmServiceJob {
    fn kind(&self) -> &'static str {
        KIND
    }

    fn steps(&self) -> &'static [&'static str] {
        &["prepare", "apply", "converge"]
    }

    fn required_action(&self, payload: &Value) -> Action {
        // A deploy that binds host folders outside the approved roots needs
        // what it needed when it was submitted.
        let privileged = payload["privileged_binds"].as_array().is_some_and(|a| !a.is_empty());
        if privileged { Action::StackPrivileged } else { Action::Deploy }
    }

    async fn run_step(&self, index: usize, ctx: &mut StepContext) -> Result<(), StepError> {
        let p: Payload =
            serde_json::from_value(ctx.job.payload.clone()).map_err(|e| fatal(format!("bad payload: {e}")))?;
        self.step(index, ctx, &p).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(desired: u64, running: u64, update: &str) -> SwarmServiceStatus {
        SwarmServiceStatus { exists: true, desired, running, update_state: update.into(), ..Default::default() }
    }

    #[test]
    fn progress_follows_swarm() {
        assert_eq!(progress(ServiceAction::Deploy, &status(3, 3, "")), Progress::Done);
        assert_eq!(progress(ServiceAction::Deploy, &status(3, 3, "completed")), Progress::Done);
        assert_eq!(progress(ServiceAction::Deploy, &status(3, 1, "")), Progress::Running);
        assert_eq!(progress(ServiceAction::Deploy, &status(3, 3, "updating")), Progress::Running);
        assert!(matches!(progress(ServiceAction::Deploy, &status(3, 3, "rollback_completed")), Progress::Failed(_)));
        assert!(matches!(progress(ServiceAction::Deploy, &status(3, 2, "paused")), Progress::Failed(_)));
        assert_eq!(progress(ServiceAction::Rollback, &status(3, 3, "rollback_completed")), Progress::Done);
        assert_eq!(progress(ServiceAction::Deploy, &status(3, 3, "rollback_started")), Progress::Running);
        // Old tasks still running while Swarm picks up the new definition.
        let mut updating = status(3, 3, "");
        updating.spec = "new".into();
        updating.running_current = 0;
        assert_eq!(progress(ServiceAction::Deploy, &updating), Progress::Running);
        updating.running_current = 3;
        assert_eq!(progress(ServiceAction::Deploy, &updating), Progress::Done);
        let gone = SwarmServiceStatus::default();
        assert_eq!(progress(ServiceAction::Remove, &gone), Progress::Done);
        assert!(matches!(progress(ServiceAction::Scale { replicas: 2 }, &gone), Progress::Failed(_)));
    }
}
