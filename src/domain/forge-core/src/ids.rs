//! Strongly typed identifiers. Every id is a UUIDv7 (time-ordered) wrapped in
//! its own type so an `WorkspaceId` can never be passed where a `ServerId` is due.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

macro_rules! typed_id {
    ($($(#[$m:meta])* $name:ident),+ $(,)?) => {$(
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub Uuid);

        impl $name {
            /// Generates a new time-ordered identifier.
            #[must_use]
            pub fn new() -> Self { Self(Uuid::now_v7()) }
        }

        impl Default for $name {
            fn default() -> Self { Self::new() }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { self.0.fmt(f) }
        }

        impl FromStr for $name {
            type Err = uuid::Error;
            fn from_str(s: &str) -> Result<Self, Self::Err> { Uuid::parse_str(s).map(Self) }
        }
    )+};
}

typed_id!(
    /// Organization identifier.
    WorkspaceId,
    /// Tenant (customer) identifier, always inside one org.
    TenantId,
    /// Environment identifier (dev, uat, staging, prod...).
    EnvironmentId,
    /// Project identifier: the product that groups environments (D-102).
    ProjectId,
    /// Managed server identifier; also the agent's identity in its certificate.
    ServerId,
    /// Service identifier (Phase 3 extension point).
    ServiceId,
    /// Immutable release identifier (#73).
    ReleaseId,
    /// One rollout of a release (#73).
    DeploymentId,
    /// Cluster (named group of nodes) identifier.
    ClusterId,
    /// Vault secret identifier.
    SecretId,
    /// Credential vault item identifier.
    VaultItemId,
    /// Uploaded TLS certificate identifier.
    CertificateId,
    /// Durable job identifier.
    JobId,
    /// Alert rule identifier (#11).
    AlertRuleId,
    /// Alarm episode identifier (#11).
    AlarmId,
    /// Notification channel identifier (#11).
    ChannelId,
    /// Notification policy identifier (#11).
    NotificationPolicyId,
    /// Notification delivery identifier (#11).
    DeliveryId,
    /// Silence (or maintenance window) identifier (#11).
    SilenceId,
    /// Compose stack identifier (issue #7).
    StackId,
    /// Container registry identifier (D-158).
    RegistryId,
    /// Deploy webhook identifier (D-159).
    WebhookId,
    /// Webhook delivery identifier (D-159).
    WebhookDeliveryId,
    /// Human or service user identifier.
    UserId,
    /// Role binding identifier (one Cedar template link).
    RoleBindingId,
    /// Installed plugin record identifier.
    PluginInstallId,
    InstallationId,
    /// Object store (one RustFS or Garage deployment) identifier.
    ObjectStoreId,
    /// Explicit tunnel grant identifier (D-100).
    TunnelGrantId,
    /// Bucket grant identifier: one person's access to one bucket (D-108).
    BucketGrantId,
    /// Forge's record of one RustFS access key it manages (cache and audit trail).
    StoreKeyId,
    /// Authorization profile (a named, versioned set of actions; D-086).
    PermissionProfileId,
    /// One assignment of an authorization profile at a scope (D-086).
    ProfileAssignmentId,
    /// A group of users that access profiles can be assigned to (#13).
    GroupId,
    /// A service log ingest key (Seq-compatible API key).
    LogKeyId,
);
