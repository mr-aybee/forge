//! `swarm.member`: a node joins or leaves its Swarm cluster's Docker Swarm
//! (D-154).
//!
//! Join payload: `{"cluster_id", "server_id", "action": "join", "role",
//! "advertise_addr"?, "init"?}`. Steps:
//! 1. `prepare`: the node reports its swarm state. Without a running Docker
//!    Engine the step retries, so an install started next to the join (the
//!    API starts one) finishes first.
//! 2. `membership`: the first node of the cluster creates the swarm (it is
//!    always a manager) and Forge keeps the join tokens sealed; every later
//!    node joins through the managers Forge knows. A node that is already
//!    in this swarm is left as it is; one in another swarm is refused.
//! 3. `record`: the node's swarm id, role and address are stored.
//!
//! Leave payload: `{"cluster_id", "server_id", "action": "leave", "force"?}`.
//! Steps: `prepare` drains the node through another manager (best effort),
//! `membership` makes it leave and removes it from the node list, `record`
//! forgets it, and the swarm too once no node is left.
//!
//! Every step can run twice: init and join answer with the current state
//! when the node is already in the swarm, leaving twice finds nothing to
//! leave, and the rows are written with upserts. Join tokens never appear in
//! job state or output; they are sealed with the cluster id as context.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use forge_catalog::swarm::{Availability, JoinTokens, SwarmOp, SwarmStatus};
use forge_core::swarm::{ClusterSwarm, MemberRole, MemberState, SwarmMember, SwarmRepository};
use forge_core::{Action, ClusterId, ServerId};
use forge_jobs::{JobHandler, StepContext, StepError};
use forge_proto::Command;
use forge_proto::messages::SwarmAction;
use forge_secrets::Envelope;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::node_op::NodeOps;

