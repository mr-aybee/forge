//! Which nodes an agent update reaches, and why not: every node with its
//! [`update_verdict`](crate::agent_update_policy::update_verdict), from the
//! node list and the self-update capability in each node's facts. One read
//! of each per call.

use std::sync::Arc;

use forge_core::model::Server;
use forge_core::ports::ServerRepository;
use forge_core::tags::FactsRepository;
use forge_core::{DomainResult, ServerId};

use crate::agent_update_policy::{NodeVerdict, SelfUpdateCapability, update_verdict};
use crate::install::AgentDist;

/// One node and what an update would do on it.
#[derive(Debug, Clone)]
pub struct NodeUpdate {
    pub server: Server,
    pub verdict: NodeVerdict,
}

#[derive(Debug, Clone)]
pub struct AgentUpdateOverview {
    servers: Arc<dyn ServerRepository>,
    facts: Arc<dyn FactsRepository>,
    dist: Arc<AgentDist>,
}

impl AgentUpdateOverview {
    #[must_use]
    pub fn new(servers: Arc<dyn ServerRepository>, facts: Arc<dyn FactsRepository>, dist: Arc<AgentDist>) -> Self {
        Self { servers, facts, dist }
    }

    /// The agent version on offer.
    #[must_use]
    pub fn target(&self) -> &str {
        self.dist.version()
    }

    /// Every node (revoked ones too; callers filter) with its verdict.
    ///
    /// # Errors
    /// When the node list or the facts cannot be read.
    pub async fn nodes(&self) -> DomainResult<Vec<NodeUpdate>> {
        let servers = self.servers.list_servers(None).await?;
        let facts = self.facts.all_facts().await?;

        Ok(servers
            .into_iter()
            .map(|s| {
                let cap = facts.get(&s.id).and_then(|(f, _)| capability(s.id, f));
                let has_build = self.dist.binary("linux", &s.arch, "forge-agent").is_some()
                    && self.dist.binary("linux", &s.arch, "forge-agent-helper").is_some();
                let verdict = update_verdict(&s, cap.as_ref(), self.target(), has_build);
                NodeUpdate { server: s, verdict }
            })
            .collect())
    }

    /// The nodes "update every outdated node" covers: online, outdated and
    /// able to update themselves (or not saying, older agents).
    ///
    /// # Errors
    /// When the node list or the facts cannot be read.
    pub async fn eligible_ids(&self) -> DomainResult<Vec<ServerId>> {
        Ok(self.nodes().await?.into_iter().filter(|n| n.verdict.verdict.eligible()).map(|n| n.server.id).collect())
    }
}

/// The node's reported capability. A value that does not parse is logged
/// and treated as unknown, which is safe: the node is then handled like an
/// older agent and its own refusal decides.
fn capability(id: ServerId, facts: &serde_json::Value) -> Option<SelfUpdateCapability> {
    let raw = facts.get("self_update").filter(|v| !v.is_null())?;
    match serde_json::from_value(raw.clone()) {
        Ok(c) => Some(c),
        Err(e) => {
            tracing::warn!(server = %id, error = %e, "unreadable self_update fact; treated as unknown");
            None
        }
    }
}
