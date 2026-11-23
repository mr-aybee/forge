//! Docker Swarm in Swarm clusters (D-154): what the API needs beyond the
//! `swarm.member` job. It reads the swarm and its members, checks join and
//! leave requests, and runs quick node operations through a manager.
//!
//! Quick operations (node list, drain, promote, labels, token rotation) are
//! one signed command to a manager and an answer within seconds; joins and
//! leaves are durable jobs ([`crate::jobs::swarm`]).

use std::sync::Arc;
use std::time::Duration;

use forge_catalog::swarm::{Availability, JoinTokens, SwarmNode, SwarmOp, SwarmRole};
use forge_core::model::{Cluster, ClusterMode};
use forge_core::ports::{ClusterRepository, ServerRepository};
use forge_core::swarm::{ClusterSwarm, MemberRole, MemberState, SwarmMember, SwarmRepository};
use forge_core::{Clock, ClusterId, DomainError, DomainResult, ServerId};
use forge_proto::messages::SwarmAction;
use forge_proto::{Command, CommandSigner};
use forge_secrets::Envelope;
use serde::Serialize;

use crate::agent_hub::CommandQueue;
use crate::jobs::swarm::tokens_aad;

/// How long a quick operation may take on the manager.
const QUICK: Duration = Duration::from_secs(20);

/// A member with the node's name, for the console.
#[derive(Debug, Clone, Serialize)]
pub struct MemberView {
    #[serde(flatten)]
    pub member: SwarmMember,
    pub hostname: String,
}

/// The swarm of a cluster as the console shows it.
#[derive(Debug, Clone, Serialize)]
pub struct SwarmView {
    pub cluster_id: ClusterId,
    pub mode: ClusterMode,
    /// `None` until the first node joined.
    pub swarm: Option<ClusterSwarm>,
    pub members: Vec<MemberView>,
    /// The live node list from a manager, when one answered.
    pub nodes: Option<Vec<SwarmNode>>,
    /// Why `nodes` is missing.
    pub nodes_error: Option<String>,
    pub quorum: Quorum,
}

/// Manager-count health: Raft needs a majority of managers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Quorum {
    pub managers: usize,
    /// Managers that may fail while the swarm keeps working.
    pub tolerates: usize,
    /// `ok`, `single` (no failure tolerated) or `even` (an even count adds
    /// no tolerance over one fewer).
    pub status: &'static str,
}

#[must_use]
pub fn quorum(managers: usize) -> Quorum {
    let tolerates = managers.saturating_sub(1) / 2;
    let status = if managers <= 1 {
        "single"
    } else if managers.is_multiple_of(2) {
        "even"
    } else {
        "ok"
    };
    Quorum { managers, tolerates, status }
}

/// A change to one swarm node.
#[derive(Debug, Clone, Default)]
pub struct NodeChange {
    pub role: Option<MemberRole>,
    pub availability: Option<Availability>,
    pub labels: Option<std::collections::BTreeMap<String, String>>,
}

#[derive(Debug)]
pub struct SwarmService {
    pub swarms: Arc<dyn SwarmRepository>,
    pub servers: Arc<dyn ServerRepository>,
    pub clusters: Arc<dyn ClusterRepository>,
    pub hub: Arc<CommandQueue>,
    pub signer: Arc<CommandSigner>,
    pub envelope: Envelope,
    pub clock: Arc<dyn Clock>,
}

fn invalid(msg: impl Into<String>) -> DomainError {
    DomainError::Invalid(msg.into())
}

impl SwarmService {
    /// The cluster, which must be a Swarm cluster.
    pub async fn swarm_cluster(&self, id: ClusterId) -> DomainResult<Cluster> {
        let c = self.clusters.get_cluster(id).await?;
        if c.mode != ClusterMode::Swarm {
            return Err(invalid("this cluster is not a Docker Swarm cluster"));
        }
        Ok(c)
    }

