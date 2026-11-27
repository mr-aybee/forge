//! Ports: the traits through which services reach infrastructure.
//!
//! Each trait is small and focused (interface segregation) so a service only
//! depends on the capabilities it uses. `forge-store` implements all of them
//! on SQL; tests can substitute in-memory fakes.

use async_trait::async_trait;
use serde_json::Value;

use crate::access::{AccessRequest, EntityRef, ResourceRef};
use crate::clock::Timestamp;
use crate::error::DomainResult;
use crate::ids::*;
use crate::model::*;

/// Decides whether a principal may perform an action on a resource.
#[async_trait]
pub trait Authorizer: Send + Sync + std::fmt::Debug {
    /// Returns `Ok(())` when allowed and `DomainError::Denied` otherwise.
    async fn authorize(&self, request: &AccessRequest) -> DomainResult<()>;

    /// Convenience boolean form.
    async fn is_allowed(&self, request: &AccessRequest) -> bool {
        self.authorize(request).await.is_ok()
    }

    /// Boolean form for many requests (list endpoints). Implementations may
    /// resolve the resources in one batch; the decisions are the same as
    /// calling [`Authorizer::is_allowed`] for each.
    async fn is_allowed_many(&self, requests: &[AccessRequest]) -> Vec<bool> {
        let mut out = Vec::with_capacity(requests.len());

        for r in requests {
            out.push(self.is_allowed(r).await);
        }

        out
    }

    /// Whether each principal holds its action *over* its resource: a grant
    /// that covers the resource and everything beneath it. Unlike
    /// [`Authorizer::is_allowed_many`], the action need not apply to the
    /// resource's own type (`container.manage` over an environment means: on
    /// every node in it). Used for grant checks (escalation guard, effective
    /// access), never to authorize an operation. Default: the same as
    /// [`Authorizer::is_allowed_many`].
    async fn holds_over(&self, requests: &[AccessRequest]) -> Vec<bool> {
        self.is_allowed_many(requests).await
    }
}

/// Resolves a resource's ancestry (server → environment → workspace).
#[async_trait]
pub trait ResourceResolver: Send + Sync + std::fmt::Debug {
    async fn resolve(&self, entity: &EntityRef) -> DomainResult<ResourceRef>;

    /// Resolves many entities; implementations may share lookups between them.
    async fn resolve_many(&self, entities: &[EntityRef]) -> Vec<DomainResult<ResourceRef>> {
        let mut out = Vec::with_capacity(entities.len());

        for e in entities {
            out.push(self.resolve(e).await);
        }

        out
    }
}

#[async_trait]
pub trait TenancyRepository: Send + Sync + std::fmt::Debug {
    async fn create_workspace(&self, org: &Workspace) -> DomainResult<()>;
    async fn get_workspace(&self, id: WorkspaceId) -> DomainResult<Workspace>;
    async fn list_workspaces(&self) -> DomainResult<Vec<Workspace>>;
    async fn create_tenant(&self, tenant: &Tenant) -> DomainResult<()>;
    async fn get_tenant(&self, id: TenantId) -> DomainResult<Tenant>;
    async fn list_tenants(&self, org: WorkspaceId) -> DomainResult<Vec<Tenant>>;
    /// Creates an environment; `Conflict` if its cluster already has one with
    /// that name.
    async fn create_environment(&self, ns: &Environment) -> DomainResult<()>;
    async fn get_environment(&self, id: EnvironmentId) -> DomainResult<Environment>;
    /// A workspace's environments, or only one cluster's.
    async fn list_environments(&self, ws: WorkspaceId, cluster: Option<ClusterId>) -> DomainResult<Vec<Environment>>;
    /// Moves an environment into a cluster (its services follow);
    /// `Conflict` if the target cluster already has that name.
    async fn set_environment_cluster(&self, id: EnvironmentId, cluster: ClusterId) -> DomainResult<()>;
    /// A workspace's environments in one project (D-102).
    async fn list_environments_in_group(&self, ws: WorkspaceId, group: ProjectId) -> DomainResult<Vec<Environment>>;
    /// Attaches an environment to a project (or detaches it with `None`).
    async fn set_environment_project(&self, id: EnvironmentId, group: Option<ProjectId>) -> DomainResult<()>;
    /// Renames an environment; `Conflict` if its cluster already has that name.
    async fn rename_environment(&self, id: EnvironmentId, name: &str) -> DomainResult<()>;
    /// Deletes an empty environment. Callers check for services and stacks
    /// first; rows that still reference it make the delete fail.
    async fn delete_environment(&self, id: EnvironmentId) -> DomainResult<()>;
}

