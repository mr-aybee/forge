//! The console's public address: the URL people are sent to in links
//! (alert emails, webhook payloads, approvals, device sign-in).
//!
//! Three sources, in order: the runtime setting [`SETTING_KEY`] (edited in
//! Settings → Server), the configured `public_url` (optional), and the
//! address an administrator last signed in on ([`LAST_ADMIN_KEY`], recorded
//! automatically, D-167). The first two "pin" the address. The value is
//! shared by everything that builds links without a request to copy from
//! (alert emails, webhooks, OIDC), so a change applies without a restart.

use std::sync::{Arc, PoisonError, RwLock};

/// Settings key that holds the runtime public address (empty or absent:
/// the configured `public_url`).
pub const SETTING_KEY: &str = "server.public_address";

/// Settings key that holds the console address an administrator last
/// signed in on (written by the server, read-only for people).
pub const LAST_ADMIN_KEY: &str = "server.last_admin_address";

/// Shared, runtime-editable public address with the configured fallback.
#[derive(Debug, Clone)]
pub struct PublicAddress {
    configured: Arc<str>,
    runtime: Arc<RwLock<Option<String>>>,
    last_admin: Arc<RwLock<Option<String>>>,
}

impl Default for PublicAddress {
    fn default() -> Self {
        Self::new("")
    }
}

impl From<&str> for PublicAddress {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

impl PublicAddress {
    /// `configured` is the `forge.toml` `public_url` (empty: not set).
    #[must_use]
    pub fn new(configured: &str) -> Self {
        Self {
            configured: Arc::from(configured.trim_end_matches('/')),
            runtime: Arc::default(),
            last_admin: Arc::default(),
        }
    }

    /// The address links use: the pinned address, else the last
    /// administrator address, else empty (no address known yet). Never ends
    /// in `/`.
    #[must_use]
    pub fn get(&self) -> String {
        self.pinned().or_else(|| self.last_admin()).unwrap_or_default()
    }

    /// The pinned address: the runtime setting, else the configured URL.
    #[must_use]
    pub fn pinned(&self) -> Option<String> {
        self.runtime().or_else(|| Some(self.configured.to_string()).filter(|c| !c.is_empty()))
    }

    /// The address an administrator last signed in on, if recorded.
    #[must_use]
    pub fn last_admin(&self) -> Option<String> {
        self.last_admin.read().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Replaces the last administrator address. The caller validates it.
    pub fn set_last_admin(&self, value: Option<String>) {
        let v = value.map(|v| v.trim_end_matches('/').to_owned()).filter(|v| !v.is_empty());
        *self.last_admin.write().unwrap_or_else(PoisonError::into_inner) = v;
    }

    /// The runtime setting, if one is set.
    #[must_use]
    pub fn runtime(&self) -> Option<String> {
        self.runtime.read().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// The configured `public_url` (the fallback).
    #[must_use]
    pub fn configured(&self) -> &str {
        &self.configured
    }

    /// Replaces the runtime setting (`None` or empty: use the configured URL).
    /// The caller validates the value first.
    pub fn set_runtime(&self, value: Option<String>) {
        let v = value.map(|v| v.trim_end_matches('/').to_owned()).filter(|v| !v.is_empty());
        *self.runtime.write().unwrap_or_else(PoisonError::into_inner) = v;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_value_wins_over_configured_and_clears_back() {
        let a = PublicAddress::new("http://192.0.2.1:55130/");
        assert_eq!(a.get(), "http://192.0.2.1:55130");
        let shared = a.clone();
        shared.set_runtime(Some("https://forge.example.com/".into()));
        assert_eq!(a.get(), "https://forge.example.com", "clones share the value");
        assert_eq!(a.configured(), "http://192.0.2.1:55130");
        a.set_runtime(Some(String::new()));
        assert_eq!(a.get(), "http://192.0.2.1:55130");
        assert_eq!(a.runtime(), None);
    }

    #[test]
    fn last_admin_address_is_used_only_without_a_pinned_one() {
        let a = PublicAddress::new("");
        assert_eq!(a.get(), "", "nothing known yet");
        assert_eq!(a.pinned(), None);
        a.set_last_admin(Some("http://192.0.2.7:3880/".into()));
        assert_eq!(a.get(), "http://192.0.2.7:3880");
        a.set_runtime(Some("https://forge.example.com".into()));
        assert_eq!(a.get(), "https://forge.example.com", "a pinned address wins");
        let c = PublicAddress::new("https://configured.example.com");
        c.set_last_admin(Some("http://192.0.2.7:3880".into()));
        assert_eq!(c.get(), "https://configured.example.com");
    }
}
