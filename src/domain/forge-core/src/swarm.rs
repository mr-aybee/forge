//! Docker Swarm state of a Swarm cluster (D-154): the swarm itself and which
//! of the cluster's nodes are in it. The live node list comes from a manager;
//! this is what Forge needs to join, leave and re-join nodes.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::clock::Timestamp;
use crate::error::{DomainError, DomainResult};
use crate::ids::{ClusterId, ServerId};

/// The swarm behind a Swarm cluster. Absent until the first node joined.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClusterSwarm {
    pub cluster_id: ClusterId,
    /// Docker's swarm id.
    pub swarm_id: String,
    /// Swarm management port on the managers.
    pub port: u16,
    /// Address pools for overlay networks (CIDR), as created.
    pub default_addr_pool: Vec<String>,
    pub autolock: bool,
    /// The worker and manager join tokens, sealed. Never serialized.
    #[serde(skip)]
    pub tokens_sealed: Option<String>,
    /// While `swarm_id` is empty: the node creating the swarm.
    pub founder: Option<ServerId>,
    pub updated_at: Timestamp,
}

impl ClusterSwarm {
    /// The swarm is still being created.
    #[must_use]
    pub fn pending(&self) -> bool {
        self.swarm_id.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemberRole {
    Manager,
    Worker,
}

impl MemberRole {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Manager => "manager",
            Self::Worker => "worker",
        }
    }
}

impl std::str::FromStr for MemberRole {
    type Err = DomainError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "manager" => Ok(Self::Manager),
            "worker" => Ok(Self::Worker),
            other => Err(DomainError::internal(format!("unknown swarm role {other:?}"))),
        }
    }
}

/// Where a node is on its way into or out of the swarm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemberState {
    /// A join job is queued or running.
    Joining,
    Joined,
    /// A leave job is queued or running.
    Leaving,
    /// The last join or leave failed; `error` says why.
    Failed,
}

impl MemberState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Joining => "joining",
            Self::Joined => "joined",
            Self::Leaving => "leaving",
            Self::Failed => "failed",
        }
    }
}

impl std::str::FromStr for MemberState {
    type Err = DomainError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "joining" => Ok(Self::Joining),
            "joined" => Ok(Self::Joined),
            "leaving" => Ok(Self::Leaving),
            "failed" => Ok(Self::Failed),
            other => Err(DomainError::internal(format!("unknown swarm member state {other:?}"))),
        }
    }
}

/// One Forge node's membership in its cluster's swarm.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwarmMember {
    pub server_id: ServerId,
    pub cluster_id: ClusterId,
    /// Docker's node id once joined.
    pub node_id: String,
    pub role: MemberRole,
    /// Address or interface the node advertises (empty: Docker's choice).
    pub advertise_addr: String,
    /// The address managers are reached on (`ip:port`), for managers.
    pub manager_addr: String,
    pub state: MemberState,
    pub error: String,
    pub updated_at: Timestamp,
}

#[async_trait]
pub trait SwarmRepository: Send + Sync + std::fmt::Debug {
    async fn get_swarm(&self, cluster: ClusterId) -> DomainResult<Option<ClusterSwarm>>;
    /// Inserts `swarm` only when the cluster has no swarm row yet; `true`
    /// when this call inserted it. Atomic: of two concurrent claims, one wins.
    async fn claim_swarm(&self, swarm: &ClusterSwarm) -> DomainResult<bool>;
    /// The cluster that already records Docker swarm `swarm_id`, if any.
    async fn cluster_of_swarm(&self, swarm_id: &str) -> DomainResult<Option<ClusterId>>;
    /// Inserts or replaces the cluster's swarm row.
    async fn put_swarm(&self, swarm: &ClusterSwarm) -> DomainResult<()>;
    /// Removes the swarm row (the last node left).
    async fn delete_swarm(&self, cluster: ClusterId) -> DomainResult<()>;
    async fn list_members(&self, cluster: ClusterId) -> DomainResult<Vec<SwarmMember>>;
    async fn get_member(&self, server: ServerId) -> DomainResult<Option<SwarmMember>>;
    /// Inserts or replaces a node's membership.
    async fn put_member(&self, member: &SwarmMember) -> DomainResult<()>;
    async fn delete_member(&self, server: ServerId) -> DomainResult<()>;
}