/// Projects: the products that group environments (D-102).
#[async_trait]
pub trait ProjectRepository: Send + Sync + std::fmt::Debug {
    /// `Conflict` if the workspace already has a group with that name.
    async fn create_project(&self, group: &Project) -> DomainResult<()>;
    async fn get_project(&self, id: ProjectId) -> DomainResult<Project>;
    async fn list_projects(&self, ws: WorkspaceId) -> DomainResult<Vec<Project>>;
    /// Sets name and description; `Conflict` on a duplicate name.
    async fn update_project(&self, id: ProjectId, name: &str, description: &str) -> DomainResult<()>;
    /// Deletes a group. Callers refuse while environments still point at it.
    async fn delete_project(&self, id: ProjectId) -> DomainResult<()>;
}

#[async_trait]
pub trait RoleBindingRepository: Send + Sync + std::fmt::Debug {
    async fn create_binding(&self, binding: &RoleBinding) -> DomainResult<()>;
    async fn delete_binding(&self, id: RoleBindingId) -> DomainResult<()>;
    async fn list_bindings(&self) -> DomainResult<Vec<RoleBinding>>;
    async fn list_bindings_for_user(&self, user: UserId) -> DomainResult<Vec<RoleBinding>>;
}

#[async_trait]
pub trait UserRepository: Send + Sync + std::fmt::Debug {
    async fn create_user(&self, user: &User) -> DomainResult<()>;
    async fn get_user(&self, id: UserId) -> DomainResult<User>;
    async fn find_user_by_email(&self, email: &str) -> DomainResult<Option<User>>;
    async fn find_user_by_username(&self, username: &str) -> DomainResult<Option<User>>;
    /// Sets username, email and display name (already validated and lowercased).
    async fn update_user_profile(
        &self,
        id: UserId,
        username: Option<&str>,
        email: Option<&str>,
        display_name: &str,
    ) -> DomainResult<()>;
    async fn set_password_hash(&self, id: UserId, hash: &str) -> DomainResult<()>;
    async fn find_user_by_oidc_subject(&self, subject: &str) -> DomainResult<Option<User>>;
    async fn list_users(&self) -> DomainResult<Vec<User>>;
    /// The users with these ids, in no particular order; ids without a user
    /// are left out. For lists that name a few people, instead of reading
    /// every account.
    async fn get_users(&self, ids: &[UserId]) -> DomainResult<Vec<User>>;
    async fn set_totp_secret(&self, id: UserId, sealed: Option<&str>) -> DomainResult<()>;
    async fn set_oidc_subject(&self, id: UserId, subject: &str) -> DomainResult<()>;
}

#[async_trait]
pub trait SessionRepository: Send + Sync + std::fmt::Debug {
    async fn create_session(&self, session: &Session) -> DomainResult<()>;
    async fn get_session(&self, id_hash: &str) -> DomainResult<Option<Session>>;
    async fn find_by_refresh(&self, refresh_hash: &str) -> DomainResult<Option<Session>>;
    async fn delete_session(&self, id_hash: &str) -> DomainResult<()>;
    async fn delete_user_sessions(&self, user: UserId) -> DomainResult<()>;
    async fn create_oauth_code(&self, code: &OAuthCode) -> DomainResult<()>;
    /// Atomically removes and returns the code (single use).
    async fn take_oauth_code(&self, code_hash: &str) -> DomainResult<Option<OAuthCode>>;
}

