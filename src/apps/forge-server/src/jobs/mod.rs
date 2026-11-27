//! Job kinds provided by the control plane. Later phases add deploy,
//! proxy and DNS jobs by registering more handlers.

pub mod agent_command;
pub mod agent_rollout;
pub mod agent_update;
pub mod app;
pub mod deploy;
mod diagnostics;
pub mod docker_endpoint;
pub mod docker_op;
pub mod node_bootstrap;
pub mod node_op;
pub mod package;
pub mod stack;
pub mod stack_rollout;
pub mod swarm;
pub mod swarm_rollout;
pub mod swarm_service;
pub mod swarm_stack;

pub use agent_command::AgentCommandJob;
pub use app::AppJob;
pub use diagnostics::DiagnosticsJob;
pub use node_bootstrap::NodeBootstrapJob;
pub use package::PackageJob;
pub use stack::StackJob;

/// AAD binding a credential vault item's sealed secret to its id (the vault
/// API seals with it too).
#[must_use]
pub fn vault_aad(id: forge_core::VaultItemId) -> String {
    format!("vault:{id}")
}
