//! `forge-analytics-worker` (D-141): the one process that owns analytics.
//!
//! forge-server spawns and supervises it, authorizes and audits every
//! request, and talks to it over a private control socket
//! (`forge-analytics-proto`). The worker owns the analytics store and, for
//! the embedded engine, the engine process; the node-minute and container
//! buffers, the log batch writer, log searches, retention, runtime tuning
//! and usage reports. Every buffer and queue in it is bounded.

pub mod analytics;
pub mod batcher;
pub mod control;
pub mod embedded;
pub mod ingest;
pub mod maps;
pub mod memory;
pub mod search;
pub mod server;
pub mod wiring;