/// MCP connectors: dynamically registered OAuth clients and the
/// connections people approved (issue #6).
#[async_trait]
pub trait ConnectorRepository: Send + Sync + std::fmt::Debug {
    async fn create_client(&self, client: &OAuthClientRecord) -> DomainResult<()>;
    async fn get_client(&self, client_id: &str) -> DomainResult<Option<OAuthClientRecord>>;
    async fn list_clients(&self) -> DomainResult<Vec<OAuthClientRecord>>;
    /// Clients registered at or after `since` (registration rate limit).
    async fn count_clients_since(&self, since: Timestamp) -> DomainResult<u64>;
    /// Marks the client revoked and revokes every connection made with it.
    async fn revoke_client(&self, client_id: &str, at: Timestamp) -> DomainResult<()>;
    async fn create_connection(&self, connection: &AgentConnection) -> DomainResult<()>;
    async fn get_connection(&self, id: &str) -> DomainResult<Option<AgentConnection>>;
    /// Connections of one person, or everyone's with `None`. Newest first.
    async fn list_connections(&self, user: Option<UserId>) -> DomainResult<Vec<AgentConnection>>;
    /// Marks the connection revoked and deletes its sessions.
    async fn revoke_connection(&self, id: &str, at: Timestamp) -> DomainResult<()>;
    async fn touch_connection(&self, id: &str, at: Timestamp) -> DomainResult<()>;
}

/// Approvals of changes an AI connector asked for (D-073).
#[async_trait]
pub trait ApprovalRepository: Send + Sync + std::fmt::Debug {
    async fn create_approval(&self, approval: &Approval) -> DomainResult<()>;
    async fn get_approval(&self, id: &str) -> DomainResult<Option<Approval>>;
    /// Newest first; one person's, or everyone's with `None`.
    async fn list_approvals(&self, user: Option<UserId>, limit: u32) -> DomainResult<Vec<Approval>>;
    async fn count_pending(&self, connection_id: &str) -> DomainResult<u64>;
    /// Moves a pending, unexpired approval to `status` (single use): `false`
    /// when it was decided already or expired.
    async fn decide_approval(&self, id: &str, status: &str, by: UserId, at: Timestamp) -> DomainResult<bool>;
    async fn finish_approval(
        &self,
        id: &str,
        status: &str,
        result: Option<&serde_json::Value>,
        error: Option<&str>,
    ) -> DomainResult<()>;
    /// Housekeeping: marks pending approvals past `now` as `expired`, and
    /// deletes decided ones created before `drop_before`. Returns the number
    /// of rows changed.
    async fn sweep_approvals(&self, now: Timestamp, drop_before: Timestamp) -> DomainResult<u64>;
    /// Cancels a connection's pending approvals (status `cancelled`), for
    /// example when the connection is revoked. Returns their ids.
    async fn cancel_pending_approvals(
        &self,
        connection_id: &str,
        by: UserId,
        at: Timestamp,
    ) -> DomainResult<Vec<String>>;
}

/// `Idempotency-Key` records (D-074).
#[async_trait]
pub trait IdempotencyRepository: Send + Sync + std::fmt::Debug {
    /// Claims `(principal, key)` for a new request, after dropping expired
    /// rows and this key's pending row older than `stale_pending_before`.
    async fn begin_idempotent(
        &self,
        principal: &str,
        key: &str,
        request_hash: &str,
        now: Timestamp,
        expires_at: Timestamp,
        stale_pending_before: Timestamp,
    ) -> DomainResult<IdempotencyBegin>;
    /// Stores the response of the request that claimed the key: only that
    /// claim (`request_hash`, `claimed_at`), never a newer one for the key.
    async fn finish_idempotent(
        &self,
        principal: &str,
        key: &str,
        claim: (&str, Timestamp),
        status: u16,
        content_type: Option<&str>,
        sealed_body: Option<&str>,
    ) -> DomainResult<()>;
    /// Frees this request's pending claim (the request failed; a retry runs again).
    async fn release_idempotent(&self, principal: &str, key: &str, claim: (&str, Timestamp)) -> DomainResult<()>;
    /// Unexpired keys `principal` holds (bounds what one person can store).
    async fn count_idempotent(&self, principal: &str, now: Timestamp) -> DomainResult<u64>;
}

