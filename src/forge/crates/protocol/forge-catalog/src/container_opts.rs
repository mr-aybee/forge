//! Container runtime options beyond the basic service spec
//! (`plans/service-orchestration.md` step 1): restart policy with attempts,
//! capabilities, ulimits, tmpfs mounts, DNS servers, sysctls, devices,
//! `init` and extra networks.
//!
//! The values sit flat on [`crate::AppSpec`] (so a Compose key maps onto one
//! field); this module holds their types, their validation and the list of
//! settings that need `stack.privileged`, the same rule the stack path
//! applies ([`crate::stacks::check`]).

use std::net::IpAddr;

use serde::{Deserialize, Serialize};

/// When Docker (or systemd) restarts a stopped service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum RestartPolicy {
    /// Never restart.
    No,
    /// Always restart, also after the node reboots.
    Always,
    /// Restart unless an operator stopped it (Forge's default).
    UnlessStopped,
    /// Restart after a non-zero exit, up to `restart_max_attempts` times
    /// (`0` = no limit).
    OnFailure,
}

impl RestartPolicy {
    /// Docker's name for the policy.
    #[must_use]
    pub fn docker_name(self) -> &'static str {
        match self {
            Self::No => "no",
            Self::Always => "always",
            Self::UnlessStopped => "unless-stopped",
            Self::OnFailure => "on-failure",
        }
    }
}

/// A resource limit (`ulimit`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Ulimit {
    /// `nofile`, `nproc`, `memlock`, …
    pub name: String,
    pub soft: i64,
    pub hard: i64,
}

/// A tmpfs mount inside the container.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Tmpfs {
    /// Absolute path inside the container.
    pub target: String,
    /// Size limit in MiB (`0` = Docker's default, half the node's memory).
    #[serde(default)]
    pub size_mb: u32,
}

/// A host device passed into the container.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Device {
    /// Host path, under `/dev/`.
    pub host: String,
    /// Path inside the container (empty = the host path).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub container: String,
    /// Cgroup permissions, some of `r`, `w`, `m` (empty = `rwm`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub permissions: String,
}

/// The container options as owned values: what an API input or a stored
/// service spec carries (flattened next to the other spec fields).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ContainerOptions {
    /// `no`, `always`, `unless-stopped` or `on-failure`; absent keeps the
    /// older `restart` flag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restart_policy: Option<RestartPolicy>,
    /// `on-failure` only: give up after this many restarts (`0` = no limit).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub restart_max_attempts: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cap_add: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cap_drop: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ulimits: Vec<Ulimit>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tmpfs: Vec<Tmpfs>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dns: Vec<IpAddr>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sysctls: Vec<(String, String)>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub devices: Vec<Device>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub init: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub networks: Vec<String>,
}

impl ContainerOptions {
    /// Borrowed, for validation.
    #[must_use]
    pub fn view(&self) -> Options<'_> {
        Options {
            restart_policy: self.restart_policy,
            restart_max_attempts: self.restart_max_attempts,
            cap_add: &self.cap_add,
            cap_drop: &self.cap_drop,
            ulimits: &self.ulimits,
            tmpfs: &self.tmpfs,
            dns: &self.dns,
            sysctls: &self.sysctls,
            devices: &self.devices,
            init: self.init,
            networks: &self.networks,
        }
    }
}

#[allow(clippy::trivially_copy_pass_by_ref)] // serde's skip_serializing_if passes a reference
fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// Known ulimit names (Docker refuses others).
const ULIMITS: &[&str] = &[
    "core",
    "cpu",
    "data",
    "fsize",
    "locks",
    "memlock",
    "msgqueue",
    "nice",
    "nofile",
    "nproc",
    "rss",
    "rtprio",
    "rttime",
    "sigpending",
    "stack",
];

/// A capability name: `NET_ADMIN` (a `CAP_` prefix is accepted), or `ALL`.
#[must_use]
pub fn valid_capability(c: &str) -> bool {
    let c = c.strip_prefix("CAP_").unwrap_or(c);

    (1..=32).contains(&c.len()) && c.bytes().all(|b| b.is_ascii_uppercase() || b == b'_')
}

/// A sysctl key: dotted lowercase words, for example `net.core.somaxconn`.
#[must_use]
pub fn valid_sysctl_key(k: &str) -> bool {
    k.len() <= 128
        && k.contains('.')
        && k.split('.').all(|w| !w.is_empty() && w.bytes().all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b)))
}

