//! `agent.rollout` (issue #4 follow-up, D-071): updates many nodes' agents
//! in batches. The first batch is the canary: each batch is a set of
//! ordinary `agent.update` jobs, and the next batch starts only when every
//! node of this one is back on the target version or was skipped. A node
//! that cannot take the update (offline until the end of its batch, not
//! Linux, refused as `unsupported` or `downgrade`) is skipped and listed in
//! `state.nodes` with its reason; the rollout carries on. The first real
//! failure stops the rollout; nodes in later batches are not touched.
//!
//! Payload: `{"server_ids": [...], "workspaces": {"<node>": "<workspace>"},
//! "batch": N, "origin": "manual" | "auto", "target": "<version>"}`. The
//! target is the agent version on offer when the rollout was submitted; each
//! child job carries it and fails rather than send another version.
//! One step, `roll`, safe to run again after a crash: each child job has the
//! idempotency key `agent.update:<node>:<version>:<rollout>`, so a batch that
//! already ran is found, not started twice, and a later rollout can retry a
//! node whose update failed. Each child job is
//! authorized as `server.command` (MFA) for its node with the rollout's
//! principal and context, and filed under that node's own workspace. A node
//! that already has another update queued or running stops the rollout: one
//! node never gets two concurrent `AgentUpdate` commands.
//!
//! The rollout job itself sits on the platform (`platform:root`, no
//! workspace), so only people with a platform-wide role see or cancel it.
//! Only one rollout is queued or running at a time (`POST /agents/rollouts`
//! refuses a second), so a waiting rollout holds at most one worker slot.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use forge_core::model::{AgentStatus, JobStatus, NewJob};
use forge_core::ports::JobRepository;
use forge_core::{Action, EntityKind, EntityRef, ServerId, WorkspaceId};
use forge_jobs::{JobEngine, JobHandler, StepContext, StepError};
use serde_json::{Value, json};

use super::agent_update::{SUBMIT_LOCK, active_update};

/// Largest batch.
pub const MAX_BATCH: usize = 100;
/// Extra time a batch gets after its nodes' deadline, for nodes that came
/// online at the last moment to swap and reconnect (the update's `await`).
const BATCH_GRACE: Duration = Duration::from_secs(10 * 60);

#[derive(Debug)]
pub struct AgentRolloutJob {
    /// Set once the engine exists (it holds this handler).
    pub engine: Arc<OnceLock<Arc<JobEngine>>>,
    pub jobs: Arc<dyn JobRepository>,
    pub poll: Duration,
    /// The agents on offer: the target of a rollout queued without one.
    pub dist: Arc<crate::install::AgentDist>,
    /// Node status: offline nodes are waited for by the rollout itself.
    pub servers: Arc<dyn forge_core::ports::ServerRepository>,
    pub clock: Arc<dyn forge_core::Clock>,
    /// How long one batch waits for its nodes (`[update]
    /// agent_batch_timeout_secs`); a node still offline then is skipped.
    pub batch_deadline: Duration,
}

/// What a finished child job means for the rollout.
fn child_outcome(j: &forge_core::model::Job) -> &'static str {
    use super::agent_update::SkipReason;
    let reason = j.state.get("skip_reason").and_then(|v| serde_json::from_value::<SkipReason>(v.clone()).ok());
    match (j.status, reason) {
        (JobStatus::Succeeded, None | Some(SkipReason::Current)) => "succeeded",
        (JobStatus::Succeeded, Some(_)) => "skipped",
        (JobStatus::Failed | JobStatus::Cancelled, _) => "failed",
        _ => "pending",
    }
}

/// The node list split into batches of `batch`.
#[must_use]
pub fn batches(ids: &[ServerId], batch: usize) -> Vec<Vec<ServerId>> {
    ids.chunks(batch.clamp(1, MAX_BATCH)).map(<[ServerId]>::to_vec).collect()
}

