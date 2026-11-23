//! `swarm.service`: a container service in a Docker Swarm cluster (D-154).
//!
//! Payload: `{"service_id", "cluster_id", "action"}` where `action` is
//! `{"kind": "deploy"}`, `{"kind": "scale", "replicas"}`, `{"kind":
//! "restart"}`, `{"kind": "rollback"}` or `{"kind": "remove"}`. Steps:
//! 1. `prepare`: picks a manager; a deploy also makes sure the
//!    environment's overlay network exists.
//! 2. `apply`: one operation on the manager. A deploy turns the service
//!    into a Swarm `ServiceSpec` (secrets resolved here, never stored in the
//!    job) and creates or updates it.
//! 3. `converge`: waits until Swarm runs the tasks asked for. An update that
//!    Swarm rolled back or paused fails the job with Swarm's message.
//!
//! Every step can run again: applying the same spec is an update to the
//! same state, scaling to the same count is a no-op, and removing a service
//! that is gone succeeds.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use forge_catalog::swarm::{SwarmOp, SwarmServiceStatus};
use forge_catalog::swarm_service::{network_name, service_name, service_spec};
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
}

#[derive(Debug)]
pub struct SwarmServiceJob {
    pub ops: NodeOps,
    pub swarms: Arc<dyn SwarmRepository>,
    pub services: Arc<dyn ServiceRepository>,
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
    let rolled_back = s.update_state.starts_with("rollback");
    match s.update_state.as_str() {
        "paused" | "rollback_paused" => {
            return Progress::Failed(format!("Swarm paused the update: {}", s.update_message));
        }
        "rollback_completed" if action != ServiceAction::Rollback => {
            return Progress::Failed(format!("Swarm rolled the update back: {}", s.update_message));
        }
        "updating" | "rollback_started" => return Progress::Running,
        _ => {}
    }
    if rolled_back && action != ServiceAction::Rollback && s.update_state != "rollback_completed" {
        return Progress::Running;
    }
    if s.running >= s.desired { Progress::Done } else { Progress::Running }
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

    async fn names(&self, id: ServiceId) -> Result<(forge_core::model::Service, String, String), StepError> {
        let app = self.services.get_service(id).await.map_err(retry)?.ok_or_else(|| fatal("the service is gone"))?;
        let env = app.spec.get("environment").and_then(Value::as_str).unwrap_or_default().to_owned();
        let name = service_name(&env, &app.name);
        Ok((app, env, name))
    }

    async fn step(&self, index: usize, ctx: &mut StepContext, p: &Payload) -> Result<(), StepError> {
        let manager = self.manager(ctx, p.cluster_id).await?;
        let (app, env, name) = self.names(p.service_id).await?;
        match index {
            0 => {
                if p.action == ServiceAction::Deploy {
                    self.op(
                        ctx,
                        manager,
                        "network_op",
                        &SwarmOp::EnsureNetwork { name: network_name(&env) },
                        QUICK_WAIT,
                    )
                    .await?;
                }
                Ok(())
            }
            1 => {
                let op = match p.action {
                    ServiceAction::Deploy => {
                        let spec = crate::jobs::app::build_spec(&app, Some(&self.envelope)).map_err(fatal)?;
                        let settings = spec.swarm.clone().unwrap_or_default();
                        SwarmOp::ServiceApply { spec: service_spec(&spec, &settings, &env).map_err(fatal)? }
                    }
                    ServiceAction::Scale { replicas } => SwarmOp::ServiceScale { name, replicas },
                    ServiceAction::Restart => SwarmOp::ServiceRestart { name },
                    ServiceAction::Rollback => SwarmOp::ServiceRollback { name },
                    ServiceAction::Remove => SwarmOp::ServiceRemove { name },
                };
                self.op(ctx, manager, "apply_op", &op, QUICK_WAIT).await?;
                Ok(())
            }
            2 => {
                let deadline = tokio::time::Instant::now() + CONVERGE_FOR;
                let mut round = 0u32;
                loop {
                    round += 1;
                    let key = format!("status_op_{round}");
                    let status: SwarmServiceStatus = serde_json::from_value(
                        self.op(ctx, manager, &key, &SwarmOp::ServiceStatus { name: name.clone() }, QUICK_WAIT).await?,
                    )
                    .map_err(fatal)?;
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

    fn required_action(&self, _payload: &Value) -> Action {
        Action::Deploy
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
        let gone = SwarmServiceStatus::default();
        assert_eq!(progress(ServiceAction::Remove, &gone), Progress::Done);
        assert!(matches!(progress(ServiceAction::Scale { replicas: 2 }, &gone), Progress::Failed(_)));
    }
}
