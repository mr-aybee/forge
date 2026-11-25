//! `swarm.stack`: a compose stack deployed to, or removed from, a Docker
//! Swarm cluster (D-171).
//!
//! Payload: `{"stack_id", "cluster_id", "action", "privileged"}` where
//! `action` is `{"kind": "deploy", "revision"}` or `{"kind": "remove"}`.
//! Steps:
//! 1. `prepare`: picks a manager.
//! 2. `apply`: a deploy builds the stack's swarm part (the `.env` opened and
//!    the secret versions read here, never stored in the job), re-checks the
//!    policy and runs `docker stack deploy`; a removal runs `docker stack rm`.
//! 3. `converge`: waits until every service runs its tasks (or, after a
//!    removal, until the services are gone). An update Swarm paused or
//!    rolled back fails the job with Swarm's message.
//!
//! Every step can run again: `stack deploy` converges to the file, and
//! removing a stack that is gone succeeds.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use forge_catalog::swarm::SwarmOp;
use forge_catalog::swarm_stack::StackStatus;
use forge_core::stacks::StackRepository;
use forge_core::swarm::{MemberRole, MemberState, SwarmRepository};
use forge_core::{Action, ClusterId, ServerId, StackId};
use forge_jobs::{JobHandler, StepContext, StepError};
use forge_proto::Command;
use forge_proto::messages::SwarmAction;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::node_op::NodeOps;

pub const KIND: &str = "swarm.stack";
const QUICK_WAIT: Duration = Duration::from_secs(60);
/// `docker stack deploy` resolves every image against its registry.
const APPLY_WAIT: Duration = Duration::from_secs(6 * 60);
const CONVERGE_FOR: Duration = Duration::from_secs(5 * 60);
const POLL: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StackAction {
    Deploy { revision: i64 },
    Remove,
}

impl StackAction {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Deploy { .. } => "deploy",
            Self::Remove => "remove",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Payload {
    pub stack_id: StackId,
    pub cluster_id: ClusterId,
    pub action: StackAction,
    /// The submitter was authorized for `stack.privileged`.
    #[serde(default)]
    pub privileged: bool,
    /// The stack's name, for the job list.
    #[serde(default)]
    pub stack_name: String,
}

#[derive(Debug)]
pub struct SwarmStackJob {
    pub ops: NodeOps,
    pub stacks: Arc<crate::swarm_stacks::SwarmStacks>,
    pub store: Arc<dyn StackRepository>,
    pub swarms: Arc<dyn SwarmRepository>,
}

fn retry(e: impl std::fmt::Display) -> StepError {
    StepError::Retryable(e.to_string())
}

fn fatal(e: impl std::fmt::Display) -> StepError {
    StepError::Fatal(e.to_string())
}

/// Whether the stack reached what `action` asked for.
pub fn settled(action: StackAction, s: &StackStatus) -> Result<bool, String> {
    if action == StackAction::Remove {
        return Ok(s.services.is_empty());
    }

    if let Some(why) = s.failure() {
        return Err(why);
    }

    Ok(!s.services.is_empty() && s.converged())
}

impl SwarmStackJob {
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
            .ok_or_else(|| fatal("the cluster has no swarm manager; create the swarm first"))?;
        ctx.set_state("manager", Value::String(m.server_id.to_string()));

        Ok(m.server_id)
    }

    async fn apply(&self, ctx: &mut StepContext, p: &Payload, manager: ServerId) -> Result<(), StepError> {
        let stack = self.store.get_stack(p.stack_id).await.map_err(retry)?.ok_or_else(|| fatal("the stack is gone"))?;
        let namespace = self.stacks.namespace(&stack).await.map_err(fatal)?;

        match p.action {
            StackAction::Deploy { revision } => {
                let principal = ctx.job.principal.to_string();
                let prepared = self.stacks.prepare(&stack, revision, p.privileged, &principal).await.map_err(fatal)?;

                if prepared.report.privileged && !p.privileged {
                    return Err(fatal(
                        "the stack now uses host access (privileged settings or host folders); deploy again as an \
                         administrator with MFA (stack.privileged)",
                    ));
                }

                let op = SwarmOp::StackDeploy { part: Box::new(prepared.part) };
                self.op(ctx, manager, "apply_op", &op, APPLY_WAIT).await?;
                self.store.set_deployed_revision(stack.id, revision).await.map_err(retry)
            }
            StackAction::Remove => {
                self.op(ctx, manager, "remove_op", &SwarmOp::StackRemove { namespace }, APPLY_WAIT).await?;
                self.store.set_deployed_revision(stack.id, 0).await.map_err(retry)
            }
        }
    }

