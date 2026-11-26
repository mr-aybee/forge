//! The lookups behind `forge.stack.yaml` plan and apply (D-174): where a
//! document's stack lives and runs, which revision applying it saves and
//! whether a rollout is already running. Reads ports only; the API handler
//! authorizes, saves and audits.

use std::collections::BTreeMap;

use forge_catalog::stack_doc::StackDoc;
use forge_core::model::{Environment, JobFilter, JobStatus};
use forge_core::ports::{JobRepository, TenancyRepository};
use forge_core::stacks::{Stack, StackRepository};
use forge_core::{DomainError, DomainResult, JobId, ServerId, StackId, WorkspaceId};
use serde_json::json;

use crate::stacks::{Deps, Plan, open_env};

/// Jobs scanned per status when looking for a running rollout.
const ACTIVE_SCAN: u32 = 500;

/// The repositories the lookups read.
#[derive(Debug)]
pub struct StackApplyPorts<'a> {
    pub tenancy: &'a dyn TenancyRepository,
    pub stacks: &'a dyn StackRepository,
    pub jobs: &'a dyn JobRepository,
    pub deps: Deps<'a>,
}

impl StackApplyPorts<'_> {
    /// The environment whose slug is `slug` (`metadata.environment`).
    pub async fn environment(&self, ws: WorkspaceId, slug: &str) -> DomainResult<Environment> {
        self.tenancy
            .list_environments(ws, None)
            .await?
            .into_iter()
            .find(|n| crate::names::environment_slug(&n.name) == slug)
            .ok_or_else(|| DomainError::NotFound(format!("metadata.environment: no environment `{slug}`")))
    }

    /// The stack the document names in `environment`, if it exists.
    pub async fn existing(&self, environment: &Environment, name: &str) -> DomainResult<Option<Stack>> {
        self.stacks.stack_by_name(environment.id, name).await
    }

    /// The stack's default node: the first of `placement.nodes`, the first
    /// node of `placement.cluster`, or the existing stack's. A placement
    /// that names nothing usable is `Invalid`.
    pub async fn default_server(
        &self,
        ws: WorkspaceId,
        doc: &StackDoc,
        existing: Option<&Stack>,
    ) -> DomainResult<ServerId> {
        let p = &doc.spec.placement;

        if let Some(first) = p.nodes.first() {
            let all = self.deps.servers.list_servers(Some(ws)).await?;
            let s = crate::stacks::resolve_node(&self.deps, ws, &all, first)
                .await
                .map_err(|e| DomainError::Invalid(format!("spec.placement.nodes: {e}")))?;

            return Ok(s.id);
        }

        if let Some(c) = &p.cluster {
            let cluster = self
                .deps
                .clusters
                .list_clusters(ws)
                .await?
                .into_iter()
                .find(|x| x.name == *c)
                .ok_or_else(|| DomainError::Invalid(format!("spec.placement.cluster: no cluster named `{c}`")))?;
            let mut nodes: Vec<_> = self
                .deps
                .servers
                .list_servers(Some(ws))
                .await?
                .into_iter()
                .filter(|s| s.cluster_id == Some(cluster.id))
                .collect();
            nodes.sort_by(|a, b| a.hostname.cmp(&b.hostname));

            return nodes
                .first()
                .map(|s| s.id)
                .ok_or_else(|| DomainError::Invalid(format!("spec.placement.cluster: cluster `{c}` has no nodes")));
        }

        existing.map(|s| s.default_server).ok_or_else(|| {
            DomainError::Invalid("spec.placement: name the nodes or the cluster the stack runs on".into())
        })
    }

    /// Nodes the existing stack runs on that the new plan leaves out.
    pub async fn removes(&self, existing: Option<&Stack>, plan: &Plan) -> DomainResult<Vec<ServerId>> {
        let Some(s) = existing else { return Ok(Vec::new()) };

        Ok(self
            .stacks
            .list_nodes(s.id)
            .await?
            .into_iter()
            .map(|n| n.server_id)
            .filter(|id| plan.node(*id).is_none())
            .collect())
    }

    /// The revision an apply would save: the next one when the compose
    /// text or the `.env` variables change (as `update_stack` decides).
    pub async fn revision_next(
        &self,
        envelope: &forge_secrets::Envelope,
        existing: Option<&Stack>,
        compose: &str,
        env: &BTreeMap<String, String>,
    ) -> DomainResult<i64> {
        let Some(s) = existing else { return Ok(1) };
        let current = self.stacks.get_revision(s.id, s.revision).await?;
        let stored_env = open_env(envelope, s.id, s.env_sealed.as_deref()).map_err(DomainError::Internal)?;
        let same = current.is_some_and(|r| r.content == compose) && stored_env == *env;

        Ok(if same { s.revision } else { s.revision + 1 })
    }

    /// A queued or running `kind` rollout of `stack`, if any.
    pub async fn active_rollout(&self, kind: &str, stack: StackId) -> DomainResult<Option<JobId>> {
        for status in [JobStatus::Queued, JobStatus::Running] {
            let filter = JobFilter { workspace_id: None, status: Some(status), limit: ACTIVE_SCAN };
            let found = self
                .jobs
                .list_jobs(&filter)
                .await?
                .into_iter()
                .find(|j| j.kind == kind && j.payload["stack_id"] == json!(stack));

            if let Some(j) = found {
                return Ok(Some(j.id));
            }
        }

        Ok(None)
    }
}
