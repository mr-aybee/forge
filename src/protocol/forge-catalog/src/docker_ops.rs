//! Docker images, volumes and networks on a node (D-157).
//!
//! A closed set of operations the root helper runs over the local Engine
//! API from a signed command, so every node can be managed without opening
//! its Docker API ([`crate::docker_tls`] is the opt-in for the full API).
//! Only plain local volumes and plain bridge or overlay networks can be
//! created: driver options can reach the host and are never accepted.
//! System networks and Forge's own networks cannot be removed.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Networks Docker or Forge rely on.
const PROTECTED_NETWORKS: &[&str] = &["bridge", "host", "none", "ingress", "docker_gwbridge"];
const MAX_LABELS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkDriver {
    Bridge,
    Overlay,
}

impl NetworkDriver {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Bridge => "bridge",
            Self::Overlay => "overlay",
        }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum DockerOp {
    /// Disk use of images, containers, volumes and build cache.
    DiskUsage {},
    Images {},
    ImageInspect {
        id: String,
    },
    /// Image history (layers).
    ImageHistory {
        id: String,
    },
    ImageRemove {
        id: String,
        #[serde(default)]
        force: bool,
    },
    /// Removes unused images; `all: false` removes dangling ones only.
    ImagePrune {
        #[serde(default)]
        all: bool,
    },
    ImageTag {
        id: String,
        repo: String,
        tag: String,
    },
    /// Pulls `image`; `registry_auth` is Docker's `X-Registry-Auth` value
    /// (secret: never logged or returned).
    ImagePull {
        image: String,
        #[serde(default)]
        registry_auth: Option<String>,
    },
    ImagePush {
        image: String,
        #[serde(default)]
        registry_auth: Option<String>,
    },
    Volumes {},
    VolumeInspect {
        name: String,
    },
    VolumeCreate {
        name: String,
        #[serde(default)]
        labels: BTreeMap<String, String>,
    },
    VolumeRemove {
        name: String,
        #[serde(default)]
        force: bool,
    },
    VolumePrune {},
    Networks {},
    NetworkInspect {
        id: String,
    },
    NetworkCreate {
        name: String,
        driver: NetworkDriver,
        /// CIDR, e.g. `10.30.0.0/24`; `None`: Docker chooses.
        #[serde(default)]
        subnet: Option<String>,
        #[serde(default)]
        gateway: Option<String>,
        /// No outside connectivity.
        #[serde(default)]
        internal: bool,
        /// Overlay only: standalone containers may join.
        #[serde(default)]
        attachable: bool,
        #[serde(default)]
        labels: BTreeMap<String, String>,
    },
    NetworkRemove {
        id: String,
    },
    NetworkConnect {
        network: String,
        container: String,
    },
    NetworkDisconnect {
        network: String,
        container: String,
        #[serde(default)]
        force: bool,
    },
    NetworkPrune {},
    /// Docker events of the last `since_secs` seconds (at most 1000).
    Events {
        #[serde(default = "five_minutes")]
        since_secs: u32,
    },
}

const fn five_minutes() -> u32 {
    300
}

impl std::fmt::Debug for DockerOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Registry credentials never print.
        match self {
            Self::ImagePull { image, registry_auth } | Self::ImagePush { image, registry_auth } => f
                .debug_struct(self.name())
                .field("image", image)
                .field("registry_auth", &registry_auth.as_ref().map(|_| "***"))
                .finish(),
            other => f.write_str(other.name()),
        }
    }
}

/// An image id (`sha256:<hex>` or a short hex id) or an image reference.
fn image_ok(s: &str) -> bool {
    let hex = s.strip_prefix("sha256:").unwrap_or(s);
    (hex.len() >= 12 && hex.len() <= 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
        || crate::apps::valid_image_ref(s)
}

/// Docker object names and ids: letters, digits, `_`, `.`, `-`.
fn name_ok(s: &str) -> bool {
    (1..=128).contains(&s.len())
        && s.bytes().next().is_some_and(|b| b.is_ascii_alphanumeric())
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}

fn labels_ok(l: &BTreeMap<String, String>) -> bool {
    l.len() <= MAX_LABELS
        && l.iter()
            .all(|(k, v)| name_ok(k) && !k.starts_with("forge.") && v.len() <= 1024 && !v.chars().any(char::is_control))
}

fn cidr_ok(c: &str) -> bool {
    let Some((ip, len)) = c.split_once('/') else { return false };
    let Ok(ip) = ip.parse::<std::net::IpAddr>() else { return false };
    let max = if ip.is_ipv4() { 32 } else { 128 };
    len.parse::<u8>().is_ok_and(|n| (8..=max).contains(&n))
}

/// Registry auth is base64url JSON, bounded.
fn auth_ok(a: Option<&String>) -> bool {
    a.is_none_or(|a| {
        a.len() <= 16 * 1024
            && a.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'=' | b'+' | b'/'))
    })
}

