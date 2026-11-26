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
        /// A network share mounted by the local driver (D-171). The only
        /// driver options accepted, built from checked fields.
        #[serde(default)]
        remote: Option<VolumeRemote>,
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

/// A network share for a local volume: NFS or CIFS/SMB.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum VolumeRemote {
    Nfs {
        /// The NFS server's IP address.
        server: String,
        /// The exported path (absolute).
        path: String,
        /// `3`, `4`, `4.1` or `4.2` (default 4).
        #[serde(default)]
        version: Option<String>,
        #[serde(default)]
        read_only: bool,
    },
    Cifs {
        /// Host name or IP address of the file server.
        server: String,
        share: String,
        #[serde(default)]
        username: Option<String>,
        /// Kept by Docker in the volume's options on that node.
        #[serde(default)]
        password: Option<String>,
        #[serde(default)]
        read_only: bool,
    },
}

impl std::fmt::Debug for VolumeRemote {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Nfs { server, path, .. } => write!(f, "nfs {server}:{path}"),
            Self::Cifs { server, share, .. } => write!(f, "cifs //{server}/{share}"),
        }
    }
}

/// A mount option value: no `,` or `=` (they would add options), printable.
fn opt_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= 256 && !s.chars().any(|c| c.is_control() || matches!(c, ',' | '=' | ' ' | '"'))
}

impl VolumeRemote {
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Nfs { server, path, version, .. } => {
                if server.parse::<std::net::IpAddr>().is_err() {
                    return Err("the NFS server must be an IP address".into());
                }

                if !path.starts_with('/') || path.contains("..") || !opt_ok(path) || path.contains(':') {
                    return Err("the NFS export must be an absolute path".into());
                }

                if version.as_deref().is_some_and(|v| !matches!(v, "3" | "4" | "4.1" | "4.2")) {
                    return Err("the NFS version is 3, 4, 4.1 or 4.2".into());
                }

                Ok(())
            }
            Self::Cifs { server, share, username, password, .. } => {
                let host_ok = server.parse::<std::net::IpAddr>().is_ok()
                    || (server.len() <= 253
                        && server.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-')));

                if !host_ok || server.is_empty() {
                    return Err("the CIFS server must be a host name or IP address".into());
                }

                if !opt_ok(share) || share.contains('/') || share.contains('\\') {
                    return Err("the CIFS share is one name without `/`, `,` or `=`".into());
                }

                if username.as_deref().is_some_and(|u| !opt_ok(u)) || password.as_deref().is_some_and(|p| !opt_ok(p)) {
                    return Err("the CIFS user name and password cannot contain `,`, `=`, spaces or quotes".into());
                }

                Ok(())
            }
        }
    }

    /// The local driver's options: `type`, `o` and `device`.
    #[must_use]
    pub fn driver_opts(&self) -> BTreeMap<String, String> {
        let (kind, mut o, device) = match self {
            Self::Nfs { server, path, version, .. } => (
                "nfs",
                vec![format!("addr={server}"), format!("nfsvers={}", version.as_deref().unwrap_or("4"))],
                format!(":{path}"),
            ),
            Self::Cifs { server, share, username, password, .. } => {
                let mut o = Vec::new();
                o.extend(username.as_ref().map(|u| format!("username={u}")));
                o.extend(password.as_ref().map(|p| format!("password={p}")));

                if username.is_none() {
                    o.push("guest".to_owned());
                }

                ("cifs", o, format!("//{server}/{share}"))
            }
        };

        if matches!(self, Self::Nfs { read_only: true, .. } | Self::Cifs { read_only: true, .. }) {
            o.push("ro".to_owned());
        }

        BTreeMap::from([
            ("type".to_owned(), kind.to_owned()),
            ("o".to_owned(), o.join(",")),
            ("device".to_owned(), device),
        ])
    }
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

