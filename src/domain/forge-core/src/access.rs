//! Access-control vocabulary shared by the API, job runner, agent hub and
//! plugin host. The Cedar engine in `forge-authz` is one implementation of
//! the [`crate::ports::Authorizer`] port that consumes these types.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::ids::UserId;

/// Built-in roles. Each role maps to a Cedar policy template that is linked
/// once per role binding (user + scope).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    PlatformAdmin,
    WorkspaceAdmin,
    Operator,
    Developer,
    Dba,
    Auditor,
    Customer,
}

impl Role {
    pub const ALL: [Role; 7] = [
        Role::PlatformAdmin,
        Role::WorkspaceAdmin,
        Role::Operator,
        Role::Developer,
        Role::Dba,
        Role::Auditor,
        Role::Customer,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Role::PlatformAdmin => "platform_admin",
            Role::WorkspaceAdmin => "workspace_admin",
            Role::Operator => "operator",
            Role::Developer => "developer",
            Role::Dba => "dba",
            Role::Auditor => "auditor",
            Role::Customer => "customer",
        }
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Role {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Role::ALL.into_iter().find(|r| r.as_str() == s).ok_or_else(|| format!("unknown role `{s}`"))
    }
}

/// Every action Forge authorizes. Phase 2+ actions are declared now so the
/// Cedar schema and role templates never need a breaking change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Action {
    #[serde(rename = "view")]
    View,
    #[serde(rename = "audit.read")]
    AuditRead,
    #[serde(rename = "workspace.create")]
    WorkspaceCreate,
    #[serde(rename = "workspace.manage")]
    WorkspaceManage,
    #[serde(rename = "user.manage")]
    UserManage,
    #[serde(rename = "server.enroll")]
    ServerEnroll,
    #[serde(rename = "node.onboard")]
    NodeOnboard,
    #[serde(rename = "server.manage")]
    ServerManage,
    #[serde(rename = "server.command")]
    ServerCommand,
    /// End processes (one or a whole tree) and change process priority on a
    /// node. Root-equivalent: MFA.
    #[serde(rename = "process.manage")]
    ProcessManage,
    /// Open port tunnels from the CLI to a service's ports (D-091, D-100,
    /// MFA): on an environment or service, its services; on a server, the
    /// App Store packages installed there.
    #[serde(rename = "network.tunnel")]
    NetworkTunnel,
    /// Open a tunnel to any port of a node, listed or not (D-100). An
    /// administrator exception, MFA.
    #[serde(rename = "network.tunnel.any")]
    NetworkTunnelAny,
    /// Rename a node: its console name and its DNS label (D-084).
    #[serde(rename = "server.rename")]
    ServerRename,
    /// Drain or undrain a node (D-177): placement skips it and the service
    /// reconciler moves its movable services elsewhere.
    #[serde(rename = "node.drain")]
    NodeDrain,
    /// See alarms and alert rules (#11).
    #[serde(rename = "alert.view")]
    AlertView,
    /// Create, change and delete alert rules; acknowledge alarms and add notes (#11).
    #[serde(rename = "alert.manage")]
    AlertManage,
    /// Create, change, delete and test notification channels: where alarm
    /// messages leave Forge (#11, D-110). Administrators, MFA.
    #[serde(rename = "notification.manage")]
    NotificationManage,
    /// Install, remove, start or stop App Store packages on a node (root).
    #[serde(rename = "package.manage")]
    PackageManage,
    /// Start, stop, restart, remove containers and pull images on a node.
    #[serde(rename = "container.manage")]
    ContainerManage,
    /// Read a container's environment *values* on `inspect` (they often hold
    /// secrets): MFA guardrail; never persisted.
    #[serde(rename = "container.inspect_env")]
    ContainerInspectEnv,
    /// Open an interactive shell inside a container (`docker exec`) through
    /// the node's terminal account. Not under the MFA guardrail (D-100).
    #[serde(rename = "container.exec")]
    ContainerExec,
    /// Configure reverse-proxy sites and DNS on a node (MFA).
    #[serde(rename = "proxy.manage")]
    ProxyManage,
    #[serde(rename = "job.create")]
    JobCreate,
    #[serde(rename = "job.cancel")]
    JobCancel,
    #[serde(rename = "plugin.manage")]
    PluginManage,
    #[serde(rename = "plugin.invoke")]
    PluginInvoke,
    /// Read health checks and their observations (D-085).
    #[serde(rename = "health.view")]
    HealthView,
    /// Change a service's health checks (HTTP and TCP probes).
    #[serde(rename = "health.configure")]
    HealthConfigure,
    /// Health checks that run code on the node (exec, shell). MFA.
    #[serde(rename = "health.command")]
    HealthCommand,
    /// Read a service's logs in the log console (D-128).
    #[serde(rename = "log.read")]
    LogRead,
    /// Create and revoke a service's log ingest keys (D-128).
    #[serde(rename = "log.manage")]
    LogManage,
    // ---- extension points (Phase 2+) ----
    #[serde(rename = "terminal.open")]
    TerminalOpen,
    #[serde(rename = "deploy")]
    Deploy,
    /// Approve or reject a deployment that waits for an approval (strategy
    /// approval gates, manual strategy batches). Not developers.
    #[serde(rename = "deploy.approve")]
    DeployApprove,
    #[serde(rename = "secret.read")]
    SecretRead,
    /// Create, publish and archive access profiles; create, disable and
    /// issue tokens for service accounts (D-086). Administrators, MFA.
    #[serde(rename = "access.manage")]
    AccessManage,
    /// Create, change and delete credential vault items. Platform admins,
    /// MFA (#13 inventory: the vault used a role lookup instead of Cedar).
    #[serde(rename = "secret.manage")]
    SecretManage,
    #[serde(rename = "db.admin")]
    DbAdmin,
    /// Update Forge itself (download, verify and install a release).
    #[serde(rename = "system.update")]
    SystemUpdate,
    /// Change analytics retention, resource limits and search and ingest
    /// limits, and restart the managed analytics engine (D-136).
    #[serde(rename = "analytics.configure")]
    AnalyticsConfigure,
    /// List, stat, view, download and search files on a node.
    #[serde(rename = "file.read")]
    FileRead,
    /// Create, upload, save, rename and copy files (node-level: MFA).
    #[serde(rename = "file.write")]
    FileWrite,
    /// Delete files and empty the trash (node-level: MFA).
    #[serde(rename = "file.delete")]
    FileDelete,
    /// Protected roots, ownership, permissions and ACLs (MFA).
    #[serde(rename = "file.admin")]
    FileAdmin,
    /// Open the whole-node file root (`/`, every drive). Without it a user
    /// sees only approved service and container directories.
    #[serde(rename = "file.node")]
    FileNode,
    /// Create, deploy, update and remove compose stacks in an environment (issue #7).
    #[serde(rename = "stack.manage")]
    StackManage,
    /// Deploy stacks with host-access settings (`privileged`, host network,
    /// Docker socket, binds outside approved roots); MFA, administrators.
    #[serde(rename = "stack.privileged")]
    StackPrivileged,
    /// Create, join and leave a Docker Swarm, and change swarm nodes (role,
    /// availability, labels, removal) in a Swarm cluster (D-154).
    /// Administrators, MFA.
    #[serde(rename = "swarm.manage")]
    SwarmManage,
    /// Pull, push, tag, remove and prune Docker images on a node (D-157).
    #[serde(rename = "docker.image.manage")]
    ImageManage,
    /// Create, remove and prune Docker volumes on a node (D-157).
    #[serde(rename = "docker.volume.manage")]
    VolumeManage,
    /// Create, remove, connect and prune Docker networks on a node (D-157).
    #[serde(rename = "docker.network.manage")]
    NetworkManage,
    /// Add, change and remove container registries and their credentials
    /// (D-158). Administrators, MFA.
    #[serde(rename = "registry.manage")]
    RegistryManage,
    /// Create, rotate and remove deploy webhooks (D-159). A webhook is a
    /// long-lived deploy credential: MFA.
    #[serde(rename = "webhook.manage")]
    WebhookManage,
    /// Send an uploaded TLS certificate's private key to an edge node when an
    /// endpoint is applied (administrators, MFA).
    #[serde(rename = "certificate.use")]
    CertificateUse,
}