/// Forge's own networks (environment overlays) and Docker's.
#[must_use]
pub fn network_protected(name: &str) -> bool {
    PROTECTED_NETWORKS.contains(&name) || name.starts_with("forge-")
}

impl DockerOp {
    /// Stable name, for audit and logs.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::DiskUsage {} => "disk_usage",
            Self::Images {} => "images",
            Self::ImageInspect { .. } => "image_inspect",
            Self::ImageHistory { .. } => "image_history",
            Self::ImageRemove { .. } => "image_remove",
            Self::ImagePrune { .. } => "image_prune",
            Self::ImageTag { .. } => "image_tag",
            Self::ImagePull { .. } => "image_pull",
            Self::ImagePush { .. } => "image_push",
            Self::Volumes {} => "volumes",
            Self::VolumeInspect { .. } => "volume_inspect",
            Self::VolumeCreate { .. } => "volume_create",
            Self::VolumeRemove { .. } => "volume_remove",
            Self::VolumePrune {} => "volume_prune",
            Self::Networks {} => "networks",
            Self::NetworkInspect { .. } => "network_inspect",
            Self::NetworkCreate { .. } => "network_create",
            Self::NetworkRemove { .. } => "network_remove",
            Self::NetworkConnect { .. } => "network_connect",
            Self::NetworkDisconnect { .. } => "network_disconnect",
            Self::NetworkPrune {} => "network_prune",
            Self::Events { .. } => "events",
        }
    }

    /// Reads change nothing on the node.
    #[must_use]
    pub const fn read_only(&self) -> bool {
        matches!(
            self,
            Self::DiskUsage {}
                | Self::Images {}
                | Self::ImageInspect { .. }
                | Self::ImageHistory { .. }
                | Self::Volumes {}
                | Self::VolumeInspect { .. }
                | Self::Networks {}
                | Self::NetworkInspect { .. }
                | Self::Events { .. }
        )
    }

    /// What the operation acts on.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Images {}
            | Self::ImageInspect { .. }
            | Self::ImageHistory { .. }
            | Self::ImageRemove { .. }
            | Self::ImagePrune { .. }
            | Self::ImageTag { .. }
            | Self::ImagePull { .. }
            | Self::ImagePush { .. } => "image",
            Self::Volumes {}
            | Self::VolumeInspect { .. }
            | Self::VolumeCreate { .. }
            | Self::VolumeRemove { .. }
            | Self::VolumePrune {} => "volume",
            Self::Networks {}
            | Self::NetworkInspect { .. }
            | Self::NetworkCreate { .. }
            | Self::NetworkRemove { .. }
            | Self::NetworkConnect { .. }
            | Self::NetworkDisconnect { .. }
            | Self::NetworkPrune {} => "network",
            Self::DiskUsage {} | Self::Events { .. } => "system",
        }
    }

    /// Checks the operation before anything runs as root.
    pub fn validate(&self) -> Result<(), String> {
        let ok = match self {
            Self::DiskUsage {} | Self::Images {} | Self::Volumes {} | Self::Networks {} => true,
            Self::ImagePrune { .. } | Self::VolumePrune {} | Self::NetworkPrune {} => true,
            Self::ImageInspect { id } | Self::ImageHistory { id } | Self::ImageRemove { id, .. } => image_ok(id),
            Self::ImageTag { id, repo, tag } => {
                image_ok(id) && crate::apps::valid_image_ref(&format!("{repo}:{tag}")) && !tag.contains('/')
            }
            Self::ImagePull { image, registry_auth } | Self::ImagePush { image, registry_auth } => {
                crate::apps::valid_image_ref(image) && auth_ok(registry_auth.as_ref())
            }
            Self::VolumeInspect { name } | Self::VolumeRemove { name, .. } => name_ok(name),
            Self::VolumeCreate { name, labels } => name_ok(name) && labels_ok(labels),
            Self::NetworkInspect { id } => name_ok(id),
            Self::NetworkRemove { id } => {
                if network_protected(id) {
                    return Err(format!("network `{id}` belongs to Docker or Forge and cannot be removed"));
                }
                name_ok(id)
            }
            Self::NetworkCreate { name, driver, subnet, gateway, attachable, labels, .. } => {
                if network_protected(name) {
                    return Err(format!("the network name `{name}` is reserved"));
                }
                name_ok(name)
                    && labels_ok(labels)
                    && subnet.as_deref().is_none_or(cidr_ok)
                    && gateway.as_deref().is_none_or(|g| g.parse::<std::net::IpAddr>().is_ok())
                    && (gateway.is_none() || subnet.is_some())
                    && (!attachable || *driver == NetworkDriver::Overlay)
            }
            Self::NetworkConnect { network, container } | Self::NetworkDisconnect { network, container, .. } => {
                if network_protected(network) {
                    return Err(format!("network `{network}` is managed by Docker or Forge"));
                }
                name_ok(network) && name_ok(container)
            }
            Self::Events { since_secs } => *since_secs <= 86_400,
        };
        if ok { Ok(()) } else { Err(format!("invalid {} request", self.name())) }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn creates_take_no_driver_options() {
        let v = serde_json::json!({"op": "volume_create", "name": "data", "driver_opts": {"device": "/"}});
        assert!(serde_json::from_value::<DockerOp>(v).is_err(), "unknown fields are refused");
        let v = serde_json::json!({"op": "network_create", "name": "app", "driver": "macvlan"});
        assert!(serde_json::from_value::<DockerOp>(v).is_err(), "only bridge and overlay");
    }

    #[test]
    fn protected_networks_stay() {
        for n in ["bridge", "host", "ingress", "forge-net-0190"] {
            assert!(DockerOp::NetworkRemove { id: n.into() }.validate().is_err(), "{n}");
        }
        assert!(DockerOp::NetworkRemove { id: "app_default".into() }.validate().is_ok());
        let create =
            |name: &str, subnet: Option<&str>, attachable: bool, driver: NetworkDriver| DockerOp::NetworkCreate {
                name: name.into(),
                driver,
                subnet: subnet.map(str::to_owned),
                gateway: None,
                internal: false,
                attachable,
                labels: BTreeMap::new(),
            };
        assert!(create("app", Some("10.30.0.0/24"), false, NetworkDriver::Bridge).validate().is_ok());
        assert!(create("forge-x", None, false, NetworkDriver::Bridge).validate().is_err());
        assert!(create("app", Some("10.30.0.0"), false, NetworkDriver::Bridge).validate().is_err());
        assert!(create("app", None, true, NetworkDriver::Bridge).validate().is_err(), "attachable is overlay only");
    }

    #[test]
    fn names_and_images_are_checked() {
        assert!(DockerOp::ImageRemove { id: "sha256:".to_owned() + &"a".repeat(64), force: false }.validate().is_ok());
        assert!(DockerOp::ImageRemove { id: "nginx:1.27".into(), force: false }.validate().is_ok());
        assert!(DockerOp::ImageRemove { id: "../../x".into(), force: false }.validate().is_err());
        assert!(DockerOp::VolumeRemove { name: "-rf".into(), force: false }.validate().is_err());
        let mut labels = BTreeMap::new();
        labels.insert("forge.managed".to_owned(), "true".to_owned());
        assert!(DockerOp::VolumeCreate { name: "data".into(), labels }.validate().is_err(), "forge.* is reserved");
        assert!(DockerOp::ImagePull { image: "nginx".into(), registry_auth: Some("a b".into()) }.validate().is_err());
    }

    #[test]
    fn credentials_never_print() {
        let op = DockerOp::ImagePull { image: "ghcr.io/a/b:1".into(), registry_auth: Some("c2VjcmV0".into()) };
        assert!(!format!("{op:?}").contains("c2VjcmV0"));
        assert!(!op.read_only());
        assert!(DockerOp::Images {}.read_only());
        assert_eq!(op.kind(), "image");
    }
}