#[async_trait]
pub trait ServerRepository: Send + Sync + std::fmt::Debug {
    async fn create_server(&self, server: &Server) -> DomainResult<()>;
    async fn get_server(&self, id: ServerId) -> DomainResult<Server>;
    /// Sets (or clears) the console display name.
    async fn rename_server(&self, id: ServerId, display_name: Option<&str>) -> DomainResult<()>;
    async fn list_servers(&self, org: Option<WorkspaceId>) -> DomainResult<Vec<Server>>;
    async fn find_server_by_hostname(&self, org: WorkspaceId, hostname: &str) -> DomainResult<Option<Server>>;
    async fn record_heartbeat(&self, id: ServerId, at: Timestamp, agent_version: &str) -> DomainResult<()>;
    async fn set_status(&self, id: ServerId, status: AgentStatus) -> DomainResult<()>;
    /// Deletes a node and everything recorded for it, in one transaction:
    /// facts, logins, SSH, Docker endpoint, files, installations, app
    /// instances, stack parts, deploy slots, store members, tunnels,
    /// observations, alarms, its DNS label, tags and node-scoped access.
    /// Stacks whose default node it was move to another of their nodes, or
    /// are deleted when they have none. Queued jobs for the node are
    /// cancelled. The audit log and revoked certificates are kept.
    async fn delete_server(&self, id: ServerId, at: Timestamp) -> DomainResult<()> {
        let _ = (id, at);
        Err(crate::error::DomainError::Internal("deleting nodes is not supported by this store".into()))
    }
    async fn set_certificate(&self, id: ServerId, serial: &str, not_after: Timestamp) -> DomainResult<()>;
    /// Drains (or undrains) a node (D-177). Idempotent.
    async fn set_drained(&self, id: ServerId, drained: bool) -> DomainResult<()> {
        let _ = (id, drained);
        Err(crate::error::DomainError::Internal("draining nodes is not supported by this store".into()))
    }
    /// Marks agents that have not been seen since `cutoff` as offline; returns their ids.
    async fn mark_stale_offline(&self, cutoff: Timestamp) -> DomainResult<Vec<ServerId>>;
    /// Sets a node's DNS label (D-084); `explicit`: a user chose it. A label
    /// another node of the workspace holds is a conflict.
    async fn set_node_label(
        &self,
        id: ServerId,
        workspace: WorkspaceId,
        label: &str,
        explicit: bool,
        at: Timestamp,
    ) -> DomainResult<()>;
    /// Stores a derived label for a node that has none yet. `false` when the
    /// node already has one; a conflict when another node holds the label.
    async fn claim_node_label(
        &self,
        id: ServerId,
        workspace: WorkspaceId,
        label: &str,
        at: Timestamp,
    ) -> DomainResult<bool>;
    /// Stored labels: node, label, whether a user set it (all workspaces
    /// with `None`).
    async fn list_node_labels(&self, workspace: Option<WorkspaceId>) -> DomainResult<Vec<(ServerId, String, bool)>>;
}

#[async_trait]
pub trait EnrollmentRepository: Send + Sync + std::fmt::Debug {
    async fn create_token(&self, token: &EnrollmentToken) -> DomainResult<()>;
    /// Atomically marks an unused, unexpired token as used. Returns `None`
    /// if the token is unknown, expired or already consumed.
    async fn consume_token(
        &self,
        token_hash: &str,
        now: Timestamp,
        server: ServerId,
    ) -> DomainResult<Option<EnrollmentToken>>;
    async fn revoke_certificate(&self, serial: &str, server: ServerId, at: Timestamp) -> DomainResult<()>;
    async fn is_certificate_revoked(&self, serial: &str) -> DomainResult<bool>;
    /// Looks up a token by id (used to find the server it enrolled).
    async fn get_token(&self, id: &str) -> DomainResult<Option<EnrollmentToken>>;
    /// Looks up a token by the hash of its secret, used or not (installer
    /// progress reports authenticate with the token).
    async fn find_token_by_hash(&self, token_hash: &str) -> DomainResult<Option<EnrollmentToken>>;
    /// Remembers the cluster (and expected host name) a token's node goes to.
    async fn put_enrollment_target(
        &self,
        token_id: &str,
        cluster: Option<ClusterId>,
        hostname: Option<&str>,
        at: Timestamp,
    ) -> DomainResult<()>;
    /// `(cluster, hostname)` recorded for a token, if any.
    async fn get_enrollment_target(&self, token_id: &str) -> DomainResult<Option<(Option<ClusterId>, Option<String>)>>;
}