impl Action {
    pub const ALL: [Action; 54] = [
        Action::View,
        Action::AuditRead,
        Action::WorkspaceCreate,
        Action::WorkspaceManage,
        Action::UserManage,
        Action::ServerEnroll,
        Action::NodeOnboard,
        Action::ServerManage,
        Action::ServerCommand,
        Action::ProcessManage,
        Action::NetworkTunnel,
        Action::NetworkTunnelAny,
        Action::ServerRename,
        Action::NodeDrain,
        Action::AlertView,
        Action::AlertManage,
        Action::NotificationManage,
        Action::PackageManage,
        Action::ContainerManage,
        Action::ContainerInspectEnv,
        Action::ContainerExec,
        Action::ProxyManage,
        Action::JobCreate,
        Action::JobCancel,
        Action::PluginManage,
        Action::PluginInvoke,
        Action::HealthView,
        Action::HealthConfigure,
        Action::HealthCommand,
        Action::LogRead,
        Action::LogManage,
        Action::TerminalOpen,
        Action::Deploy,
        Action::DeployApprove,
        Action::SecretRead,
        Action::AccessManage,
        Action::SecretManage,
        Action::DbAdmin,
        Action::SystemUpdate,
        Action::AnalyticsConfigure,
        Action::FileRead,
        Action::FileWrite,
        Action::FileDelete,
        Action::FileAdmin,
        Action::FileNode,
        Action::StackManage,
        Action::StackPrivileged,
        Action::SwarmManage,
        Action::ImageManage,
        Action::VolumeManage,
        Action::NetworkManage,
        Action::RegistryManage,
        Action::WebhookManage,
        Action::CertificateUse,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Action::View => "view",
            Action::AuditRead => "audit.read",
            Action::WorkspaceCreate => "workspace.create",
            Action::WorkspaceManage => "workspace.manage",
            Action::UserManage => "user.manage",
            Action::ServerEnroll => "server.enroll",
            Action::NodeOnboard => "node.onboard",
            Action::ServerManage => "server.manage",
            Action::ServerCommand => "server.command",
            Action::ProcessManage => "process.manage",
            Action::NetworkTunnel => "network.tunnel",
            Action::NetworkTunnelAny => "network.tunnel.any",
            Action::ServerRename => "server.rename",
            Action::NodeDrain => "node.drain",
            Action::AlertView => "alert.view",
            Action::AlertManage => "alert.manage",
            Action::NotificationManage => "notification.manage",
            Action::PackageManage => "package.manage",
            Action::ContainerManage => "container.manage",
            Action::ContainerInspectEnv => "container.inspect_env",
            Action::ContainerExec => "container.exec",
            Action::ProxyManage => "proxy.manage",
            Action::JobCreate => "job.create",
            Action::JobCancel => "job.cancel",
            Action::PluginManage => "plugin.manage",
            Action::PluginInvoke => "plugin.invoke",
            Action::HealthView => "health.view",
            Action::HealthConfigure => "health.configure",
            Action::HealthCommand => "health.command",
            Action::LogRead => "log.read",
            Action::LogManage => "log.manage",
            Action::TerminalOpen => "terminal.open",
            Action::Deploy => "deploy",
            Action::DeployApprove => "deploy.approve",
            Action::SecretRead => "secret.read",
            Action::AccessManage => "access.manage",
            Action::SecretManage => "secret.manage",
            Action::DbAdmin => "db.admin",
            Action::SystemUpdate => "system.update",
            Action::AnalyticsConfigure => "analytics.configure",
            Action::FileRead => "file.read",
            Action::FileWrite => "file.write",
            Action::FileDelete => "file.delete",
            Action::FileAdmin => "file.admin",
            Action::FileNode => "file.node",
            Action::StackManage => "stack.manage",
            Action::StackPrivileged => "stack.privileged",
            Action::SwarmManage => "swarm.manage",
            Action::ImageManage => "docker.image.manage",
            Action::VolumeManage => "docker.volume.manage",
            Action::NetworkManage => "docker.network.manage",
            Action::RegistryManage => "registry.manage",
            Action::WebhookManage => "webhook.manage",
            Action::CertificateUse => "certificate.use",
        }
    }
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Action {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Action::ALL.into_iter().find(|a| a.as_str() == s).ok_or_else(|| format!("unknown action `{s}`"))
    }
}