    pub async fn view(&self, cluster: ClusterId, principal: &str) -> DomainResult<SwarmView> {
        let c = self.clusters.get_cluster(cluster).await?;
        let mut members = Vec::new();
        for m in self.swarms.list_members(cluster).await? {
            let hostname = match self.servers.get_server(m.server_id).await {
                Ok(s) => s.display_name.unwrap_or(s.hostname),
                Err(_) => String::new(),
            };
            members.push(MemberView { member: m, hostname });
        }
        let managers = members
            .iter()
            .filter(|m| m.member.role == MemberRole::Manager && m.member.state == MemberState::Joined)
            .count();
        let (nodes, nodes_error) = if c.mode == ClusterMode::Swarm && managers > 0 {
            match self.on_manager(cluster, principal, &SwarmOp::Nodes {}).await {
                Ok(v) => (Some(serde_json::from_value(v).map_err(DomainError::internal)?), None),
                Err(e) => (None, Some(e.to_string())),
            }
        } else {
            (None, None)
        };
        Ok(SwarmView {
            cluster_id: cluster,
            mode: c.mode,
            swarm: self.swarms.get_swarm(cluster).await?,
            members,
            nodes,
            nodes_error,
            quorum: quorum(managers),
        })
    }

    /// Checks that `server` may join `cluster`'s swarm now and marks it
    /// `joining`, so a second request is refused while the job runs.
    pub async fn begin_join(
        &self,
        cluster: ClusterId,
        server: ServerId,
        role: MemberRole,
        advertise_addr: Option<&str>,
    ) -> DomainResult<()> {
        self.swarm_cluster(cluster).await?;
        let s = self.servers.get_server(server).await?;
        if s.cluster_id != Some(cluster) {
            return Err(invalid("the node is not in this cluster; assign it to the cluster first"));
        }
        if s.os != "linux" {
            return Err(invalid("Docker Swarm nodes must run Linux"));
        }
        if let Some(m) = self.swarms.get_member(server).await?
            && matches!(m.state, MemberState::Joining | MemberState::Joined | MemberState::Leaving)
        {
            return Err(DomainError::Conflict(format!(
                "the node is already {} the swarm",
                match m.state {
                    MemberState::Joined => "in",
                    MemberState::Leaving => "leaving",
                    _ => "joining",
                }
            )));
        }
        let has_swarm = self.swarms.get_swarm(cluster).await?.is_some();
        if !has_swarm && role != MemberRole::Manager {
            return Err(invalid("the first node of a swarm must be a manager"));
        }
        self.swarms
            .put_member(&SwarmMember {
                server_id: server,
                cluster_id: cluster,
                node_id: String::new(),
                role,
                advertise_addr: advertise_addr.unwrap_or_default().to_owned(),
                manager_addr: String::new(),
                state: MemberState::Joining,
                error: String::new(),
                updated_at: self.clock.now(),
            })
            .await
    }

    /// Checks that `server` may leave: it is a member, and it is not the last
    /// manager while workers remain.
    pub async fn check_leave(&self, cluster: ClusterId, server: ServerId) -> DomainResult<SwarmMember> {
        self.swarm_cluster(cluster).await?;
        let m = self.swarms.get_member(server).await?.filter(|m| m.cluster_id == cluster);
        let m = m.ok_or_else(|| DomainError::NotFound("the node is not in this swarm".into()))?;
        if m.state == MemberState::Leaving {
            return Err(DomainError::Conflict("the node is already leaving".into()));
        }
        let all = self.swarms.list_members(cluster).await?;
        let other_managers = all
            .iter()
            .filter(|x| x.server_id != server && x.role == MemberRole::Manager && x.state == MemberState::Joined)
            .count();
        let others = all.iter().filter(|x| x.server_id != server).count();
        if m.role == MemberRole::Manager && other_managers == 0 && others > 0 {
            return Err(DomainError::Conflict(
                "this is the last manager: promote another node first, or remove the workers".into(),
            ));
        }
        Ok(m)
    }

    /// Changes one node through a manager.
    pub async fn update_node(
        &self,
        cluster: ClusterId,
        server: ServerId,
        principal: &str,
        change: NodeChange,
    ) -> DomainResult<SwarmNode> {
        self.swarm_cluster(cluster).await?;
        let mut m = self
            .swarms
            .get_member(server)
            .await?
            .filter(|m| m.cluster_id == cluster && m.state == MemberState::Joined && !m.node_id.is_empty())
            .ok_or_else(|| DomainError::NotFound("the node is not in this swarm".into()))?;
        if change.role == Some(MemberRole::Worker) && m.role == MemberRole::Manager {
            let managers = self
                .swarms
                .list_members(cluster)
                .await?
                .iter()
                .filter(|x| x.role == MemberRole::Manager && x.state == MemberState::Joined)
                .count();
            if managers <= 1 {
                return Err(DomainError::Conflict("the swarm needs at least one manager".into()));
            }
        }
        let op = SwarmOp::NodeUpdate {
            node_id: m.node_id.clone(),
            role: change.role.map(|r| match r {
                MemberRole::Manager => SwarmRole::Manager,
                MemberRole::Worker => SwarmRole::Worker,
            }),
            availability: change.availability,
            labels: change.labels,
        };
        op.validate().map_err(invalid)?;
        // A demoted manager answers no more; use another manager when there is one.
        let node: SwarmNode =
            serde_json::from_value(self.on_manager_except(cluster, principal, &op, Some(server)).await?)
                .map_err(DomainError::internal)?;
        if let Some(r) = change.role {
            m.role = r;
            if r == MemberRole::Worker {
                m.manager_addr.clear();
            } else if m.manager_addr.is_empty() && !node.addr.is_empty() {
                m.manager_addr = format!("{}:{}", node.addr, forge_catalog::swarm::DEFAULT_PORT);
            }
            m.updated_at = self.clock.now();
            self.swarms.put_member(&m).await?;
        }
        Ok(node)
    }

