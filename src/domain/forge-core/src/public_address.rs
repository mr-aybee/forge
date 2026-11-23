//! The console's public address: the URL people are sent to in links
//! (alert emails, webhook payloads, approvals, device sign-in).
//!
//! Two sources, in order: the runtime setting [`SETTING_KEY`] (edited in
//! Settings → General) and the configured `public_url`. The value is shared
//! by everything that builds links, so a change applies without a restart.

use std::sync::{Arc, PoisonError, RwLock};

/// Settings key that holds the runtime public address (empty or absent:
/// the configured `public_url`).
pub const SETTING_KEY: &str = "server.public_address";

/// Shared, runtime-editable public address with the configured fallback.
#[derive(Debug, Clone)]
pub struct PublicAddress {
    configured: Arc<str>,
    runtime: Arc<RwLock<Option<String>>>,
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
    /// `configured` is the `forge.toml` `public_url`.
    #[must_use]
    pub fn new(configured: &str) -> Self {
        Self { configured: Arc::from(configured.trim_end_matches('/')), runtime: Arc::default() }
    }

    /// The address links use: the runtime setting, else the configured URL.
    /// Never ends in `/`.
    #[must_use]
    pub fn get(&self) -> String {
        self.runtime().unwrap_or_else(|| self.configured.to_string())
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
}
