//! All forge-store integration tests in one binary: each file under
//! `tests/it/` is a module, so the crate links once instead of per file.
#![allow(clippy::unwrap_used)]

mod alerts;
mod audit;
mod deploy;
mod files;
mod migrations;
mod notifications;
mod observations;
mod permissions;
mod stacks;
mod store;
