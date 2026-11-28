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
use forge_core::ports::{ClusterRepository, ServerRepository, ServiceRepository, TenancyRepository};
use forge_core::swarm::{ClusterSwarm, MemberRole, MemberState, SwarmMember, SwarmRepository};
use forge_core::{Clock, ClusterId, DomainError, DomainResult, EnvironmentId, ServerId};
use forge_proto::Command;
use forge_proto::messages::SwarmAction;
use forge_secrets::Envelope;
use serde::Serialize;

use crate::jobs::swarm::tokens_aad;
use crate::node_rpc::RpcError;

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
    pub tenancy: Arc<dyn TenancyRepository>,
    pub files: Arc<dyn forge_core::files::FileRepository>,
    pub deploys: Arc<dyn forge_core::deploy::DeploymentRepository>,
    pub services: Arc<dyn ServiceRepository>,
    pub rpc: crate::node_rpc::NodeRpc,
    pub envelope: Envelope,
    pub clock: Arc<dyn Clock>,
    /// Recent live node lists per cluster (see `live_nodes`).
    pub node_cache: std::sync::Mutex<std::collections::HashMap<ClusterId, NodeCacheEntry>>,
}

/// When a live node list was read, and what it was.
pub type NodeCacheEntry = (std::time::Instant, Result<Vec<SwarmNode>, String>);