/// Kinds of entities that participate in authorization. Mirrors the entity
/// types in `forge-authz/schema/forge.cedarschema`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    Platform,
    Workspace,
    Tenant,
    Environment,
    Cluster,
    Server,
    Job,
    Plugin,
    AuditLog,
    User,
    /// A service; authorized through its environment (D-085).
    Service,
    /// A project (product); parent of its environments (D-102).
    Project,
    /// A package installation on a node (alarm resources only, D-118).
    /// Not a Cedar entity: it is authorized through its node.
    Installation,
    /// An uploaded ingress TLS certificate (alarm resources only, D-118).
    /// Not a Cedar entity: it is authorized through its workspace.
    Certificate,
}

impl EntityKind {
    pub const ALL: [EntityKind; 14] = [
        EntityKind::Platform,
        EntityKind::Workspace,
        EntityKind::Tenant,
        EntityKind::Environment,
        EntityKind::Cluster,
        EntityKind::Server,
        EntityKind::Job,
        EntityKind::Plugin,
        EntityKind::AuditLog,
        EntityKind::User,
        EntityKind::Service,
        EntityKind::Project,
        EntityKind::Installation,
        EntityKind::Certificate,
    ];

    /// Cedar entity type name (without namespace).
    #[must_use]
    pub const fn cedar_type(self) -> &'static str {
        match self {
            EntityKind::Platform => "Platform",
            EntityKind::Workspace => "Workspace",
            EntityKind::Tenant => "Tenant",
            EntityKind::Environment => "Environment",
            EntityKind::Cluster => "Cluster",
            EntityKind::Server => "Server",
            EntityKind::Job => "Job",
            EntityKind::Plugin => "Plugin",
            EntityKind::AuditLog => "AuditLog",
            EntityKind::User => "User",
            EntityKind::Service => "Service",
            EntityKind::Project => "Project",
            EntityKind::Installation => "Installation",
            EntityKind::Certificate => "Certificate",
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            EntityKind::Platform => "platform",
            EntityKind::Workspace => "workspace",
            EntityKind::Tenant => "tenant",
            EntityKind::Environment => "environment",
            EntityKind::Cluster => "cluster",
            EntityKind::Server => "server",
            EntityKind::Job => "job",
            EntityKind::Plugin => "plugin",
            EntityKind::AuditLog => "audit_log",
            EntityKind::User => "user",
            EntityKind::Service => "service",
            EntityKind::Project => "project",
            EntityKind::Installation => "installation",
            EntityKind::Certificate => "certificate",
        }
    }
}

