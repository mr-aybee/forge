//! Object storage: stores Forge installs and manages on nodes (RustFS first,
//! Garage later behind the same model). The engine stores, distributes and
//! recovers data; Forge tracks where it runs, how to reach it and which
//! credentials manage it.
//!
//! * [`ObjectStore`]: one deployment (standalone or distributed) with its
//!   provider, release, runtime, endpoint and management credential.
//! * [`StoreMember`]: one node of a store with its exclusive data path (the
//!   volume attachment) and observed capacity.
//! * [`BucketGrant`]: one person's access to one bucket of a store, given by
//!   an administrator without a role (D-108).

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{
    BucketGrantId, DomainResult, EnvironmentId, JobId, ObjectStoreId, Principal, ServerId, Timestamp, UserId,
    VaultItemId, WorkspaceId,
};

pub const MODE_MANAGED: &str = "managed";
pub const MODE_EXTERNAL: &str = "external";
pub const TOPOLOGY_STANDALONE: &str = "standalone";
/// RustFS erasure-coded cluster across several nodes (high availability).
pub const TOPOLOGY_DISTRIBUTED: &str = "distributed";

/// One object store: a deployment Forge manages, or a connected service.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectStore {
    pub id: ObjectStoreId,
    pub workspace_id: WorkspaceId,
    /// The environment the store belongs to (D-107); `None` for
    /// a shared platform store. Changed only through
    /// [`ObjectStoreRepository::set_object_store_owner`].
    #[serde(default)]
    pub environment_id: Option<EnvironmentId>,
    /// Name shown in the console.
    pub name: String,
    /// Instance name on the node: service, container and path suffix.
    /// Unique per node, so several stores can share one node.
    pub slug: String,
    /// `rustfs` (`garage` later) for managed stores; for connected ones the
    /// kind of service: `aws`, `s3` (any S3-compatible), `rustfs`, `minio`...
    pub provider: String,
    /// `managed`: Forge installed it and runs its lifecycle on member nodes.
    /// `external`: an existing S3 service (cloud or a node Forge does not
    /// manage) connected with a credential; buckets and objects only.
    pub mode: String,
    /// S3 signing region (`us-east-1` for most S3-compatible services).
    pub region: String,
    /// Path-style requests (`endpoint/bucket/key`) instead of virtual-hosted
    /// (`bucket.endpoint/key`). Self-hosted stores need path style.
    pub path_style: bool,
    /// Release requested at install (`latest` or an exact release).
    pub version: String,
    /// Release the node reported after install or status.
    pub deployed_version: Option<String>,
    /// `native` (system service) or `docker`.
    pub runtime: String,
    /// `standalone` (distributed layouts later).
    pub topology: String,
    /// S3 API port on the member nodes.
    pub s3_port: u16,
    /// Provider console port, if the console is enabled.
    pub console_port: Option<u16>,
    /// Address the control plane uses for S3 calls: `http(s)://host:port`.
    pub endpoint: String,
    /// Vault item holding the access key (username) and secret key: the
    /// bootstrap (root) credential of a managed store, or the credential a
    /// connected store was added with.
    pub credential_id: VaultItemId,
    /// `installing`, `running`, `stopped`, `failed`, `removing`, `missing`...
    pub status: String,
    pub last_error: Option<String>,
    pub last_job_id: Option<JobId>,
    pub created_by: Principal,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

/// One node of a store and its drives (volume attachments).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoreMember {
    pub store_id: ObjectStoreId,
    pub server_id: ServerId,
    /// Exclusive data paths on the node, one per drive (dedicated mounts or
    /// directories). Distributed stores have the same count on every member.
    pub data_paths: Vec<String>,
    /// Address the other members and the control plane reach this node on.
    pub endpoint_host: String,
    /// Health observed on the last check: `running`, `stopped`, `missing`...
    pub status: String,
    /// Size and free space of the filesystems holding the data paths, in bytes.
    pub capacity_bytes: Option<u64>,
    pub free_bytes: Option<u64>,
    /// Refuse to start unless every data path is its own mount point.
    pub require_mount: bool,
    /// Whether every data path is its own mount point (dedicated volumes).
    pub dedicated: Option<bool>,
    pub observed_at: Option<Timestamp>,
}

