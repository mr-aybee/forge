//! Domain entities. These are plain data; behaviour lives in services.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::access::{AuthzContext, EntityRef, Principal, Role};
use crate::clock::Timestamp;
use crate::ids::*;

// ---------------------------------------------------------------- tenancy

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub name: String,
    pub slug: String,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tenant {
    pub id: TenantId,
    pub workspace_id: WorkspaceId,
    pub name: String,
    pub created_at: Timestamp,
}

/// A project: the product ("shop") whose environments are environments
/// (D-102). The UI calls it an *Service*; the API and database keep
/// `projects` because `services` already names the services.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub id: ProjectId,
    pub workspace_id: WorkspaceId,
    /// Lower-case letters, digits and dashes; unique per workspace.
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub created_at: Timestamp,
}

/// An environment: a named group of services inside one cluster. Names are
/// unique per cluster (D-056). To the user it is an *environment* of an app
/// group (D-102).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Environment {
    pub id: EnvironmentId,
    pub workspace_id: WorkspaceId,
    /// The project (product) this environment belongs to. Required for new
    /// environments; `None` only for environments created before projects
    /// existed, until they are attached (D-102).
    #[serde(default)]
    pub project_id: Option<ProjectId>,
    /// The cluster this environment (and its services) belongs to. New
    /// environments always have one; `None` only for environments created before
    /// clusters were required, until they are moved into one.
    #[serde(default)]
    pub cluster_id: Option<ClusterId>,
    pub name: String,
    /// Protected environments (typically prod) are subject to stricter guardrails.
    pub protected: bool,
    pub created_at: Timestamp,
}

/// How a cluster orchestrates its workloads (D-154). Chosen when the cluster
/// is created; it cannot be changed afterwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClusterMode {
    /// Forge's own orchestration: system services and containers.
    #[default]
    Default,
    /// Docker Swarm schedules containers; container services only.
    Swarm,
}

impl ClusterMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Swarm => "swarm",
        }
    }
}

impl std::str::FromStr for ClusterMode {
    type Err = crate::DomainError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "default" => Ok(Self::Default),
            "swarm" => Ok(Self::Swarm),
            other => Err(crate::DomainError::internal(format!("unknown cluster mode {other:?}"))),
        }
    }
}

/// A named group of nodes inside a workspace. Services and sites run on a
/// cluster; environments may map onto clusters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cluster {
    pub id: ClusterId,
    pub workspace_id: WorkspaceId,
    pub name: String,
    pub description: String,
    pub environment_id: Option<EnvironmentId>,
    /// Short label for the navigation rail (empty: initials of the name).
    #[serde(default)]
    pub abbr: String,
    /// Badge image as a `data:image/...;base64,` URL.
    #[serde(default)]
    pub icon: Option<String>,
    /// Orchestration mode; clusters created before modes existed are `default`.
    #[serde(default)]
    pub mode: ClusterMode,
    pub created_at: Timestamp,
}

/// An envelope-encrypted secret held by the control plane. `sealed` is never
/// serialized; the API only ever exposes metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretRecord {
    pub id: SecretId,
    pub workspace_id: WorkspaceId,
    /// e.g. `ssh.bootstrap`.
    pub kind: String,
    /// Human label such as `forgeops@10.0.0.5:22`.
    pub label: String,
    #[serde(skip)]
    pub sealed: String,
    /// Purged after this time (`None` = kept until deleted).
    pub expires_at: Option<Timestamp>,
    pub created_by: Principal,
    pub created_at: Timestamp,
}

// ---------------------------------------------------------------- servers

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    /// Enrolled but never connected.
    Pending,
    Online,
    Offline,
    Revoked,
}

impl AgentStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            AgentStatus::Pending => "pending",
            AgentStatus::Online => "online",
            AgentStatus::Offline => "offline",
            AgentStatus::Revoked => "revoked",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Self {
        match s {
            "online" => AgentStatus::Online,
            "offline" => AgentStatus::Offline,
            "revoked" => AgentStatus::Revoked,
            _ => AgentStatus::Pending,
        }
    }
}

