//! Docker images, volumes and networks on nodes (D-157): what the API runs
//! beyond authorization and audit. Reads and quick changes are one signed
//! command ([`crate::node_rpc`]); pulls, pushes and prunes are `docker.op`
//! jobs ([`crate::jobs::docker_op`]).

use std::time::Duration;

use forge_catalog::docker_ops::DockerOp;
use forge_core::{Action, DomainError, DomainResult, ServerId};
use forge_proto::Command;
use forge_proto::messages::DockerAction;
use serde_json::Value;

use crate::node_rpc::NodeRpc;

/// How long a quick operation may take on the node.
const QUICK: Duration = Duration::from_secs(45);

/// The action an operation needs on the node.
#[must_use]
pub fn action_for(op: &DockerOp) -> Action {
    if op.read_only() {
        return Action::ServerManage;
    }

    // A push uses the workspace's stored registry credentials.
    if matches!(op, DockerOp::ImagePush { .. }) {
        return Action::RegistryManage;
    }

    match op.kind() {
        "image" => Action::ImageManage,
        "volume" => Action::VolumeManage,
        "network" => Action::NetworkManage,
        _ => Action::ServerManage,
    }
}

/// Pulls, pushes and prunes can take minutes: they run as jobs.
#[must_use]
pub const fn is_slow(op: &DockerOp) -> bool {
    matches!(
        op,
        DockerOp::ImagePull { .. }
            | DockerOp::ImagePush { .. }
            | DockerOp::ImagePrune { .. }
            | DockerOp::VolumePrune {}
            | DockerOp::NetworkPrune {}
    )
}

/// The command for `op`.
pub fn command(op: &DockerOp) -> DomainResult<Command> {
    op.validate().map_err(DomainError::Invalid)?;
    let op_json = serde_json::to_string(op).map_err(DomainError::internal)?;

    Ok(Command::DockerAction(DockerAction { op_json }))
}

#[derive(Debug)]
pub struct DockerResources {
    pub rpc: NodeRpc,
    pub servers: std::sync::Arc<dyn forge_core::ports::ServerRepository>,
}

impl DockerResources {
    /// The workspace of `server` (for jobs and audit).
    pub async fn workspace(&self, server: ServerId) -> DomainResult<forge_core::WorkspaceId> {
        Ok(self.servers.get_server(server).await?.workspace_id)
    }

    /// Runs a quick operation on `server` for `principal`.
    pub async fn run(&self, server: ServerId, principal: &str, op: &DockerOp) -> DomainResult<Value> {
        Ok(self.rpc.call(server, principal, command(op)?, QUICK).await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actions_follow_what_the_operation_touches() {
        assert_eq!(action_for(&DockerOp::Images {}), Action::ServerManage);
        assert_eq!(action_for(&DockerOp::ImageRemove { id: "nginx".into(), force: false }), Action::ImageManage);
        assert_eq!(action_for(&DockerOp::VolumePrune {}), Action::VolumeManage);
        assert_eq!(action_for(&DockerOp::NetworkRemove { id: "app".into() }), Action::NetworkManage);
        assert!(is_slow(&DockerOp::ImagePull { image: "nginx".into(), registry_auth: None }));
        assert!(!is_slow(&DockerOp::VolumeCreate { name: "data".into(), labels: Default::default(), remote: None }));
    }
}