#[async_trait]
pub trait JobRepository: Send + Sync + std::fmt::Debug {
    /// Inserts a job, or returns the existing one with the same
    /// `(kind, idempotency_key)`. The boolean is `true` if newly created.
    async fn insert_job(&self, job: NewJob, now: Timestamp) -> DomainResult<(Job, bool)>;
    async fn get_job(&self, id: JobId) -> DomainResult<Job>;
    /// The job with this `(kind, idempotency_key)`, if any.
    async fn find_job_by_key(&self, kind: &str, key: &str) -> DomainResult<Option<Job>>;
    async fn list_jobs(&self, filter: &JobFilter) -> DomainResult<Vec<Job>>;
    async fn list_steps(&self, id: JobId) -> DomainResult<Vec<JobStepRecord>>;
    /// Claims the next runnable job: queued and due, or running with an
    /// expired lease (its previous owner crashed).
    async fn claim_job(&self, worker: &str, now: Timestamp, lease_until: Timestamp) -> DomainResult<Option<Job>>;
    async fn renew_lease(&self, id: JobId, token: i64, lease_until: Timestamp) -> DomainResult<()>;
    async fn start_step(&self, id: JobId, token: i64, index: u32, name: &str, now: Timestamp) -> DomainResult<()>;
    /// Records a finished step and advances `current_step`, fenced by `token`.
    async fn complete_step(&self, id: JobId, token: i64, index: u32, state: &Value, now: Timestamp)
    -> DomainResult<()>;
    async fn finish_job(
        &self,
        id: JobId,
        token: i64,
        status: JobStatus,
        error: Option<&str>,
        now: Timestamp,
    ) -> DomainResult<()>;
    async fn schedule_retry(
        &self,
        id: JobId,
        token: i64,
        next_run_at: Timestamp,
        error: &str,
        now: Timestamp,
    ) -> DomainResult<()>;
    /// Cancels a non-terminal job. Returns `false` if it was already terminal.
    async fn cancel_job(&self, id: JobId, now: Timestamp) -> DomainResult<bool>;
}

#[async_trait]
pub trait AuditRepository: Send + Sync + std::fmt::Debug {
    async fn last_entry(&self) -> DomainResult<Option<AuditEntry>>;
    /// Inserts an entry. Must fail with `Conflict` if `seq` already exists,
    /// which keeps the chain linear even with concurrent writers.
    async fn insert_entry(&self, entry: &AuditEntry) -> DomainResult<()>;
    async fn entries_from(&self, from_seq: i64, limit: u32) -> DomainResult<Vec<AuditEntry>>;
    async fn recent_entries(&self, org: Option<WorkspaceId>, limit: u32) -> DomainResult<Vec<AuditEntry>>;
    /// Entries matching `filter`, newest first, at most `filter.limit` (clamped to 1..=500).
    async fn search_entries(&self, filter: &AuditFilter) -> DomainResult<Vec<AuditEntry>>;
    /// How many entries match `filter`, ignoring `before` and `limit`.
    async fn count_entries(&self, filter: &AuditFilter) -> DomainResult<i64>;
    /// Entries matching `filter` (ignoring `before` and `limit`) counted per
    /// outcome and per `step_ms` bucket counted from `from`. Only entries at
    /// or after `from` are counted; buckets without entries are left out.
    async fn bucket_counts(&self, filter: &AuditFilter, from: i64, step_ms: i64)
    -> DomainResult<Vec<AuditBucketCount>>;
    /// Every distinct action with its count, sorted by action.
    async fn distinct_actions(&self, org: Option<WorkspaceId>) -> DomainResult<Vec<AuditActionCount>>;
}

#[async_trait]
pub trait PluginRepository: Send + Sync + std::fmt::Debug {
    async fn upsert_plugin(&self, record: &PluginRecord) -> DomainResult<()>;
    async fn get_plugin(&self, plugin_id: &str) -> DomainResult<Option<PluginRecord>>;
    async fn list_plugins(&self) -> DomainResult<Vec<PluginRecord>>;
    async fn delete_plugin(&self, plugin_id: &str) -> DomainResult<()>;
}

/// Records audit events. Implemented by `forge_audit::AuditLog`.
#[async_trait]
pub trait AuditRecorder: Send + Sync + std::fmt::Debug {
    async fn record(&self, event: AuditEvent) -> DomainResult<AuditEntry>;
}

#[async_trait]
pub trait ClusterRepository: Send + Sync + std::fmt::Debug {
    async fn create_cluster(&self, cluster: &Cluster) -> DomainResult<()>;
    async fn get_cluster(&self, id: ClusterId) -> DomainResult<Cluster>;
    async fn list_clusters(&self, workspace: WorkspaceId) -> DomainResult<Vec<Cluster>>;
    async fn update_cluster(&self, cluster: &Cluster) -> DomainResult<()>;
    /// Switches the cluster's mode (a checked conversion, D-171).
    async fn set_cluster_mode(&self, id: ClusterId, mode: ClusterMode) -> DomainResult<()>;
    /// Deletes a cluster; its nodes become unassigned.
    async fn delete_cluster(&self, id: ClusterId) -> DomainResult<()>;
    async fn assign_server(
        &self,
        server: ServerId,
        cluster: Option<ClusterId>,
        environment: Option<EnvironmentId>,
    ) -> DomainResult<()>;
}

