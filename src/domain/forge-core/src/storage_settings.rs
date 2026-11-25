//! Container settings of a RustFS store on Docker (D-170): image, name,
//! restart policy, network, limits, extra engine variables and, per drive, a
//! host path or a Docker named volume. Stored with the store and sent to the
//! node inside the store spec; validated on both sides.

use std::collections::BTreeMap;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{DomainResult, ObjectStoreId, Timestamp};

/// The image repository Forge installs by default.
pub const DEFAULT_IMAGE: &str = "rustfs/rustfs";

/// Engine variables an operator may set. Credentials, listen addresses,
/// volumes and the console switch are Forge's and can never be overridden.
pub const ENV_ALLOWLIST: &[&str] = &[
    "RUSTFS_SERVER_DOMAINS",
    "RUSTFS_CORS_ALLOWED_ORIGINS",
    "RUSTFS_CONSOLE_CORS_ALLOWED_ORIGINS",
    "RUSTFS_OBS_LOGGER_LEVEL",
    "RUSTFS_OBS_ENVIRONMENT",
    "RUSTFS_COMPRESSION_ENABLED",
    "RUSTFS_STORAGE_CLASS_STANDARD",
    "RUSTFS_STORAGE_CLASS_RRS",
    "RUSTFS_SCANNER_SPEED",
];

/// Most extra variables one store may carry.
pub const MAX_ENV: usize = 16;

/// Docker restart policy of the container.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum RestartPolicy {
    #[default]
    UnlessStopped,
    Always,
    OnFailure,
    No,
}

impl RestartPolicy {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UnlessStopped => "unless-stopped",
            Self::Always => "always",
            Self::OnFailure => "on-failure",
            Self::No => "no",
        }
    }
}

/// Container network mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum NetworkMode {
    /// Published ports on the default bridge.
    #[default]
    Bridge,
    /// The host's network: the engine listens on the chosen ports directly.
    Host,
}

/// Where one drive's data lives on the node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DriveVolume {
    /// A host directory. Empty `host_path`: the drive's data path itself
    /// (mounted at the same path in the container).
    Bind {
        #[serde(default, skip_serializing_if = "String::is_empty")]
        host_path: String,
    },
    /// A Docker named volume, created when missing.
    Named {
        name: String,
        /// Volume driver; empty for Docker's default (`local`).
        #[serde(default, skip_serializing_if = "String::is_empty")]
        driver: String,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        options: BTreeMap<String, String>,
    },
}

/// Container settings of a Docker store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RustfsSettings {
    /// Image repository; the tag is the store's release.
    #[serde(default = "default_image")]
    pub image: String,
    /// Container name; empty for `forge-objstore-<slug>`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub container_name: String,
    #[serde(default)]
    pub restart: RestartPolicy,
    #[serde(default)]
    pub network: NetworkMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_mb: Option<u32>,
    /// CPU limit in thousandths of a CPU (1000 = one CPU).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_millis: Option<u32>,
    /// Extra engine variables, keys from [`ENV_ALLOWLIST`].
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// One entry per data path, in order. Empty: every path is a bind mount.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub volumes: Vec<DriveVolume>,
    /// A container Forge adopted (it found it running, it did not create
    /// it): never reinstalled, and its data is never deleted by Forge.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub adopted: bool,
}

fn default_image() -> String {
    DEFAULT_IMAGE.into()
}

impl Default for RustfsSettings {
    fn default() -> Self {
        Self {
            image: default_image(),
            container_name: String::new(),
            restart: RestartPolicy::default(),
            network: NetworkMode::default(),
            memory_mb: None,
            cpu_millis: None,
            env: BTreeMap::new(),
            volumes: Vec::new(),
            adopted: false,
        }
    }
}

/// Image repositories: `[registry[:port]/]name[/name...]`, lowercase, no tag.
#[must_use]
pub fn valid_image_repo(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 200
        && s.as_bytes()[0].is_ascii_alphanumeric()
        && !s.contains("//")
        && !s.ends_with('/')
        && !s.contains('@')
        && s.rsplit('/').next().is_some_and(|last| !last.contains(':'))
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'/' | b'.' | b'_' | b'-' | b':'))
}