fn payload(ctx: &StepContext, offered: &str) -> Result<(Vec<ServerId>, usize, String), StepError> {
    let ids: Vec<ServerId> = serde_json::from_value(ctx.job.payload["server_ids"].clone())
        .map_err(|_| StepError::Fatal("payload needs server_ids".into()))?;
    let batch = ctx.job.payload["batch"].as_u64().and_then(|b| usize::try_from(b).ok()).unwrap_or(1);

    // A rollout queued by an older server has no target: the agents on offer.
    let target = ctx.job.payload["target"].as_str().unwrap_or(offered).to_owned();

    Ok((ids, batch, target))
}

/// The workspace node `id` belongs to, from the payload (set at submit).
fn node_workspace(payload: &Value, id: ServerId) -> Option<WorkspaceId> {
    payload.get("workspaces")?.get(id.to_string())?.as_str()?.parse().ok()
}

#[async_trait]
impl JobHandler for AgentRolloutJob {
    fn kind(&self) -> &'static str {
        "agent.rollout"
    }

    fn steps(&self) -> &'static [&'static str] {
        &["roll"]
    }

    /// Starting a rollout needs `server.command` on every node anyway (each
    /// child job checks it); the rollout itself only reads. Its resource is
    /// the platform, so this is `view` on the platform.
    fn required_action(&self, _payload: &Value) -> Action {
        Action::View
    }

    async fn run_step(&self, _index: usize, ctx: &mut StepContext) -> Result<(), StepError> {
        let engine = self.engine.get().cloned().ok_or_else(|| StepError::Retryable("job engine starting".into()))?;
        let (ids, size, target) = payload(ctx, self.dist.version())?;
        let plan = batches(&ids, size);
        ctx.set_state("target", json!(target));
        ctx.set_state("batches", json!(plan.len()));
        let mut results: Vec<Value> = Vec::new();
        let mut skipped = 0usize;

        for (n, batch) in plan.iter().enumerate() {
            let at = format!("batch {} of {}", n + 1, plan.len());
            let states = self.run_batch(&engine, ctx, batch, &target, &at).await;
            let states = match states {
                Ok(s) => s,
                Err((s, e)) => {
                    results.extend(s);
                    ctx.set_state("nodes", json!(results));

                    return Err(e);
                }
            };

            skipped += states.iter().filter(|s| s["outcome"] == "skipped").count();
            results.extend(states);
            ctx.set_state("nodes", json!(results));
            ctx.set_state("skipped", json!(skipped));
            ctx.set_state("done_batches", json!(n + 1));
        }

        Ok(())
    }
}

/// A node of the batch: waiting to come online, or its update job.
enum Slot {
    Waiting,
    Job(forge_core::JobId),
    Skipped,
}

type BatchError = (Vec<Value>, StepError);

impl AgentRolloutJob {
    /// Submits the `agent.update` of node `id` for this rollout.
    async fn submit_child(
        &self,
        engine: &JobEngine,
        ctx: &StepContext,
        id: ServerId,
        target: &str,
        deadline_ms: i64,
    ) -> forge_core::DomainResult<forge_core::model::Job> {
        // Per rollout: a re-run after a crash finds its own jobs; a later
        // rollout may retry a node whose update failed.
        let key = format!("agent.update:{id}:{target}:{}", ctx.job.id);
        let _guard = SUBMIT_LOCK.lock().await;

        if let Some(other) = active_update(self.jobs.as_ref(), id).await?
            && other.idempotency_key.as_deref() != Some(key.as_str())
        {
            return Err(forge_core::DomainError::Conflict(format!(
                "another update of this node is queued or running (job {})",
                other.id
            )));
        }

        engine
            .submit(NewJob {
                kind: "agent.update".into(),
                workspace_id: node_workspace(&ctx.job.payload, id),
                resource: EntityRef::new(EntityKind::Server, id),
                principal: ctx.job.principal.clone(),
                context: ctx.job.context,
                idempotency_key: Some(key),
                payload: json!({
                    "server_id": id, "rollout": ctx.job.id, "target": target, "deadline_ms": deadline_ms,
                }),
                max_attempts: 3,
            })
            .await
    }

