//! Orbyn Forge node agent.
//!
//! The agent dials out to the control plane (no inbound ports), authenticates
//! with its per-agent certificate, long-polls for signed commands over one
//! HTTP/2 connection, and executes only commands that pass:
//!
//! 1. Ed25519 signature by the control plane's command key,
//! 2. target server id == this agent,
//! 3. validity window, maximum TTL and restart boundary,
//! 4. replay cache,
//! 5. the local allowlist in `agent.toml` (the operator of the server has the
//!    final say over what the agent may do).
//!
//! Privileged operations go through `forge-agent-helper`, which accepts a
//! fixed set of typed requests only.

pub mod agent;
#[cfg(unix)]
pub mod apps;
pub mod config;
pub mod container_exec;
pub mod container_telemetry;
#[cfg(unix)]
pub mod docker;
#[cfg(unix)]
pub mod docker_ops;
#[cfg(unix)]
pub mod docker_tls;
pub mod enroll;
pub mod exec_stream;
pub mod executor;
pub mod fabric;
pub mod files;
pub mod health;
pub mod health_exec;
pub mod health_probe;
pub mod health_secrets;
pub mod helper;
pub mod helper_trust;
pub mod host_names;
pub mod ingress;
pub mod link;
pub mod log_listener;
pub mod metrics;
pub mod node_analytics;
pub mod pins;
pub mod policy;
pub mod probe;
pub mod processes;
#[cfg(windows)]
pub mod processes_windows;
pub mod progress;
pub mod recipes;
pub mod self_update;
pub mod service;
pub mod sites;
pub mod spool;
pub mod spool_upload;
pub mod ssh_principals;
#[cfg(unix)]
pub mod stacks;
#[cfg(unix)]
pub mod swarm;
#[cfg(unix)]
pub mod swarm_stack;
pub mod terminal_accounts;
pub mod tls;
pub mod tunnel;
#[cfg(windows)]
pub mod winservice;

pub use agent::Agent;
pub use config::{AgentConfig, AgentIdentity, StatePaths};

/// Agent version reported to the control plane.
/// `FORGE_BUILD_VERSION` at compile time overrides it (local test releases).
pub const VERSION: &str = match option_env!("FORGE_BUILD_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};