/// SSH endpoints of nodes (browser terminal).
#[async_trait]
pub trait SshEndpointRepository: Send + Sync + std::fmt::Debug {
    async fn get_ssh_endpoint(&self, server: ServerId) -> DomainResult<Option<SshEndpoint>>;
    /// Inserts or replaces the endpoint of `endpoint.server_id`.
    async fn put_ssh_endpoint(&self, endpoint: &SshEndpoint) -> DomainResult<()>;
}

/// Nodes' Docker TLS endpoints (D-125).
#[async_trait]
pub trait DockerEndpointRepository: Send + Sync + std::fmt::Debug {
    async fn get_docker_endpoint(&self, server: ServerId) -> DomainResult<Option<crate::model::DockerEndpoint>>;
    /// Inserts or replaces the endpoint of `endpoint.server_id`.
    async fn put_docker_endpoint(&self, endpoint: &crate::model::DockerEndpoint) -> DomainResult<()>;
    async fn delete_docker_endpoint(&self, server: ServerId) -> DomainResult<()>;
}

/// Service log ingest keys (D-128). Only token hashes are stored.
#[async_trait]
pub trait LogKeyRepository: Send + Sync + std::fmt::Debug {
    async fn create_log_key(&self, key: &crate::logs::LogKey, token_hash: &str) -> DomainResult<()>;
    async fn list_log_keys(&self, service: ServiceId) -> DomainResult<Vec<crate::logs::LogKey>>;
    async fn get_log_key(&self, id: LogKeyId) -> DomainResult<Option<crate::logs::LogKey>>;
    /// The key whose token hashes to `token_hash`.
    async fn find_log_key(&self, token_hash: &str) -> DomainResult<Option<crate::logs::LogKey>>;
    async fn delete_log_key(&self, id: LogKeyId) -> DomainResult<()>;
    /// Records use; called at most once a minute per key.
    async fn touch_log_key(&self, id: LogKeyId, at: Timestamp) -> DomainResult<()>;
    /// Every key's token hash and service, at most `limit`, for the
    /// analytics worker's maps.
    async fn list_log_key_hashes(&self, limit: u32) -> DomainResult<Vec<crate::analytics::KeyService>>;
}

/// Reverse-proxy sites.
#[async_trait]
pub trait SiteRepository: Send + Sync + std::fmt::Debug {
    async fn put_site(&self, site: &Site) -> DomainResult<()>;
    async fn get_site(&self, id: &str) -> DomainResult<Option<Site>>;
    async fn list_sites(&self) -> DomainResult<Vec<Site>>;
    async fn delete_site(&self, id: &str) -> DomainResult<()>;
}

/// Per-cluster orchestrator / network / ingress profile.
#[async_trait]
pub trait ClusterProfileRepository: Send + Sync + std::fmt::Debug {
    async fn put_cluster_profile(&self, p: &ClusterProfileRecord) -> DomainResult<()>;
    async fn get_cluster_profile(&self, cluster: ClusterId) -> DomainResult<Option<ClusterProfileRecord>>;
    async fn delete_cluster_profile(&self, cluster: ClusterId) -> DomainResult<()>;
    /// Every stored profile (background health checks).
    async fn list_cluster_profiles(&self) -> DomainResult<Vec<ClusterProfileRecord>>;
}

/// Per-cluster Forge network fabric.
#[async_trait]
pub trait FabricRepository: Send + Sync + std::fmt::Debug {
    async fn put_fabric(&self, f: &FabricRecord) -> DomainResult<()>;
    async fn get_fabric(&self, cluster: ClusterId) -> DomainResult<Option<FabricRecord>>;
    /// Fabrics of one workspace, or of every workspace with `None`.
    async fn list_fabrics(&self, workspace: Option<WorkspaceId>) -> DomainResult<Vec<FabricRecord>>;
    async fn delete_fabric(&self, cluster: ClusterId) -> DomainResult<()>;
}

