//! Docker Swarm membership and node management on a node (D-154).
//!
//! The root helper runs these operations from a signed command only, over the
//! local Docker Engine API. Join tokens travel in signed commands and results
//! only; the agent never logs them, and the control plane stores them sealed.
//!
//! * [`SwarmOp::Status`] reports whether the node is in a swarm, and its role.
//! * [`SwarmOp::Init`] creates a swarm with this node as the first manager.
//! * [`SwarmOp::Join`] joins an existing swarm as a worker or manager.
//! * [`SwarmOp::Leave`] leaves the swarm.
//! * [`SwarmOp::JoinTokens`] and [`SwarmOp::RotateTokens`] (managers only).
//! * [`SwarmOp::Nodes`], [`SwarmOp::NodeUpdate`], [`SwarmOp::NodeRemove`]
//!   (managers only): the swarm's node list and lifecycle.

use std::collections::BTreeMap;
use std::net::IpAddr;

use serde::{Deserialize, Serialize};

/// Docker's default swarm management port.
pub const DEFAULT_PORT: u16 = 2377;
const MAX_LABELS: usize = 64;
const MAX_POOLS: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SwarmRole {
    Manager,
    Worker,
}

impl SwarmRole {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Manager => "manager",
            Self::Worker => "worker",
        }
    }
}

/// Whether the swarm schedules tasks on a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Availability {
    /// Tasks run here.
    Active,
    /// No new tasks; running ones stay.
    Pause,
    /// Tasks move to other nodes.
    Drain,
}