    /// Rotates join tokens on a manager and keeps the new ones sealed.
    pub async fn rotate_tokens(
        &self,
        cluster: ClusterId,
        principal: &str,
        worker: bool,
        manager: bool,
    ) -> DomainResult<()> {
        self.swarm_cluster(cluster).await?;
        let mut s =
            self.swarms.get_swarm(cluster).await?.ok_or_else(|| DomainError::NotFound("no swarm yet".into()))?;
        let op = SwarmOp::RotateTokens { worker, manager };
        op.validate().map_err(invalid)?;
        let tokens: JoinTokens =
            serde_json::from_value(self.on_manager(cluster, principal, &op).await?).map_err(DomainError::internal)?;
        let raw = serde_json::to_vec(&tokens).map_err(DomainError::internal)?;
        s.tokens_sealed = Some(self.envelope.seal(&raw, &tokens_aad(cluster)).map_err(DomainError::internal)?);
        s.updated_at = self.clock.now();
        self.swarms.put_swarm(&s).await
    }

    async fn on_manager(&self, cluster: ClusterId, principal: &str, op: &SwarmOp) -> DomainResult<serde_json::Value> {
        self.on_manager_except(cluster, principal, op, None).await
    }

    /// Runs `op` on the first joined manager that answers, skipping `except`
    /// when another one exists.
    async fn on_manager_except(
        &self,
        cluster: ClusterId,
        principal: &str,
        op: &SwarmOp,
        except: Option<ServerId>,
    ) -> DomainResult<serde_json::Value> {
        let mut managers: Vec<SwarmMember> = self
            .swarms
            .list_members(cluster)
            .await?
            .into_iter()
            .filter(|m| m.role == MemberRole::Manager && m.state == MemberState::Joined)
            .collect();
        if managers.len() > 1 {
            managers.retain(|m| Some(m.server_id) != except);
        }
        let mut last = String::from("no manager of this swarm is in Forge");
        for m in managers {
            match self.rpc(m.server_id, principal, op).await {
                Ok(v) => return Ok(v),
                Err(e) => last = e,
            }
        }
        Err(DomainError::Conflict(format!("no swarm manager answered: {last}")))
    }

    /// One signed swarm command to a node, waiting for its answer.
    async fn rpc(&self, server: ServerId, principal: &str, op: &SwarmOp) -> Result<serde_json::Value, String> {
        let op_json = serde_json::to_string(op).map_err(|e| e.to_string())?;
        let command = Command::SwarmAction(SwarmAction { op_json });
        let (body, signed) = self.signer.issue(&server.to_string(), principal, command, self.clock.now(), QUICK);
        self.hub.enqueue(server, &body.operation_id, signed)?;
        let result = self.hub.wait(&body.operation_id, QUICK).await.ok_or("the manager did not answer in time")?;
        if !result.ok {
            return Err(result.error);
        }
        serde_json::from_str(&result.output).map_err(|e| format!("the manager's answer is not JSON: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quorum_counts_tolerated_failures() {
        assert_eq!(quorum(0).status, "single");
        assert_eq!(quorum(1), Quorum { managers: 1, tolerates: 0, status: "single" });
        assert_eq!(quorum(2), Quorum { managers: 2, tolerates: 0, status: "even" });
        assert_eq!(quorum(3), Quorum { managers: 3, tolerates: 1, status: "ok" });
        assert_eq!(quorum(4).tolerates, 1);
        assert_eq!(quorum(5), Quorum { managers: 5, tolerates: 2, status: "ok" });
    }
}