/// Credential vault items. Implementations never see plaintext.
#[async_trait]
pub trait VaultRepository: Send + Sync + std::fmt::Debug {
    async fn put_vault_item(&self, item: &VaultItem) -> DomainResult<()>;
    async fn get_vault_item(&self, id: VaultItemId) -> DomainResult<Option<VaultItem>>;
    async fn list_vault_items(&self, workspace: WorkspaceId) -> DomainResult<Vec<VaultItem>>;
    async fn delete_vault_item(&self, id: VaultItemId) -> DomainResult<()>;
}

/// Uploaded TLS certificates. Implementations never see a plaintext key.
#[async_trait]
pub trait CertificateRepository: Send + Sync + std::fmt::Debug {
    async fn put_certificate(&self, c: &TlsCertificate) -> DomainResult<()>;
    async fn get_certificate(&self, id: CertificateId) -> DomainResult<Option<TlsCertificate>>;
    async fn list_certificates(&self, workspace: Option<WorkspaceId>) -> DomainResult<Vec<TlsCertificate>>;
    async fn delete_certificate(&self, id: CertificateId) -> DomainResult<()>;
}

/// Instance-wide settings (key/value).
#[async_trait]
pub trait SettingsRepository: Send + Sync + std::fmt::Debug {
    async fn all_settings(&self) -> DomainResult<Vec<(String, String)>>;
    async fn put_setting(&self, key: &str, value: &str, now: Timestamp) -> DomainResult<()>;
    async fn delete_setting(&self, key: &str) -> DomainResult<()>;
}

/// Personal API keys.
#[async_trait]
pub trait ApiKeyRepository: Send + Sync + std::fmt::Debug {
    async fn create_api_key(&self, key: &ApiKey) -> DomainResult<()>;
    async fn api_key_by_hash(&self, hash: &str) -> DomainResult<Option<ApiKey>>;
    async fn list_api_keys(&self, user: UserId) -> DomainResult<Vec<ApiKey>>;
    async fn delete_api_key(&self, user: UserId, id: &str) -> DomainResult<bool>;
    /// Deletes every API key of `user`; returns how many.
    async fn delete_user_api_keys(&self, user: UserId) -> DomainResult<u64>;
    async fn touch_api_key(&self, id: &str, now: Timestamp) -> DomainResult<()>;
}

/// Services and their placements.
#[async_trait]
pub trait ServiceRepository: Send + Sync + std::fmt::Debug {
    /// Inserts or replaces by id.
    async fn put_service(&self, app: &Service) -> DomainResult<()>;
    /// Saves an existing service only when its stored version is still
    /// `expected`, bumping it. `false` when someone saved in between.
    async fn update_service_if(&self, app: &Service, expected: i64) -> DomainResult<bool>;
    async fn get_service(&self, id: ServiceId) -> DomainResult<Option<Service>>;
    /// The services with these ids, in no particular order; unknown ids
    /// are skipped. One query per 200 ids instead of one per service.
    async fn get_services(&self, ids: &[ServiceId]) -> DomainResult<Vec<Service>>;
    async fn list_services(&self, environment: Option<EnvironmentId>) -> DomainResult<Vec<Service>>;
    /// Deletes the service and its placements.
    async fn delete_service(&self, id: ServiceId) -> DomainResult<()>;
    async fn put_instance(&self, i: &ServiceInstance) -> DomainResult<()>;
    async fn list_instances(&self, app: ServiceId) -> DomainResult<Vec<ServiceInstance>>;
    /// Placements of every service, or of one environment's (list views:
    /// one query instead of one per service).
    async fn list_all_instances(&self, environment: Option<EnvironmentId>) -> DomainResult<Vec<ServiceInstance>>;
    /// Services in a cluster's environments, and their placements.
    async fn list_cluster_services(&self, cluster: ClusterId) -> DomainResult<(Vec<Service>, Vec<ServiceInstance>)>;
    async fn delete_instance(&self, app: ServiceId, server: ServerId) -> DomainResult<()>;
    /// The services placed on one node (one query).
    async fn list_server_services(&self, server: ServerId) -> DomainResult<Vec<Service>>;
    /// Marks services as needing a redeploy because a vault item they
    /// mount as a file changed at `at` (D-179). Keeps an earlier mark.
    async fn mark_secrets_changed(&self, ids: &[ServiceId], at: Timestamp) -> DomainResult<()> {
        let _ = (ids, at);
        Err(crate::error::DomainError::Internal("secret file marks are not supported by this store".into()))
    }
    /// Clears that mark when it is no later than `deploy_started` (a deploy
    /// that began before the change does not carry the new value).
    async fn clear_secrets_changed(&self, id: ServiceId, deploy_started: Timestamp) -> DomainResult<()> {
        let _ = (id, deploy_started);
        Err(crate::error::DomainError::Internal("secret file marks are not supported by this store".into()))
    }
    /// Records what the node reports for an instance (D-177): its state and
    /// last exit code, without touching status or the last job. `false`
    /// when there is no such instance.
    async fn observe_instance(
        &self,
        app: ServiceId,
        server: ServerId,
        observed: &InstanceObservation,
    ) -> DomainResult<bool> {
        let _ = (app, server, observed);
        Err(crate::error::DomainError::Internal("instance reports are not supported by this store".into()))
    }
    /// Sets an instance's status to `to` only while it is still `from`
    /// (D-177), so a background change never overwrites a job's. `false`
    /// when it was not `from`.
    async fn swap_instance_status(&self, app: ServiceId, server: ServerId, change: &StatusSwap) -> DomainResult<bool> {
        let _ = (app, server, change);
        Err(crate::error::DomainError::Internal("instance status changes are not supported by this store".into()))
    }
}

