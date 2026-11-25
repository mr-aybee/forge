//! Docker Engine for container services (issue #64).
//!
//! A container service needs Docker Engine on every node it is placed on.
//! Deploying one installs the App Store's Docker Engine first on a node that
//! has none, through the same install job as any other package. A node only
//! gets an install when it is known to lack the engine:
//!
//! * Forge has no Docker Engine installation for it that is installed or
//!   being installed, **and**
//! * the node itself answers that its Docker Engine cannot be reached.
//!
//! So an engine that was installed outside Forge is never reinstalled, and a
//! node that is offline or silent is left alone (its deploy job then reports
//! the node's own error).

use std::time::Duration;

use forge_catalog::apps::{DOCKER_PACKAGE, DOCKER_UNAVAILABLE};
use forge_core::ServerId;
use forge_core::model::{Installation, Server};
use forge_core::ports::{InstallationRepository, ServerRepository};
use forge_proto::Command;
use forge_proto::messages::{CommandResult, ContainerAction};

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// How long the node may take to answer the probe.
const PROBE_WAIT: Duration = Duration::from_secs(15);

/// What is known about Docker Engine on a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    /// The node's Docker Engine answered.
    Present,
    /// The node answered that it has no reachable Docker Engine.
    Missing,
    /// The node did not answer, or failed for another reason.
    Unknown,
}

/// What a deploy does about the Docker Engine installation Forge recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recorded {
    /// Nothing recorded, or a record that says nothing about the engine now
    /// (a failed install, a package the node reported as gone).
    None,
    /// An install or another action is running: wait for it.
    Busy,
    /// Installed.
    Installed,
    /// The last action failed.
    Failed,
}

/// Forge's Docker Engine installation on a node, among the node's `installations`.
#[must_use]
pub fn installation(installations: &[Installation]) -> Option<&Installation> {
    installations.iter().find(|i| i.package == DOCKER_PACKAGE)
}

/// What Forge's own record says about Docker Engine on a node.
#[must_use]
pub fn recorded(installations: &[Installation]) -> Recorded {
    match installation(installations).map(|i| i.status.as_str()) {
        None | Some("missing") => Recorded::None,
        Some("installed") => Recorded::Installed,
        Some("failed") => Recorded::Failed,
        // `pending`, `installing`, `updating`, `restarting`, …
        Some(_) => Recorded::Busy,
    }
}

/// Reads the node's answer to a Docker `info` command.
#[must_use]
pub fn classify(answer: Option<&CommandResult>) -> Engine {
    match answer {
        Some(r) if r.ok => Engine::Present,
        Some(r) if r.error.starts_with(DOCKER_UNAVAILABLE) => Engine::Missing,
        _ => Engine::Unknown,
    }
}

/// Whether a deploy asks the node about its Docker Engine: only when Forge
/// has no usable record of one. Forge's own installation is trusted.
#[must_use]
pub fn asks_node(recorded: Recorded) -> bool {
    matches!(recorded, Recorded::None | Recorded::Failed)
}

/// Whether a deploy installs Docker Engine on a node: Forge has no usable
/// record of it and the node says it is missing.
#[must_use]
pub fn install_needed(recorded: Recorded, engine: Engine) -> bool {
    asks_node(recorded) && engine == Engine::Missing
}

/// The version of the Docker Engine package (the catalog has one line).
pub fn package_version() -> ApiResult<&'static str> {
    forge_catalog::find(DOCKER_PACKAGE)
        .and_then(|p| p.versions.first().copied())
        .ok_or_else(|| ApiError::conflict("the App Store has no Docker Engine package"))
}

/// Asks the node whether its Docker Engine answers. Read-only (`docker
/// info`), signed for `principal`; a node whose agent is not polling this
/// server right now is not asked.
async fn probe(state: &AppState, server: &Server, principal: &str) -> Engine {
    if !crate::agent_hub::linked(state, server.id) {
        return Engine::Unknown;
    }
    let command = Command::ContainerAction(ContainerAction { op: "info".into(), ..ContainerAction::default() });
    let (body, signed) = state.signer.issue(&server.id.to_string(), principal, command, state.clock.now(), PROBE_WAIT);
    if let Err(e) = state.hub.enqueue(server.id, &body.operation_id, signed) {
        tracing::warn!(server = %server.id, error = %e, "could not ask the node about Docker Engine");
        return Engine::Unknown;
    }
    classify(state.hub.wait(&body.operation_id, PROBE_WAIT).await.as_ref())
}

