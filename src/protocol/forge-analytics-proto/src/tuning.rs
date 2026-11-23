//! The analytics settings contract (D-136): every runtime setting, its
//! bounds and its group. forge-server stores administrators' overrides and
//! validates changes; the analytics worker applies the effective values.
//!
//! Every setting is a number. Each has one name, used in the API, the
//! audit log and the console. `0` turns the CPU cap off, and leaves the
//! engine's default for search threads and memory.

use forge_core::analytics::Retention;
use serde::{Deserialize, Deserializer, Serialize};

/// `Some(None)` for an explicit `null`, so a patch can tell "reset" from
/// "leave alone" (an absent field stays `None`).
fn present<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

/// Where a setting takes effect, which decides how it is applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Group {
    /// TTLs of the analytics tables.
    Retention,
    /// The managed engine's process: its CPU cap (CPU affinity and
    /// thread pools).
    Process,
    /// The managed engine's own memory budget (its configuration file).
    EngineMemory,
    /// The analytics worker's log search, ingest queue and disk check.
    Forge,
}

macro_rules! tuning {
    ($( $(#[doc = $doc:literal])* $name:ident : $ty:ty, $group:ident, $min:expr, $max:expr; )*) => {
        /// Effective analytics settings.
        #[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
        pub struct Tuning {
            $( $(#[doc = $doc])* pub $name: $ty, )*
        }

        /// Administrators' overrides; `None` uses the `forge.toml` value.
        #[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
        #[serde(deny_unknown_fields)]
        pub struct Overrides {
            $( #[serde(default, skip_serializing_if = "Option::is_none")] pub $name: Option<$ty>, )*
        }

        /// A change request: an absent field is left alone, `null` removes
        /// the override (back to `forge.toml`), a number sets it.
        #[derive(Debug, Clone, Copy, Default, Deserialize, schemars::JsonSchema)]
        #[serde(deny_unknown_fields)]
        pub struct Patch {
            $( #[serde(default, deserialize_with = "present")] pub $name: Option<Option<$ty>>, )*
        }

        /// The name of one setting.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
        #[serde(rename_all = "snake_case")]
        #[allow(non_camel_case_types)] // reason: variants are the settings' wire names
        pub enum Setting {
            $( $name, )*
        }

        impl Setting {
            /// Every setting, in display order.
            pub const ALL: &'static [Setting] = &[ $( Setting::$name, )* ];

            #[must_use]
            pub fn group(self) -> Group {
                match self { $( Setting::$name => Group::$group, )* }
            }

            /// Inclusive bounds of a valid value.
            #[must_use]
            pub fn bounds(self) -> (f64, f64) {
                match self { $( Setting::$name => (f64::from($min), f64::from($max)), )* }
            }
        }

        impl Tuning {
            /// `self` with `o` applied on top.
            #[must_use]
            pub fn with(self, o: &Overrides) -> Self {
                Self { $( $name: o.$name.unwrap_or(self.$name), )* }
            }

            /// The value of `s`.
            #[must_use]
            pub fn get(&self, s: Setting) -> f64 {
                match s { $( Setting::$name => f64::from(self.$name), )* }
            }
        }

        impl Overrides {
            /// Applies `p` to the overrides.
            pub fn apply(&mut self, p: &Patch) {
                $( if let Some(v) = p.$name { self.$name = v; } )*
            }

            /// The settings `p` touches, with the value it sets (`None`: reset).
            #[must_use]
            pub fn touched(p: &Patch) -> Vec<(Setting, Option<f64>)> {
                let mut out = Vec::new();
                $( if let Some(v) = p.$name { out.push((Setting::$name, v.map(f64::from))); } )*
                out
            }

            /// The value of `s`, when overridden.
            #[must_use]
            pub fn get(&self, s: Setting) -> Option<f64> {
                match s { $( Setting::$name => self.$name.map(f64::from), )* }
            }
        }
    };
}

tuning! {
    /// Per-minute node metrics, days.
    node_metrics_days: u32, Retention, 1, 3650;
    /// Per-minute top processes, days.
    top_processes_days: u32, Retention, 1, 3650;
    /// Hourly node rollups, days.
    node_rollup_days: u32, Retention, 1, 3650;
    /// Service logs from ingestion time, days.
    app_logs_days: u32, Retention, 1, 3650;
    /// CPU cap of the managed engine, vCPUs, rounded up to whole CPUs (`0`: none).
    cpu_quota_vcpu: f64, Process, 0.0, 256.0;
    /// The managed engine's memory cap (`max_server_memory_usage`), MiB.
    memory_budget_mb: u32, EngineMemory, 256, 1_048_576;
    /// Log searches running at once.
    search_max_concurrent: u32, Forge, 1, 256;
    /// Seconds one search may run.
    search_timeout_secs: u32, Forge, 1, 3600;
    /// How far back a search may reach, days (`0`: no limit).
    search_max_lookback_days: u32, Forge, 0, 3650;
    /// Threads per search (`0`: engine default).
    search_threads: u32, Forge, 0, 256;
    /// Memory per search, MiB (`0`: engine default).
    search_memory_mb: u32, Forge, 0, 1_048_576;
    /// Log batches waiting to be stored before ingest answers 503, MiB.
    ingest_max_queued_mb: u32, Forge, 1, 65_536;
    /// Rows in one insert.
    ingest_max_batch_rows: u32, Forge, 100, 1_000_000;
    /// How long the writer waits for more batches before an insert, ms.
    ingest_flush_interval_ms: u32, Forge, 10, 10_000;
    /// Free disk the engine must have for ingest to accept batches, MiB.
    min_free_disk_mb: u32, Forge, 0, 10_485_760;
}

const MIB: u64 = 1024 * 1024;

impl Tuning {
    #[must_use]
    pub fn retention(&self) -> Retention {
        Retention {
            node_metrics_days: self.node_metrics_days,
            top_processes_days: self.top_processes_days,
            node_rollup_days: self.node_rollup_days,
            app_logs_days: self.app_logs_days,
        }
    }

    #[must_use]
    pub fn search_limits(&self) -> forge_core::logs::SearchLimits {
        forge_core::logs::SearchLimits {
            timeout_secs: self.search_timeout_secs,
            threads: self.search_threads,
            memory_bytes: u64::from(self.search_memory_mb) * MIB,
        }
    }

    #[must_use]
    pub fn max_queued_bytes(&self) -> u64 {
        u64::from(self.ingest_max_queued_mb) * MIB
    }

    #[must_use]
    pub fn min_free_disk_bytes(&self) -> u64 {
        u64::from(self.min_free_disk_mb) * MIB
    }
}

/// Why a value was refused.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("{setting:?}: {reason}")]
pub struct Invalid {
    pub setting: Setting,
    pub reason: String,
}

/// Checks every overridden value against its bounds.
pub fn validate(o: &Overrides) -> Result<(), Invalid> {
    for s in Setting::ALL {
        let Some(v) = o.get(*s) else { continue };
        let (lo, hi) = s.bounds();
        if !v.is_finite() || v < lo || v > hi {
            return Err(Invalid { setting: *s, reason: format!("must be between {lo} and {hi}") });
        }
        if *s == Setting::cpu_quota_vcpu && v > 0.0 && v < 0.1 {
            return Err(Invalid { setting: *s, reason: "use 0 (no cap) or at least 0.1 vCPU".into() });
        }
    }
    Ok(())
}

/// The settings a change shortens retention for: `(setting, old, new)`.
#[must_use]
pub fn shortened_retention(old: &Tuning, new: &Tuning) -> Vec<(Setting, f64, f64)> {
    Setting::ALL
        .iter()
        .filter(|s| s.group() == Group::Retention)
        .filter(|s| new.get(**s) < old.get(**s))
        .map(|s| (*s, old.get(*s), new.get(*s)))
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // reason: tests
mod tests {
    use super::*;

    fn base() -> Tuning {
        Tuning {
            node_metrics_days: 7,
            top_processes_days: 7,
            node_rollup_days: 365,
            app_logs_days: 30,
            cpu_quota_vcpu: 0.0,
            memory_budget_mb: 1024,
            search_max_concurrent: 8,
            search_timeout_secs: 20,
            search_max_lookback_days: 0,
            search_threads: 0,
            search_memory_mb: 0,
            ingest_max_queued_mb: 256,
            ingest_max_batch_rows: 20_000,
            ingest_flush_interval_ms: 100,
            min_free_disk_mb: 1024,
        }
    }

    #[test]
    fn patches_set_reset_and_leave_alone() {
        let mut o = Overrides { app_logs_days: Some(10), search_threads: Some(4), ..Overrides::default() };
        let p: Patch = serde_json::from_str(r#"{"app_logs_days": null, "cpu_quota_vcpu": 1.5}"#).unwrap();
        assert_eq!(Overrides::touched(&p), vec![(Setting::app_logs_days, None), (Setting::cpu_quota_vcpu, Some(1.5))]);
        o.apply(&p);
        assert_eq!(o.app_logs_days, None, "null resets");
        assert_eq!(o.search_threads, Some(4), "absent is left alone");
        assert_eq!(o.cpu_quota_vcpu, Some(1.5));
        assert!((base().with(&o).cpu_quota_vcpu - 1.5).abs() < f64::EPSILON);
        assert!(serde_json::from_str::<Patch>(r#"{"nope": 1}"#).is_err(), "unknown names are refused");
    }

    #[test]
    fn validation_and_shortening() {
        assert!(validate(&Overrides { app_logs_days: Some(0), ..Overrides::default() }).is_err());
        assert!(validate(&Overrides { cpu_quota_vcpu: Some(0.05), ..Overrides::default() }).is_err());
        assert!(validate(&Overrides { cpu_quota_vcpu: Some(0.0), ..Overrides::default() }).is_ok());
        assert!(validate(&Overrides { memory_budget_mb: Some(100), ..Overrides::default() }).is_err());
        let old = base();
        let new = old.with(&Overrides { app_logs_days: Some(7), node_rollup_days: Some(400), ..Overrides::default() });
        assert_eq!(shortened_retention(&old, &new), vec![(Setting::app_logs_days, 30.0, 7.0)]);
    }
}