impl FromStr for EntityKind {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        EntityKind::ALL.into_iter().find(|k| k.as_str() == s).ok_or_else(|| format!("unknown entity kind `{s}`"))
    }
}

/// A reference to one entity, e.g. `server:0192...`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EntityRef {
    pub kind: EntityKind,
    pub id: String,
}

impl EntityRef {
    pub fn new(kind: EntityKind, id: impl ToString) -> Self {
        Self { kind, id: id.to_string() }
    }

    /// The single platform root entity.
    #[must_use]
    pub fn platform() -> Self {
        Self::new(EntityKind::Platform, "root")
    }
}

impl fmt::Display for EntityRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.kind.as_str(), self.id)
    }
}

impl FromStr for EntityRef {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (k, id) = s.split_once(':').ok_or_else(|| format!("bad entity ref `{s}`"))?;
        if id.is_empty() {
            return Err(format!("bad entity ref `{s}`"));
        }

        Ok(Self { kind: k.parse()?, id: id.to_owned() })
    }
}

/// A resource plus its ancestry. Most resources have one chain of parents
/// (server → environment → workspace), but an entity may have
/// several parents (a server in a cluster *and* an environment), so the
/// ancestry is kept as a small graph. The platform root is implied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceRef {
    pub entity: EntityRef,
    /// Every ancestor, deduplicated, nearest first.
    pub ancestors: Vec<EntityRef>,
    /// Parent edges for every node in the graph (including `entity`).
    pub parents: Vec<(EntityRef, Vec<EntityRef>)>,
}