/// Object stores and their members.
#[async_trait]
pub trait ObjectStoreRepository: Send + Sync + std::fmt::Debug {
    /// Inserts the store, or updates its mutable fields. The owner
    /// (`environment_id`) is written on insert only, so a job that saves a
    /// store it loaded earlier never undoes a change of owner.
    async fn put_object_store(&self, store: &ObjectStore) -> DomainResult<()>;
    /// Assigns the store to an environment, or clears the owner (`None`),
    /// and sets `updated_at`. `NotFound` when the store, or the environment it
    /// is assigned to, does not exist: the check and the write are one
    /// statement, so an environment deleted meanwhile is never recorded.
    async fn set_object_store_owner(
        &self,
        id: ObjectStoreId,
        environment: Option<EnvironmentId>,
        at: Timestamp,
    ) -> DomainResult<()>;
    async fn get_object_store(&self, id: ObjectStoreId) -> DomainResult<Option<ObjectStore>>;
    /// All stores, or those of one workspace.
    async fn list_object_stores(&self, workspace: Option<WorkspaceId>) -> DomainResult<Vec<ObjectStore>>;
    /// Deletes the store, its members and the bucket grants on it, in one
    /// transaction (never the data on the nodes).
    async fn delete_object_store(&self, id: ObjectStoreId) -> DomainResult<()>;
    async fn put_store_member(&self, member: &StoreMember) -> DomainResult<()>;
    async fn list_store_members(&self, store: ObjectStoreId) -> DomainResult<Vec<StoreMember>>;
    /// Members on one node (to keep slugs, ports and data paths distinct).
    async fn list_node_store_members(&self, server: ServerId) -> DomainResult<Vec<StoreMember>>;
}

/// What a key or a grant may do in its bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BucketAccess {
    /// List and download objects.
    Read,
    /// Also upload and delete objects.
    #[serde(rename = "readwrite")]
    ReadWrite,
}

impl BucketAccess {
    pub const ALL: [BucketAccess; 2] = [Self::Read, Self::ReadWrite];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::ReadWrite => "readwrite",
        }
    }

    #[must_use]
    pub const fn writes(self) -> bool {
        matches!(self, Self::ReadWrite)
    }

    /// Whether holding `self` is enough for a request that needs `need`.
    #[must_use]
    pub const fn allows(self, need: BucketAccess) -> bool {
        self.writes() || !need.writes()
    }

    /// The access named `s`; `None` for any other word.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.as_str() == s)
    }
}

impl std::fmt::Display for BucketAccess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Where a grant stands at one moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GrantStatus {
    Active,
    Expired,
    Revoked,
}

/// Explicit permission for one person to work with the objects of one bucket
/// through Forge (console, API, CLI), issued by someone who may manage the
/// store, for people without a role on the store's node, workspace or
/// environment (D-108). The grant alone opens the bucket; it never issues an
/// S3 credential and never reaches anything else of the store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BucketGrant {
    pub id: BucketGrantId,
    pub workspace_id: WorkspaceId,
    pub user_id: UserId,
    pub store_id: ObjectStoreId,
    /// The bucket's exact name.
    pub bucket: String,
    pub access: BucketAccess,
    /// Principal of the person who issued it.
    pub created_by: String,
    pub created_at: Timestamp,
    /// [`crate::NEVER_EXPIRES`] for a grant without an end.
    pub expires_at: Timestamp,
    pub revoked_at: Option<Timestamp>,
    pub note: String,
}

impl BucketGrant {
    /// Not revoked and not expired at `now`.
    #[must_use]
    pub fn is_active(&self, now: Timestamp) -> bool {
        self.revoked_at.is_none() && now < self.expires_at
    }

    #[must_use]
    pub fn never_expires(&self) -> bool {
        self.expires_at >= crate::NEVER_EXPIRES
    }

    #[must_use]
    pub fn status(&self, now: Timestamp) -> GrantStatus {
        if self.revoked_at.is_some() {
            GrantStatus::Revoked
        } else if now >= self.expires_at {
            GrantStatus::Expired
        } else {
            GrantStatus::Active
        }
    }

    /// Whether this grant lets `user` do what needs `need` in `bucket` of
    /// `store` at `now`.
    #[must_use]
    pub fn covers(&self, user: UserId, store: ObjectStoreId, bucket: &str, need: BucketAccess, now: Timestamp) -> bool {
        self.is_active(now)
            && self.user_id == user
            && self.store_id == store
            && self.bucket == bucket
            && self.access.allows(need)
    }
}

