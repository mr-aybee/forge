use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use forge_core::{DomainError, DomainResult};
use forge_sdk::runner_protocol::{FailureKind, HostReply, RunRequest, RunnerMessage};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

/// Answers host calls for one invocation.
#[async_trait]
pub trait HostCalls: Send + Sync {
    /// Answers one host function call.
    async fn call(&self, function: &str, args: Value) -> Result<Value, String>;
}

/// Executes a component invocation in isolation.
#[async_trait]
pub trait PluginRunner: Send + Sync + std::fmt::Debug {
    /// Runs one invocation, answering its host calls through `host`.
    async fn run(&self, req: RunRequest, host: &dyn HostCalls) -> DomainResult<RunnerMessage>;
}

/// Spawns the `forge-plugin-runner` binary for each invocation.
#[derive(Debug, Clone)]
pub struct ProcessRunner {
    binary: PathBuf,
    /// Extra wall time for process start-up and engine compilation.
    startup_grace: Duration,
}

impl ProcessRunner {
    /// Creates a runner that spawns `binary`.
    #[must_use]
    pub fn new(binary: PathBuf) -> Self {
        Self { binary, startup_grace: Duration::from_secs(5) }
    }
}

/// Upper bound on a single protocol line from the runner.
const MAX_LINE: u64 = 1024 * 1024;

#[async_trait]
impl PluginRunner for ProcessRunner {
    async fn run(&self, req: RunRequest, host: &dyn HostCalls) -> DomainResult<RunnerMessage> {
        let mut child = tokio::process::Command::new(&self.binary)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| DomainError::Internal(format!("cannot start plugin runner: {e}")))?;
        let mut stdin = child.stdin.take().ok_or_else(|| DomainError::internal("runner stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| DomainError::internal("runner stdout"))?;
        let mut lines = BufReader::new(stdout);

        let deadline = Duration::from_millis(req.timeout_ms) + self.startup_grace;
        let mut first = serde_json::to_string(&req).map_err(DomainError::internal)?;
        first.push('\n');

        let conversation = async {
            stdin.write_all(first.as_bytes()).await.map_err(DomainError::internal)?;
            stdin.flush().await.map_err(DomainError::internal)?;

            loop {
                let mut line = String::new();
                let n = (&mut lines).take(MAX_LINE).read_line(&mut line).await.map_err(DomainError::internal)?;
                if n == 0 {
                    return Ok(RunnerMessage::Failed {
                        kind: FailureKind::Trap,
                        message: "runner exited unexpectedly".into(),
                    });
                }

                let msg: RunnerMessage = match serde_json::from_str(&line) {
                    Ok(m) => m,
                    Err(_) => {
                        return Ok(RunnerMessage::Failed {
                            kind: FailureKind::OutputExceeded,
                            message: "malformed or oversized runner output".into(),
                        });
                    }
                };

                match msg {
                    RunnerMessage::HostCall { id, function, args } => {
                        let reply = match host.call(&function, args).await {
                            Ok(v) => HostReply { id, result: Some(v), error: None },
                            Err(e) => HostReply { id, result: None, error: Some(e) },
                        };

                        let mut out = serde_json::to_string(&reply).map_err(DomainError::internal)?;
                        out.push('\n');
                        stdin.write_all(out.as_bytes()).await.map_err(DomainError::internal)?;
                        stdin.flush().await.map_err(DomainError::internal)?;
                    }
                    terminal => return Ok(terminal),
                }
            }
        };

        let outcome = match tokio::time::timeout(deadline, conversation).await {
            Ok(r) => r,
            Err(_) => {
                Ok(RunnerMessage::Failed { kind: FailureKind::Timeout, message: "wall-clock deadline exceeded".into() })
            }
        };

        // Always reap the process; kill it if it is still running.
        let _ = child.start_kill();
        let _ = child.wait().await;

        outcome
    }
}
