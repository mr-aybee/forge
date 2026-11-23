//! Compose stacks (issue #7): a compose document in an environment, kept as
//! revisions, deployed per node. The compose policy, placement and split
//! live in `forge_catalog::stacks` (shared with the node helper).

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::access::Principal;
use crate::clock::Timestamp;
use crate::error::DomainResult;
use crate::ids::{EnvironmentId, ServerId, StackId, WorkspaceId};

/// How a stack was made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StackKind {
    /// Uploaded or pasted compose file.
    Compose,
    /// Made by **Create container** (one service; the compose is generated).
    Container,
}

impl StackKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Compose => "compose",
            Self::Container => "container",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "compose" => Some(Self::Compose),
            "container" => Some(Self::Container),
            _ => None,
        }
    }
}

/// A stack. Its content lives in [`StackRevision`]s.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stack {
    pub id: StackId,
    pub workspace_id: WorkspaceId,
    pub environment_id: EnvironmentId,
    /// `[a-z0-9][a-z0-9_-]{0,62}`, unique in the environment.
    pub name: String,
    pub kind: StackKind,
    /// The node the stack was created from: services without placement run here.
    pub default_server: ServerId,
    /// Folder on each node; `None` for `/opt/forge/stacks/<environment>/<stack>`.
    pub dir: Option<String>,
    /// Latest revision number (1-based).
    pub revision: i64,
    /// Revision currently deployed (0: never).
    pub deployed_revision: i64,
    /// `.env` variables sealed with the vault envelope (JSON map), storage-internal.
    #[serde(default, skip_serializing)]
    pub env_sealed: Option<String>,
    /// Names of the `.env` variables (values are never returned).
    #[serde(default)]
    pub env_keys: Vec<String>,
    pub created_by: Principal,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

/// One saved version of a stack's compose document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StackRevision {
    pub stack_id: StackId,
    pub revision: i64,
    /// The compose document as uploaded (YAML).
    pub content: String,
    /// Lower-case hex SHA-256 of `content`.
    pub sha256: String,
    pub note: Option<String>,
    pub author: Principal,
    pub created_at: Timestamp,
}

/// A stack's part on one node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StackNode {
    pub stack_id: StackId,
    pub server_id: ServerId,
    /// Services of this node's part.
    pub services: Vec<String>,
    /// Services started last time (the ticked ones and their dependencies).
    pub selected: Vec<String>,
    pub revision: i64,
    /// `pending`, `deploying`, `deployed`, `failed`, `removing`.
    pub status: String,
    /// Last `docker compose ps` (normalized), as JSON.
    pub state: serde_json::Value,
    pub last_job_id: Option<crate::ids::JobId>,
    pub last_error: Option<String>,
    pub updated_at: Timestamp,
}

/// Stacks, their revisions and per-node parts.
#[async_trait]
pub trait StackRepository: Send + Sync + std::fmt::Debug {
    /// Inserts a stack and its first revision.
    async fn create_stack(&self, stack: &Stack, first: &StackRevision) -> DomainResult<()>;
    /// Updates the stack row (name, dir, revision counters, env).
    async fn put_stack(&self, stack: &Stack) -> DomainResult<()>;
    /// Claims an edit of the stack: sets `updated_at` to `now` only when it
    /// still is `seen`. `false`: someone changed the stack since it was read
    /// (optimistic concurrency for `PUT /stacks/{id}`).
    async fn claim_stack_edit(&self, id: StackId, seen: Timestamp, now: Timestamp) -> DomainResult<bool>;
    /// Records the revision a deploy put on its nodes, and nothing else.
    async fn set_deployed_revision(&self, id: StackId, revision: i64) -> DomainResult<()>;
    async fn get_stack(&self, id: StackId) -> DomainResult<Option<Stack>>;
    async fn stack_by_name(&self, environment: EnvironmentId, name: &str) -> DomainResult<Option<Stack>>;
    async fn list_stacks(&self, environment: Option<EnvironmentId>) -> DomainResult<Vec<Stack>>;
    /// Deletes the stack, its revisions and node parts.
    async fn delete_stack(&self, id: StackId) -> DomainResult<()>;
    /// Adds the next revision and bumps `stack.revision` atomically; returns its number.
    async fn add_revision(&self, rev: &StackRevision) -> DomainResult<i64>;
    async fn get_revision(&self, stack: StackId, revision: i64) -> DomainResult<Option<StackRevision>>;
    /// Revisions newest first, without content.
    async fn list_revisions(&self, stack: StackId) -> DomainResult<Vec<StackRevision>>;
    async fn put_node(&self, node: &StackNode) -> DomainResult<()>;
    async fn list_nodes(&self, stack: StackId) -> DomainResult<Vec<StackNode>>;
    /// Parts on one server (the node's Containers view).
    async fn nodes_on_server(&self, server: ServerId) -> DomainResult<Vec<StackNode>>;
    async fn delete_node(&self, stack: StackId, server: ServerId) -> DomainResult<()>;
}