/// A Docker network name.
#[must_use]
pub fn valid_network(n: &str) -> bool {
    (1..=64).contains(&n.len())
        && n.bytes().next().is_some_and(|b| b.is_ascii_alphanumeric())
        && n.bytes().all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
}

fn valid_container_path(p: &str) -> bool {
    p.starts_with('/')
        && p.len() <= 512
        && !p.split('/').any(|c| c == "..")
        && !p.contains([':', ' ', '\n', '\r', '\0', ','])
}

/// The container options of a spec, borrowed for validation.
#[derive(Debug, Clone, Copy)]
pub struct Options<'a> {
    pub restart_policy: Option<RestartPolicy>,
    pub restart_max_attempts: u32,
    pub cap_add: &'a [String],
    pub cap_drop: &'a [String],
    pub ulimits: &'a [Ulimit],
    pub tmpfs: &'a [Tmpfs],
    pub dns: &'a [IpAddr],
    pub sysctls: &'a [(String, String)],
    pub devices: &'a [Device],
    pub init: bool,
    pub networks: &'a [String],
}

impl Options<'_> {
    /// Whether any container-only option is set (`restart_policy` applies to
    /// native services too).
    #[must_use]
    pub fn container_only(&self) -> bool {
        !self.cap_add.is_empty()
            || !self.cap_drop.is_empty()
            || !self.ulimits.is_empty()
            || !self.tmpfs.is_empty()
            || !self.dns.is_empty()
            || !self.sysctls.is_empty()
            || !self.devices.is_empty()
            || self.init
            || !self.networks.is_empty()
    }

    /// Whether any option a pre-step-1 agent does not know is set. Such
    /// agents refuse unknown fields, so the server sends these only to an
    /// agent that reports the `container_options` fact.
    #[must_use]
    pub fn needs_new_agent(&self) -> bool {
        self.restart_policy.is_some() || self.container_only()
    }

    /// Checks shape and bounds.
    ///
    /// # Errors
    /// The first invalid option, in words.
    pub fn validate(&self) -> Result<(), String> {
        if self.restart_max_attempts > 0 && self.restart_policy != Some(RestartPolicy::OnFailure) {
            return Err("restart attempts apply to the on-failure policy only".into());
        }

        if self.restart_max_attempts > 1000 {
            return Err("at most 1000 restart attempts".into());
        }

        if self.cap_add.len() + self.cap_drop.len() > 64 {
            return Err("at most 64 capabilities".into());
        }

        if let Some(c) = self.cap_add.iter().chain(self.cap_drop).find(|c| !valid_capability(c)) {
            return Err(format!("invalid capability `{c}` (for example NET_ADMIN)"));
        }

        self.validate_ulimits()?;
        self.validate_mounts()?;

        if self.dns.len() > 8 {
            return Err("at most 8 DNS servers".into());
        }

        if self.sysctls.len() > 50 {
            return Err("at most 50 sysctls".into());
        }

        for (k, v) in self.sysctls {
            if !valid_sysctl_key(k) || v.is_empty() || v.len() > 256 || v.contains(['\n', '\r', '\0']) {
                return Err(format!("invalid sysctl `{k}`"));
            }
        }

        if self.networks.len() > 10 {
            return Err("at most 10 extra networks".into());
        }

        if let Some(n) = self.networks.iter().find(|n| !valid_network(n) || n.as_str() == "host") {
            return Err(format!("invalid network `{n}`"));
        }

        Ok(())
    }

    fn validate_ulimits(&self) -> Result<(), String> {
        if self.ulimits.len() > ULIMITS.len() {
            return Err("too many ulimits".into());
        }

        let mut seen = std::collections::HashSet::new();

        for u in self.ulimits {
            if !ULIMITS.contains(&u.name.as_str()) {
                return Err(format!("unknown ulimit `{}`", u.name));
            }

            if !seen.insert(u.name.as_str()) {
                return Err(format!("ulimit `{}` is listed twice", u.name));
            }

            if u.soft < -1 || u.hard < -1 || (u.hard != -1 && (u.soft == -1 || u.soft > u.hard)) {
                return Err(format!("ulimit `{}`: soft must not exceed hard (-1 = unlimited)", u.name));
            }
        }

        Ok(())
    }

    fn validate_mounts(&self) -> Result<(), String> {
        if self.tmpfs.len() > 20 || self.devices.len() > 20 {
            return Err("at most 20 tmpfs mounts and 20 devices".into());
        }

        if let Some(t) = self.tmpfs.iter().find(|t| !valid_container_path(&t.target) || t.size_mb > 1_048_576) {
            return Err(format!("invalid tmpfs mount `{}`", t.target));
        }

        for d in self.devices {
            let host_ok = d.host.starts_with("/dev/") && valid_container_path(&d.host);
            let target_ok = d.container.is_empty() || valid_container_path(&d.container);
            let perms_ok = d.permissions.len() <= 3 && d.permissions.bytes().all(|b| b"rwm".contains(&b));

            if !host_ok || !target_ok || !perms_ok {
                return Err(format!("invalid device `{}` (a /dev path, permissions from rwm)", d.host));
            }
        }

        Ok(())
    }

    /// The settings that need `stack.privileged`, as on the stack path:
    /// added capabilities, devices and sysctls. Each entry names the
    /// setting (`cap_add:NET_ADMIN`, `device:/dev/fuse`, `sysctl:net.core.somaxconn`)
    /// so an approval covers exactly what was shown.
    #[must_use]
    pub fn privileged(&self) -> Vec<String> {
        let caps = self.cap_add.iter().map(|c| format!("cap_add:{}", c.strip_prefix("CAP_").unwrap_or(c)));
        let devices = self.devices.iter().map(|d| format!("device:{}", d.host));
        let sysctls = self.sysctls.iter().map(|(k, _)| format!("sysctl:{k}"));

        caps.chain(devices).chain(sysctls).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> Options<'static> {
        Options {
            restart_policy: None,
            restart_max_attempts: 0,
            cap_add: &[],
            cap_drop: &[],
            ulimits: &[],
            tmpfs: &[],
            dns: &[],
            sysctls: &[],
            devices: &[],
            init: false,
            networks: &[],
        }
    }

    #[test]
    fn empty_options_need_no_new_agent() {
        assert!(opts().validate().is_ok());
        assert!(!opts().needs_new_agent());
        assert!(opts().privileged().is_empty());
    }

    #[test]
    fn attempts_need_the_on_failure_policy() {
        let mut o = opts();
        o.restart_max_attempts = 3;
        assert!(o.validate().is_err());

        o.restart_policy = Some(RestartPolicy::OnFailure);
        assert!(o.validate().is_ok());
        assert!(o.needs_new_agent());
    }

    #[test]
    fn capabilities_are_checked_and_added_ones_are_privileged() {
        let add = ["CAP_NET_ADMIN".to_owned()];
        let drop = ["ALL".to_owned()];
        let mut o = opts();
        o.cap_add = &add;
        o.cap_drop = &drop;
        assert!(o.validate().is_ok());
        assert_eq!(o.privileged(), vec!["cap_add:NET_ADMIN"]);

        let bad = ["net_admin; rm".to_owned()];
        o.cap_add = &bad;
        assert!(o.validate().is_err());
    }

    #[test]
    fn devices_must_be_dev_paths_and_are_privileged() {
        let ok = [Device { host: "/dev/fuse".into(), container: String::new(), permissions: "rw".into() }];
        let mut o = opts();
        o.devices = &ok;
        assert!(o.validate().is_ok());
        assert_eq!(o.privileged(), vec!["device:/dev/fuse"]);

        let bad = [Device { host: "/etc/shadow".into(), container: String::new(), permissions: String::new() }];
        o.devices = &bad;
        assert!(o.validate().is_err());
    }

    #[test]
    fn ulimits_tmpfs_sysctls_and_networks_are_bounded() {
        let u = [Ulimit { name: "nofile".into(), soft: 1024, hard: 512 }];
        let mut o = opts();
        o.ulimits = &u;
        assert!(o.validate().is_err(), "soft above hard");

        let t = [Tmpfs { target: "relative".into(), size_mb: 0 }];
        o = opts();
        o.tmpfs = &t;
        assert!(o.validate().is_err());

        let s = [("kernel".to_owned(), "1".to_owned())];
        o = opts();
        o.sysctls = &s;
        assert!(o.validate().is_err());

        let n = ["host".to_owned()];
        o = opts();
        o.networks = &n;
        assert!(o.validate().is_err());
    }
}
