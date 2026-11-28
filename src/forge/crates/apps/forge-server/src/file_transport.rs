//! [`FileTransport`] over the agent link: one signed `FileAction` command
//! per operation, delivered through the hub and awaited directly (no job;
//! listings are interactive). Agents run file commands in the background,
//! so a long copy or search never holds up heartbeats.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use forge_core::files::{FileOp, FileTransport};
use forge_core::{Clock, ServerId};
use forge_proto::messages::FileAction;
use forge_proto::{Command, CommandSigner, MAX_COMMAND_TTL};
use serde_json::Value;

use crate::agent_hub::CommandQueue;

#[derive(Debug)]
pub struct HubFileTransport {
    pub signer: Arc<CommandSigner>,
    pub hub: Arc<CommandQueue>,
    pub clock: Arc<dyn Clock>,
}

/// How long to wait for the node's answer.
fn wait_for(op: &FileOp) -> Duration {
    match op {
        FileOp::Search { .. } => Duration::from_secs(90),
        // Copies and moves of up to 1 GiB, hashing a finished upload.
        FileOp::Copy { .. }
        | FileOp::Rename { .. }
        | FileOp::Delete { .. }
        | FileOp::UploadCommit { .. }
        | FileOp::TrashRestore { .. }
        | FileOp::TrashPurge { .. }
        | FileOp::Hash { .. } => Duration::from_secs(10 * 60),
        _ => Duration::from_secs(30),
    }
}

#[async_trait]
impl FileTransport for HubFileTransport {
    async fn call(&self, server: ServerId, principal: &str, op: &FileOp) -> Result<Value, String> {
        let op_json = serde_json::to_string(op).map_err(|e| e.to_string())?;
        let wait = wait_for(op);

        // The command must be *started* within its TTL; it may run longer.
        let (body, signed) = self.signer.issue(
            &server.to_string(),
            principal,
            Command::FileAction(FileAction { op_json }),
            self.clock.now(),
            wait.min(MAX_COMMAND_TTL),
        );
        self.hub
            .enqueue(server, &body.operation_id, signed)
            .map_err(|e| format!("transport: cannot reach the node: {e}"))?;
        let result = self.hub.wait(&body.operation_id, wait).await.ok_or_else(|| {
            "transport: the node did not answer in time: it may be offline, or its agent may be too old for the file explorer"
                .to_owned()
        })?;

        if result.ok {
            serde_json::from_str(&result.output).map_err(|_| "transport: the node sent an invalid answer".to_owned())
        } else {
            Err(result.error)
        }
    }

    /// Queues every operation back to back (so the node receives them in
    /// order, usually in one heartbeat answer, and runs them in order), then
    /// waits for each answer.
    async fn call_many(&self, server: ServerId, principal: &str, ops: &[FileOp]) -> Vec<Result<Value, String>> {
        let mut queued = Vec::with_capacity(ops.len());

        for op in ops {
            let op_json = match serde_json::to_string(op) {
                Ok(j) => j,
                Err(e) => {
                    queued.push(Err(e.to_string()));
                    continue;
                }
            };

            let wait = wait_for(op);
            let (body, signed) = self.signer.issue(
                &server.to_string(),
                principal,
                Command::FileAction(FileAction { op_json }),
                self.clock.now(),
                wait.min(MAX_COMMAND_TTL),
            );
            queued.push(
                self.hub
                    .enqueue(server, &body.operation_id, signed)
                    .map(|()| (body.operation_id, wait))
                    .map_err(|e| format!("transport: cannot reach the node: {e}")),
            );
        }

        let mut out = Vec::with_capacity(ops.len());

        for q in queued {
            out.push(match q {
                Err(e) => Err(e),
                Ok((id, wait)) => match self.hub.wait(&id, wait).await {
                    None => Err("transport: the node did not answer in time".to_owned()),
                    Some(r) if r.ok => serde_json::from_str(&r.output)
                        .map_err(|_| "transport: the node sent an invalid answer".to_owned()),
                    Some(r) => Err(r.error),
                },
            });
        }

        out
    }
}
