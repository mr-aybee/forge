//! Orbyn Forge agent protocol.
//!
//! * [`messages`] — protobuf messages (prost), mirrored in `proto/`.
//! * [`CommandSigner`] — control-plane side: builds and signs commands.
//! * [`CommandVerifier`] — agent side: checks signature, target, validity
//!   window, maximum TTL, restart boundary and replays.
//! * [`facts`] — typed parts of the agent's `CollectFacts` answer.

pub mod exec_frame;
pub mod facts;
pub mod messages;
mod signing;

pub use messages::command_body::Command;
pub use signing::{CommandSigner, CommandVerifier, VerifyError};

/// Media type for protobuf bodies.
pub const CONTENT_TYPE: &str = "application/x-protobuf";

/// Agent link endpoints.
pub mod paths {
    /// GET: the CA certificate PEM (public; verified against the token pin).
    pub const CA: &str = "/agent/v1/ca";
    pub const ENROLL: &str = "/agent/v1/enroll";
    pub const HEARTBEAT: &str = "/agent/v1/heartbeat";
    pub const RENEW: &str = "/agent/v1/renew";
    /// POST `/agent/v1/tunnel/<connection_id>`: a port-tunnel data stream (D-091).
    pub const TUNNEL: &str = "/agent/v1/tunnel/";
    /// POST `/agent/v1/exec/<session_id>`: a container shell stream (D-114).
    pub const EXEC: &str = "/agent/v1/exec/";
    /// POST: vault-backed header values of one HTTP health check (D-111).
    pub const HEALTH_SECRETS: &str = "/agent/v1/health-secrets";
    /// Spool upload to the analytics worker's agent listener (D-142).
    pub const SPOOL: &str = "/agent/v1/spool";
}

/// Longest validity a command may carry.
pub const MAX_COMMAND_TTL: std::time::Duration = std::time::Duration::from_secs(300);
/// Clock skew tolerated between control plane and agent.
pub const MAX_CLOCK_SKEW: std::time::Duration = std::time::Duration::from_secs(30);
