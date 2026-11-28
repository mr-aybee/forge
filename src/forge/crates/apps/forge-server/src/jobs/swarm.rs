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
        if let StepError::Fatal(msg) = &e {
            if let Err(err) = self.mark(p, MemberState::Failed, msg).await {
                tracing::warn!(server = %p.server_id, error = ?err, "could not record the swarm failure");
            }

            // A founder that failed gives up its claim, so another node can
            // create the swarm.
            if p.action == MemberAction::Join
                && let Ok(Some(s)) = self.swarms.get_swarm(p.cluster_id).await
                && s.pending()
                && s.founder == Some(p.server_id)
                && let Err(err) = self.swarms.delete_swarm(p.cluster_id).await
            {
                tracing::warn!(cluster = %p.cluster_id, error = ?err, "could not release the swarm claim");
            }
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

    async fn swarm_row(&self, p: &Payload) -> Result<ClusterSwarm, StepError> {
        self.swarms
            .get_swarm(p.cluster_id)
            .await
            .map_err(retry)?
            .ok_or_else(|| fatal("the cluster's swarm record is gone; add the node again"))
    }

    /// Step 1 of a join: what the node is in now. A node already in a swarm
    /// that this cluster has not recorded is refused: Forge never adopts a
    /// swarm made outside it or by another cluster.
    async fn prepare_join(&self, ctx: &mut StepContext, p: &Payload) -> Result<(), StepError> {
        let status: SwarmStatus = self.op(ctx, p.server_id, "status_op", &SwarmOp::Status {}, QUICK_WAIT).await?;
        if !status.docker {
            return Err(retry("waiting for Docker Engine on the node"));
        }

        if status.state == "active" {
            let swarm = self.swarm_row(p).await?;
            if swarm.pending() || status.swarm_id.as_deref() != Some(swarm.swarm_id.as_str()) {
                return Err(fatal(
                    "the node is already in a Docker Swarm this cluster did not create; make it leave that swarm first",
                ));
            }
        }

        Ok(())
    }

    async fn join(&self, ctx: &mut StepContext, p: &Payload) -> Result<(), StepError> {
        let swarm = self.swarm_row(p).await?;
        if swarm.pending() {
            if swarm.founder != Some(p.server_id) {
                return Err(retry("another node is creating the swarm"));
            }

            // Init answers with the current state when the node already made
            // the swarm (a retried step), so this is safe to run again.
            let op = SwarmOp::Init {
                advertise_addr: p.advertise_addr.clone(),
                port: forge_catalog::swarm::DEFAULT_PORT,
                default_addr_pool: p.init.default_addr_pool.clone(),
                autolock: p.init.autolock,
            };
            let _: SwarmStatus = self.op(ctx, p.server_id, "init_op", &op, CONSENSUS_WAIT).await?;

            return self.adopt(ctx, p).await;
        }

        let status: SwarmStatus = self.op(ctx, p.server_id, "join_status_op", &SwarmOp::Status {}, QUICK_WAIT).await?;
        if status.state == "active" && status.swarm_id.as_deref() == Some(swarm.swarm_id.as_str()) {
            return Ok(());
        }

        let tokens = self.open_tokens(&swarm)?;
        let token = if p.role == MemberRole::Manager { tokens.manager } else { tokens.worker };
        let mut remote: Vec<String> = self
            .swarms
            .list_members(p.cluster_id)
            .await
            .map_err(retry)?
            .into_iter()
            .filter(|x| x.role == MemberRole::Manager && x.state == MemberState::Joined && !x.manager_addr.is_empty())
            .map(|x| x.manager_addr)
            .collect();

        if remote.is_empty() {
            return Err(fatal("no manager of this swarm is reachable through Forge; re-join a manager first"));
        }

        remote.truncate(7);
        let op = SwarmOp::Join { remote_addrs: remote, join_token: token, advertise_addr: p.advertise_addr.clone() };
        let _: SwarmStatus = self.op(ctx, p.server_id, "join_op", &op, CONSENSUS_WAIT).await?;

        Ok(())
    }

    /// Records the swarm the founder just created, with its join tokens
    /// sealed. A swarm id another cluster records is refused.
    async fn adopt(&self, ctx: &mut StepContext, p: &Payload) -> Result<(), StepError> {
        let status: SwarmStatus = self.op(ctx, p.server_id, "adopt_status_op", &SwarmOp::Status {}, QUICK_WAIT).await?;
        let swarm_id = status
            .swarm_id
            .filter(|_| status.manager)
            .ok_or_else(|| fatal("the node did not become a swarm manager"))?;

        if let Some(other) = self.swarms.cluster_of_swarm(&swarm_id).await.map_err(retry)?
            && other != p.cluster_id
        {
            return Err(fatal("this Docker Swarm belongs to another cluster"));
        }

        let tokens: JoinTokens = self.op(ctx, p.server_id, "tokens_op", &SwarmOp::JoinTokens {}, QUICK_WAIT).await?;
        let sealed = self
            .envelope
            .seal(&serde_json::to_vec(&tokens).map_err(fatal)?, &tokens_aad(p.cluster_id))
            .map_err(fatal)?;
        let swarm = ClusterSwarm {
            cluster_id: p.cluster_id,
            swarm_id,
            port: forge_catalog::swarm::DEFAULT_PORT,
            default_addr_pool: p.init.default_addr_pool.clone(),
            autolock: p.init.autolock,
            tokens_sealed: Some(sealed),
            founder: None,
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

    /// Step 1 of a leave: through another manager, a manager is demoted first
    /// (so it leaves without force and the swarm keeps its quorum), and the
    /// node is drained so its tasks move.
    async fn drain(&self, ctx: &mut StepContext, p: &Payload) -> Result<(), StepError> {
        let (Some(member), Some(manager)) =
            (self.swarms.get_member(p.server_id).await.map_err(retry)?, self.other_manager(p).await?)
        else {
            return Ok(());
        };

        if member.node_id.is_empty() {
            return Ok(());
        }

        let role = (member.role == MemberRole::Manager).then_some(forge_catalog::swarm::SwarmRole::Worker);
        let op = SwarmOp::NodeUpdate {
            node_id: member.node_id,
            role,
            availability: Some(Availability::Drain),
            labels: None,
        };

        match self.ops.run(ctx, manager.server_id, "drain_op", swarm_command(&op)?, QUICK_WAIT).await {
            Ok(_) => Ok(()),
            // A failed node cannot be drained; it is removed with force.
            Err(e) if p.force => {
                tracing::warn!(server = %p.server_id, error = ?e, "could not drain the node before it leaves the swarm");
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    async fn leave(&self, ctx: &mut StepContext, p: &Payload) -> Result<(), StepError> {
        let member = self.swarms.get_member(p.server_id).await.map_err(retry)?;
        let manager = self.other_manager(p).await?;

        // Force only for a failed node, or for the very last node (which
        // dissolves the swarm). A demoted manager leaves without it.
        let others = self
            .swarms
            .list_members(p.cluster_id)
            .await
            .map_err(retry)?
            .iter()
            .filter(|m| m.server_id != p.server_id)
            .count();
        let force = p.force || others == 0;
        let leave =
            self.ops.run(ctx, p.server_id, "leave_op", swarm_command(&SwarmOp::Leave { force })?, CONSENSUS_WAIT).await;

        if let Err(e) = leave
            && !p.force
        {
            return Err(e);
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
                self.prepare_join(ctx, p).await
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