/// Container and volume names: `[A-Za-z0-9][A-Za-z0-9_.-]{0,63}`.
#[must_use]
pub fn valid_docker_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.as_bytes()[0].is_ascii_alphanumeric()
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}

/// Characters a value that reaches a node command line may hold: no
/// whitespace, quotes, `$`, backslash or backtick.
#[must_use]
pub fn safe_token(s: &str, max: usize) -> bool {
    !s.is_empty()
        && s.len() <= max
        && s.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'/' | b':' | b'=' | b',' | b'@' | b'+')
        })
}

/// Why settings were refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct SettingsError(pub String);

fn bad(m: impl Into<String>) -> SettingsError {
    SettingsError(m.into())
}

impl RustfsSettings {
    /// Checks every field; `drives` is the number of data paths of a member.
    pub fn validate(&self, drives: usize) -> Result<(), SettingsError> {
        if !valid_image_repo(&self.image) {
            return Err(bad("image: a repository such as rustfs/rustfs, without a tag"));
        }

        if !self.container_name.is_empty() && !valid_docker_name(&self.container_name) {
            return Err(bad("container name: letters, digits, _ . -, up to 64"));
        }

        if self.memory_mb.is_some_and(|m| !(256..=1_048_576).contains(&m)) {
            return Err(bad("memory limit: 256 MB to 1 TB"));
        }

        if self.cpu_millis.is_some_and(|c| !(100..=256_000).contains(&c)) {
            return Err(bad("CPU limit: 0.1 to 256 CPUs"));
        }

        if self.env.len() > MAX_ENV {
            return Err(bad(format!("at most {MAX_ENV} extra variables")));
        }

        for (k, v) in &self.env {
            if !ENV_ALLOWLIST.contains(&k.as_str()) {
                return Err(bad(format!("variable {k} cannot be set; allowed: {}", ENV_ALLOWLIST.join(", "))));
            }

            if v.is_empty()
                || v.len() > 512
                || !v.bytes().all(|b| b.is_ascii_graphic() && !matches!(b, b'"' | b'\'' | b'`' | b'$' | b'\\'))
            {
                return Err(bad(format!("variable {k}: up to 512 visible characters without quotes, $, ` or \\")));
            }
        }

        if !self.volumes.is_empty() && self.volumes.len() != drives {
            return Err(bad("one volume entry per data path"));
        }

        let mut names = std::collections::BTreeSet::new();

        for v in &self.volumes {
            match v {
                DriveVolume::Bind { host_path } => {
                    if !host_path.is_empty() && !safe_host_path(host_path) {
                        return Err(bad(format!("host path `{host_path}`: an absolute path")));
                    }
                }
                DriveVolume::Named { name, driver, options } => {
                    if !valid_docker_name(name) {
                        return Err(bad(format!("volume name `{name}`: letters, digits, _ . -, up to 64")));
                    }

                    if !names.insert(name.as_str()) {
                        return Err(bad(format!("volume `{name}` is used twice")));
                    }

                    if !driver.is_empty() && !valid_docker_name(driver) {
                        return Err(bad("volume driver: letters, digits, _ . -"));
                    }

                    if options.len() > 16 {
                        return Err(bad("at most 16 driver options"));
                    }

                    for (k, val) in options {
                        if !valid_docker_name(k) || !safe_token(val, 256) {
                            return Err(bad(format!("driver option `{k}`: name and value without spaces or quotes")));
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Settings that reach beyond the store's own container and data: an
    /// image other than [`DEFAULT_IMAGE`], or a named volume with a driver
    /// or driver options (network and bind mounts). They need administrator
    /// rights on the member nodes as well as `storage.manage`.
    #[must_use]
    pub fn reaches_host(&self) -> bool {
        self.image != DEFAULT_IMAGE
            || self.volumes.iter().any(|v| {
                matches!(v, DriveVolume::Named { driver, options, .. } if !driver.is_empty() || !options.is_empty())
            })
    }

    /// Whether drive `i` is a named volume.
    #[must_use]
    pub fn is_named(&self, i: usize) -> bool {
        matches!(self.volumes.get(i), Some(DriveVolume::Named { .. }))
    }
}

/// An absolute Linux path with plain characters and no `..` (bind sources
/// of adopted containers; Forge never creates or deletes them).
#[must_use]
pub fn safe_host_path(p: &str) -> bool {
    p.starts_with('/')
        && p.len() <= 200
        && !p.split('/').any(|c| c == "..")
        && !p.contains("//")
        && p.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'_' | b'.' | b'-'))
}

/// Container settings of stores, one row per store.
#[async_trait]
pub trait StoreSettingsRepository: Send + Sync + std::fmt::Debug {
    async fn get_store_settings(&self, store: ObjectStoreId) -> DomainResult<Option<RustfsSettings>>;
    async fn put_store_settings(
        &self,
        store: ObjectStoreId,
        settings: &RustfsSettings,
        at: Timestamp,
    ) -> DomainResult<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_settings() {
        let ok = RustfsSettings::default();
        assert!(ok.validate(1).is_ok());

        for img in ["rustfs/rustfs:1.0", "", "Rustfs/x", "a b", "reg.example.test:5000/rustfs/rustfs@sha256:x"] {
            assert!(RustfsSettings { image: img.into(), ..ok.clone() }.validate(1).is_err(), "{img}");
        }

        assert!(
            RustfsSettings { image: "reg.example.test:5000/rustfs/rustfs".into(), ..ok.clone() }.validate(1).is_ok()
        );
        let env = |k: &str, v: &str| RustfsSettings { env: BTreeMap::from([(k.into(), v.into())]), ..ok.clone() };
        assert!(env("RUSTFS_SERVER_DOMAINS", "s3.example.test").validate(1).is_ok());

        for k in ["RUSTFS_ACCESS_KEY", "RUSTFS_SECRET_KEY", "RUSTFS_VOLUMES", "RUSTFS_ADDRESS", "PATH"] {
            assert!(env(k, "x").validate(1).is_err(), "{k}");
        }

        for v in ["a b", "$(id)", "a'b", "", "x\ny"] {
            assert!(env("RUSTFS_SERVER_DOMAINS", v).validate(1).is_err(), "{v:?}");
        }

        assert!(RustfsSettings { memory_mb: Some(64), ..ok.clone() }.validate(1).is_err());
        assert!(RustfsSettings { cpu_millis: Some(1500), ..ok.clone() }.validate(1).is_ok());
        let named = |n: &str| DriveVolume::Named { name: n.into(), driver: String::new(), options: BTreeMap::new() };
        assert!(RustfsSettings { volumes: vec![named("d0")], ..ok.clone() }.validate(2).is_err(), "count");
        assert!(RustfsSettings { volumes: vec![named("d0"), named("d0")], ..ok.clone() }.validate(2).is_err());
        assert!(RustfsSettings { volumes: vec![named("-x")], ..ok.clone() }.validate(1).is_err());
        let opts = DriveVolume::Named {
            name: "d0".into(),
            driver: "local".into(),
            options: BTreeMap::from([("o".into(), "addr=192.0.2.1,rw".into())]),
        };
        assert!(RustfsSettings { volumes: vec![opts], ..ok.clone() }.validate(1).is_ok());
        let inj = DriveVolume::Named {
            name: "d0".into(),
            driver: String::new(),
            options: BTreeMap::from([("o".into(), "a b".into())]),
        };
        assert!(RustfsSettings { volumes: vec![inj], ..ok.clone() }.validate(1).is_err());
        assert!(
            RustfsSettings { volumes: vec![DriveVolume::Bind { host_path: "/srv/../etc".into() }], ..ok }
                .validate(1)
                .is_err()
        );
    }

    #[test]
    fn settings_round_trip_and_reject_unknown_fields() {
        let s: RustfsSettings =
            serde_json::from_str(r#"{"restart":"on-failure","network":"host"}"#).unwrap_or_default();
        assert_eq!(s.restart, RestartPolicy::OnFailure);
        assert_eq!(s.image, DEFAULT_IMAGE);
        assert!(serde_json::from_str::<RustfsSettings>(r#"{"privileged":true}"#).is_err());
    }
}
