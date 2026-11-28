//! Which agents an update reaches: version ordering for agents against the
//! version this control plane hands out (D-039, D-098, amended by the
//! release-ordering decision in DECISIONS.md).
//!
//! Pure functions, no I/O: the API, the update job and the rollout all ask
//! here, so "outdated" means the same thing everywhere. [`update_verdict`]
//! says what an update would do on one node.

use std::cmp::Ordering;

use forge_core::model::{AgentStatus, Server};
pub use forge_proto::facts::{SelfUpdateCapability, UpdateIssue};
pub use forge_sdk::release::compare_release;
use serde::Serialize;

/// What an agent update would do on a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Outdated, online, and the node says it can update itself.
    WillUpdate,
    /// Outdated and cannot self-update: reinstall it (see `reasons`).
    NeedsReinstall,
    /// Outdated but offline: a rollout waits for it, then skips it.
    Offline,
    /// Runs a newer version than the one on offer: left alone.
    Ahead,
    /// Runs the version on offer.
    Current,
    /// Outdated and online, from an agent that does not report whether it
    /// can update itself (older agents). Eligible: its own answer decides.
    Unknown,
}

impl Verdict {
    /// Whether a rollout over "every outdated node" includes the node now.
    #[must_use]
    pub const fn eligible(self) -> bool {
        matches!(self, Self::WillUpdate | Self::Unknown)
    }
}

/// A node's verdict and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NodeVerdict {
    pub verdict: Verdict,
    /// Why it needs a reinstall.
    pub reasons: Vec<UpdateIssue>,
    /// Notes that do not block the update (an image-layer container).
    pub warnings: Vec<UpdateIssue>,
}

/// What an update to `target` would do on `node`. `capability` is the
/// node's reported self-update capability (`None` from older agents);
/// `has_build`: the control plane has agent binaries for the node's
/// architecture. Facts can be stale, so this only selects and informs: the
/// update job still relies on the node's own refusal.
#[must_use]
pub fn update_verdict(
    node: &Server,
    capability: Option<&SelfUpdateCapability>,
    target: &str,
    has_build: bool,
) -> NodeVerdict {
    let warnings = capability.map(|c| c.warnings.clone()).unwrap_or_default();
    let plain = |verdict| NodeVerdict { verdict, reasons: Vec::new(), warnings: warnings.clone() };

    if !is_outdated(&node.agent_version, target) {
        return plain(if is_ahead(&node.agent_version, target) { Verdict::Ahead } else { Verdict::Current });
    }

    let mut reasons = Vec::new();

    if !node.os.eq_ignore_ascii_case("linux") {
        reasons.push(UpdateIssue::NotLinux);
    } else {
        if !has_build {
            reasons.push(UpdateIssue::NoAgentBuild);
        }

        if let Some(c) = capability.filter(|c| !c.ready) {
            reasons.extend(c.reasons.iter().copied());
        }
    }

    if !reasons.is_empty() {
        return NodeVerdict { verdict: Verdict::NeedsReinstall, reasons, warnings };
    }

    if node.status != AgentStatus::Online {
        return plain(Verdict::Offline);
    }

    plain(if capability.is_some() { Verdict::WillUpdate } else { Verdict::Unknown })
}

/// The node runs an older version than `target`: an update would move it
/// forward. A node that runs a newer version (`ahead`) is never outdated, so
/// it is never rolled back to the target by a rollout.
#[must_use]
pub fn is_outdated(node_version: &str, target: &str) -> bool {
    compare_release(node_version, target) == Ordering::Less
}

/// The node runs `target` or a newer version: an update to `target` has
/// nothing left to do.
#[must_use]
pub fn has_reached(node_version: &str, target: &str) -> bool {
    !is_outdated(node_version, target)
}

/// The node runs a newer version than `target` (a hand-installed build, or
/// a control plane that was rolled back).
#[must_use]
pub fn is_ahead(node_version: &str, target: &str) -> bool {
    compare_release(node_version, target) == Ordering::Greater
}

#[cfg(test)]
mod tests {
    use forge_core::model::NodeRole;
    use forge_core::{ServerId, Timestamp, WorkspaceId};
    use forge_proto::facts::Supervisor;

    use super::*;

