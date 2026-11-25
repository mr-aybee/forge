//! All forge-server integration tests in one binary.
//!
//! Each file under `tests/it/` is a module; the shared harness in
//! `tests/common/mod.rs` is compiled once. One binary instead of one per file
//! keeps linking time and `target/` size down: every integration test binary
//! statically links the whole server.
//!
//! Run one file's tests with a module filter, e.g.
//! `cargo test -p forge-server --test it api_authz::`.
#![allow(clippy::unwrap_used)]

#[path = "../common/mod.rs"]
mod common;

mod access_profiles;
mod account;
mod agent_binary_smoke;
mod agent_cert_renewal;
mod agent_link;
mod agent_pinning;
mod agent_rollout;
mod alert_templates;
mod alerts;
mod analytics_settings;
// Every test in this module is Unix-only; its helpers are dead code elsewhere.
#[cfg(unix)]
mod analytics_worker;
mod any_address;
mod api_authz;
mod app_deploy;
mod app_health;
mod app_mounts;
mod app_roots;
mod audit;
mod captcha;
mod cluster_modes;
mod compression;
mod console_access;
mod container_shell;
mod device_login;
mod docker_resources;
mod edge_security;
mod elevated_tokens;
mod environments;
mod files;
mod idempotency;
mod installer_docs;
mod login_challenge;
mod mcp;
mod names;
mod native_login;
mod notifications;
mod notifications_cli;
mod object_storage;
mod observations;
mod passkeys;
mod pins;
mod process_actions;
mod resume;
mod scale;
mod server_settings;
mod sftp;
mod silences;
mod stack_versions;
mod stacks;
mod swarm;
mod tags;
mod terminal_accounts;
mod terminal_cli;
mod tunnels;
mod webhooks;
mod workspace;
