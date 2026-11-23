//! One signed command to a node, answered within seconds: what API handlers
//! use for quick reads and changes (durable work goes through jobs and
//! [`crate::jobs::node_op::NodeOps`]).

use std::sync::Arc;
use std::time::Duration;

use forge_core::{Clock, ServerId};
use forge_proto::{Command, CommandSigner};
use serde_json::Value;

use crate::agent_hub::CommandQueue;

#[derive(Debug, Clone)]
pub struct NodeRpc {
    pub hub: Arc<CommandQueue>,
    pub signer: Arc<CommandSigner>,
    pub clock: Arc<dyn Clock>,
}

/// Why a quick command did not succeed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RpcError {
    #[error("the node is not connected: {0}")]
    Unreachable(String),
    #[error("the node did not answer in time")]
    Timeout,
    /// The node ran the command and reported this error.
    #[error("{0}")]
    Failed(String),
    #[error("the node's answer is not JSON: {0}")]
    BadAnswer(String),
}

impl From<RpcError> for forge_core::DomainError {
    fn from(e: RpcError) -> Self {
        match e {
            RpcError::Failed(m) => Self::Conflict(m),
            other => Self::Conflict(other.to_string()),
        }
    }
}

impl NodeRpc {
    /// Signs `command` for `principal`, sends it to `server` and waits up to
    /// `wait` for its answer, parsed as JSON.
    pub async fn call(
        &self,
        server: ServerId,
        principal: &str,
        command: Command,
        wait: Duration,
    ) -> Result<Value, RpcError> {
        let (body, signed) = self.signer.issue(&server.to_string(), principal, command, self.clock.now(), wait);
        self.hub.enqueue(server, &body.operation_id, signed).map_err(RpcError::Unreachable)?;
        let result = self.hub.wait(&body.operation_id, wait).await.ok_or(RpcError::Timeout)?;
        if !result.ok {
            return Err(RpcError::Failed(result.error));
        }
        serde_json::from_str(&result.output).map_err(|e| RpcError::BadAnswer(e.to_string()))
    }
}
