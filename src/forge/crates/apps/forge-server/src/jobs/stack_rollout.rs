//! `stack.rollout` (D-174): rolls one revision
//! of a compose stack out to its nodes, started by `POST /stacks/apply`.
//!
//! Payload ([`RolloutPayload`], no secrets): the stack, the revision, the
//! nodes in order, the nodes that no longer run anything (`remove`), the
//! order, the health timeout and whether the stack is privileged. One step,
//! `next`, deploys the nodes through the ordinary per-node `stack.action`
//! jobs: one at a time and waiting for each (`node-by-node`), or all at once
//! and waiting for all (`all-at-once`). A failed node fails the rollout and
//! nodes not yet started are not deployed. When every node is live, the
//! nodes in `remove` get a `down` job.
//!
//! Safe to run again after a crash: each child job has the idempotency key
//! `stack.rollout:<rollout>:<node>:<op>`, so a node that already ran is found,
//! not started twice; a node already at the revision with reported
//! containers is skipped. The engine re-authorizes `stack.manage` (or
//! `stack.privileged`) before the step and every child job checks it again.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use forge_catalog::stack_doc::RolloutOrder;
use forge_core::model::{Job, JobStatus, NewJob};
use forge_core::ports::JobRepository;
use forge_core::stacks::{Stack, StackRepository};
use forge_core::{Action, EntityKind, EntityRef, JobId, ServerId, StackId};
use forge_jobs::{JobEngine, JobHandler, StepContext, StepError};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The job kind.
pub const KIND: &str = "stack.rollout";
/// Time a node gets beyond the health timeout: pulling images, the
/// command's round trip and Compose's own work.
const NODE_GRACE: Duration = Duration::from_secs(20 * 60);

/// What a rollout deploys.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RolloutPayload {
    pub stack_id: StackId,
    pub revision: i64,
    /// Nodes to deploy, in order.
    pub nodes: Vec<ServerId>,
    /// Nodes of an earlier revision that no longer run a service.
    #[serde(default)]
    pub remove: Vec<ServerId>,
    #[serde(default)]
    pub order: RolloutOrder,
    pub health_timeout_s: u64,
    /// Submitted with `stack.privileged` (re-authorized at every step).
    #[serde(default)]
    pub privileged: bool,
    /// The first node still to deploy (`node-by-node`).
    #[serde(default)]
    pub index: usize,
}

#[derive(Debug)]
pub struct StackRolloutJob {
    /// Set once the engine exists (it holds this handler).
    pub engine: Arc<OnceLock<Arc<JobEngine>>>,
    pub jobs: Arc<dyn JobRepository>,
    pub stacks: Arc<dyn StackRepository>,
    /// How often child jobs are checked.
    pub poll: Duration,
}

fn retry(e: impl std::fmt::Display) -> StepError {
    StepError::Retryable(e.to_string())
}

fn fatal(e: impl std::fmt::Display) -> StepError {
    StepError::Fatal(e.to_string())
}

fn payload(ctx: &StepContext) -> Result<RolloutPayload, StepError> {
    serde_json::from_value(ctx.job.payload.clone()).map_err(|e| fatal(format!("bad payload: {e}")))
}

const fn finished(s: JobStatus) -> bool {
    matches!(s, JobStatus::Succeeded | JobStatus::Failed | JobStatus::Cancelled)
}

/// A child job and the node it acts on.
struct Child {
    server: ServerId,
    job: JobId,
}

impl StackRolloutJob {
    /// Whether `server` already runs `revision` with reported containers.
    async fn up_to_date(&self, p: &RolloutPayload, server: ServerId) -> Result<bool, StepError> {
        let nodes = self.stacks.list_nodes(p.stack_id).await.map_err(retry)?;

        Ok(nodes.iter().any(|n| {
            n.server_id == server
                && n.revision == p.revision
                && n.status == "deployed"
                && n.state.as_array().is_some_and(|a| !a.is_empty())
        }))
    }

    /// Submits (or finds, after a crash) the `stack.action` job of one node.
    async fn submit(
        &self,
        engine: &JobEngine,
        ctx: &StepContext,
        stack: &Stack,
        p: &RolloutPayload,
        server: ServerId,
        op: &str,
    ) -> Result<Child, StepError> {
        let body = match op {
            "deploy" => json!({
                "stack_id": stack.id, "server_id": server, "op": "deploy", "revision": p.revision,
                "services": [], "pull": false, "privileged": p.privileged,
            }),
            _ => json!({ "stack_id": stack.id, "server_id": server, "op": "down", "volumes": false }),
        };

        let job = engine
            .submit(NewJob {
                kind: super::stack::KIND.into(),
                workspace_id: Some(stack.workspace_id),
                resource: EntityRef::new(EntityKind::Environment, stack.environment_id),
                principal: ctx.job.principal.clone(),
                context: ctx.job.context,
                idempotency_key: Some(format!("{KIND}:{}:{server}:{op}", ctx.job.id)),
                payload: body,
                max_attempts: 2,
            })
            .await
            .map_err(|e| fatal(format!("node {server}: {e}")))?;

        Ok(Child { server, job: job.id })
    }