pub const KIND: &str = "swarm.member";
const QUICK_WAIT: Duration = Duration::from_secs(60);
/// Init and join wait for the managers' consensus on the node.
const CONSENSUS_WAIT: Duration = Duration::from_secs(3 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemberAction {
    Join,
    Leave,
}

/// How a new swarm is created (first node only).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InitSettings {
    #[serde(default)]
    pub default_addr_pool: Vec<String>,
    #[serde(default)]
    pub autolock: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Payload {
    pub cluster_id: ClusterId,
    pub server_id: ServerId,
    pub action: MemberAction,
    #[serde(default = "worker")]
    pub role: MemberRole,
    #[serde(default)]
    pub advertise_addr: Option<String>,
    #[serde(default)]
    pub init: InitSettings,
    #[serde(default)]
    pub force: bool,
}

const fn worker() -> MemberRole {
    MemberRole::Worker
}

/// The context join tokens are sealed under.
#[must_use]
pub fn tokens_aad(cluster: ClusterId) -> String {
    format!("swarm-tokens:{cluster}")
}

#[derive(Debug)]
pub struct SwarmMemberJob {
    pub ops: NodeOps,
    pub swarms: Arc<dyn SwarmRepository>,
    pub envelope: Envelope,
}

fn retry(e: impl std::fmt::Display) -> StepError {
    StepError::Retryable(e.to_string())
}

fn fatal(e: impl std::fmt::Display) -> StepError {
    StepError::Fatal(e.to_string())
}

fn swarm_command(op: &SwarmOp) -> Result<Command, StepError> {
    let op_json = serde_json::to_string(op).map_err(fatal)?;
    Ok(Command::SwarmAction(SwarmAction { op_json }))
}

fn decode<T: serde::de::DeserializeOwned>(v: Value) -> Result<T, StepError> {
    serde_json::from_value(v).map_err(|e| fatal(format!("unexpected answer from the node: {e}")))
}

impl SwarmMemberJob {
    async fn op<T: serde::de::DeserializeOwned>(
        &self,
        ctx: &mut StepContext,
        server: ServerId,
        key: &str,
        op: &SwarmOp,
        wait: Duration,
    ) -> Result<T, StepError> {
        decode(self.ops.run(ctx, server, key, swarm_command(op)?, wait).await?)
    }

    /// Writes the member row; a fatal error is recorded on it first.
    async fn mark(&self, p: &Payload, state: MemberState, error: &str) -> Result<(), StepError> {
        let mut m = self.swarms.get_member(p.server_id).await.map_err(retry)?.unwrap_or(SwarmMember {
            server_id: p.server_id,
            cluster_id: p.cluster_id,
            node_id: String::new(),
            role: p.role,
            advertise_addr: p.advertise_addr.clone().unwrap_or_default(),
            manager_addr: String::new(),
            state,
            error: String::new(),
            updated_at: self.ops.clock.now(),
        });
        m.state = state;
        error.clone_into(&mut m.error);
        m.updated_at = self.ops.clock.now();
        self.swarms.put_member(&m).await.map_err(retry)
    }

    async fn failed(&self, p: &Payload, e: StepError) -> StepError {
        if let StepError::Fatal(msg) = &e
            && let Err(err) = self.mark(p, MemberState::Failed, msg).await
        {
            tracing::warn!(server = %p.server_id, error = ?err, "could not record the swarm failure");
        }
        e
    }

    /// Another node of the cluster that is a joined manager.
    async fn other_manager(&self, p: &Payload) -> Result<Option<SwarmMember>, StepError> {
        Ok(self
            .swarms
            .list_members(p.cluster_id)
            .await
            .map_err(retry)?
            .into_iter()
            .find(|m| m.server_id != p.server_id && m.role == MemberRole::Manager && m.state == MemberState::Joined))
    }

    async fn join(&self, ctx: &mut StepContext, p: &Payload) -> Result<(), StepError> {
        let status: SwarmStatus = serde_json::from_value(ctx.state_value("status").cloned().unwrap_or_default())
            .map_err(|e| fatal(format!("lost the node's swarm state: {e}")))?;
        let swarm = self.swarms.get_swarm(p.cluster_id).await.map_err(retry)?;
        let manager = self.other_manager(p).await?;
        if status.state == "active" {
            return match (&swarm, &status.swarm_id) {
                (Some(s), Some(id)) if &s.swarm_id == id => Ok(()),
                (None, _) if manager.is_none() => self.adopt(ctx, p).await,
                _ => Err(fatal("the node is already in another Docker Swarm; make it leave that swarm first")),
            };
        }
        match (swarm, manager) {
            (Some(s), Some(m)) => {
                let tokens = self.open_tokens(&s)?;
                let token = if p.role == MemberRole::Manager { tokens.manager } else { tokens.worker };
                let mut remote: Vec<String> = self
                    .swarms
                    .list_members(p.cluster_id)
                    .await
                    .map_err(retry)?
                    .into_iter()
                    .filter(|x| x.role == MemberRole::Manager && x.state == MemberState::Joined)
                    .map(|x| x.manager_addr)
                    .filter(|a| !a.is_empty())
                    .collect();
                if remote.is_empty() {
                    remote.push(m.manager_addr);
                }
                remote.truncate(7);
                let op =
                    SwarmOp::Join { remote_addrs: remote, join_token: token, advertise_addr: p.advertise_addr.clone() };
                let _: SwarmStatus = self.op(ctx, p.server_id, "join_op", &op, CONSENSUS_WAIT).await?;
                Ok(())
            }
            (None, None) => {
                let op = SwarmOp::Init {
                    advertise_addr: p.advertise_addr.clone(),
                    port: forge_catalog::swarm::DEFAULT_PORT,
                    default_addr_pool: p.init.default_addr_pool.clone(),
                    autolock: p.init.autolock,
                };
                let _: SwarmStatus = self.op(ctx, p.server_id, "init_op", &op, CONSENSUS_WAIT).await?;
                self.adopt(ctx, p).await
            }
            (Some(_), None) => {
                Err(fatal("no manager of this swarm is reachable through Forge; re-join a manager first"))
            }
            (None, Some(_)) => Err(fatal("the swarm record is missing; remove the cluster's nodes and start again")),
        }
    }

    /// Records the swarm this node manages (just created, or found running
    /// on the cluster's first node) with its join tokens sealed.
    async fn adopt(&self, ctx: &mut StepContext, p: &Payload) -> Result<(), StepError> {
        let status: SwarmStatus = self.op(ctx, p.server_id, "adopt_status_op", &SwarmOp::Status {}, QUICK_WAIT).await?;
        if !status.manager {
            return Err(fatal("the first node of a swarm must be a manager"));
        }
        let tokens: JoinTokens = self.op(ctx, p.server_id, "tokens_op", &SwarmOp::JoinTokens {}, QUICK_WAIT).await?;
        let sealed = self
            .envelope
            .seal(&serde_json::to_vec(&tokens).map_err(fatal)?, &tokens_aad(p.cluster_id))
            .map_err(fatal)?;
        let swarm = ClusterSwarm {
            cluster_id: p.cluster_id,
            swarm_id: status.swarm_id.unwrap_or_default(),
            port: forge_catalog::swarm::DEFAULT_PORT,
            default_addr_pool: p.init.default_addr_pool.clone(),
            autolock: p.init.autolock,
            tokens_sealed: Some(sealed),
            updated_at: self.ops.clock.now(),
        };
        self.swarms.put_swarm(&swarm).await.map_err(retry)
    }

    fn open_tokens(&self, s: &ClusterSwarm) -> Result<JoinTokens, StepError> {
        let sealed = s.tokens_sealed.as_deref().ok_or_else(|| fatal("the swarm's join tokens are missing"))?;
        let raw = self.envelope.open(sealed, &tokens_aad(s.cluster_id)).map_err(fatal)?;
        serde_json::from_slice(&raw).map_err(fatal)
    }

    async fn record_join(&self, ctx: &mut StepContext, p: &Payload) -> Result<(), StepError> {
        let status: SwarmStatus = self.op(ctx, p.server_id, "record_op", &SwarmOp::Status {}, QUICK_WAIT).await?;
        if status.state != "active" {
            return Err(fatal(format!("the node did not join: its swarm state is {}", status.state)));
        }
        let role = if status.manager { MemberRole::Manager } else { MemberRole::Worker };
        let manager_addr = match (&status.node_addr, role) {
            (Some(a), MemberRole::Manager) => format!("{a}:{}", forge_catalog::swarm::DEFAULT_PORT),
            _ => String::new(),
        };
        let m = SwarmMember {
            server_id: p.server_id,
            cluster_id: p.cluster_id,
            node_id: status.node_id.unwrap_or_default(),
            role,
            advertise_addr: p.advertise_addr.clone().unwrap_or_default(),
            manager_addr,
            state: MemberState::Joined,
            error: String::new(),
            updated_at: self.ops.clock.now(),
        };
        self.swarms.put_member(&m).await.map_err(retry)
    }

    async fn drain(&self, ctx: &mut StepContext, p: &Payload) -> Result<(), StepError> {
        let (Some(member), Some(manager)) =
            (self.swarms.get_member(p.server_id).await.map_err(retry)?, self.other_manager(p).await?)
        else {
            return Ok(());
        };
        if member.node_id.is_empty() {
            return Ok(());
        }
        let op = SwarmOp::NodeUpdate {
            node_id: member.node_id,
            role: None,
            availability: Some(Availability::Drain),
            labels: None,
        };
        // Best effort: a node that cannot be drained still leaves.
        if let Err(e) = self.ops.run(ctx, manager.server_id, "drain_op", swarm_command(&op)?, QUICK_WAIT).await {
            tracing::warn!(server = %p.server_id, error = ?e, "could not drain the node before it leaves the swarm");
        }
        Ok(())
    }

    async fn leave(&self, ctx: &mut StepContext, p: &Payload) -> Result<(), StepError> {
        let member = self.swarms.get_member(p.server_id).await.map_err(retry)?;
        let manager = self.other_manager(p).await?;
        let last_manager = member.as_ref().is_some_and(|m| m.role == MemberRole::Manager) && manager.is_none();
        let leave = self
            .ops
            .run(
                ctx,
                p.server_id,
                "leave_op",
                swarm_command(&SwarmOp::Leave { force: last_manager || p.force })?,
                CONSENSUS_WAIT,
            )
            .await;
        if let Err(e) = leave {
            // A node that is gone can still be removed from the list with force.
            if !p.force {
                return Err(e);
            }
        }
        if let (Some(m), Some(mgr)) = (member, manager)
            && !m.node_id.is_empty()
        {
            let op = SwarmOp::NodeRemove { node_id: m.node_id, force: true };
            self.ops.run(ctx, mgr.server_id, "remove_op", swarm_command(&op)?, QUICK_WAIT).await?;
        }
        Ok(())
    }

    async fn forget(&self, p: &Payload) -> Result<(), StepError> {
        self.swarms.delete_member(p.server_id).await.map_err(retry)?;
        if self.swarms.list_members(p.cluster_id).await.map_err(retry)?.is_empty() {
            self.swarms.delete_swarm(p.cluster_id).await.map_err(retry)?;
        }
        Ok(())
    }

    async fn step(&self, index: usize, ctx: &mut StepContext, p: &Payload) -> Result<(), StepError> {
        match (p.action, index) {
            (MemberAction::Join, 0) => {
                self.mark(p, MemberState::Joining, "").await?;
                let status: SwarmStatus =
                    self.op(ctx, p.server_id, "status_op", &SwarmOp::Status {}, QUICK_WAIT).await?;
                if !status.docker {
                    return Err(retry("waiting for Docker Engine on the node"));
                }
                ctx.set_state("status", serde_json::to_value(&status).map_err(fatal)?);
                Ok(())
            }
            (MemberAction::Join, 1) => self.join(ctx, p).await,
            (MemberAction::Join, 2) => self.record_join(ctx, p).await,
            (MemberAction::Leave, 0) => {
                self.mark(p, MemberState::Leaving, "").await?;
                self.drain(ctx, p).await
            }
            (MemberAction::Leave, 1) => self.leave(ctx, p).await,
            (MemberAction::Leave, 2) => self.forget(p).await,
            _ => Ok(()),
        }
    }
}

#[async_trait]
impl JobHandler for SwarmMemberJob {
    fn kind(&self) -> &'static str {
        KIND
    }

    fn steps(&self) -> &'static [&'static str] {
        &["prepare", "membership", "record"]
    }

    fn required_action(&self, _payload: &Value) -> Action {
        Action::SwarmManage
    }

    async fn run_step(&self, index: usize, ctx: &mut StepContext) -> Result<(), StepError> {
        let p: Payload =
            serde_json::from_value(ctx.job.payload.clone()).map_err(|e| fatal(format!("bad payload: {e}")))?;
        match self.step(index, ctx, &p).await {
            Ok(()) => Ok(()),
            Err(e) => Err(self.failed(&p, e).await),
        }
    }
}