impl Availability {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Pause => "pause",
            Self::Drain => "drain",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum SwarmOp {
    Status {},
    Init {
        /// Address or interface other nodes reach this manager on; `None`
        /// lets Docker choose (works when the node has one address).
        #[serde(default)]
        advertise_addr: Option<String>,
        #[serde(default = "default_port")]
        port: u16,
        /// Address pools for overlay networks (CIDR); empty: Docker's default.
        #[serde(default)]
        default_addr_pool: Vec<String>,
        /// Lock managers' keys at rest; unlocking after a restart needs the key.
        #[serde(default)]
        autolock: bool,
    },
    Join {
        /// Managers to contact (`ip:port`).
        remote_addrs: Vec<String>,
        join_token: String,
        #[serde(default)]
        advertise_addr: Option<String>,
    },
    Leave {
        /// Needed for the last manager, which dissolves the swarm.
        #[serde(default)]
        force: bool,
    },
    JoinTokens {},
    RotateTokens {
        #[serde(default)]
        worker: bool,
        #[serde(default)]
        manager: bool,
    },
    Nodes {},
    NodeUpdate {
        node_id: String,
        #[serde(default)]
        role: Option<SwarmRole>,
        #[serde(default)]
        availability: Option<Availability>,
        /// Replaces the node's labels when present.
        #[serde(default)]
        labels: Option<BTreeMap<String, String>>,
    },
    NodeRemove {
        node_id: String,
        #[serde(default)]
        force: bool,
    },
    /// Creates the encrypted overlay network an environment's services share.
    EnsureNetwork {
        environment_id: String,
    },
    /// Creates the service, or updates it to `def`. An existing service is
    /// updated only when it carries the same Forge service id.
    ServiceApply {
        def: Box<crate::swarm_service::ServiceDef>,
    },
    ServiceStatus {
        service_id: String,
        #[serde(default)]
        slot: String,
    },
    ServiceScale {
        service_id: String,
        #[serde(default)]
        slot: String,
        replicas: u32,
    },
    /// Replaces every task with the same spec (`--force`).
    ServiceRestart {
        service_id: String,
        #[serde(default)]
        slot: String,
    },
    /// Back to the previous spec.
    ServiceRollback {
        service_id: String,
        #[serde(default)]
        slot: String,
    },
    ServiceRemove {
        service_id: String,
        #[serde(default)]
        slot: String,
    },
    ServiceLogs {
        service_id: String,
        #[serde(default)]
        slot: String,
        #[serde(default = "default_tail")]
        tail: u32,
    },
}

const fn default_tail() -> u32 {
    200
}

const fn default_port() -> u16 {
    DEFAULT_PORT
}

/// Swarm node ids are 25 lower-case letters and digits.
fn node_id_ok(id: &str) -> bool {
    (1..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

/// `SWMTKN-1-<hash>-<secret>`: letters, digits and dashes.
fn token_ok(t: &str) -> bool {
    t.starts_with("SWMTKN-1-") && t.len() <= 256 && t.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn remote_ok(a: &str) -> bool {
    a.parse::<std::net::SocketAddr>().is_ok()
}

fn cidr_ok(c: &str) -> bool {
    let Some((ip, len)) = c.split_once('/') else { return false };
    let Ok(ip) = ip.parse::<IpAddr>() else { return false };
    let max = if ip.is_ipv4() { 32 } else { 128 };
    len.parse::<u8>().is_ok_and(|n| (8..=max).contains(&n))
}

/// Label keys follow Docker's convention; values are printable and short.
fn label_ok(k: &str, v: &str) -> bool {
    (1..=128).contains(&k.len())
        && k.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b'/'))
        && v.len() <= 256
        && !v.chars().any(char::is_control)
}

/// An IP address other nodes can reach, or a network interface name.
fn addr_ok(a: Option<&str>) -> bool {
    let Some(a) = a else { return true };
    match a.parse::<IpAddr>() {
        Ok(ip) => !ip.is_unspecified() && !ip.is_multicast() && !ip.is_loopback(),
        Err(_) => {
            (1..=15).contains(&a.len())
                && a.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
        }
    }
}

fn slot_checked(id: &str, slot: &str) -> Result<(), String> {
    id_checked(id)?;
    if crate::swarm_service::slot_ok(slot) { Ok(()) } else { Err("a slot is empty, `a` or `b`".into()) }
}

fn id_checked(id: &str) -> Result<(), String> {
    if crate::swarm_service::id_ok(id) { Ok(()) } else { Err("invalid Forge id".into()) }
}

impl SwarmOp {
    /// Checks the operation before anything runs as root.
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Init { advertise_addr, port, default_addr_pool, .. } => {
                if !addr_ok(advertise_addr.as_deref()) {
                    return Err("the advertise address must be a reachable IP address or an interface name".into());
                }
                if *port < 1024 {
                    return Err("the swarm port must be 1024 or higher".into());
                }
                if default_addr_pool.len() > MAX_POOLS || !default_addr_pool.iter().all(|c| cidr_ok(c)) {
                    return Err("address pools must be CIDR ranges (at most 8)".into());
                }
                Ok(())
            }
            Self::Join { remote_addrs, join_token, advertise_addr } => {
                if remote_addrs.is_empty() || remote_addrs.len() > 7 || !remote_addrs.iter().all(|a| remote_ok(a)) {
                    return Err("managers must be 1 to 7 `ip:port` addresses".into());
                }
                if !token_ok(join_token) {
                    return Err("not a swarm join token".into());
                }
                if !addr_ok(advertise_addr.as_deref()) {
                    return Err("the advertise address must be a reachable IP address or an interface name".into());
                }
                Ok(())
            }
            Self::NodeUpdate { node_id, labels, role, availability } => {
                if !node_id_ok(node_id) {
                    return Err("invalid swarm node id".into());
                }
                if let Some(l) = labels
                    && (l.len() > MAX_LABELS || !l.iter().all(|(k, v)| label_ok(k, v)))
                {
                    return Err("labels: keys of letters, digits and . - _ /, at most 64".into());
                }
                if role.is_none() && availability.is_none() && labels.is_none() {
                    return Err("nothing to change".into());
                }
                Ok(())
            }
            Self::NodeRemove { node_id, .. } => {
                if node_id_ok(node_id) {
                    Ok(())
                } else {
                    Err("invalid swarm node id".into())
                }
            }
            Self::RotateTokens { worker, manager } => {
                if *worker || *manager {
                    Ok(())
                } else {
                    Err("choose the worker or manager token".into())
                }
            }
            Self::EnsureNetwork { environment_id: id } => id_checked(id),
            Self::ServiceStatus { service_id: id, slot }
            | Self::ServiceRestart { service_id: id, slot }
            | Self::ServiceRollback { service_id: id, slot }
            | Self::ServiceRemove { service_id: id, slot } => slot_checked(id, slot),
            Self::ServiceScale { service_id, slot, replicas } => {
                slot_checked(service_id, slot)?;
                if *replicas > 1000 { Err("at most 1000 replicas".into()) } else { Ok(()) }
            }
            Self::ServiceLogs { service_id, slot, tail } => {
                slot_checked(service_id, slot)?;
                if *tail > 10_000 { Err("at most 10000 log lines".into()) } else { Ok(()) }
            }
            Self::ServiceApply { def } => def.validate(),
            Self::Status {} | Self::Leave { .. } | Self::JoinTokens {} | Self::Nodes {} => Ok(()),
        }
    }
}

/// What [`SwarmOp::Status`] reports.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwarmStatus {
    /// Docker Engine answers on the node.
    pub docker: bool,
    /// `inactive`, `pending`, `active`, `error` or `locked` (Docker's
    /// `LocalNodeState`).
    pub state: String,
    pub swarm_id: Option<String>,
    pub node_id: Option<String>,
    /// The node is a manager.
    pub manager: bool,
    /// Address the node advertises.
    pub node_addr: Option<String>,
    /// Managers the node knows (`ip:port`).
    pub managers: Vec<String>,
}

/// Join tokens (managers only). Secret: sealed at rest, never logged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JoinTokens {
    pub worker: String,
    pub manager: String,
}

