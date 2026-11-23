//! Which Docker containers a node collects logs and metrics from.
//!
//! The node's helper follows the output of each running container and
//! samples its resources once a minute; the agent sends both with its
//! heartbeat, and forge-server forwards them to the analytics worker as
//! received (D-141), which attributes and stores them. What stays here is
//! the node setting (`node.<id>.container_telemetry`), sent back with every
//! heartbeat answer.

use std::collections::HashMap;
use std::sync::Arc;

use forge_core::ports::SettingsRepository;
use forge_core::{Clock, DomainError, ServerId};
use forge_proto::messages::{ContainerTelemetryConfig, MAX_CONTAINER_EXCLUDES, MAX_CONTAINER_FIELD_BYTES};
use serde::{Deserialize, Serialize};

/// Audit action for a change of a node's collection settings.
pub const AUDIT_SETTINGS_UPDATE: &str = "server.container_telemetry.update";
const SETTINGS_PREFIX: &str = "node.";
const SETTINGS_SUFFIX: &str = ".container_telemetry";

/// The settings key of a node's collection settings.
#[must_use]
pub fn settings_key(server: ServerId) -> String {
    format!("{SETTINGS_PREFIX}{server}{SETTINGS_SUFFIX}")
}

/// What a node collects from its Docker containers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContainerTelemetrySettings {
    /// Collect logs and metrics of every container (default `true`).
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Containers left out: a container name, or `key=value` to match a
    /// label. At most 64 entries of up to 256 characters.
    #[serde(default)]
    pub exclude: Vec<String>,
}

fn yes() -> bool {
    true
}

impl Default for ContainerTelemetrySettings {
    fn default() -> Self {
        Self { enabled: true, exclude: Vec::new() }
    }
}

impl ContainerTelemetrySettings {
    /// Trimmed, de-duplicated and checked.
    pub fn normalized(mut self) -> Result<Self, TelemetryError> {
        let mut seen = Vec::with_capacity(self.exclude.len());
        for e in self.exclude.drain(..) {
            let e = e.trim().to_owned();
            if e.is_empty() || e.len() > MAX_CONTAINER_FIELD_BYTES || e.chars().any(char::is_control) {
                return Err(TelemetryError::Invalid(
                    "an exclude entry is 1 to 256 characters, without control characters",
                ));
            }
            if !seen.contains(&e) {
                seen.push(e);
            }
        }
        if seen.len() > MAX_CONTAINER_EXCLUDES {
            return Err(TelemetryError::Invalid("at most 64 exclude entries"));
        }
        self.exclude = seen;
        Ok(self)
    }
}

/// Why container telemetry could not be stored or changed.
#[derive(Debug, thiserror::Error)]
pub enum TelemetryError {
    #[error("{0}")]
    Invalid(&'static str),
    #[error(transparent)]
    Domain(#[from] DomainError),
}

/// Settings stored under [`settings_key`], from `(key, value)` rows. A row
/// that cannot be read is skipped with a warning: the node keeps the default.
fn parse_settings(rows: Vec<(String, String)>) -> HashMap<ServerId, ContainerTelemetrySettings> {
    rows.into_iter()
        .filter_map(|(k, v)| {
            let id = k.strip_prefix(SETTINGS_PREFIX)?.strip_suffix(SETTINGS_SUFFIX)?.parse().ok()?;
            match serde_json::from_str(&v) {
                Ok(s) => Some((id, s)),
                Err(e) => {
                    tracing::warn!(key = %k, error = %e, "unreadable container telemetry setting; using the default");
                    None
                }
            }
        })
        .collect()
}

/// Wiring for [`ContainerTelemetry::new`].
#[derive(Debug)]
pub struct TelemetryDeps {
    pub settings: Arc<dyn SettingsRepository>,
    pub clock: Arc<dyn Clock>,
    /// Analytics is on: something can store what nodes collect.
    pub analytics_enabled: bool,
}

/// Holds the node collection settings.
pub struct ContainerTelemetry {
    settings: Arc<dyn SettingsRepository>,
    clock: Arc<dyn Clock>,
    analytics_enabled: bool,
    /// Every node's settings, read once on first use (`None` until then).
    node_settings: tokio::sync::Mutex<Option<HashMap<ServerId, ContainerTelemetrySettings>>>,
}

impl std::fmt::Debug for ContainerTelemetry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ContainerTelemetry").finish_non_exhaustive()
    }
}

