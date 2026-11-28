//! forge-server's side of the analytics worker (D-141): the control-socket
//! client, the supervisor that runs the worker, and the maps it is fed.
//! No analytics data, buffer or queue lives in forge-server.

pub mod client;
pub mod maps;
pub mod supervisor;

pub use client::{CallError, WorkerClient};
pub use maps::{MapsDeps, MapsPublisher};
pub use supervisor::{
    AUDIT_WORKER_RESTART, ListenerCert, ListenerCerts, RestartReason, SupervisorConfig, SupervisorDeps, WorkerState,
    WorkerStatus, WorkerSupervisor,
};