    async fn converge(&self, ctx: &mut StepContext, p: &Payload, manager: ServerId) -> Result<(), StepError> {
        let stack = self.store.get_stack(p.stack_id).await.map_err(retry)?.ok_or_else(|| fatal("the stack is gone"))?;
        let op = SwarmOp::StackStatus { namespace: self.stacks.namespace(&stack).await.map_err(fatal)? };
        let deadline = tokio::time::Instant::now() + CONVERGE_FOR;

        loop {
            // One key: a finished operation is no longer tracked, so each
            // round sends a fresh status request.
            let status: StackStatus =
                serde_json::from_value(self.op(ctx, manager, "status_op", &op, QUICK_WAIT).await?).map_err(fatal)?;
            ctx.set_state("status", serde_json::to_value(&status).map_err(fatal)?);

            match settled(p.action, &status) {
                Ok(true) => return Ok(()),
                Err(why) => return Err(fatal(why)),
                Ok(false) if tokio::time::Instant::now() >= deadline => {
                    let running: u64 = status.services.iter().map(|s| s.running).sum();
                    let desired: u64 = status.services.iter().map(|s| s.desired).sum();

                    return Err(retry(format!("still converging: {running} of {desired} tasks running")));
                }
                Ok(false) => tokio::time::sleep(POLL).await,
            }
        }
    }
}

#[async_trait]
impl JobHandler for SwarmStackJob {
    fn kind(&self) -> &'static str {
        KIND
    }

    fn steps(&self) -> &'static [&'static str] {
        &["prepare", "apply", "converge"]
    }

    fn required_action(&self, payload: &Value) -> Action {
        if payload["privileged"].as_bool() == Some(true) { Action::StackPrivileged } else { Action::StackManage }
    }

    async fn run_step(&self, index: usize, ctx: &mut StepContext) -> Result<(), StepError> {
        let p: Payload =
            serde_json::from_value(ctx.job.payload.clone()).map_err(|e| fatal(format!("bad payload: {e}")))?;
        let manager = self.manager(ctx, p.cluster_id).await?;

        match index {
            1 => self.apply(ctx, &p, manager).await,
            2 => self.converge(ctx, &p, manager).await,
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use forge_catalog::swarm_stack::StackService;

    use super::*;

    #[test]
    fn deploys_settle_when_every_service_runs_and_removals_when_none_is_left() {
        let deploy = StackAction::Deploy { revision: 2 };
        let svc = |desired, running, update: &str| StackService {
            name: "api".into(),
            desired,
            running,
            update_state: update.into(),
            ..Default::default()
        };
        let st = |s: Vec<StackService>| StackStatus { namespace: "forge-prod-shop".into(), services: s };
        assert_eq!(settled(deploy, &st(vec![svc(2, 2, "completed")])), Ok(true));
        assert_eq!(settled(deploy, &st(vec![svc(2, 1, "")])), Ok(false));
        assert_eq!(settled(deploy, &st(vec![svc(2, 2, "updating")])), Ok(false));
        assert_eq!(settled(deploy, &st(vec![])), Ok(false), "nothing created yet");
        assert!(settled(deploy, &st(vec![svc(2, 2, "paused")])).is_err());
        assert_eq!(settled(StackAction::Remove, &st(vec![])), Ok(true));
        assert_eq!(settled(StackAction::Remove, &st(vec![svc(1, 1, "")])), Ok(false));
    }
}