/// One task of a service.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwarmTask {
    pub id: String,
    pub node_id: String,
    /// Replica number (replicated mode).
    pub slot: u64,
    /// `running`, `starting`, `failed`, `shutdown`, …
    pub state: String,
    pub desired_state: String,
    pub error: String,
    pub image: String,
    /// RFC 3339 time of the last state change.
    pub updated_at: String,
}

/// A service as a manager reports it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwarmServiceStatus {
    pub exists: bool,
    pub id: String,
    /// `replicated` or `global`.
    pub mode: String,
    /// Replicas asked for (replicated mode).
    pub desired: u64,
    pub running: u64,
    /// Rolling update: `updating`, `completed`, `paused`, `rollback_started`,
    /// `rollback_completed`, … (empty before the first update).
    pub update_state: String,
    pub update_message: String,
    pub image: String,
    pub tasks: Vec<SwarmTask>,
}

/// One node as a manager sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwarmNode {
    pub id: String,
    pub hostname: String,
    pub role: SwarmRole,
    pub availability: Availability,
    /// `ready`, `down`, `disconnected` or `unknown`.
    pub state: String,
    pub addr: String,
    /// `reachable`, `unreachable` or empty for workers.
    pub manager_status: String,
    pub leader: bool,
    pub engine_version: String,
    pub labels: BTreeMap<String, String>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const TOKEN: &str = "SWMTKN-1-3pu6hszjas19xyp7ghgosyx9k8atbfcr8p2is99znpy26u2lkl-1awxwuwd3z9j1z3puu7rcgdbx";

    #[test]
    fn init_checks_address_port_and_pools() {
        let ok = SwarmOp::Init {
            advertise_addr: Some("192.0.2.10".into()),
            port: 2377,
            default_addr_pool: vec!["10.20.0.0/16".into()],
            autolock: false,
        };
        assert!(ok.validate().is_ok());
        let any = SwarmOp::Init {
            advertise_addr: Some("0.0.0.0".into()),
            port: 2377,
            default_addr_pool: vec![],
            autolock: false,
        };
        assert!(any.validate().is_err());
        let low = SwarmOp::Init {
            advertise_addr: Some("192.0.2.10".into()),
            port: 80,
            default_addr_pool: vec![],
            autolock: false,
        };
        assert!(low.validate().is_err());
        let pool = SwarmOp::Init {
            advertise_addr: Some("192.0.2.10".into()),
            port: 2377,
            default_addr_pool: vec!["10.0.0.0".into()],
            autolock: false,
        };
        assert!(pool.validate().is_err());
    }

    #[test]
    fn join_needs_managers_and_a_real_token() {
        let join = |remote: &str, token: &str| SwarmOp::Join {
            remote_addrs: vec![remote.into()],
            join_token: token.into(),
            advertise_addr: Some("eth0".into()),
        };
        assert!(join("192.0.2.10:2377", TOKEN).validate().is_ok());
        assert!(join("192.0.2.10", TOKEN).validate().is_err(), "a port is required");
        assert!(join("192.0.2.10:2377", "letmein").validate().is_err());
        assert!(join("192.0.2.10:2377", "SWMTKN-1-a b").validate().is_err(), "no spaces or shell characters");
    }

    #[test]
    fn node_updates_check_ids_and_labels() {
        let id = "24ifsmvkjbyhk7zxyo3wh9lkf".to_owned();
        let drain = SwarmOp::NodeUpdate {
            node_id: id.clone(),
            role: None,
            availability: Some(Availability::Drain),
            labels: None,
        };
        assert!(drain.validate().is_ok());
        let nothing = SwarmOp::NodeUpdate { node_id: id.clone(), role: None, availability: None, labels: None };
        assert!(nothing.validate().is_err());
        let bad_id = SwarmOp::NodeRemove { node_id: "../x".into(), force: false };
        assert!(bad_id.validate().is_err());
        let labels = BTreeMap::from([("zone$".to_owned(), "a".to_owned())]);
        let bad = SwarmOp::NodeUpdate { node_id: id, role: None, availability: None, labels: Some(labels) };
        assert!(bad.validate().is_err());
    }

    #[test]
    fn ops_round_trip_and_reject_unknown_fields() {
        let op = SwarmOp::RotateTokens { worker: true, manager: false };
        let json = serde_json::to_string(&op).unwrap();
        assert_eq!(serde_json::from_str::<SwarmOp>(&json).unwrap(), op);
        assert!(serde_json::from_str::<SwarmOp>(r#"{"op":"leave","force":true,"x":1}"#).is_err());
        assert!(
            SwarmOp::ServiceRemove { service_id: "01900000-0000-7000-8000-000000000001".into(), slot: "a".into() }
                .validate()
                .is_ok()
        );
        assert!(SwarmOp::ServiceRemove { service_id: "../x".into(), slot: String::new() }.validate().is_err());
    }
}