    /// Waits for every child to finish; the first failure is a fatal error
    /// that names its node.
    async fn wait_all(&self, children: &[Child], wait: Duration) -> Result<Vec<Job>, StepError> {
        let deadline = tokio::time::Instant::now() + wait;

        loop {
            let mut done = Vec::new();

            for c in children {
                let j = self.jobs.get_job(c.job).await.map_err(retry)?;

                if finished(j.status) {
                    done.push(j);
                }
            }

            if let Some(bad) = done.iter().find(|j| j.status != JobStatus::Succeeded) {
                let node = children.iter().find(|c| c.job == bad.id).map(|c| c.server);
                let why = bad.last_error.clone().unwrap_or_else(|| bad.status.as_str().to_owned());

                return Err(fatal(format!(
                    "node {} failed ({why})",
                    node.map_or_else(|| "?".to_owned(), |n| n.to_string())
                )));
            }

            if done.len() == children.len() {
                return Ok(done);
            }

            if tokio::time::Instant::now() >= deadline {
                return Err(fatal(format!("the nodes did not finish within {} minutes", wait.as_secs() / 60)));
            }

            tokio::time::sleep(self.poll).await;
        }
    }

    /// Deploys `servers` together; nodes already live are skipped.
    async fn deploy(
        &self,
        engine: &JobEngine,
        ctx: &mut StepContext,
        stack: &Stack,
        p: &RolloutPayload,
        servers: &[ServerId],
    ) -> Result<(), StepError> {
        let mut children = Vec::new();
        let mut entries: Vec<Value> = ctx.state_value("nodes").and_then(Value::as_array).cloned().unwrap_or_default();

        for s in servers {
            if self.up_to_date(p, *s).await? {
                entries.push(json!({ "server_id": s, "outcome": "skipped", "reason": "already at this revision" }));
                continue;
            }

            children.push(self.submit(engine, ctx, stack, p, *s, "deploy").await?);
        }

        let wait = Duration::from_secs(p.health_timeout_s) + NODE_GRACE;
        let result = self.wait_all(&children, wait).await;

        for c in &children {
            let outcome = if result.is_ok() { "deployed" } else { "ended" };
            entries.push(json!({ "server_id": c.server, "job_id": c.job, "outcome": outcome }));
        }

        ctx.set_state("nodes", json!(entries));

        result.map(|_| ())
    }

    async fn next(&self, ctx: &mut StepContext) -> Result<(), StepError> {
        let engine = self.engine.get().cloned().ok_or_else(|| retry("job engine starting"))?;
        let p = payload(ctx)?;
        let stack = self
            .stacks
            .get_stack(p.stack_id)
            .await
            .map_err(retry)?
            .ok_or_else(|| fatal("the stack no longer exists"))?;
        let start =
            ctx.state_value("index").and_then(Value::as_u64).and_then(|i| usize::try_from(i).ok()).unwrap_or(p.index);

        match p.order {
            RolloutOrder::NodeByNode => {
                for (i, server) in p.nodes.iter().enumerate().skip(start) {
                    if let Err(e) = self.deploy(&engine, ctx, &stack, &p, &[*server]).await {
                        let left = p.nodes.len() - i - 1;

                        return Err(fatal(format!(
                            "stopped at node {} of {}: {}; {left} later node(s) were not deployed",
                            i + 1,
                            p.nodes.len(),
                            match e {
                                StepError::Fatal(m) | StepError::Retryable(m) => m,
                            }
                        )));
                    }

                    ctx.set_state("index", json!(i + 1));
                }
            }
            RolloutOrder::AllAtOnce => {
                let pending: Vec<ServerId> = p.nodes.iter().skip(start).copied().collect();
                self.deploy(&engine, ctx, &stack, &p, &pending).await?;
                ctx.set_state("index", json!(p.nodes.len()));
            }
        }

        // Every node is live: nodes the revision left behind are taken down.
        let mut downs = Vec::new();

        for s in &p.remove {
            downs.push(self.submit(&engine, ctx, &stack, &p, *s, "down").await?);
        }

        self.wait_all(&downs, NODE_GRACE).await?;
        ctx.set_state("removed", json!(p.remove));

        Ok(())
    }
}

#[async_trait]
impl JobHandler for StackRolloutJob {
    fn kind(&self) -> &'static str {
        KIND
    }

    fn steps(&self) -> &'static [&'static str] {
        &["next"]
    }

    fn required_action(&self, payload: &Value) -> Action {
        if payload.get("privileged").and_then(Value::as_bool) == Some(true) {
            return Action::StackPrivileged;
        }

        Action::StackManage
    }

    async fn run_step(&self, _index: usize, ctx: &mut StepContext) -> Result<(), StepError> {
        self.next(ctx).await
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // reason: tests
mod tests {
    use super::*;

    #[test]
    fn the_payload_reads_with_defaults() {
        let s = StackId::new();
        let n = ServerId::new();
        let p: RolloutPayload =
            serde_json::from_value(json!({ "stack_id": s, "revision": 3, "nodes": [n], "health_timeout_s": 300 }))
                .unwrap();
        assert_eq!(p.order, RolloutOrder::NodeByNode);
        assert!(p.remove.is_empty() && !p.privileged && p.index == 0);
        assert!(finished(JobStatus::Cancelled) && !finished(JobStatus::Running));
    }
}