/// What a node is for. The control node hosts forge-server itself and is
/// managed by its own local agent, like every worker node.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeRole {
    Control,
    #[default]
    Worker,
}

impl NodeRole {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Control => "control",
            Self::Worker => "worker",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Self {
        if s == "control" { Self::Control } else { Self::Worker }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Server {
    pub id: ServerId,
    pub workspace_id: WorkspaceId,
    pub environment_id: Option<EnvironmentId>,
    /// Cluster the node belongs to, if any.
    pub cluster_id: Option<ClusterId>,
    /// `control` for the machine hosting forge-server, `worker` otherwise.
    #[serde(default)]
    pub role: NodeRole,
    /// Name shown in the console (set by users); `hostname` stays the
    /// machine's own name, used for probes and SSH fallbacks.
    #[serde(default)]
    pub display_name: Option<String>,
    pub hostname: String,
    pub os: String,
    pub arch: String,
    pub agent_version: String,
    pub status: AgentStatus,
    pub last_seen_at: Option<Timestamp>,
    pub cert_serial: Option<String>,
    pub cert_not_after: Option<Timestamp>,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnrollmentToken {
    pub id: String,
    pub workspace_id: WorkspaceId,
    pub environment_id: Option<EnvironmentId>,
    #[serde(skip_serializing)]
    pub token_hash: String,
    pub created_by: Principal,
    pub expires_at: Timestamp,
    pub used_at: Option<Timestamp>,
    pub server_id: Option<ServerId>,
}

// ---------------------------------------------------------------- identity

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct User {
    pub id: UserId,
    /// Sign-in name (lowercase). At least one of `username` and `email` is set.
    pub username: Option<String>,
    /// Lowercase email; optional for accounts that sign in with a username.
    pub email: Option<String>,
    pub display_name: String,
    /// Argon2id PHC string. Never serialized.
    #[serde(skip)]
    pub password_hash: Option<String>,
    /// Envelope-encrypted TOTP secret. Never serialized.
    #[serde(skip)]
    pub totp_secret_sealed: Option<String>,
    /// Subject at the external identity provider, when linked via OIDC.
    pub oidc_subject: Option<String>,
    pub disabled: bool,
    pub created_at: Timestamp,
}

impl User {
    #[must_use]
    pub fn mfa_enabled(&self) -> bool {
        self.totp_secret_sealed.is_some()
    }

    /// How the user is named in the UI, audit details and authenticator apps:
    /// the username, else the email.
    #[must_use]
    pub fn login(&self) -> &str {
        self.username.as_deref().or(self.email.as_deref()).unwrap_or_default()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleBinding {
    pub id: RoleBindingId,
    pub user_id: UserId,
    pub role: Role,
    pub scope: EntityRef,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionKind {
    /// Browser session: HttpOnly cookie + CSRF token.
    Web,
    /// App/CLI session: bearer access token obtained via OAuth + PKCE.
    Bearer,
}

impl SessionKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            SessionKind::Web => "web",
            SessionKind::Bearer => "bearer",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    /// SHA-256 of the session secret; the secret itself is never stored.
    pub id_hash: String,
    pub user_id: UserId,
    pub kind: SessionKind,
    /// SHA-256 of the CSRF token (web sessions only).
    pub csrf_hash: Option<String>,
    /// SHA-256 of the refresh token (bearer sessions only).
    pub refresh_hash: Option<String>,
    pub mfa: bool,
    pub created_at: Timestamp,
    pub expires_at: Timestamp,
    /// Set for an MCP connector's bearer session ([`AgentConnection`]):
    /// the token is bound to `/mcp` and refused on the rest of the API.
    pub connection_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthCode {
    pub code_hash: String,
    pub user_id: UserId,
    pub client_id: String,
    pub redirect_uri: String,
    pub code_challenge: String,
    pub mfa: bool,
    pub expires_at: Timestamp,
    /// The connection a person approved on the consent page (connectors).
    pub connection_id: Option<String>,
}

// ---------------------------------------------------------------- MCP connectors

/// An OAuth client registered through dynamic client registration
/// (RFC 7591): a public client such as an AI connector. Never has a secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OAuthClientRecord {
    pub client_id: String,
    pub name: String,
    pub redirect_uris: Vec<String>,
    pub created_at: Timestamp,
    pub created_ip: Option<String>,
    pub revoked_at: Option<Timestamp>,
    pub last_used_at: Option<Timestamp>,
}

/// What a connected agent may do, enforced by the server (issue #6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentMode {
    /// Only read tools are listed and run.
    ReadOnly,
    /// Every change waits for the person's approval in the console.
    Ask,
    /// Changes within the person's permissions run directly; destructive
    /// and MFA-guarded actions still wait for approval in the console.
    Auto,
}

impl AgentMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read_only",
            Self::Ask => "ask",
            Self::Auto => "auto",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "read_only" => Some(Self::ReadOnly),
            "ask" => Some(Self::Ask),
            "auto" => Some(Self::Auto),
            _ => None,
        }
    }
}

/// A change waiting for a person's decision (issue #6 Ask mode, D-073).
/// `request` is what runs once approved; it came from an AI agent.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Approval {
    pub id: String,
    /// `mcp.tool` today; the CLI's approvals (#5) share the table.
    pub kind: String,
    /// The person who decides (and whose connector asked).
    pub user_id: UserId,
    pub connection_id: Option<String>,
    /// Who asked, for people: the connector's name.
    pub requester: String,
    /// The tool (or operation) asked for.
    pub action: String,
    pub request: Value,
    pub summary: String,
    pub mfa_required: bool,
    /// `pending`, `approved` (running), `executed`, `failed`, `denied`.
    pub status: String,
    pub created_at: Timestamp,
    pub expires_at: Timestamp,
    pub decided_by: Option<UserId>,
    pub decided_at: Option<Timestamp>,
    pub result: Option<Value>,
    pub error: Option<String>,
}