/// A network subnet: IPv4 /16 to /30 or IPv6 /48 to /124, never loopback,
/// link-local, multicast or unspecified (a wide subnet would take routes
/// from the node).
pub fn subnet(c: &str) -> Option<(std::net::IpAddr, u8)> {
    let (ip, len) = c.split_once('/')?;
    let ip: std::net::IpAddr = ip.parse().ok()?;
    let len: u8 = len.parse().ok()?;

    let ok = match ip {
        std::net::IpAddr::V4(v) => {
            let o = v.octets();

            // 0/8 (this network), 240/4 (reserved) and 100.64/10 (carrier
            // NAT, used by overlay VPNs such as Tailscale) are left alone.
            let reserved = o[0] == 0 || o[0] >= 240 || (o[0] == 100 && (o[1] & 0xc0) == 64);
            (16..=30).contains(&len)
                && !v.is_loopback()
                && !v.is_link_local()
                && !v.is_multicast()
                && !v.is_unspecified()
                && !reserved
        }
        std::net::IpAddr::V6(v) => {
            (48..=124).contains(&len)
                && !v.is_loopback()
                && !v.is_multicast()
                && !v.is_unspecified()
                && (v.segments()[0] & 0xffc0) != 0xfe80
        }
    };

    ok.then_some((ip, len))
}

/// `ip` lies in `net`/`len` (same family).
#[must_use]
pub fn contains(net: std::net::IpAddr, len: u8, ip: std::net::IpAddr) -> bool {
    match (net, ip) {
        (std::net::IpAddr::V4(n), std::net::IpAddr::V4(i)) => {
            let mask = u32::MAX.checked_shl(32 - u32::from(len)).unwrap_or(0);
            (u32::from(n) & mask) == (u32::from(i) & mask)
        }
        (std::net::IpAddr::V6(n), std::net::IpAddr::V6(i)) => {
            let mask = u128::MAX.checked_shl(128 - u32::from(len)).unwrap_or(0);
            (u128::from(n) & mask) == (u128::from(i) & mask)
        }
        _ => false,
    }
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

    /// Reads of one object's details (audited: they can show secrets).
    #[must_use]
    pub const fn is_inspect(&self) -> bool {
        matches!(
            self,
            Self::ImageInspect { .. }
                | Self::ImageHistory { .. }
                | Self::VolumeInspect { .. }
                | Self::NetworkInspect { .. }
        )
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
            Self::VolumeCreate { name, labels, remote } => {
                if let Some(r) = remote {
                    r.validate()?;
                }

                name_ok(name) && labels_ok(labels)
            }
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

                let net = match subnet.as_deref() {
                    Some(c) => match self::subnet(c) {
                        Some(n) => Some(n),
                        None => {
                            return Err("the subnet is IPv4 /16 to /30 (or IPv6 /48 to /124), not loopback, link-local or multicast".into());
                        }
                    },
                    None => None,
                };

                let gateway_ok = match (gateway.as_deref(), net) {
                    (None, _) => true,
                    (Some(g), Some((n, len))) => g.parse().is_ok_and(|ip| contains(n, len, ip)),
                    (Some(_), None) => false,
                };

                name_ok(name) && labels_ok(labels) && gateway_ok && (!attachable || *driver == NetworkDriver::Overlay)
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
        assert!(create("app", Some("10.0.0.0/8"), false, NetworkDriver::Bridge).validate().is_err(), "too wide");
        assert!(create("app", Some("127.0.0.0/16"), false, NetworkDriver::Bridge).validate().is_err());
        assert!(create("app", Some("169.254.0.0/16"), false, NetworkDriver::Bridge).validate().is_err());
        let mut gw = create("app", Some("10.30.0.0/24"), false, NetworkDriver::Bridge);
        if let DockerOp::NetworkCreate { gateway, .. } = &mut gw {
            *gateway = Some("10.31.0.1".into());
        }
        assert!(gw.validate().is_err(), "the gateway lies in the subnet");
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
        assert!(
            DockerOp::VolumeCreate { name: "data".into(), labels, remote: None }.validate().is_err(),
            "forge.* is reserved"
        );
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