/// Bucket grants (D-108).
#[async_trait]
pub trait BucketGrantRepository: Send + Sync + std::fmt::Debug {
    /// Inserts the grant, or records its revocation (the only field that
    /// changes after a grant is issued). A revocation is final: saving a
    /// grant that is already revoked keeps the first `revoked_at`, and
    /// saving it without one does not bring it back.
    async fn put_bucket_grant(&self, grant: &BucketGrant) -> DomainResult<()>;
    async fn get_bucket_grant(&self, id: BucketGrantId) -> DomainResult<Option<BucketGrant>>;
    /// The grants of one person that are not revoked and end after `now`,
    /// newest first. A person holds few; the list is not paged.
    async fn list_live_bucket_grants(&self, user: UserId, now: Timestamp) -> DomainResult<Vec<BucketGrant>>;
    /// The grants for one bucket of a store that are not revoked and end
    /// after `now`, whoever holds them.
    async fn list_live_grants_of_bucket(
        &self,
        store: ObjectStoreId,
        bucket: &str,
        now: Timestamp,
    ) -> DomainResult<Vec<BucketGrant>>;
    /// The grants on one store, newest first, at most `limit` rows: every
    /// grant, or with `live_at` only those not revoked that end after it.
    async fn list_store_bucket_grants(
        &self,
        store: ObjectStoreId,
        live_at: Option<Timestamp>,
        limit: u32,
    ) -> DomainResult<Vec<BucketGrant>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grant() -> BucketGrant {
        BucketGrant {
            id: BucketGrantId::new(),
            workspace_id: WorkspaceId::new(),
            user_id: UserId::new(),
            store_id: ObjectStoreId::new(),
            bucket: "media".into(),
            access: BucketAccess::Read,
            created_by: "user:x".into(),
            created_at: Timestamp(0),
            expires_at: Timestamp(1_000),
            revoked_at: None,
            note: String::new(),
        }
    }

    #[test]
    fn access_names_round_trip_and_readwrite_includes_read() {
        for a in BucketAccess::ALL {
            assert_eq!(BucketAccess::parse(a.as_str()), Some(a));
        }

        assert_eq!(BucketAccess::parse("admin"), None);
        assert!(BucketAccess::Read.allows(BucketAccess::Read));
        assert!(!BucketAccess::Read.allows(BucketAccess::ReadWrite));
        assert!(BucketAccess::ReadWrite.allows(BucketAccess::Read));
        assert!(BucketAccess::ReadWrite.allows(BucketAccess::ReadWrite));
    }

    #[test]
    fn a_grant_covers_its_bucket_only_while_active() {
        let g = grant();
        let now = Timestamp(500);
        let read = BucketAccess::Read;
        assert!(g.covers(g.user_id, g.store_id, "media", read, now));
        assert!(!g.covers(UserId::new(), g.store_id, "media", read, now), "another person");
        assert!(!g.covers(g.user_id, ObjectStoreId::new(), "media", read, now), "another store");
        assert!(!g.covers(g.user_id, g.store_id, "media-2", read, now), "another bucket");
        assert!(!g.covers(g.user_id, g.store_id, "Media", read, now), "names are exact");
        assert!(!g.covers(g.user_id, g.store_id, "media", BucketAccess::ReadWrite, now), "read does not write");
        assert!(!g.covers(g.user_id, g.store_id, "media", read, Timestamp(1_000)), "expired");
        assert_eq!(g.status(Timestamp(1_000)), GrantStatus::Expired);
        let mut revoked = g.clone();
        revoked.revoked_at = Some(Timestamp(400));
        assert!(!revoked.covers(g.user_id, g.store_id, "media", read, now));
        assert_eq!(revoked.status(now), GrantStatus::Revoked);
        let mut forever = g;
        forever.expires_at = crate::NEVER_EXPIRES;
        forever.access = BucketAccess::ReadWrite;
        assert!(forever.never_expires());
        assert_eq!(forever.status(Timestamp(crate::NEVER_EXPIRES.0 - 1)), GrantStatus::Active);
        assert!(forever.covers(forever.user_id, forever.store_id, "media", BucketAccess::ReadWrite, now));
    }
}
