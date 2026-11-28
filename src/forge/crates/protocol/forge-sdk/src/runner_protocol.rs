//! Line-delimited JSON protocol between the plugin host and the runner process.
//!
//! 1. Host writes one [`RunRequest`] line to the runner's stdin.
//! 2. Runner writes [`RunnerMessage`] lines to stdout. For each `HostCall`
//!    the host answers with one [`HostReply`] line on stdin.
//! 3. Runner ends with `Done` or `Failed` and exits.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunRequest {
    /// Path of the verified component on disk.
    pub artifact_path: String,
    pub export: String,
    pub input: u32,
    pub fuel: u64,
    pub memory_bytes: u64,
    pub timeout_ms: u64,
    pub max_output_bytes: u32,
    /// SHA-256 (lowercase hex) of the installed artifact. The runner checks
    /// the bytes it actually loads against it, so a file swapped after the
    /// host's own check is never run.
    #[serde(default)]
    pub artifact_sha256: String,
}

impl RunRequest {
    /// Checks `bytes` against [`Self::artifact_sha256`]; an empty digest is
    /// refused too.
    pub fn verify_artifact(&self, bytes: &[u8]) -> Result<(), String> {
        let digest = ring::digest::digest(&ring::digest::SHA256, bytes);
        let hex: String = digest.as_ref().iter().map(|b| format!("{b:02x}")).collect();
        if self.artifact_sha256.is_empty() || hex != self.artifact_sha256 {
            return Err("the artifact does not match its installed digest".into());
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    FuelExhausted,
    Timeout,
    MemoryExceeded,
    OutputExceeded,
    Denied,
    Trap,
    Invalid,
}

impl FailureKind {
    /// Budget violations quarantine the plugin immediately.
    #[must_use]
    pub const fn is_budget_violation(self) -> bool {
        matches!(self, Self::FuelExhausted | Self::Timeout | Self::MemoryExceeded | Self::OutputExceeded)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RunnerMessage {
    HostCall { id: u64, function: String, args: Value },
    Done { output: u32, fuel_used: u64 },
    Failed { kind: FailureKind, message: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HostReply {
    pub id: u64,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub error: Option<String>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn req(sha: &str) -> RunRequest {
        RunRequest {
            artifact_path: String::new(),
            export: "run".into(),
            input: 0,
            fuel: 1,
            memory_bytes: 1,
            timeout_ms: 1,
            max_output_bytes: 1,
            artifact_sha256: sha.into(),
        }
    }

    /// Security review: the runner read the artifact again after the host
    /// checked it; it now checks the bytes it actually loads.
    #[test]
    fn the_runner_checks_the_bytes_it_loads() {
        let abc = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert!(req(abc).verify_artifact(b"abc").is_ok());
        assert!(req(abc).verify_artifact(b"abd").is_err(), "a swapped file is refused");
        assert!(req("").verify_artifact(b"abc").is_err(), "no digest, no run");
    }
}