/// How long a live node list is reused.
const NODE_CACHE: Duration = Duration::from_secs(5);

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

    /// The live node list from a manager, cached briefly per cluster: the
    /// console polls the view, and each read is a command to a root helper.
    async fn live_nodes(&self, cluster: ClusterId, principal: &str) -> Result<Vec<SwarmNode>, String> {
        if let Ok(cache) = self.node_cache.lock()
            && let Some((at, nodes)) = cache.get(&cluster)
            && at.elapsed() < NODE_CACHE
        {
            return nodes.clone();
        }

        let fresh = match self.on_manager(cluster, principal, &SwarmOp::Nodes {}).await {
            Ok(v) => serde_json::from_value(v).map_err(|e| e.to_string()),
            Err(e) => Err(e.to_string()),
        };

        if let Ok(mut cache) = self.node_cache.lock() {
            cache.retain(|_, (at, _)| at.elapsed() < NODE_CACHE);
            cache.insert(cluster, (std::time::Instant::now(), fresh.clone()));
        }

        fresh
    }

    /// The Swarm cluster an environment belongs to, if its cluster is one.
    pub async fn swarm_of_environment(&self, env: EnvironmentId) -> DomainResult<Option<ClusterId>> {
        let Some(cluster) = self.tenancy.get_environment(env).await?.cluster_id else { return Ok(None) };
        let c = self.clusters.get_cluster(cluster).await?;

        Ok((c.mode == ClusterMode::Swarm).then_some(c.id))
    }

    /// Refuses system services in a Swarm cluster: Swarm runs containers only.
    pub async fn check_runtime(&self, env: EnvironmentId, runtime: &str) -> DomainResult<()> {
        if runtime != "container" && self.swarm_of_environment(env).await?.is_some() {
            return Err(invalid("this environment is in a Docker Swarm cluster, which runs container services only"));
        }

        Ok(())
    }

    /// The binds among `sources` that need `stack.privileged`: a task may run
    /// on any node of the swarm, so a host folder is approved only when it
    /// lies inside an approved file root on every node of the swarm.
    pub async fn privileged_binds(&self, cluster: ClusterId, sources: &[String]) -> DomainResult<Vec<String>> {
        if sources.is_empty() {
            return Ok(Vec::new());
        }

        let members = self.swarms.list_members(cluster).await?;
        let mut per_node = Vec::new();

        for m in &members {
            per_node.push(crate::stacks::bind_roots(self.files.as_ref(), m.server_id).await);
        }

        Ok(sources
            .iter()
            .filter(|src| {
                per_node.is_empty()
                    || !per_node.iter().all(|roots| {
                        roots
                            .iter()
                            .filter_map(|r| forge_catalog::stacks::normalize_abs(r))
                            .any(|r| forge_catalog::stacks::within(src, &r))
                    })
            })
            .cloned()
            .collect())
    }

    /// The slot of `app` that serves now: the weighted one when it runs in
    /// slots (Blue-Green, Canary), else the unslotted service (`""`).
    pub async fn serving_slot(&self, app: forge_core::ServiceId) -> DomainResult<String> {
        let slots = self.deploys.list_slots(app).await?;
        Ok(slots.into_iter().filter(|s| s.weight > 0).max_by_key(|s| s.weight).map(|s| s.slot).unwrap_or_default())
    }

    /// How many managers of the cluster's swarm are joined.
    pub async fn view_managers(&self, cluster: ClusterId) -> DomainResult<usize> {
        Ok(self
            .swarms
            .list_members(cluster)
            .await?
            .iter()
            .filter(|m| m.role == MemberRole::Manager && m.state == MemberState::Joined)
            .count())
    }

    /// A joined manager of the cluster's swarm.
    pub async fn manager(&self, cluster: ClusterId) -> DomainResult<ServerId> {
        self.swarms
            .list_members(cluster)
            .await?
            .into_iter()
            .find(|m| m.role == MemberRole::Manager && m.state == MemberState::Joined)
            .map(|m| m.server_id)
            .ok_or_else(|| {
                DomainError::Conflict("the cluster has no swarm manager yet; add a node on the Swarm tab".into())
            })
    }

    /// Runs `op` on a manager of the cluster's swarm.
    pub async fn run(&self, cluster: ClusterId, principal: &str, op: &SwarmOp) -> DomainResult<serde_json::Value> {
        op.validate().map_err(invalid)?;
        self.on_manager(cluster, principal, op).await
    }

    pub async fn view(&self, cluster: ClusterId, principal: &str) -> DomainResult<SwarmView> {
        let c = self.clusters.get_cluster(cluster).await?;
        let mut members = Vec::new();

        for m in self.swarms.list_members(cluster).await? {
            let hostname = match self.servers.get_server(m.server_id).await {
                Ok(s) => s.display_name.unwrap_or(s.hostname),
                // A node row may be gone while its member row is cleaned up;
                // the console then shows the member without a name.
                Err(DomainError::NotFound(_)) => String::new(),
                Err(e) => return Err(e),
            };

            members.push(MemberView { member: m, hostname });
        }

        let managers = members
            .iter()
            .filter(|m| m.member.role == MemberRole::Manager && m.member.state == MemberState::Joined)
            .count();

        let (nodes, nodes_error) = if c.mode == ClusterMode::Swarm && managers > 0 {
            match self.live_nodes(cluster, principal).await {
                Ok(n) => (Some(n), None),
                Err(e) => (None, Some(e)),
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

    /// Refuses a membership change while another one runs in the cluster:
    /// joins and leaves happen one at a time, so quorum and the first-node
    /// rules are judged on settled state.
    async fn one_change_at_a_time(&self, cluster: ClusterId, server: ServerId) -> DomainResult<Vec<SwarmMember>> {
        let all = self.swarms.list_members(cluster).await?;
        if all.iter().any(|m| m.server_id != server && matches!(m.state, MemberState::Joining | MemberState::Leaving)) {
            return Err(DomainError::Conflict(
                "another node is joining or leaving the swarm; try again when it is done".into(),
            ));
        }

        Ok(all)
    }

    /// Checks that `server` may join `cluster`'s swarm now and marks it
    /// `joining`, so a second request is refused while the job runs. The
    /// first node claims the swarm row atomically: of two concurrent first
    /// joins, one gets a conflict instead of creating a second swarm.
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
            && m.state != MemberState::Failed
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

        self.one_change_at_a_time(cluster, server).await?;
        let claim = ClusterSwarm {
            cluster_id: cluster,
            swarm_id: String::new(),
            port: forge_catalog::swarm::DEFAULT_PORT,
            default_addr_pool: Vec::new(),
            autolock: false,
            tokens_sealed: None,
            founder: Some(server),
            updated_at: self.clock.now(),
        };

        let founding = if self.swarms.claim_swarm(&claim).await? {
            true
        } else {
            let current = self.swarms.get_swarm(cluster).await?;
            match current {
                Some(c) if c.pending() && c.founder == Some(server) => true,
                Some(c) if c.pending() => {
                    return Err(DomainError::Conflict(
                        "the swarm is still being created; add more nodes once the first node has joined".into(),
                    ));
                }
                _ => false,
            }
        };

        if founding && role != MemberRole::Manager {
            if self.swarms.get_swarm(cluster).await?.is_some_and(|c| c.pending() && c.founder == Some(server)) {
                self.swarms.delete_swarm(cluster).await?;
            }

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

    /// Checks that `server` may leave: it is a member, no other change runs,
    /// and it is not the last manager while other nodes remain. `force` is
    /// only for a node that failed (it cannot leave cleanly any more).
    pub async fn check_leave(&self, cluster: ClusterId, server: ServerId, force: bool) -> DomainResult<SwarmMember> {
        self.swarm_cluster(cluster).await?;
        let m = self.swarms.get_member(server).await?.filter(|m| m.cluster_id == cluster);
        let m = m.ok_or_else(|| DomainError::NotFound("the node is not in this swarm".into()))?;
        if matches!(m.state, MemberState::Leaving | MemberState::Joining) {
            return Err(DomainError::Conflict(format!("the node is already {}", m.state.as_str())));
        }

        if force && m.state != MemberState::Failed {
            return Err(invalid("force is only for a node whose join or leave failed; leave it normally"));
        }

        let all = self.one_change_at_a_time(cluster, server).await?;
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
    /// Records `replicas` in the service's definition, so the next deploy (a
    /// webhook, a redeploy) keeps the new scale instead of reverting it. A
    /// global service runs one task per matching node and cannot be scaled.
    /// One write; the scale job that follows only applies it.
    pub async fn set_replicas(&self, app: &forge_core::model::Service, replicas: u32) -> DomainResult<()> {
        let mut app = app.clone();
        let spec = app.spec.as_object_mut().ok_or_else(|| invalid("the service definition is not an object"))?;
        let swarm = spec.entry("swarm").or_insert_with(|| serde_json::json!({}));
        if swarm.get("mode").and_then(serde_json::Value::as_str) == Some("global") {
            return Err(invalid("a global service runs one task per node and cannot be scaled"));
        }

        match swarm.as_object_mut() {
            Some(o) => {
                o.insert("replicas".into(), replicas.into());
            }
            None => *swarm = serde_json::json!({ "replicas": replicas }),
        }

        app.updated_at = self.clock.now();

        self.services.put_service(&app).await
    }

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

        // Not atomic: the manager has rotated already. If storing fails, the
        // current tokens are read again and stored, so Forge never keeps the
        // old (now invalid) ones; if that fails too, the error says so.
        if let Err(first) = self.store_tokens(&mut s, &tokens).await {
            tracing::warn!(%cluster, error = %first, "storing rotated join tokens failed; reading them again");
            let current: JoinTokens =
                serde_json::from_value(self.on_manager(cluster, principal, &SwarmOp::JoinTokens {}).await?)
                    .map_err(DomainError::internal)?;
            self.store_tokens(&mut s, &current).await?;
        }

        Ok(())
    }

    async fn store_tokens(&self, s: &mut ClusterSwarm, tokens: &JoinTokens) -> DomainResult<()> {
        let raw = serde_json::to_vec(tokens).map_err(DomainError::internal)?;
        s.tokens_sealed = Some(self.envelope.seal(&raw, &tokens_aad(s.cluster_id)).map_err(DomainError::internal)?);
        s.updated_at = self.clock.now();

        self.swarms.put_swarm(s).await
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
                // Not connected: nothing ran, the next manager may.
                Err(RpcError::Unreachable(e)) => last = e,
                // A timeout may have run it; a failure is the swarm's answer.
                Err(e) => return Err(e.into()),
            }
        }

        Err(DomainError::Conflict(format!("no swarm manager answered: {last}")))
    }

    /// Runs `op` on one node of the swarm (not only managers).
    pub async fn run_on(&self, server: ServerId, principal: &str, op: &SwarmOp) -> DomainResult<serde_json::Value> {
        op.validate().map_err(invalid)?;
        self.rpc(server, principal, op).await.map_err(Into::into)
    }

    /// Runs a fabric operation on one node (conversion removes the cluster
    /// container network with it).
    pub async fn fabric_on(
        &self,
        server: ServerId,
        principal: &str,
        op: &forge_catalog::fabric::FabricOp,
    ) -> Result<serde_json::Value, RpcError> {
        op.validate().map_err(RpcError::BadAnswer)?;
        let op_json = serde_json::to_string(op).map_err(|e| RpcError::BadAnswer(e.to_string()))?;
        let cmd = Command::FabricAction(forge_proto::messages::FabricAction { op_json });

        self.rpc.call(server, principal, cmd, QUICK).await
    }

    /// One signed swarm command to a node, waiting for its answer.
    async fn rpc(&self, server: ServerId, principal: &str, op: &SwarmOp) -> Result<serde_json::Value, RpcError> {
        let op_json = serde_json::to_string(op).map_err(|e| RpcError::BadAnswer(e.to_string()))?;
        self.rpc.call(server, principal, Command::SwarmAction(SwarmAction { op_json }), QUICK).await
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
