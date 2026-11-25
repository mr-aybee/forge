//! Sending one signed command to a node from a job step and waiting for its
//! answer, resumably: the operation id is kept in the job state, so a step
//! that runs again waits for the same operation while the hub still tracks
//! it, and sends it again after a control-plane restart.

use std::sync::Arc;
use std::time::Duration;

use forge_core::model::AgentStatus;
use forge_core::ports::ServerRepository;
use forge_core::{Clock, ServerId};
use forge_jobs::{StepContext, StepError};
use forge_proto::{Command, CommandSigner};
use serde_json::{Value, json};

use crate::agent_hub::CommandQueue;

/// How long a signed command stays valid.
const COMMAND_TTL: Duration = Duration::from_secs(120);

/// What a job needs to reach nodes.
#[derive(Debug, Clone)]
pub struct NodeOps {
    pub hub: Arc<CommandQueue>,
    pub signer: Arc<CommandSigner>,
    pub servers: Arc<dyn ServerRepository>,
    pub clock: Arc<dyn Clock>,
}

impl NodeOps {
    /// Sends `command` to `server` (once per `key`) and waits up to `wait`
    /// for the answer. A failed answer is fatal; no answer is retryable.
    /// Returns the node's output, parsed as JSON.
    pub async fn run(
        &self,
        ctx: &mut StepContext,
        server: ServerId,
        key: &str,
        command: Command,
        wait: Duration,
    ) -> Result<Value, StepError> {
        let s = self.servers.get_server(server).await.map_err(|e| StepError::Retryable(e.to_string()))?;
        if s.status == AgentStatus::Revoked {
            return Err(StepError::Fatal("server is revoked".into()));
        }

        let tracked =
            ctx.state_value(key).and_then(Value::as_str).filter(|op| self.hub.is_tracked(op)).map(str::to_owned);

        let op_id = match tracked {
            Some(op_id) => op_id,
            None => {
                let (body, signed) = self.signer.issue(
                    &server.to_string(),
                    &ctx.job.principal.to_string(),
                    command,
                    self.clock.now(),
                    COMMAND_TTL,
                );
                self.hub.enqueue(server, &body.operation_id, signed).map_err(StepError::Retryable)?;
                ctx.set_state(key, json!(body.operation_id));
                body.operation_id
            }
        };

        let result = self
            .hub
            .wait(&op_id, wait)
            .await
            .ok_or_else(|| StepError::Retryable("the node did not answer in time".into()))?;

        if !result.ok {
            return Err(StepError::Fatal(result.error));
        }

        serde_json::from_str(&result.output)
            .map_err(|e| StepError::Fatal(format!("the node's answer is not JSON: {e}")))
    }
}
