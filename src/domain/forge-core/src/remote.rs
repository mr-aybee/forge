//! Port for reaching a not-yet-enrolled machine over SSH (node bootstrap).
//!
//! Only the bootstrap job uses this. Once the agent is enrolled, all
//! management goes through the agent link and SSH is never used again.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::DomainResult;

/// Where to connect, with the host key the user confirmed (pinned).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteTarget {
    pub host: String,
    pub port: u16,
    pub username: String,
    /// `SHA256:…` fingerprint confirmed by the user. Any other key is refused.
    pub host_key_fingerprint: String,
}

/// Login material. `Debug` never prints secrets.
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RemoteAuth {
    Password {
        password: String,
    },
    PrivateKey {
        pem: String,
        passphrase: Option<String>,
    },
    /// Ephemeral key plus a short-lived OpenSSH user certificate signed by
    /// Forge's SSH user CA (browser terminal sessions). Never persisted.
    Certificate {
        key_openssh: String,
        cert_openssh: String,
    },
}

impl std::fmt::Debug for RemoteAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RemoteAuth::Password { .. } => f.write_str("RemoteAuth::Password(<redacted>)"),
            RemoteAuth::PrivateKey { .. } => f.write_str("RemoteAuth::PrivateKey(<redacted>)"),
            RemoteAuth::Certificate { .. } => f.write_str("RemoteAuth::Certificate(<redacted>)"),
        }
    }
}

/// What the probe saw before any credential was sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostKeyInfo {
    pub algorithm: String,
    pub fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecResult {
    pub status: u32,
    pub stdout: String,
    pub stderr: String,
}

impl ExecResult {
    #[must_use]
    pub fn ok(&self) -> bool {
        self.status == 0
    }
}

#[async_trait]
pub trait RemoteSession: Send + Sync {
    /// Runs one command; `stdin` is written then closed.
    async fn exec(&self, command: &str, stdin: Option<Vec<u8>>) -> DomainResult<ExecResult>;
    /// Opens an interactive login shell on a pseudo-terminal.
    async fn shell(&self, term: &str, size: TermSize) -> DomainResult<(Box<dyn ShellReader>, Box<dyn ShellWriter>)>;
    /// Runs `command` on a pseudo-terminal (an interactive program such as
    /// `docker exec -it`) with the same reader/writer pair as [`Self::shell`].
    /// Connectors that cannot do this return `Invalid("… not supported")`.
    async fn shell_command(
        &self,
        term: &str,
        size: TermSize,
        command: &str,
    ) -> DomainResult<(Box<dyn ShellReader>, Box<dyn ShellWriter>)> {
        let _ = (term, size, command);
        Err(crate::error::DomainError::Invalid("interactive commands are not supported by this connector".into()))
    }
    /// Runs one command without a terminal and streams its output: stdout
    /// and stderr apart, then the exit status. The writer feeds stdin
    /// (`close` sends end-of-file); `resize` does nothing.
    async fn exec_stream(&self, command: &str) -> DomainResult<(Box<dyn ExecReader>, Box<dyn ShellWriter>)>;
    async fn close(self: Box<Self>);
}

/// One event of a streamed command ([`RemoteSession::exec_stream`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecEvent {
    Stdout(Vec<u8>),
    Stderr(Vec<u8>),
    /// The command ended with this status (128 + signal number when killed).
    Exit(u32),
}

/// Output side of a streamed command.
#[async_trait]
pub trait ExecReader: Send {
    /// The next event; `None` once the channel closed.
    async fn recv(&mut self) -> Option<ExecEvent>;
}

/// Terminal size in character cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TermSize {
    pub cols: u32,
    pub rows: u32,
}

impl TermSize {
    /// Clamps to sane bounds so a client cannot request absurd sizes.
    #[must_use]
    pub fn clamped(self) -> Self {
        Self { cols: self.cols.clamp(10, 1000), rows: self.rows.clamp(2, 500) }
    }
}

/// Output side of an interactive shell.
#[async_trait]
pub trait ShellReader: Send {
    /// Next chunk of terminal output; `None` once the shell has exited.
    async fn recv(&mut self) -> Option<Vec<u8>>;
}

/// Input side of an interactive shell.
#[async_trait]
pub trait ShellWriter: Send + Sync {
    async fn write(&self, data: &[u8]) -> DomainResult<()>;
    async fn resize(&self, size: TermSize) -> DomainResult<()>;
    /// End of input (stdin closed); output keeps flowing.
    async fn eof(&self) -> DomainResult<()> {
        Ok(())
    }
    async fn close(&self);
}

#[async_trait]
pub trait RemoteConnector: Send + Sync + std::fmt::Debug {
    /// Connects without authenticating and reports the host key.
    async fn probe(&self, host: &str, port: u16) -> DomainResult<HostKeyInfo>;
    /// Connects, refusing any host key but the pinned one, and authenticates.
    async fn connect(&self, target: &RemoteTarget, auth: &RemoteAuth) -> DomainResult<Box<dyn RemoteSession>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_redacts_secrets() {
        let a = RemoteAuth::Password { password: "hunter2".into() };
        assert!(!format!("{a:?}").contains("hunter2"));
        let k = RemoteAuth::PrivateKey { pem: "-----BEGIN".into(), passphrase: Some("p".into()) };
        assert!(!format!("{k:?}").contains("BEGIN"));
    }
}