    fn node(os: &str, version: &str, status: AgentStatus) -> Server {
        Server {
            id: ServerId::new(),
            workspace_id: WorkspaceId::new(),
            environment_id: None,
            cluster_id: None,
            role: NodeRole::default(),
            display_name: None,
            hostname: "n".into(),
            os: os.into(),
            arch: "x86_64".into(),
            agent_version: version.into(),
            status,
            last_seen_at: Some(Timestamp(0)),
            cert_serial: None,
            cert_not_after: None,
            drained: false,
            created_at: Timestamp(0),
        }
    }

    fn cap(reasons: &[UpdateIssue], warnings: &[UpdateIssue]) -> SelfUpdateCapability {
        SelfUpdateCapability {
            supervisor: Supervisor::Systemd,
            ready: reasons.is_empty(),
            reasons: reasons.to_vec(),
            warnings: warnings.to_vec(),
            helper_version: "1.0.0".into(),
        }
    }

    #[test]
    fn verdicts_cover_every_case() {
        use AgentStatus::{Offline, Online};
        use UpdateIssue::{EphemeralContainer, NoAgentBuild, NoCurl, NoSystemd, NotLinux};
        let ready = cap(&[], &[]);
        let v = |n: &Server, c: Option<&SelfUpdateCapability>, build: bool| update_verdict(n, c, "2.0.0", build);
        let lin = |ver: &str, st| node("linux", ver, st);
        assert_eq!(v(&lin("2.0.0", Online), Some(&ready), true).verdict, Verdict::Current);
        assert_eq!(v(&lin("2.1.0", Online), Some(&ready), true).verdict, Verdict::Ahead);
        assert_eq!(v(&lin("1.0.0", Online), Some(&ready), true).verdict, Verdict::WillUpdate);

        // An older agent reports nothing: unknown, still eligible.
        let old = v(&lin("1.0.0", Online), None, true);
        assert_eq!(old.verdict, Verdict::Unknown);
        assert!(old.verdict.eligible() && Verdict::WillUpdate.eligible());
        assert_eq!(v(&lin("1.0.0", Offline), Some(&ready), true).verdict, Verdict::Offline);

        // Blockers win over offline: they need a person either way.
        let blocked = v(&lin("1.0.0", Offline), Some(&cap(&[NoSystemd, NoCurl], &[])), true);
        assert_eq!((blocked.verdict, blocked.reasons), (Verdict::NeedsReinstall, vec![NoSystemd, NoCurl]));
        let win = v(&node("windows", "1.0.0", Online), None, true);
        assert_eq!((win.verdict, win.reasons), (Verdict::NeedsReinstall, vec![NotLinux]));
        let nobuild = v(&lin("1.0.0", Online), Some(&ready), false);
        assert_eq!((nobuild.verdict, nobuild.reasons), (Verdict::NeedsReinstall, vec![NoAgentBuild]));

        // A warning does not block.
        let eph = v(&lin("1.0.0", Online), Some(&cap(&[], &[EphemeralContainer])), true);
        assert_eq!((eph.verdict, eph.warnings), (Verdict::WillUpdate, vec![EphemeralContainer]));
        assert!(!Verdict::NeedsReinstall.eligible() && !Verdict::Offline.eligible() && !Verdict::Ahead.eligible());
    }

    #[test]
    fn outdated_means_older_not_different() {
        assert!(is_outdated("0.1.0", "0.2.0"));
        assert!(is_outdated("1.0.0-rc-01", "1.0.0"), "a candidate is older than its final");
        assert!(is_outdated("", "0.1.0"), "an unknown version is older than any release");
        assert!(!is_outdated("0.2.0", "0.2.0"));
        assert!(!is_outdated("0.3.0", "0.2.0"), "a newer agent is ahead, not outdated");
        assert!(!is_outdated("1.0.0", "1.0.0-rc-01"));
        assert!(is_ahead("0.3.0", "0.2.0"));
        assert!(!is_ahead("0.2.0", "0.2.0"));
        assert!(has_reached("0.2.0", "0.2.0") && has_reached("0.2.1", "0.2.0"));
        assert!(!has_reached("0.2.0-dev.20261122.1", "0.2.0"));
    }
}