/// The nodes among `servers` that lack Docker Engine. Nodes on an OS the
/// package does not support are skipped: the deploy job reports what the
/// node says.
pub async fn nodes_without_engine(state: &AppState, principal: &str, servers: &[ServerId]) -> ApiResult<Vec<ServerId>> {
    let Some(package) = forge_catalog::find(DOCKER_PACKAGE) else { return Ok(Vec::new()) };
    let mut missing = Vec::new();
    for &id in servers {
        let server = state.store.get_server(id).await.map_err(|_| ApiError::not_found("server not found"))?;
        if !package.os.contains(&server.os.as_str()) {
            continue;
        }
        let record = recorded(&state.store.list_installations(Some(id)).await?);
        if asks_node(record) && install_needed(record, probe(state, &server, principal).await) {
            missing.push(id);
        }
    }
    Ok(missing)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn docker(status: &str) -> Installation {
        Installation {
            id: forge_core::InstallationId::new(),
            workspace_id: forge_core::WorkspaceId::new(),
            server_id: ServerId::new(),
            package: DOCKER_PACKAGE.into(),
            version: "stable".into(),
            params: std::collections::BTreeMap::new(),
            status: status.into(),
            state: None,
            env: std::collections::BTreeMap::new(),
            last_job_id: None,
            last_error: None,
            versions: None,
            update_policy: "notify".into(),
            created_at: forge_core::Timestamp(0),
            updated_at: forge_core::Timestamp(0),
        }
    }

    fn answer(ok: bool, error: &str) -> CommandResult {
        CommandResult {
            operation_id: "op".into(),
            ok,
            output: String::new(),
            error: error.into(),
            ..Default::default()
        }
    }

    #[test]
    fn the_record_says_installed_busy_failed_or_nothing() {
        assert_eq!(recorded(&[]), Recorded::None);
        assert_eq!(recorded(&[docker("installed")]), Recorded::Installed);
        for busy in ["pending", "installing", "updating", "restarting"] {
            assert_eq!(recorded(&[docker(busy)]), Recorded::Busy, "{busy}");
        }
        assert_eq!(recorded(&[docker("failed")]), Recorded::Failed);
        // `missing` ends in "ing" but is no action in progress: the node reported the package gone.
        assert_eq!(recorded(&[docker("missing")]), Recorded::None);
        let mut other = docker("installed");
        other.package = "nginx".into();
        assert_eq!(recorded(&[other]), Recorded::None);
    }

    #[test]
    fn only_the_nodes_own_answer_makes_the_engine_missing() {
        let gone = format!("{DOCKER_UNAVAILABLE} (/var/run/docker.sock: No such file or directory)");
        assert_eq!(classify(Some(&answer(false, &gone))), Engine::Missing);
        assert_eq!(classify(Some(&answer(true, ""))), Engine::Present);
        assert_eq!(classify(Some(&answer(false, "unknown command"))), Engine::Unknown, "an old agent");
        assert_eq!(classify(None), Engine::Unknown, "no answer in time");
    }

    #[test]
    fn docker_is_installed_only_when_unrecorded_and_missing() {
        assert!(install_needed(Recorded::None, Engine::Missing));
        assert!(install_needed(Recorded::Failed, Engine::Missing), "a failed install is tried again");
        // An engine installed outside Forge, or a silent node: left alone.
        assert!(!install_needed(Recorded::None, Engine::Present));
        assert!(!install_needed(Recorded::None, Engine::Unknown));
        // Forge's own installation is trusted; the node is not even asked.
        for known in [Recorded::Installed, Recorded::Busy] {
            assert!(!asks_node(known) && !install_needed(known, Engine::Missing));
        }
        assert!(package_version().is_ok(), "the catalog has the Docker Engine package");
    }
}
