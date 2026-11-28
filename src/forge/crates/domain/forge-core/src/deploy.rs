//! Releases and deployments of services (#73).
//!
//! A [`Release`] is an immutable snapshot of a service: its spec, its
//! environment (secret values stay sealed), the artifact it runs and the
//! rollout policy. A [`Deployment`] rolls one release out to the service's
//! nodes, step by step, under a lock: one active deployment per service.
//! [`SlotRecord`]s are the copies running on each node and their share of
//! traffic. The document format, policy and planner live in
//! `forge_catalog::deploy`; here they are carried as JSON.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::access::Principal;
use crate::clock::Timestamp;
use crate::error::DomainResult;
use crate::ids::{DeploymentId, JobId, ReleaseId, ServerId, ServiceId, WorkspaceId};
use crate::model::EnvVar;

/// An immutable snapshot of a service.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Release {
    pub id: ReleaseId,
    pub service_id: ServiceId,
    pub workspace_id: WorkspaceId,
    /// 1, 2, 3, … per service.
    pub number: i64,
    /// The service's runtime fields (`forge_catalog::AppSpec` without env).
    pub spec: serde_json::Value,
    /// The environment; secret values sealed, never returned.
    pub env: Vec<EnvVar>,
    /// `forge_catalog::deploy::ArtifactRef`.
    pub artifact: serde_json::Value,
    /// `forge_catalog::deploy::RolloutPolicy`.
    pub policy: serde_json::Value,
    /// The deployment document as submitted (YAML), if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Lower-case hex SHA-256 of `source`.
    pub source_sha256: Option<String>,
    pub description: String,
    pub created_by: Principal,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeploymentState {
    /// Created; the job has not started.
    Pending,
    Running,
    /// Held by someone; resumes on request.
    Paused,
    /// Waiting for an approval.
    AwaitingApproval,
    Succeeded,
    Failed,
    /// Stopped on request.
    Aborted,
    /// Failed, then the previous release was put back.
    RolledBack,
}

impl DeploymentState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::AwaitingApproval => "awaiting_approval",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Aborted => "aborted",
            Self::RolledBack => "rolled_back",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "pending" => Self::Pending,
            "running" => Self::Running,
            "paused" => Self::Paused,
            "awaiting_approval" => Self::AwaitingApproval,
            "succeeded" => Self::Succeeded,
            "failed" => Self::Failed,
            "aborted" => Self::Aborted,
            "rolled_back" => Self::RolledBack,
            _ => return None,
        })
    }

    /// Still holds the service's deployment lock.
    #[must_use]
    pub const fn active(self) -> bool {
        matches!(self, Self::Pending | Self::Running | Self::Paused | Self::AwaitingApproval)
    }
}

/// Why a deployment was made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeploymentKind {
    /// A new or chosen release.
    Apply,
    /// Back to an earlier release, on request.
    Rollback,
    /// Back to the previous release after a failed rollout.
    AutoRollback,
}

impl DeploymentKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Apply => "apply",
            Self::Rollback => "rollback",
            Self::AutoRollback => "auto_rollback",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "apply" => Self::Apply,
            "rollback" => Self::Rollback,
            "auto_rollback" => Self::AutoRollback,
            _ => return None,
        })
    }
}

/// What someone asked a running deployment to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeploymentRequest {
    Pause,
    Resume,
    Abort,
    Approve,
    Reject,
}

impl DeploymentRequest {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pause => "pause",
            Self::Resume => "resume",
            Self::Abort => "abort",
            Self::Approve => "approve",
            Self::Reject => "reject",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "pause" => Self::Pause,
            "resume" => Self::Resume,
            "abort" => Self::Abort,
            "approve" => Self::Approve,
            "reject" => Self::Reject,
            _ => return None,
        })
    }
}

/// One rollout of a release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Deployment {
    pub id: DeploymentId,
    pub service_id: ServiceId,
    pub workspace_id: WorkspaceId,
    pub release_id: ReleaseId,
    pub release_number: i64,
    /// The release that served when the deployment started (0: none).
    pub from_release: i64,
    pub kind: DeploymentKind,
    /// `forge_catalog::deploy::Strategy` name.
    pub strategy: String,
    pub state: DeploymentState,
    /// The last request (pause, abort, approve, …) not yet acted on.
    pub request: Option<DeploymentRequest>,
    pub job_id: Option<JobId>,
    /// `forge_catalog::deploy::Plan`.
    pub plan: serde_json::Value,
    /// Steps of the plan done so far.
    pub step: i64,
    pub error: Option<String>,
    pub created_by: Principal,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub finished_at: Option<Timestamp>,
}