/// An `Idempotency-Key` of a mutating API request (issue #5, D-074).
/// `body` is the stored response, sealed with the vault envelope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdempotencyRecord {
    pub principal: String,
    pub key: String,
    /// SHA-256 over method, path with query, and body.
    pub request_hash: String,
    /// `false` while the first request is still running.
    pub done: bool,
    pub status: Option<u16>,
    pub content_type: Option<String>,
    pub body: Option<String>,
    pub created_at: Timestamp,
    pub expires_at: Timestamp,
}

/// What [`crate::ports::IdempotencyRepository::begin_idempotent`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdempotencyBegin {
    /// The key was free: run the request.
    Started,
    /// The key is taken (running or done).
    Existing(IdempotencyRecord),
}

/// One consent a person gave a connector. Its bearer sessions point to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AgentConnection {
    pub id: String,
    pub user_id: UserId,
    pub client_id: String,
    pub client_name: String,
    /// Host of the redirect URI shown on the consent page.
    pub redirect_host: String,
    /// The protected resource (RFC 8707) the tokens are bound to.
    pub resource: String,
    pub mode: AgentMode,
    pub created_at: Timestamp,
    pub last_used_at: Option<Timestamp>,
    pub revoked_at: Option<Timestamp>,
}

// ---------------------------------------------------------------- jobs

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

impl JobStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            JobStatus::Queued => "queued",
            JobStatus::Running => "running",
            JobStatus::Succeeded => "succeeded",
            JobStatus::Failed => "failed",
            JobStatus::Cancelled => "cancelled",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Self {
        match s {
            "running" => JobStatus::Running,
            "succeeded" => JobStatus::Succeeded,
            "failed" => JobStatus::Failed,
            "cancelled" => JobStatus::Cancelled,
            _ => JobStatus::Queued,
        }
    }

    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, JobStatus::Succeeded | JobStatus::Failed | JobStatus::Cancelled)
    }
}