impl ResourceRef {
    /// A resource with a single chain of ancestors (nearest first).
    #[must_use]
    pub fn new(entity: EntityRef, ancestors: Vec<EntityRef>) -> Self {
        let mut r = Self { entity: entity.clone(), ancestors: vec![], parents: vec![(entity.clone(), vec![])] };
        let mut chain = vec![entity];
        chain.extend(ancestors);
        r.add_chain(&chain);

        r
    }

    #[must_use]
    pub fn platform() -> Self {
        Self::new(EntityRef::platform(), vec![])
    }

    /// Adds a chain `[child, parent, grandparent, …]`; the first element must
    /// already be part of the graph (usually `entity`).
    pub fn add_chain(&mut self, chain: &[EntityRef]) {
        for pair in chain.windows(2) {
            let (child, parent) = (&pair[0], &pair[1]);
            match self.parents.iter_mut().find(|(e, _)| e == child) {
                Some((_, ps)) if !ps.contains(parent) => ps.push(parent.clone()),
                Some(_) => {}
                None => self.parents.push((child.clone(), vec![parent.clone()])),
            }

            if !self.parents.iter().any(|(e, _)| e == parent) {
                self.parents.push((parent.clone(), vec![]));
            }

            if parent != &self.entity && !self.ancestors.contains(parent) {
                self.ancestors.push(parent.clone());
            }
        }
    }

    /// Whether `scope` is this resource or one of its ancestors.
    #[must_use]
    pub fn is_within(&self, scope: &EntityRef) -> bool {
        scope.kind == EntityKind::Platform || &self.entity == scope || self.ancestors.contains(scope)
    }
}

/// Who is asking.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", content = "id", rename_all = "snake_case")]
pub enum Principal {
    User(UserId),
}

impl fmt::Display for Principal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Principal::User(id) => write!(f, "user:{id}"),
        }
    }
}

impl FromStr for Principal {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.split_once(':') {
            Some(("user", id)) => id.parse().map(Principal::User).map_err(|e| e.to_string()),
            _ => Err(format!("bad principal `{s}`")),
        }
    }
}

/// Request context evaluated by guardrail policies.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthzContext {
    /// The session completed MFA (or step-up MFA) recently.
    pub mfa: bool,
}

/// One authorization question. The resource is a bare reference; the
/// authorizer resolves its ancestry through a [`crate::ports::ResourceResolver`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessRequest {
    pub principal: Principal,
    pub action: Action,
    pub resource: EntityRef,
    pub context: AuthzContext,
}

impl AccessRequest {
    #[must_use]
    pub fn new(principal: Principal, action: Action, resource: EntityRef, context: AuthzContext) -> Self {
        Self { principal, action, resource, context }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_round_trips_through_strings() {
        for a in Action::ALL {
            assert_eq!(a.as_str().parse::<Action>(), Ok(a));
        }
    }

    #[test]
    fn role_round_trips_through_strings() {
        for r in Role::ALL {
            assert_eq!(r.as_str().parse::<Role>(), Ok(r));
        }
    }

    #[test]
    fn entity_ref_parses() {
        let r: EntityRef = "server:abc".parse().expect("parse");
        assert_eq!(r, EntityRef::new(EntityKind::Server, "abc"));
        assert!("nope".parse::<EntityRef>().is_err());
        assert!("server:".parse::<EntityRef>().is_err());
    }

    #[test]
    fn principal_round_trips() {
        let p = Principal::User(UserId::new());
        assert_eq!(p.to_string().parse::<Principal>(), Ok(p));
    }

    #[test]
    fn resource_scope_containment() {
        let org = EntityRef::new(EntityKind::Workspace, "o1");
        let res = ResourceRef::new(EntityRef::new(EntityKind::Server, "s1"), vec![org.clone()]);
        assert!(res.is_within(&org));
        assert!(res.is_within(&EntityRef::platform()));
        assert!(!res.is_within(&EntityRef::new(EntityKind::Workspace, "o2")));
    }
}
