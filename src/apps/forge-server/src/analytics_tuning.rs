//! Analytics settings changed at runtime (D-136): retention, the managed
//! engine's CPU and memory, log search limits, the ingest queue and disk
//! protection.
//!
//! The settings themselves (names, bounds, validation) are the contract in
//! `forge_analytics_proto::tuning`, shared with the analytics worker that
//! applies them (D-141). This module reads the `forge.toml` defaults and
//! the administrators' overrides, stored in the operational database
//! (settings key [`OVERRIDES_KEY`]).

use forge_core::analytics::Retention;

pub use forge_analytics_proto::tuning::{
    Group, Invalid, Overrides, Patch, Setting, Tuning, shortened_retention, validate,
};

use crate::config::ServerConfig;

/// Settings key holding the overrides as JSON.
pub const OVERRIDES_KEY: &str = "analytics.overrides";

/// Retention from the configuration.
#[must_use]
pub fn retention(config: &ServerConfig) -> Retention {
    let r = &config.analytics.retention;
    Retention {
        node_metrics_days: r.node_metrics_days,
        top_processes_days: r.top_processes_days.unwrap_or(r.node_metrics_days),
        node_rollup_days: r.node_rollup_days,
        app_logs_days: r.app_logs_days,
    }
}

/// The `forge.toml` values.
#[must_use]
pub fn from_config(c: &ServerConfig) -> Tuning {
    let a = &c.analytics;
    let r = retention(c);
    Tuning {
        node_metrics_days: r.node_metrics_days,
        top_processes_days: r.top_processes_days,
        node_rollup_days: r.node_rollup_days,
        app_logs_days: r.app_logs_days,
        cpu_quota_vcpu: a.cpu_quota_vcpu,
        memory_budget_mb: match a.engine {
            crate::config::AnalyticsEngine::Duckdb => a.duckdb_memory_mb,
            _ => a.embedded_memory_mb,
        },
        search_max_concurrent: a.search.max_concurrent,
        search_timeout_secs: a.search.timeout_secs,
        search_max_lookback_days: a.search.max_lookback_days,
        search_threads: a.search.threads,
        search_memory_mb: a.search.memory_mb,
        ingest_max_queued_mb: a.ingest.max_queued_mb,
        ingest_max_batch_rows: a.ingest.max_batch_rows,
        ingest_flush_interval_ms: a.ingest.flush_interval_ms,
        min_free_disk_mb: a.min_free_disk_mb,
    }
}

/// Settings that no longer exist, dropped from stored overrides so the
/// others still load: `memory_limit_mb` (the cgroup memory limit, D-140).
const REMOVED: &[&str] = &["memory_limit_mb"];

/// Why stored overrides could not be read.
#[derive(Debug, thiserror::Error)]
enum StoredError {
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Invalid(#[from] Invalid),
}

fn parse_stored(raw: &str) -> Result<Overrides, StoredError> {
    let mut v: serde_json::Value = serde_json::from_str(raw)?;
    if let Some(m) = v.as_object_mut() {
        for k in REMOVED {
            if m.remove(*k).is_some() {
                tracing::info!(setting = k, "ignoring a stored analytics setting that no longer exists");
            }
        }
    }
    let o: Overrides = serde_json::from_value(v)?;
    validate(&o)?;
    Ok(o)
}

/// Reads the stored overrides. A value that no longer parses or validates
/// is ignored with a warning: the defaults from `forge.toml` are safe, and
/// a bad row must not stop the server from starting.
pub async fn load(settings: &dyn forge_core::ports::SettingsRepository) -> anyhow::Result<Overrides> {
    let stored = settings.all_settings().await?.into_iter().find(|(k, _)| k == OVERRIDES_KEY).map(|(_, v)| v);
    let Some(raw) = stored else { return Ok(Overrides::default()) };
    match parse_stored(&raw) {
        Ok(o) => Ok(o),
        Err(e) => {
            tracing::warn!(error = %e, "stored analytics settings are invalid; using forge.toml");
            Ok(Overrides::default())
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // reason: tests
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_documented_values() {
        let t = from_config(&ServerConfig::default());
        assert_eq!(
            t.retention(),
            Retention { node_metrics_days: 7, top_processes_days: 7, node_rollup_days: 365, app_logs_days: 30 }
        );
        assert_eq!(t.memory_budget_mb, 256, "duckdb_memory_mb (the default engine)");
        assert!(t.cpu_quota_vcpu.abs() < f64::EPSILON);
        let mut c = ServerConfig::default();
        c.analytics.engine = crate::config::AnalyticsEngine::Embedded;
        assert_eq!(from_config(&c).memory_budget_mb, 1024, "embedded_memory_mb");
    }

    #[test]
    fn top_processes_follow_minute_metrics_unless_set() {
        let mut c = ServerConfig::default();
        c.analytics.retention.node_metrics_days = 14;
        assert_eq!(from_config(&c).top_processes_days, 14);
        c.analytics.retention.top_processes_days = Some(3);
        assert_eq!(from_config(&c).top_processes_days, 3);
    }

    #[test]
    fn removed_settings_in_stored_overrides_are_dropped() {
        let o = parse_stored(r#"{"memory_limit_mb": 512, "app_logs_days": 9}"#).unwrap();
        assert_eq!(o, Overrides { app_logs_days: Some(9), ..Overrides::default() });
        assert!(parse_stored(r#"{"app_logs_days": 0}"#).is_err(), "out of bounds");
    }
}