/// Request to enqueue a job.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewJob {
    pub kind: String,
    pub workspace_id: Option<WorkspaceId>,
    /// Resource the job acts on; re-authorized by the runner before each step.
    pub resource: EntityRef,
    pub principal: Principal,
    /// Authorization context captured at submission (e.g. MFA), re-used by
    /// the runner when it re-authorizes each step.
    pub context: AuthzContext,
    pub idempotency_key: Option<String>,
    pub payload: Value,
    pub max_attempts: u32,
}

/// A durable job row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Job {
    pub id: JobId,
    pub kind: String,
    pub workspace_id: Option<WorkspaceId>,
    pub resource: EntityRef,
    pub principal: Principal,
    pub context: AuthzContext,
    pub idempotency_key: Option<String>,
    pub payload: Value,
    /// State carried between steps; persisted after each completed step.
    pub state: Value,
    pub status: JobStatus,
    /// Index of the next step to run.
    pub current_step: u32,
    pub attempts: u32,
    pub max_attempts: u32,
    pub next_run_at: Timestamp,
    pub lease_owner: Option<String>,
    pub lease_expires_at: Option<Timestamp>,
    /// Fencing token: incremented on every claim. Writes must present it.
    pub lease_token: i64,
    pub last_error: Option<String>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobStepRecord {
    pub job_id: JobId,
    pub index: u32,
    pub name: String,
    pub status: String,
    pub started_at: Timestamp,
    pub finished_at: Option<Timestamp>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobFilter {
    pub workspace_id: Option<WorkspaceId>,
    pub status: Option<JobStatus>,
    pub limit: u32,
}

// ---------------------------------------------------------------- audit

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditOutcome {
    Success,
    Denied,
    Failure,
}

impl AuditOutcome {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            AuditOutcome::Success => "success",
            AuditOutcome::Denied => "denied",
            AuditOutcome::Failure => "failure",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Self {
        match s {
            "denied" => AuditOutcome::Denied,
            "failure" => AuditOutcome::Failure,
            _ => AuditOutcome::Success,
        }
    }
}

/// Server-side audit search: every field narrows the result; `before` is the
/// keyset cursor (entries with a smaller `seq`). Newest first.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuditFilter {
    pub workspace: Option<WorkspaceId>,
    /// Action prefixes, any of which may match (`stack.`, `auth.login`).
    pub actions: Vec<String>,
    /// Case-insensitive substring of the actor.
    pub actor: Option<String>,
    pub outcome: Option<AuditOutcome>,
    /// Case-insensitive substring of the resource.
    pub resource: Option<String>,
    /// Free text: a case-insensitive substring of the action, actor,
    /// resource or details.
    pub text: Option<String>,
    /// Inclusive lower bound, epoch ms.
    pub since: Option<i64>,
    /// Exclusive upper bound, epoch ms.
    pub until: Option<i64>,
    /// Only entries with `seq < before`.
    pub before: Option<i64>,
    pub limit: u32,
}

/// One distinct action in the audit log and how often it occurs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditActionCount {
    pub action: String,
    pub count: i64,
}

/// How many entries with one outcome fall into one time bucket of the
/// audit histogram. `bucket` counts `step_ms` steps from the histogram start.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditBucketCount {
    pub bucket: i64,
    pub outcome: AuditOutcome,
    pub count: i64,
}

/// One persisted, hash-chained audit entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditEntry {
    pub seq: i64,
    pub id: String,
    pub at: Timestamp,
    pub actor: String,
    pub action: String,
    pub resource: String,
    pub workspace_id: Option<WorkspaceId>,
    pub outcome: AuditOutcome,
    pub details: Value,
    pub prev_hash: String,
    pub hash: String,
    pub signature: String,
}