impl ContainerTelemetry {
    #[must_use]
    pub fn new(d: TelemetryDeps) -> Self {
        Self {
            settings: d.settings,
            clock: d.clock,
            analytics_enabled: d.analytics_enabled,
            node_settings: tokio::sync::Mutex::new(None),
        }
    }

    /// A node's settings.
    pub async fn settings(&self, server: ServerId) -> Result<ContainerTelemetrySettings, TelemetryError> {
        let mut g = self.node_settings.lock().await;
        if g.is_none() {
            *g = Some(parse_settings(self.settings.all_settings().await?));
        }
        Ok(g.as_ref().and_then(|m| m.get(&server).cloned()).unwrap_or_default())
    }

    /// Stores a node's settings; returns the previous and the new ones.
    pub async fn set_settings(
        &self,
        server: ServerId,
        new: ContainerTelemetrySettings,
    ) -> Result<(ContainerTelemetrySettings, ContainerTelemetrySettings), TelemetryError> {
        let new = new.normalized()?;
        let old = self.settings(server).await?;
        let json = serde_json::to_string(&new).map_err(|e| DomainError::Internal(e.to_string()))?;
        self.settings.put_setting(&settings_key(server), &json, self.clock.now()).await?;
        if let Some(m) = self.node_settings.lock().await.as_mut() {
            m.insert(server, new.clone());
        }
        Ok((old, new))
    }

    /// What the node should collect, for its heartbeat answer: nothing
    /// while analytics is off, since nothing could be stored.
    pub async fn config_for(&self, server: ServerId) -> ContainerTelemetryConfig {
        let s = match self.settings(server).await {
            Ok(s) => s,
            Err(e) => {
                // Keep the default rather than fail the heartbeat.
                tracing::warn!(server = %server, error = %e, "reading container telemetry settings failed");
                ContainerTelemetrySettings::default()
            }
        };
        ContainerTelemetryConfig { enabled: s.enabled && self.analytics_enabled, exclude: s.exclude }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // reason: tests
mod tests {
    use super::*;

    #[test]
    fn settings_are_checked_and_parsed() {
        let s = ContainerTelemetrySettings { enabled: true, exclude: vec![" db ".into(), "db".into(), "a=b".into()] };
        assert_eq!(s.normalized().unwrap().exclude, vec!["db".to_owned(), "a=b".to_owned()]);
        let bad = ContainerTelemetrySettings { enabled: true, exclude: vec![String::new()] };
        assert!(bad.normalized().is_err());
        let many = ContainerTelemetrySettings { enabled: true, exclude: (0..65).map(|i| format!("c{i}")).collect() };
        assert!(many.normalized().is_err(), "bounded");
        let id = ServerId::new();
        let rows = vec![
            (settings_key(id), r#"{"enabled":false,"exclude":["x"]}"#.to_owned()),
            ("node.bad.container_telemetry".to_owned(), "{}".to_owned()),
            ("other".to_owned(), "1".to_owned()),
        ];
        let m = parse_settings(rows);
        assert_eq!(m.len(), 1);
        assert!(!m[&id].enabled);
        let d: ContainerTelemetrySettings = serde_json::from_str("{}").unwrap();
        assert!(d.enabled, "on by default");
    }

    async fn telemetry(dir: &std::path::Path) -> ContainerTelemetry {
        let url = format!("sqlite://{}?mode=rwc", dir.join("t.db").display());
        let store = forge_store::Store::connect(&url).await.unwrap();
        ContainerTelemetry::new(TelemetryDeps {
            settings: Arc::new(store),
            clock: Arc::new(forge_core::SystemClock),
            analytics_enabled: false,
        })
    }

    #[tokio::test]
    async fn settings_persist_and_follow_analytics() {
        let dir = tempfile::tempdir().unwrap();
        let t = telemetry(dir.path()).await;
        let node = ServerId::new();
        assert_eq!(t.settings(node).await.unwrap(), ContainerTelemetrySettings::default());
        let (old, new) = t
            .set_settings(node, ContainerTelemetrySettings { enabled: false, exclude: vec!["db".into()] })
            .await
            .unwrap();
        assert!(old.enabled && !new.enabled);
        // A fresh instance reads it back from the store.
        let again = telemetry(dir.path()).await;
        assert_eq!(again.settings(node).await.unwrap().exclude, vec!["db".to_owned()]);
        let other = ServerId::new();
        assert!(!again.config_for(other).await.enabled, "nothing is collected while analytics is off");
        assert!(
            t.set_settings(node, ContainerTelemetrySettings { enabled: true, exclude: vec!["".into()] }).await.is_err()
        );
    }
}
