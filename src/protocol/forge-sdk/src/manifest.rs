//! `forge-plugin.toml` — the plugin manifest.
//!
//! ```toml
//! [plugin]
//! id = "orbyn.uptime-probe"
//! name = "Uptime probe"
//! version = "0.1.0"
//! publisher = "orbyn"
//! forge_api = ">=0.1.0, <0.2.0"
//!
//! [runtime]
//! export = "run"
//! location = "central"
//!
//! [permissions]
//! requested = ["host:server-online"]
//!
//! [budgets]
//! fuel = 50_000_000
//! memory_mb = 16
//! timeout_ms = 2000
//! ```

use std::collections::BTreeMap;

use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};

use crate::host_functions;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ManifestError {
    #[error("manifest is not valid TOML: {0}")]
    Toml(String),
    #[error("invalid manifest: {0}")]
    Invalid(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub plugin: PluginSection,
    #[serde(default)]
    pub runtime: RuntimeSection,
    #[serde(default)]
    pub permissions: PermissionsSection,
    #[serde(default)]
    pub budgets: Budgets,
    /// Other plugins this one needs: id → semver requirement.
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
    /// JSON-schema-like description of user configuration (rendered by the UI).
    #[serde(default)]
    pub config: BTreeMap<String, toml::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginSection {
    pub id: String,
    pub name: String,
    pub version: String,
    pub publisher: String,
    /// Supported Forge plugin API range, e.g. `>=0.1.0, <0.2.0`.
    pub forge_api: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Location {
    /// Runs next to the control plane.
    #[default]
    Central,
    /// Runs on a build/worker host (Phase 3+).
    Worker,
    /// Runs on managed nodes through the agent (Phase 9).
    Node,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeSection {
    #[serde(default = "default_export")]
    pub export: String,
    #[serde(default)]
    pub location: Location,
    /// Supported OS/arch for node plugins, e.g. `["linux-x86_64"]`. Empty = any.
    #[serde(default)]
    pub targets: Vec<String>,
}

impl Default for RuntimeSection {
    fn default() -> Self {
        Self { export: default_export(), location: Location::Central, targets: vec![] }
    }
}

fn default_export() -> String {
    "run".into()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct PermissionsSection {
    /// `host:<function>` permissions the plugin asks for.
    #[serde(default)]
    pub requested: Vec<String>,
}

/// Resource budgets enforced by the runner and host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Budgets {
    /// Wasmtime fuel (roughly instructions).
    pub fuel: u64,
    pub memory_mb: u32,
    pub timeout_ms: u64,
    #[serde(default = "default_output")]
    pub max_output_bytes: u32,
    #[serde(default = "default_concurrency")]
    pub concurrency: u32,
}

fn default_output() -> u32 {
    64 * 1024
}

fn default_concurrency() -> u32 {
    2
}

impl Default for Budgets {
    fn default() -> Self {
        Self { fuel: 50_000_000, memory_mb: 16, timeout_ms: 2_000, max_output_bytes: default_output(), concurrency: 2 }
    }
}

/// Hard ceilings an admin cannot exceed through a manifest.
pub const MAX_FUEL: u64 = 10_000_000_000;
pub const MAX_MEMORY_MB: u32 = 512;
pub const MAX_TIMEOUT_MS: u64 = 60_000;

impl Manifest {
    pub fn parse(src: &str) -> Result<Self, ManifestError> {
        let m: Manifest = toml::from_str(src).map_err(|e| ManifestError::Toml(e.to_string()))?;
        m.validate()?;
        Ok(m)
    }

    pub fn version(&self) -> Result<Version, ManifestError> {
        Version::parse(&self.plugin.version).map_err(|e| ManifestError::Invalid(format!("version: {e}")))
    }

    /// Whether this plugin supports host API `api_version`.
    pub fn supports_api(&self, api_version: &str) -> Result<bool, ManifestError> {
        let req =
            VersionReq::parse(&self.plugin.forge_api).map_err(|e| ManifestError::Invalid(format!("forge_api: {e}")))?;
        let v = Version::parse(api_version).map_err(|e| ManifestError::Invalid(e.to_string()))?;
        Ok(req.matches(&v))
    }

    fn validate(&self) -> Result<(), ManifestError> {
        let invalid = |m: String| Err(ManifestError::Invalid(m));
        let id = &self.plugin.id;
        // `publisher.name`: no empty segment (so no `..`, which would reach
        // outside the artifacts folder), and the plugin's own publisher, so
        // one trusted publisher cannot install or upgrade another's plugin.
        let id_ok = id.len() <= 64
            && id.contains('.')
            && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '-'))
            && id.split('.').all(|seg| !seg.is_empty());
        if !id_ok {
            return invalid(format!("id `{id}` must be lowercase `publisher.name`"));
        }
        if !id.starts_with(&format!("{}.", self.plugin.publisher)) {
            return invalid(format!("id `{id}` must start with its publisher `{}.`", self.plugin.publisher));
        }
        self.version()?;
        VersionReq::parse(&self.plugin.forge_api).map_err(|e| ManifestError::Invalid(format!("forge_api: {e}")))?;
        for p in &self.permissions.requested {
            let known = host_functions::ALL.iter().any(|f| host_functions::permission(f) == *p);
            if !known {
                return invalid(format!("unknown permission `{p}`"));
            }
        }
        let b = &self.budgets;
        if b.fuel == 0 || b.fuel > MAX_FUEL {
            return invalid(format!("budgets.fuel must be 1..={MAX_FUEL}"));
        }
        if b.memory_mb == 0 || b.memory_mb > MAX_MEMORY_MB {
            return invalid(format!("budgets.memory_mb must be 1..={MAX_MEMORY_MB}"));
        }
        if b.timeout_ms == 0 || b.timeout_ms > MAX_TIMEOUT_MS {
            return invalid(format!("budgets.timeout_ms must be 1..={MAX_TIMEOUT_MS}"));
        }
        for (dep, req) in &self.dependencies {
            VersionReq::parse(req).map_err(|e| ManifestError::Invalid(format!("dependency {dep}: {e}")))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OK: &str = r#"
[plugin]
id = "orbyn.uptime-probe"
name = "Uptime probe"
version = "0.1.0"
publisher = "orbyn"
forge_api = ">=0.1.0, <0.2.0"

[permissions]
requested = ["host:server-online"]

[budgets]
fuel = 1000000
memory_mb = 8
timeout_ms = 500
"#;

    #[test]
    fn parses_valid_manifest() {
        let m = Manifest::parse(OK).expect("valid");
        assert_eq!(m.plugin.id, "orbyn.uptime-probe");
        assert_eq!(m.runtime.export, "run");
        assert!(m.supports_api("0.1.0").expect("req"));
        assert!(!m.supports_api("0.2.0").expect("req"));
    }

    #[test]
    fn rejects_unknown_permission_and_fields() {
        let bad = OK.replace("host:server-online", "host:exec-shell");
        assert!(matches!(Manifest::parse(&bad), Err(ManifestError::Invalid(_))));
        let extra = format!("{OK}\n[surprise]\nx = 1\n");
        assert!(matches!(Manifest::parse(&extra), Err(ManifestError::Toml(_))));
    }

    #[test]
    fn rejects_excessive_budgets_and_bad_ids() {
        assert!(Manifest::parse(&OK.replace("memory_mb = 8", "memory_mb = 4096")).is_err());
        assert!(Manifest::parse(&OK.replace("orbyn.uptime-probe", "Bad ID")).is_err());
        assert!(Manifest::parse(&OK.replace("version = \"0.1.0\"", "version = \"one\"")).is_err());
    }

    /// Security review: `..` in an id reached outside the artifacts folder,
    /// and any trusted publisher could claim another publisher's id.
    #[test]
    fn ids_belong_to_their_publisher_and_have_no_empty_segments() {
        for id in ["orbyn..x", "orbyn.", ".orbyn", "orbyn.a..", "other.uptime-probe"] {
            assert!(Manifest::parse(&OK.replace("orbyn.uptime-probe", id)).is_err(), "{id}");
        }
        assert!(Manifest::parse(&OK.replace("orbyn.uptime-probe", "orbyn.probe.v2")).is_ok());
    }
}