// ---------------------------------------------------------------- plugins

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginState {
    Installed,
    Enabled,
    Disabled,
    Quarantined,
}

impl PluginState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            PluginState::Installed => "installed",
            PluginState::Enabled => "enabled",
            PluginState::Disabled => "disabled",
            PluginState::Quarantined => "quarantined",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Self {
        match s {
            "enabled" => PluginState::Enabled,
            "disabled" => PluginState::Disabled,
            "quarantined" => PluginState::Quarantined,
            _ => PluginState::Installed,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginRecord {
    /// Manifest id, e.g. `orbyn.uptime-probe`.
    pub plugin_id: String,
    pub version: String,
    pub publisher: String,
    #[serde(skip_serializing)]
    pub manifest_toml: String,
    pub artifact_sha256: String,
    pub state: PluginState,
    pub pinned: bool,
    /// Permissions an administrator approved at install/upgrade time.
    pub approved_permissions: Vec<String>,
    pub fault_count: u32,
    pub quarantine_reason: Option<String>,
    pub installed_at: Timestamp,
    pub updated_at: Timestamp,
}

// ------------------------------------------------- Phase 3 extension points

/// Immutable release identified by artifact digest (Phase 3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Release {
    pub id: ReleaseId,
    pub service_id: ServiceId,
    pub version: String,
    pub digest: String,
}

/// An event to be recorded in the audit log.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditEvent {
    pub actor: String,
    pub action: String,
    pub resource: String,
    pub workspace_id: Option<WorkspaceId>,
    pub outcome: AuditOutcome,
    pub details: Value,
}

impl AuditEvent {
    pub fn new(actor: impl ToString, action: impl ToString, resource: impl ToString, outcome: AuditOutcome) -> Self {
        Self {
            actor: actor.to_string(),
            action: action.to_string(),
            resource: resource.to_string(),
            workspace_id: None,
            outcome,
            details: Value::Object(serde_json::Map::new()),
        }
    }

    #[must_use]
    pub fn in_org(mut self, org: Option<WorkspaceId>) -> Self {
        self.workspace_id = org;
        self
    }

    #[must_use]
    pub fn with_details(mut self, details: Value) -> Self {
        self.details = details;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sealed_secret_is_never_serialized() {
        let s = SecretRecord {
            id: SecretId::new(),
            workspace_id: WorkspaceId::new(),
            kind: "ssh.bootstrap".into(),
            label: "ops@h:22".into(),
            sealed: "v1.SEALED-VALUE".into(),
            expires_at: None,
            created_by: Principal::User(UserId::new()),
            created_at: Timestamp(0),
        };
        assert!(!serde_json::to_string(&s).unwrap_or_default().contains("SEALED-VALUE"));
    }
}

/// How Forge reaches a node's SSH daemon, with its pinned host key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SshEndpoint {
    pub server_id: ServerId,
    pub host: String,
    pub port: u16,
    /// `SHA256:…` fingerprint confirmed by a user or recorded at onboarding.
    pub host_key_fingerprint: String,
    pub updated_at: Timestamp,
}

/// Where a node's Docker TLS endpoint stands (D-125).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DockerEndpointState {
    /// Certificates issued; the node has not confirmed dockerd listens yet.
    Pending,
    /// dockerd answered over mutual TLS.
    Enabled,
    /// The last enable or check failed (`last_error` says why).
    Failed,
}

impl DockerEndpointState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Enabled => "enabled",
            Self::Failed => "failed",
        }
    }
}

impl std::fmt::Display for DockerEndpointState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for DockerEndpointState {
    type Err = crate::DomainError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pending" => Ok(Self::Pending),
            "enabled" => Ok(Self::Enabled),
            "failed" => Ok(Self::Failed),
            other => Err(crate::DomainError::internal(format!("unknown docker endpoint state {other}"))),
        }
    }
}