/// What a node reported for a service instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceObservation {
    /// The container state (`running`, `exited`, …); `None` when the node
    /// has no container for it.
    pub state: Option<String>,
    pub exit_code: Option<i64>,
    pub at: Timestamp,
}

/// A conditional instance status change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusSwap {
    pub from: String,
    pub to: String,
    /// Replaces `last_error` when set; `Some("")` clears it.
    pub error: Option<String>,
    pub at: Timestamp,
}

/// Who last saved a service's health policy while it referred to
/// vault items (D-111): the person on whose authority a node gets the
/// header values of such a check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthAuthority {
    pub service_id: ServiceId,
    pub saved_by: crate::access::Principal,
    pub saved_at: Timestamp,
    /// The revision of the policy they saved: the authority holds for that
    /// policy only, not for one that changed some other way.
    pub revision: i64,
}

/// One [`HealthAuthority`] per service; deleted with the service.
#[async_trait]
pub trait HealthAuthorityRepository: Send + Sync + std::fmt::Debug {
    /// Stores a changed health policy: the service (as
    /// `put_service`) and who saved it, in one transaction. `None`
    /// removes the record (a policy without vault references).
    async fn save_health_policy(&self, app: &Service, authority: Option<&HealthAuthority>) -> DomainResult<()>;
    async fn get_health_authority(&self, app: ServiceId) -> DomainResult<Option<HealthAuthority>>;
}

/// App Store installations.
#[async_trait]
pub trait InstallationRepository: Send + Sync + std::fmt::Debug {
    /// Inserts or replaces by id.
    async fn put_installation(&self, i: &Installation) -> DomainResult<()>;
    async fn get_installation(&self, id: InstallationId) -> DomainResult<Option<Installation>>;
    async fn find_installation(
        &self,
        server: ServerId,
        package: &str,
        version: &str,
    ) -> DomainResult<Option<Installation>>;
    async fn list_installations(&self, server: Option<ServerId>) -> DomainResult<Vec<Installation>>;
    async fn delete_installation(&self, id: InstallationId) -> DomainResult<()>;
    /// Replaces the stored release lines of one product.
    async fn put_releases(&self, product: &str, releases: &[PackageRelease]) -> DomainResult<()>;
    async fn list_releases(&self) -> DomainResult<Vec<PackageRelease>>;
}

/// Storage of sealed secrets. Implementations never see plaintext.
#[async_trait]
pub trait SecretRepository: Send + Sync + std::fmt::Debug {
    async fn put_secret(&self, secret: &SecretRecord) -> DomainResult<()>;
    async fn get_secret(&self, id: SecretId) -> DomainResult<Option<SecretRecord>>;
    async fn list_secrets(&self, workspace: WorkspaceId) -> DomainResult<Vec<SecretRecord>>;
    async fn delete_secret(&self, id: SecretId) -> DomainResult<()>;
    /// Deletes expired secrets; returns how many were purged.
    async fn purge_expired(&self, now: Timestamp) -> DomainResult<u64>;
}
