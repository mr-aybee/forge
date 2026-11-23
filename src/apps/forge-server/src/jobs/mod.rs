//! Job kinds provided by the control plane. Later phases add deploy,
//! proxy and DNS jobs by registering more handlers.

pub mod agent_command;
pub mod agent_rollout;
pub mod agent_update;
pub mod app;
pub mod deploy;
mod diagnostics;
pub mod docker_endpoint;
pub mod node_bootstrap;
pub mod node_op;
pub mod package;
pub mod stack;
pub mod storage;
pub mod swarm;
pub mod swarm_rollout;
pub mod swarm_service;

pub use agent_command::AgentCommandJob;
pub use app::AppJob;
pub use diagnostics::DiagnosticsJob;
pub use node_bootstrap::NodeBootstrapJob;
pub use package::PackageJob;
pub use stack::StackJob;
pub use storage::StorageJob;