/// Something that happened during a deployment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeploymentEvent {
    pub deployment_id: DeploymentId,
    pub seq: i64,
    pub at: Timestamp,
    /// `info`, `warn` or `error`.
    pub level: String,
    pub server_id: Option<ServerId>,
    pub message: String,
}

/// One copy of a service on a node: the unslotted instance (`slot` empty)
/// or slot `a`/`b`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlotRecord {
    pub service_id: ServiceId,
    pub server_id: ServerId,
    pub slot: String,
    pub release_number: i64,
    /// Share of the node's traffic, 0–100 (0: out of traffic).
    pub weight: u32,
    /// `starting`, `ready`, `failed`, `stopped`.
    pub state: String,
    /// Service port → the port this copy listens on, as JSON `{"8080": 21000}`.
    pub ports: serde_json::Value,
    pub updated_at: Timestamp,
}

/// Releases of services.
#[async_trait]
pub trait ReleaseRepository: Send + Sync + std::fmt::Debug {
    /// Stores the release under the next number (atomic); returns the number.
    async fn create_release(&self, release: &Release) -> DomainResult<i64>;
    async fn get_release(&self, id: ReleaseId) -> DomainResult<Option<Release>>;
    async fn release_by_number(&self, app: ServiceId, number: i64) -> DomainResult<Option<Release>>;
    /// Newest first, without `source`, at most `limit`.
    async fn list_releases(&self, app: ServiceId, limit: u32) -> DomainResult<Vec<Release>>;
    /// Records the image digest the first node resolved a tag to; only
    /// when none is recorded yet (a release is otherwise immutable).
    async fn set_release_artifact(&self, id: ReleaseId, artifact: &serde_json::Value) -> DomainResult<()>;
    /// Deletes releases of `app` older than the newest `keep`, never `except`.
    async fn prune_releases(&self, app: ServiceId, keep: u32, except: &[i64]) -> DomainResult<u64>;
}

/// Deployments, their events and the copies on each node.
#[async_trait]
pub trait DeploymentRepository: Send + Sync + std::fmt::Debug {
    /// Inserts the deployment. `Conflict` when the service already has an
    /// active one (the lock).
    async fn create_deployment(&self, d: &Deployment) -> DomainResult<()>;
    /// Writes state, job, plan, step, error and times; never `request`,
    /// which only [`Self::request_deployment`] and [`Self::clear_request`] set.
    async fn put_deployment(&self, d: &Deployment) -> DomainResult<()>;
    async fn get_deployment(&self, id: DeploymentId) -> DomainResult<Option<Deployment>>;
    /// Newest first, at most `limit`.
    async fn list_deployments(&self, app: ServiceId, limit: u32) -> DomainResult<Vec<Deployment>>;
    async fn active_deployment(&self, app: ServiceId) -> DomainResult<Option<Deployment>>;
    /// Sets `request` (pause, abort, …) without touching anything else.
    async fn request_deployment(
        &self,
        id: DeploymentId,
        request: DeploymentRequest,
        now: Timestamp,
    ) -> DomainResult<()>;
    /// Clears a request the job acted on.
    async fn clear_request(&self, id: DeploymentId) -> DomainResult<()>;
    async fn add_event(&self, e: &DeploymentEvent) -> DomainResult<()>;
    /// Oldest first, after `after_seq`, at most `limit`.
    async fn list_events(&self, id: DeploymentId, after_seq: i64, limit: u32) -> DomainResult<Vec<DeploymentEvent>>;
    async fn put_slot(&self, s: &SlotRecord) -> DomainResult<()>;
    async fn delete_slot(&self, app: ServiceId, server: ServerId, slot: &str) -> DomainResult<()>;
    /// Replaces every slot row of `app` with `rows`, in one transaction (a
    /// half-written routing change never reaches the edge).
    async fn replace_slots(&self, app: ServiceId, rows: &[SlotRecord]) -> DomainResult<()>;
    async fn list_slots(&self, app: ServiceId) -> DomainResult<Vec<SlotRecord>>;
    /// Copies of several services (the ingress resolves many at once).
    async fn list_slots_of(&self, apps: &[ServiceId]) -> DomainResult<Vec<SlotRecord>>;
    /// The host ports slot `slot` of `app` uses, allocating `count` from
    /// `range` on first use (unique across services).
    async fn slot_ports(&self, app: ServiceId, slot: &str, count: u32, range: (u16, u16)) -> DomainResult<Vec<u16>>;
    /// The service's last succeeded release number (0: none).
    async fn current_release(&self, app: ServiceId) -> DomainResult<i64>;
}