/// A node's Docker Engine API over mutual TLS (D-125). The node's own CA
/// signs dockerd's certificate and the control plane's client
/// certificates; its key is kept sealed and never serialized.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DockerEndpoint {
    pub server_id: ServerId,
    /// Address the control plane dials (an IP address or host name).
    pub host: String,
    pub port: u16,
    pub state: DockerEndpointState,
    /// PEM of the node's Docker CA certificate.
    pub ca_cert_pem: String,
    /// The CA's private key, sealed with the master key.
    #[serde(skip_serializing)]
    pub ca_key_sealed: String,
    /// When dockerd's certificate expires.
    pub daemon_cert_not_after: Timestamp,
    /// Engine and API version from the last successful check.
    pub engine_version: Option<String>,
    pub api_version: Option<String>,
    pub last_error: Option<String>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

/// An App Store package on a node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Installation {
    pub id: InstallationId,
    pub workspace_id: WorkspaceId,
    pub server_id: ServerId,
    pub package: String,
    pub version: String,
    /// Validated, non-secret parameters it was installed with.
    pub params: std::collections::BTreeMap<String, String>,
    /// `pending`, `installing`, `installed`, `failed`, `removing`, …
    pub status: String,
    /// Last state reported by the recipe: `running`, `stopped`, `not-installed`.
    pub state: Option<String>,
    /// The package's env file on the node, secrets redacted.
    pub env: std::collections::BTreeMap<String, String>,
    pub last_job_id: Option<JobId>,
    pub last_error: Option<String>,
    /// Builds the node last reported (package-repository installs only).
    #[serde(default)]
    pub versions: Option<PackageVersions>,
    /// `notify` (default) or `auto_patch`.
    #[serde(default = "default_update_policy")]
    pub update_policy: String,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

fn default_update_policy() -> String {
    "notify".into()
}

/// What a node reports about an installed package's builds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageVersions {
    /// `apt`, `dnf`, `yum`, `winget` or `manual`.
    pub manager: String,
    /// The OS package, e.g. `postgresql-17`.
    #[serde(default)]
    pub package: String,
    /// Installed build, e.g. `17.6-1.pgdg24.04+1`.
    #[serde(default)]
    pub installed: String,
    /// Newest build the node's repositories offer.
    #[serde(default)]
    pub candidate: String,
    /// Version the running service reports.
    #[serde(default)]
    pub running: String,
    pub checked_at: Timestamp,
}

/// A vendor release line (from endoflife.date).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageRelease {
    pub product: String,
    pub line: String,
    pub latest: String,
    pub lts: bool,
    pub released_at: Option<String>,
    pub latest_at: Option<String>,
    /// End of life: a date, or `true`/`false`.
    pub eol: Option<String>,
    pub support_until: Option<String>,
    pub fetched_at: Timestamp,
}

/// One environment variable of a service. Secret values are sealed with
/// the vault envelope and never returned by the API (`value` is `None`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvVar {
    pub key: String,
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub secret: bool,
    /// Sealed value (secrets only); storage-internal.
    #[serde(default, skip_serializing)]
    pub sealed: Option<String>,
}

/// A service: what runs (spec) and its environment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Service {
    pub id: ServiceId,
    pub workspace_id: WorkspaceId,
    pub environment_id: EnvironmentId,
    pub name: String,
    /// `service`, `job` or `container`.
    pub runtime: String,
    /// Runtime fields of `forge_catalog::AppSpec` without the environment.
    pub spec: serde_json::Value,
    pub env: Vec<EnvVar>,
    pub created_by: Principal,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

/// Placement of a service on a node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceInstance {
    pub service_id: ServiceId,
    pub server_id: ServerId,
    /// `active` or `standby`.
    pub role: String,
    /// `pending`, `deploying`, `deployed`, `failed`, …
    pub status: String,
    /// Last state reported by the node: `active`, `inactive`, `failed`, …
    pub state: Option<String>,
    pub last_job_id: Option<JobId>,
    pub last_error: Option<String>,
    pub updated_at: Timestamp,
}

