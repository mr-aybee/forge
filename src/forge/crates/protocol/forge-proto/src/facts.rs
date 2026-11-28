//! Typed parts of the agent's `CollectFacts` answer (JSON), shared by the
//! agent, which reports them, and the control plane, which reads them.

use serde::{Deserialize, Serialize};

/// Whether this node can update its own agent, as the root helper sees it
/// (`self_update` in the node's facts). Older agents do not report it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelfUpdateCapability {
    /// What restarts the agent and helper after a swap.
    pub supervisor: Supervisor,
    /// The node can take an update now: `reasons` is empty.
    pub ready: bool,
    /// Why it cannot (a reinstall or a fix on the node is needed).
    #[serde(default)]
    pub reasons: Vec<UpdateIssue>,
    /// What to know before it does, without blocking it.
    #[serde(default)]
    pub warnings: Vec<UpdateIssue>,
    /// The helper's own version (the agent's may differ mid-update).
    #[serde(default)]
    pub helper_version: String,
}

/// What supervises the agent and helper.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Supervisor {
    Systemd,
    /// A shell loop restarts them (containers without systemd).
    Respawn,
    None,
    /// Named by a newer agent.
    #[serde(other)]
    Unknown,
}

/// Why a node cannot self-update, or what to know before it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateIssue {
    /// Self-update is for Linux nodes (control plane).
    NotLinux,
    /// The control plane has no agent build for the node's platform.
    NoAgentBuild,
    NoSystemd,
    NoCurl,
    /// `/usr/local/bin` is read-only or not writable by root.
    BinDirNotWritable,
    /// Less than twice the binaries' size free next to them.
    LowDisk,
    /// The binaries live in a container's image layer: an update lasts until
    /// the container is recreated (a warning, not a blocker).
    EphemeralContainer,
    /// Named by a newer agent.
    #[serde(other)]
    Unknown,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_round_trip_and_tolerate_newer_agents() {
        let c = SelfUpdateCapability {
            supervisor: Supervisor::Systemd,
            ready: false,
            reasons: vec![UpdateIssue::NoCurl, UpdateIssue::LowDisk],
            warnings: vec![UpdateIssue::EphemeralContainer],
            helper_version: "1.2.3".into(),
        };
        let raw = serde_json::to_value(&c).unwrap_or_default();
        assert_eq!(raw["reasons"][0], "no_curl");
        assert_eq!(serde_json::from_value::<SelfUpdateCapability>(raw).ok(), Some(c));
        let newer = serde_json::json!({ "supervisor": "launchd", "ready": false, "reasons": ["quantum"] });
        let back: Option<SelfUpdateCapability> = serde_json::from_value(newer).ok();
        assert_eq!(back.as_ref().map(|c| c.supervisor), Some(Supervisor::Unknown));
        assert_eq!(back.map(|c| c.reasons), Some(vec![UpdateIssue::Unknown]));
    }
}
