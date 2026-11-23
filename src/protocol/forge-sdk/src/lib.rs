//! Orbyn Forge plugin SDK.
//!
//! Shared by plugin authors and the plugin host so both agree on one schema:
//!
//! * [`WIT`] — the component interface (`wit/forge-plugin.wit`).
//! * [`manifest`] — `forge-plugin.toml` schema and validation.
//! * [`signing`] — package signatures (Ed25519 over the manifest and artifact digests).
//! * [`release`] — verifying Forge release downloads (signed `SHA256SUMS`).
//!
//! Writing a plugin in Rust: create a `cdylib` crate, add `wit-bindgen`, call
//! `wit_bindgen::generate!({ world: "plugin", path: "<path to forge-plugin.wit>" })`,
//! implement `Guest::run`, build for `wasm32-unknown-unknown` and wrap it with
//! `wasm-tools component new`. See the plugin guide in the documentation.

pub mod manifest;
pub mod release;
pub mod runner_protocol;
pub mod signing;

/// The WIT source of the plugin interface.
pub const WIT: &str = include_str!("../wit/forge-plugin.wit");

/// Fully qualified name of the host interface instance.
pub const HOST_INTERFACE: &str = "forge:plugin/host@0.1.0";

/// The plugin API version implemented by this SDK.
pub const PLUGIN_API_VERSION: &str = "0.1.0";

/// Host functions and the permission each requires.
pub mod host_functions {
    pub const LOG: &str = "log";
    pub const SERVER_ONLINE: &str = "server-online";

    /// Permission string for a host function, e.g. `host:server-online`.
    #[must_use]
    pub fn permission(function: &str) -> String {
        format!("host:{function}")
    }

    /// All host functions known to this API version.
    pub const ALL: [&str; 2] = [LOG, SERVER_ONLINE];
}