    /// Runs one batch: an update job per online node; an offline node is
    /// waited for here (one worker slot for the whole rollout, not one per
    /// node) until the batch's node deadline, then skipped without a job.
    /// Returns each node's entry, or the entries so far and why it stopped.
    async fn run_batch(
        &self,
        engine: &JobEngine,
        ctx: &StepContext,
        batch: &[ServerId],
        target: &str,
        at: &str,
    ) -> Result<Vec<Value>, BatchError> {
        let start_ms = self.clock.now().millis();
        let node_deadline_ms =
            start_ms.saturating_add(i64::try_from(self.batch_deadline.as_millis()).unwrap_or(i64::MAX));
        let deadline = tokio::time::Instant::now() + self.batch_deadline + BATCH_GRACE;
        let mut slots: Vec<(ServerId, Slot)> = batch.iter().map(|id| (*id, Slot::Waiting)).collect();

        loop {
            let mut states = Vec::new();
            let mut pending = false;
            let mut failed = None;
            let past_node_deadline = self.clock.now().millis() >= node_deadline_ms;

            for (id, slot) in &mut slots {
                if let Slot::Waiting = slot {
                    let node = self.servers.get_server(*id).await.map_err(|e| (Vec::new(), retry(e)))?;
                    if node.status == AgentStatus::Online {
                        match self.submit_child(engine, ctx, *id, target, node_deadline_ms).await {
                            Ok(j) => *slot = Slot::Job(j.id),
                            Err(e) => {
                                states.push(json!({ "server_id": id, "status": "refused", "error": e.to_string() }));
                                return Err((
                                    states,
                                    StepError::Fatal(format!(
                                        "stopped in {at}: node {id} was refused ({e}); later nodes were not touched"
                                    )),
                                ));
                            }
                        }
                    } else if past_node_deadline {
                        *slot = Slot::Skipped;
                    }
                }

                match slot {
                    Slot::Waiting => {
                        pending = true;
                        states.push(json!({ "server_id": id, "status": "waiting", "outcome": "pending" }));
                    }
                    Slot::Skipped => states.push(json!({
                        "server_id": id, "status": "skipped", "outcome": "skipped", "skip_reason": "offline",
                        "reason": "the node stayed offline until the end of its batch; it was not updated",
                    })),
                    Slot::Job(job) => {
                        let j = self.jobs.get_job(*job).await.map_err(|e| (Vec::new(), retry(e)))?;
                        let outcome = child_outcome(&j);
                        match outcome {
                            "failed" => {
                                failed.get_or_insert((
                                    *id,
                                    j.last_error.clone().unwrap_or_else(|| format!("{:?}", j.status)),
                                ));
                            }
                            "pending" => pending = true,
                            _ => {}
                        }

                        let mut entry =
                            json!({ "server_id": id, "job_id": job, "status": j.status, "outcome": outcome });

                        if outcome == "skipped" {
                            entry["skip_reason"] = j.state["skip_reason"].clone();
                            entry["reason"] = j.state["skipped"].clone();
                        }

                        states.push(entry);
                    }
                }
            }

            if let Some((id, why)) = failed {
                return Err((
                    states,
                    StepError::Fatal(format!(
                        "stopped in {at}: node {id} failed ({why}); later nodes were not touched"
                    )),
                ));
            }

            if !pending {
                return Ok(states);
            }

            if tokio::time::Instant::now() >= deadline {
                return Err((
                    states,
                    StepError::Fatal(format!(
                        "{at} did not finish in {} minutes; later nodes were not touched",
                        (self.batch_deadline + BATCH_GRACE).as_secs() / 60
                    )),
                ));
            }

            tokio::time::sleep(self.poll).await;
        }
    }
}

fn retry(e: impl std::fmt::Display) -> StepError {
    StepError::Retryable(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batches_split_in_order() {
        let ids: Vec<ServerId> = (0..5).map(|_| ServerId::new()).collect();
        let b = batches(&ids, 2);
        assert_eq!(b.iter().map(Vec::len).collect::<Vec<_>>(), [2, 2, 1]);
        assert_eq!(b[0][0], ids[0]);
        assert_eq!(batches(&ids, 0).len(), 5, "a batch has at least one node");
        assert_eq!(batches(&ids, 1000).len(), 1);
    }
}
