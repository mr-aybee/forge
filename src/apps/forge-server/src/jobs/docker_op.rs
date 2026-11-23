//! `docker.op`: a slow Docker operation on a node (pull, push, prune;
//! D-157). Payload: `{"server_id", "op"}`. One step sends the operation and
//! waits for it. Pulls and prunes are safe to repeat; a repeated push sends
//! the same image again. Registry credentials never go into the payload
//! (it is stored); the API refuses them.

use std::time::Duration;

use async_trait::async_trait;
use forge_catalog::docker_ops::DockerOp;
use forge_core::{Action, ServerId};
use forge_jobs::{JobHandler, StepContext, StepError};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::node_op::NodeOps;

pub const KIND: &str = "docker.op";
/// Large images take a while.
const WAIT: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Payload {
    pub server_id: ServerId,
    pub op: DockerOp,
}

#[derive(Debug)]
pub struct DockerOpJob {
    pub ops: NodeOps,
}

#[async_trait]
impl JobHandler for DockerOpJob {
    fn kind(&self) -> &'static str {
        KIND
    }

    fn steps(&self) -> &'static [&'static str] {
        &["run"]
    }

    fn required_action(&self, payload: &Value) -> Action {
        // An unreadable payload needs the strictest Docker action; the step
        // then fails on it anyway.
        serde_json::from_value::<Payload>(payload.clone())
            .map_or(Action::ServerCommand, |p| crate::docker_resources::action_for(&p.op))
    }

    async fn run_step(&self, _index: usize, ctx: &mut StepContext) -> Result<(), StepError> {
        let p: Payload = serde_json::from_value(ctx.job.payload.clone())
            .map_err(|e| StepError::Fatal(format!("bad payload: {e}")))?;
        let command = crate::docker_resources::command(&p.op).map_err(|e| StepError::Fatal(e.to_string()))?;
        let out = self.ops.run(ctx, p.server_id, "docker_op", command, WAIT).await?;
        ctx.set_state("result", out);
        Ok(())
    }
}