/// A personal API key. Only a SHA-256 of the secret is stored; the full key
/// (`fk_<prefix>_<secret>`) is shown once at creation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiKey {
    pub id: String,
    pub user_id: UserId,
    pub name: String,
    /// Public part shown in lists, e.g. `fk_3f9a1c2b`.
    pub prefix: String,
    #[serde(skip)]
    pub secret_hash: String,
    pub expires_at: Option<Timestamp>,
    pub last_used_at: Option<Timestamp>,
    pub created_at: Timestamp,
    /// An elevated token may run MFA-guarded actions without a prompt
    /// (D-097), when `[security] allow_elevated_tokens` is on.
    #[serde(default)]
    pub elevated: bool,
    /// The token's ceiling (#13): `None` acts with all of the owner's
    /// access; `Some` only with what an entry lists, and never more than
    /// the owner holds at the time of the request.
    #[serde(default)]
    pub restrictions: Option<Vec<crate::permissions::TokenGrant>>,
}

/// What a vault credential belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultLink {
    /// `node`, `service` or `cluster`.
    pub kind: String,
    pub id: String,
}

/// A credential kept for people (credential vault). `sealed` holds the
/// envelope-encrypted password and is never serialized.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultItem {
    pub id: VaultItemId,
    pub workspace_id: WorkspaceId,
    pub name: String,
    pub username: String,
    pub url: String,
    pub notes: String,
    pub tags: Vec<String>,
    pub links: Vec<VaultLink>,
    #[serde(skip)]
    pub sealed: String,
    pub created_by: Principal,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    /// When the password last changed.
    pub rotated_at: Timestamp,
}

/// An uploaded TLS certificate for ingress endpoints. The chain is public;
/// the private key is sealed (envelope encryption, AAD bound to the id) and
/// only unsealed when a command carrying it is sent to an edge node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TlsCertificate {
    pub id: CertificateId,
    pub workspace_id: WorkspaceId,
    pub name: String,
    /// DNS names the leaf certificate covers (SANs, else the CN).
    pub domains: Vec<String>,
    pub issuer: String,
    pub not_before: Timestamp,
    pub not_after: Timestamp,
    /// SHA-256 of the leaf certificate (hex).
    pub fingerprint: String,
    /// Full chain, PEM (leaf first).
    pub chain_pem: String,
    #[serde(skip)]
    pub sealed_key: String,
    pub created_by: Principal,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

/// A cluster's Forge network fabric (`forge_catalog::fabric`), as JSON documents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FabricRecord {
    pub cluster_id: ClusterId,
    /// `FabricSpec`.
    pub spec: serde_json::Value,
    /// Manual `Lease`s.
    pub pins: serde_json::Value,
    /// Current `Lease`s.
    pub leases: serde_json::Value,
    /// Reconcile state per node: `{nodes: {id: {state, error, attempts, next_at}}}`.
    pub status: serde_json::Value,
    pub updated_at: Timestamp,
}

/// A cluster's orchestrator, overlay network and ingress layout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClusterProfileRecord {
    pub cluster_id: ClusterId,
    /// `forge_catalog::ingress::ClusterProfile` as JSON.
    pub profile: serde_json::Value,
    /// Last apply / provision results: `{edges: {id: {...}}, dirty, applied_at}`.
    pub status: serde_json::Value,
    pub updated_at: Timestamp,
}

/// A reverse-proxy site served by one or more edge nodes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Site {
    pub id: String,
    pub workspace_id: WorkspaceId,
    pub name: String,
    /// `{spec: SiteSpec without name, edges: [server ids], dns: {...}|null}`.
    pub config: serde_json::Value,
    /// Per edge node: `{status, error, updated_at}`.
    pub edge_status: serde_json::Value,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}
