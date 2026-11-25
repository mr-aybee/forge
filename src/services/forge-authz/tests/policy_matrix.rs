#![allow(clippy::unwrap_used)]

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use forge_authz::{CedarAuthorizer, PolicyEngine};
use forge_core::model::RoleBinding;
use forge_core::ports::{Authorizer, ResourceResolver};
use forge_core::*;

fn workspace(id: &str) -> EntityRef {
    EntityRef::new(EntityKind::Workspace, id)
}

fn server_in(workspace_id: &str, env: &str) -> ResourceRef {
    ResourceRef::new(
        EntityRef::new(EntityKind::Server, format!("srv-{workspace_id}")),
        vec![EntityRef::new(EntityKind::Environment, env), workspace(workspace_id)],
    )
}

fn bind(engine: &PolicyEngine, user: UserId, role: Role, scope: EntityRef) -> RoleBinding {
    let b = RoleBinding { id: RoleBindingId::new(), user_id: user, role, scope, created_at: Timestamp(0) };
    engine.link_binding(&b).unwrap();

    b
}

fn allowed(engine: &PolicyEngine, user: UserId, action: Action, res: &ResourceRef, mfa: bool) -> bool {
    // A request that does not fit the schema (e.g. `workspace.create` on a Server)
    // is an error, which callers must treat as deny.
    engine.evaluate(&Principal::User(user), action, res, AuthzContext { mfa }).is_ok_and(|d| d.allowed)
}

#[test]
fn policies_validate_against_schema() {
    PolicyEngine::new().expect("schema, templates and guardrails validate");
}

#[test]
fn default_is_deny() {
    let engine = PolicyEngine::new().unwrap();
    let user = UserId::new();

    for action in Action::ALL {
        assert!(!allowed(&engine, user, action, &server_in("a", "e1"), true), "{action} should be denied");
    }
}

#[test]
fn role_matrix_on_server() {
    use Action::*;

    // role -> actions expected to be allowed on a server within its org scope
    let expected: HashMap<Role, Vec<Action>> = HashMap::from([
        (
            Role::PlatformAdmin,
            vec![
                View,
                ServerManage,
                ServerCommand,
                ProcessManage,
                ServerRename,
                JobCreate,
                PluginInvoke,
                TerminalOpen,
                DbAdmin,
            ],
        ),
        (
            Role::WorkspaceAdmin,
            vec![
                View,
                ServerManage,
                ServerCommand,
                ProcessManage,
                ServerRename,
                JobCreate,
                PluginInvoke,
                TerminalOpen,
                DbAdmin,
            ],
        ),
        (
            Role::Operator,
            vec![View, ServerManage, ServerCommand, ProcessManage, ServerRename, JobCreate, PluginInvoke, TerminalOpen],
        ),
        (Role::Developer, vec![View, JobCreate, PluginInvoke]),
        (Role::Dba, vec![View, JobCreate, DbAdmin]),
        (Role::Auditor, vec![View]),
        (Role::Customer, vec![View]),
    ]);
    let server_actions = [
        View,
        ServerManage,
        ServerCommand,
        ProcessManage,
        ServerRename,
        JobCreate,
        PluginInvoke,
        TerminalOpen,
        DbAdmin,
    ];

    for (role, allowed_actions) in expected {
        let engine = PolicyEngine::new().unwrap();
        let user = UserId::new();
        let scope = if role == Role::PlatformAdmin { EntityRef::platform() } else { workspace("a") };
        bind(&engine, user, role, scope);

        for action in server_actions {
            let got = allowed(&engine, user, action, &server_in("a", "e1"), true);
            assert_eq!(got, allowed_actions.contains(&action), "role {role} action {action}");
        }
    }
}

#[test]
fn workspace_scope_does_not_leak_to_other_workspaces() {
    let engine = PolicyEngine::new().unwrap();
    let user = UserId::new();
    bind(&engine, user, Role::WorkspaceAdmin, workspace("a"));
    assert!(allowed(&engine, user, Action::View, &server_in("a", "e1"), true));
    assert!(!allowed(&engine, user, Action::View, &server_in("b", "e2"), true));
}

#[test]
fn environment_scope_is_narrower_than_workspace() {
    let engine = PolicyEngine::new().unwrap();
    let user = UserId::new();
    bind(&engine, user, Role::Operator, EntityRef::new(EntityKind::Environment, "dev"));
    assert!(allowed(&engine, user, Action::ServerManage, &server_in("a", "dev"), false));
    assert!(!allowed(&engine, user, Action::ServerManage, &server_in("a", "prod"), false));
}

#[test]
fn sensitive_actions_require_mfa_even_for_admins() {
    let engine = PolicyEngine::new().unwrap();
    let user = UserId::new();
    bind(&engine, user, Role::PlatformAdmin, EntityRef::platform());
    let srv = server_in("a", "e1");
    assert!(!allowed(&engine, user, Action::ServerCommand, &srv, false));
    assert!(allowed(&engine, user, Action::ServerCommand, &srv, true));
    assert!(!allowed(&engine, user, Action::ProcessManage, &srv, false));
    assert!(allowed(&engine, user, Action::ProcessManage, &srv, true));

    // non-sensitive action unaffected
    assert!(allowed(&engine, user, Action::View, &srv, false));
}

#[test]
fn unlinking_a_binding_revokes_access() {
    let engine = PolicyEngine::new().unwrap();
    let user = UserId::new();
    let b = bind(&engine, user, Role::Operator, workspace("a"));
    assert!(allowed(&engine, user, Action::View, &server_in("a", "e1"), false));
    engine.unlink_binding(&b.id.to_string()).unwrap();
    assert!(!allowed(&engine, user, Action::View, &server_in("a", "e1"), false));
}

#[test]
fn only_platform_admin_creates_orgs() {
    let engine = PolicyEngine::new().unwrap();
    let admin = UserId::new();
    let workspace_admin = UserId::new();
    bind(&engine, admin, Role::PlatformAdmin, EntityRef::platform());
    bind(&engine, workspace_admin, Role::WorkspaceAdmin, workspace("a"));
    let platform = ResourceRef::platform();
    assert!(allowed(&engine, admin, Action::WorkspaceCreate, &platform, false));
    assert!(!allowed(&engine, workspace_admin, Action::WorkspaceCreate, &platform, false));
}

#[test]
fn invalid_hierarchy_is_rejected_as_deny() {
    // An Workspace cannot be a child of a Server per the schema: the request must
    // fail closed rather than evaluate.
    let engine = PolicyEngine::new().unwrap();
    let user = UserId::new();
    bind(&engine, user, Role::PlatformAdmin, EntityRef::platform());
    let bogus = ResourceRef::new(workspace("a"), vec![EntityRef::new(EntityKind::Server, "s")]);
    assert!(engine.evaluate(&Principal::User(user), Action::View, &bogus, AuthzContext::default()).is_err());
}

#[derive(Debug)]
struct StaticResolver;

#[async_trait]
impl ResourceResolver for StaticResolver {
    async fn resolve(&self, entity: &EntityRef) -> DomainResult<ResourceRef> {
        Ok(ResourceRef::new(entity.clone(), vec![workspace("a")]))
    }
}

#[tokio::test]
async fn authorizer_port_maps_deny_to_error() {
    let engine = Arc::new(PolicyEngine::new().unwrap());
    let user = UserId::new();
    bind(&engine, user, Role::Auditor, workspace("a"));
    let authz = CedarAuthorizer::new(engine, Arc::new(StaticResolver));
    let srv = EntityRef::new(EntityKind::Server, "s1");
    let view = AccessRequest::new(Principal::User(user), Action::View, srv.clone(), AuthzContext::default());
    let manage = AccessRequest::new(Principal::User(user), Action::ServerManage, srv, AuthzContext::default());
    assert!(authz.authorize(&view).await.is_ok());
    assert!(matches!(authz.authorize(&manage).await, Err(DomainError::Denied(_))));
}

/// A node in a cluster *and* an environment: a cluster-scoped operator may
/// manage it, an operator on another cluster may not, and an environment
/// binding still applies through the second parent.
#[test]
fn cluster_scope_applies_through_second_parent() {
    let engine = PolicyEngine::new().unwrap();
    let ws = workspace("a");
    let cluster = EntityRef::new(EntityKind::Cluster, "c1");
    let mut node = ResourceRef::new(
        EntityRef::new(EntityKind::Server, "n1"),
        vec![EntityRef::new(EntityKind::Environment, "prod"), ws.clone()],
    );
    node.add_chain(&[node.entity.clone(), cluster.clone(), ws]);

    let cluster_op = UserId::new();
    bind(&engine, cluster_op, Role::Operator, cluster);
    assert!(allowed(&engine, cluster_op, Action::ServerManage, &node, false));

    let other_op = UserId::new();
    bind(&engine, other_op, Role::Operator, EntityRef::new(EntityKind::Cluster, "c2"));
    assert!(!allowed(&engine, other_op, Action::ServerManage, &node, false));

    let env_dev = UserId::new();
    bind(&engine, env_dev, Role::Developer, EntityRef::new(EntityKind::Environment, "prod"));
    assert!(allowed(&engine, env_dev, Action::View, &node, false));
}

#[test]
fn object_storage_roles_and_mfa() {
    let engine = PolicyEngine::new().unwrap();
    let srv = server_in("a", "e1");
    let (op, dev, auditor) = (UserId::new(), UserId::new(), UserId::new());
    bind(&engine, op, Role::Operator, workspace("a"));
    bind(&engine, dev, Role::Developer, workspace("a"));
    bind(&engine, auditor, Role::Auditor, workspace("a"));

    // Managing a store runs as root on the node: MFA required.
    assert!(!allowed(&engine, op, Action::StorageManage, &srv, false));
    assert!(allowed(&engine, op, Action::StorageManage, &srv, true));
    assert!(!allowed(&engine, dev, Action::StorageManage, &srv, true));

    // Object data: operators and developers, no MFA.
    assert!(allowed(&engine, op, Action::StorageData, &srv, false));
    assert!(allowed(&engine, dev, Action::StorageData, &srv, false));
    assert!(!allowed(&engine, auditor, Action::StorageData, &srv, true));
}

/// D-107: `storage.data` on an environment is how a store owned by that
/// environment is reached. It follows the role groups (no new grant), stays
/// inside the environment, and `storage.manage` is never evaluated there.
#[test]
fn storage_data_on_a_environment_stays_in_that_environment() {
    let engine = PolicyEngine::new().unwrap();
    let ns = |name: &str| ResourceRef::new(EntityRef::new(EntityKind::Environment, name), vec![workspace("a")]);
    let (dev, op, auditor, customer, dba) = (UserId::new(), UserId::new(), UserId::new(), UserId::new(), UserId::new());
    bind(&engine, dev, Role::Developer, EntityRef::new(EntityKind::Environment, "prod"));
    bind(&engine, op, Role::Operator, EntityRef::new(EntityKind::Environment, "prod"));
    bind(&engine, auditor, Role::Auditor, EntityRef::new(EntityKind::Environment, "prod"));
    bind(&engine, customer, Role::Customer, EntityRef::new(EntityKind::Environment, "prod"));
    bind(&engine, dba, Role::Dba, EntityRef::new(EntityKind::Environment, "prod"));

    for user in [dev, op] {
        assert!(allowed(&engine, user, Action::StorageData, &ns("prod"), false));
        assert!(!allowed(&engine, user, Action::StorageData, &ns("dev"), true), "another environment");

        // An environment role says nothing about the workspace's shared stores.
        let ws = ResourceRef::new(workspace("a"), vec![]);
        assert!(!allowed(&engine, user, Action::StorageData, &ws, true));

        // Managing a store is never decided on an environment.
        assert!(!allowed(&engine, user, Action::StorageManage, &ns("prod"), true));
    }

    for user in [auditor, customer, dba] {
        assert!(allowed(&engine, user, Action::View, &ns("prod"), false));
        assert!(!allowed(&engine, user, Action::StorageData, &ns("prod"), true), "read-only roles see, not browse");
    }
}

#[test]
fn node_onboarding_requires_mfa() {
    let engine = PolicyEngine::new().unwrap();
    let user = UserId::new();
    bind(&engine, user, Role::Operator, workspace("a"));
    let ws = ResourceRef::new(workspace("a"), vec![]);
    assert!(!allowed(&engine, user, Action::NodeOnboard, &ws, false));
    assert!(allowed(&engine, user, Action::NodeOnboard, &ws, true));
}

/// Batch evaluation (list endpoints) must give exactly the single decisions.
#[test]
fn evaluate_many_matches_evaluate() {
    let engine = PolicyEngine::new().unwrap();
    let ws = workspace("a");
    let mut resources = Vec::new();

    for i in 0..40 {
        let ns = EntityRef::new(EntityKind::Environment, format!("ns{}", i % 4));
        let mut r = ResourceRef::new(EntityRef::new(EntityKind::Server, format!("n{i}")), vec![ns, ws.clone()]);

        if i % 3 == 0 {
            r.add_chain(&[r.entity.clone(), EntityRef::new(EntityKind::Cluster, format!("c{}", i % 2)), ws.clone()]);
        }

        resources.push(r);
    }

    // Another workspace's server in the same batch.
    resources.push(server_in("b", "x"));
    let users = [UserId::new(), UserId::new(), UserId::new(), UserId::new()];
    bind(&engine, users[0], Role::Developer, EntityRef::new(EntityKind::Environment, "ns1"));
    bind(&engine, users[1], Role::Operator, EntityRef::new(EntityKind::Cluster, "c0"));
    bind(&engine, users[2], Role::WorkspaceAdmin, ws.clone());

    for user in users {
        for action in [Action::View, Action::ServerManage, Action::ServerCommand, Action::ProcessManage] {
            for mfa in [false, true] {
                let p = Principal::User(user);
                let batch = engine.evaluate_many(&p, action, &resources, AuthzContext { mfa }).unwrap();

                for (r, d) in resources.iter().zip(&batch) {
                    assert_eq!(
                        d.allowed,
                        allowed(&engine, user, action, r, mfa),
                        "{action} on {} (mfa {mfa})",
                        r.entity
                    );
                }
            }
        }
    }
}

#[test]
fn file_explorer_roles_and_mfa() {
    let engine = PolicyEngine::new().unwrap();
    let srv = server_in("a", "e1");
    let (admin, op, dev, auditor) = (UserId::new(), UserId::new(), UserId::new(), UserId::new());
    bind(&engine, admin, Role::WorkspaceAdmin, workspace("a"));
    bind(&engine, op, Role::Operator, workspace("a"));
    bind(&engine, dev, Role::Developer, workspace("a"));
    bind(&engine, auditor, Role::Auditor, workspace("a"));

    // Reads inside a root: operators and admins, no MFA.
    assert!(allowed(&engine, op, Action::FileRead, &srv, false));
    assert!(allowed(&engine, admin, Action::FileRead, &srv, false));

    // The whole-node root: admins only, no MFA.
    assert!(allowed(&engine, admin, Action::FileNode, &srv, false));
    assert!(!allowed(&engine, op, Action::FileNode, &srv, true));
    assert!(!allowed(&engine, dev, Action::FileNode, &srv, true));
    assert!(!allowed(&engine, auditor, Action::FileNode, &srv, true));
    let (dba, customer) = (UserId::new(), UserId::new());
    bind(&engine, dba, Role::Dba, workspace("a"));
    bind(&engine, customer, Role::Customer, workspace("a"));
    assert!(!allowed(&engine, dba, Action::FileNode, &srv, true));
    assert!(!allowed(&engine, customer, Action::FileNode, &srv, true));
    assert!(!allowed(&engine, dev, Action::FileRead, &srv, true));
    assert!(!allowed(&engine, auditor, Action::FileRead, &srv, true));

    // Writes and deletes: operators and admins, only with MFA.
    for a in [Action::FileWrite, Action::FileDelete] {
        assert!(!allowed(&engine, op, a, &srv, false));
        assert!(allowed(&engine, op, a, &srv, true));
        assert!(allowed(&engine, admin, a, &srv, true));
        assert!(!allowed(&engine, dev, a, &srv, true));
        assert!(!allowed(&engine, auditor, a, &srv, true));
    }

    // Approving roots: admins only, with MFA.
    assert!(!allowed(&engine, op, Action::FileAdmin, &srv, true));
    assert!(!allowed(&engine, admin, Action::FileAdmin, &srv, false));
    assert!(allowed(&engine, admin, Action::FileAdmin, &srv, true));

    // Another workspace's node is out of reach.
    assert!(!allowed(&engine, op, Action::FileRead, &server_in("b", "e2"), false));
}

/// Raw node ports (D-100) are an administrator exception: `network.tunnel.any`
/// on a server, admins only, always with MFA, never in another workspace.
#[test]
fn raw_port_tunnels_are_for_administrators_with_mfa() {
    let engine = PolicyEngine::new().unwrap();
    let srv = server_in("a", "e1");
    let other = server_in("b", "e2");

    for role in Role::ALL {
        let user = UserId::new();
        let scope = if role == Role::PlatformAdmin { EntityRef::platform() } else { workspace("a") };
        bind(&engine, user, role, scope);
        let holds = matches!(role, Role::PlatformAdmin | Role::WorkspaceAdmin);
        assert_eq!(allowed(&engine, user, Action::NetworkTunnelAny, &srv, true), holds, "{role} with MFA");
        assert!(!allowed(&engine, user, Action::NetworkTunnelAny, &srv, false), "{role} without MFA");

        if role != Role::PlatformAdmin {
            assert!(!allowed(&engine, user, Action::NetworkTunnelAny, &other, true), "{role} in another workspace");
        }
    }

    // Not declared on an environment: fails closed.
    let admin = UserId::new();
    bind(&engine, admin, Role::WorkspaceAdmin, workspace("a"));
    let ns = ResourceRef::new(EntityRef::new(EntityKind::Environment, "e1"), vec![workspace("a")]);
    assert!(!allowed(&engine, admin, Action::NetworkTunnelAny, &ns, true));
}

/// D-056: an environment sits in a cluster for placement only. A binding on the
/// cluster reaches its nodes, never the environments (or their services'
/// jobs) placed in it, and an environment binding never reaches a sibling
/// environment in the same cluster.
#[test]
fn cluster_binding_does_not_widen_to_its_environments() {
    let engine = PolicyEngine::new().unwrap();
    let ws = workspace("a");
    let dev = ResourceRef::new(EntityRef::new(EntityKind::Environment, "dev"), vec![ws.clone()]);
    let prod = ResourceRef::new(EntityRef::new(EntityKind::Environment, "prod"), vec![ws.clone()]);
    let job_in_dev = ResourceRef::new(
        EntityRef::new(EntityKind::Job, "j1"),
        vec![EntityRef::new(EntityKind::Environment, "dev"), ws.clone()],
    );

    let cluster_admin = UserId::new();
    bind(&engine, cluster_admin, Role::WorkspaceAdmin, EntityRef::new(EntityKind::Cluster, "c1"));

    for action in [Action::View, Action::WorkspaceManage, Action::Deploy] {
        assert!(!allowed(&engine, cluster_admin, action, &dev, true), "{action:?} via cluster");
    }

    assert!(!allowed(&engine, cluster_admin, Action::View, &job_in_dev, true));

    let dev_op = UserId::new();
    bind(&engine, dev_op, Role::Operator, EntityRef::new(EntityKind::Environment, "dev"));
    assert!(allowed(&engine, dev_op, Action::View, &dev, false));
    assert!(allowed(&engine, dev_op, Action::View, &job_in_dev, false));
    assert!(!allowed(&engine, dev_op, Action::View, &prod, true));
}

/// D-056: tenant bindings no longer reach environments through a tenant (the
/// migration turns that reach into explicit environment bindings instead).
/// Projects (D-102): a binding on a project covers the environments
/// in it and what runs there, but not another group's
/// environments, nor a legacy environment without a group.
#[test]
fn project_binding_covers_its_environments() {
    let engine = PolicyEngine::new().unwrap();
    let shop = EntityRef::new(EntityKind::Project, "shop");
    let dev = UserId::new();
    bind(&engine, dev, Role::Developer, shop.clone());
    let shop_test =
        ResourceRef::new(EntityRef::new(EntityKind::Environment, "shop-test"), vec![shop.clone(), workspace("a")]);
    let shop_srv = ResourceRef::new(
        EntityRef::new(EntityKind::Server, "srv1"),
        vec![EntityRef::new(EntityKind::Environment, "shop-test"), shop.clone(), workspace("a")],
    );
    let shop_app = ResourceRef::new(
        EntityRef::new(EntityKind::Service, "app1"),
        vec![EntityRef::new(EntityKind::Environment, "shop-test"), shop.clone(), workspace("a")],
    );
    let crm_test = ResourceRef::new(
        EntityRef::new(EntityKind::Environment, "crm-test"),
        vec![EntityRef::new(EntityKind::Project, "crm"), workspace("a")],
    );
    let legacy = ResourceRef::new(EntityRef::new(EntityKind::Environment, "old"), vec![workspace("a")]);
    assert!(allowed(&engine, dev, Action::View, &shop_test, false));
    assert!(allowed(&engine, dev, Action::Deploy, &shop_test, false));
    assert!(allowed(&engine, dev, Action::View, &shop_srv, false));
    assert!(allowed(&engine, dev, Action::HealthView, &shop_app, false));
    assert!(!allowed(&engine, dev, Action::View, &crm_test, true), "another group");
    assert!(!allowed(&engine, dev, Action::View, &legacy, true), "unassigned environment");
    assert!(!allowed(&engine, dev, Action::View, &ResourceRef::new(workspace("a"), vec![]), true), "not the workspace");

    // Only administrators manage the group itself.
    let group = ResourceRef::new(shop.clone(), vec![workspace("a")]);
    assert!(allowed(&engine, dev, Action::View, &group, false));
    assert!(!allowed(&engine, dev, Action::WorkspaceManage, &group, true));
    let admin = UserId::new();
    bind(&engine, admin, Role::WorkspaceAdmin, workspace("a"));
    assert!(allowed(&engine, admin, Action::WorkspaceManage, &group, false));
}

#[test]
fn tenant_binding_does_not_reach_environments() {
    let engine = PolicyEngine::new().unwrap();
    let ws = workspace("a");
    let ns = ResourceRef::new(EntityRef::new(EntityKind::Environment, "dev"), vec![ws]);
    let customer = UserId::new();
    bind(&engine, customer, Role::Customer, EntityRef::new(EntityKind::Tenant, "t1"));
    assert!(!allowed(&engine, customer, Action::View, &ns, true));
}

/// Roles can be granted on a cluster (`user.manage` applies to clusters):
/// by a workspace admin, and by an admin of that cluster, never of another.
#[test]
fn cluster_scoped_roles_can_be_granted() {
    let engine = PolicyEngine::new().unwrap();
    let ws = workspace("a");
    let c1 = ResourceRef::new(EntityRef::new(EntityKind::Cluster, "c1"), vec![ws.clone()]);
    let c2 = ResourceRef::new(EntityRef::new(EntityKind::Cluster, "c2"), vec![ws.clone()]);
    let wsa = UserId::new();
    bind(&engine, wsa, Role::WorkspaceAdmin, ws);
    assert!(allowed(&engine, wsa, Action::UserManage, &c1, true));
    let c1_admin = UserId::new();
    bind(&engine, c1_admin, Role::WorkspaceAdmin, EntityRef::new(EntityKind::Cluster, "c1"));
    assert!(allowed(&engine, c1_admin, Action::UserManage, &c1, true));
    assert!(!allowed(&engine, c1_admin, Action::UserManage, &c2, true));
    let op = UserId::new();
    bind(&engine, op, Role::Operator, EntityRef::new(EntityKind::Cluster, "c1"));
    assert!(!allowed(&engine, op, Action::UserManage, &c1, true));
}

#[test]
fn certificate_use_is_for_administrators_with_mfa() {
    let engine = PolicyEngine::new().unwrap();
    let ws = ResourceRef::new(workspace("a"), vec![]);
    let edge = ResourceRef::new(EntityRef::new(EntityKind::Server, "edge-1"), vec![workspace("a")]);
    let (admin, op, dev, other) = (UserId::new(), UserId::new(), UserId::new(), UserId::new());
    bind(&engine, admin, Role::WorkspaceAdmin, workspace("a"));
    bind(&engine, op, Role::Operator, workspace("a"));
    bind(&engine, dev, Role::Developer, workspace("a"));
    bind(&engine, other, Role::WorkspaceAdmin, workspace("b"));

    for r in [&ws, &edge] {
        assert!(!allowed(&engine, admin, Action::CertificateUse, r, false), "MFA is required");
        assert!(allowed(&engine, admin, Action::CertificateUse, r, true));

        for u in [op, dev, other] {
            assert!(!allowed(&engine, u, Action::CertificateUse, r, true));
        }
    }

    // Operators keep proxy.manage on the edge: they apply endpoints, but an
    // uploaded certificate's key needs an administrator.
    assert!(allowed(&engine, op, Action::ProxyManage, &edge, true));
}

#[test]
fn compose_stacks_roles_and_mfa() {
    let engine = PolicyEngine::new().unwrap();
    let ns = ResourceRef::new(EntityRef::new(EntityKind::Environment, "web"), vec![workspace("a")]);
    let (admin, op, dev, auditor) = (UserId::new(), UserId::new(), UserId::new(), UserId::new());
    bind(&engine, admin, Role::WorkspaceAdmin, workspace("a"));
    bind(&engine, op, Role::Operator, workspace("a"));
    bind(&engine, dev, Role::Developer, workspace("a"));
    bind(&engine, auditor, Role::Auditor, workspace("a"));

    // Ordinary stacks: whoever may deploy, without MFA.
    for u in [admin, op, dev] {
        assert!(allowed(&engine, u, Action::StackManage, &ns, false));
    }

    assert!(!allowed(&engine, auditor, Action::StackManage, &ns, true));

    // Privileged stacks: administrators only, and only with MFA.
    assert!(!allowed(&engine, admin, Action::StackPrivileged, &ns, false));
    assert!(allowed(&engine, admin, Action::StackPrivileged, &ns, true));

    for u in [op, dev, auditor] {
        assert!(!allowed(&engine, u, Action::StackPrivileged, &ns, true));
    }
}

/// Ending processes and changing their priority is root-equivalent on the
/// node: the same roles as `server.command`, always with MFA, never across
/// workspaces, and only on servers.
#[test]
fn process_manage_roles_and_mfa() {
    let engine = PolicyEngine::new().unwrap();
    let srv = server_in("a", "e1");
    let other = server_in("b", "e2");
    let users: Vec<(Role, UserId)> = Role::ALL.into_iter().map(|r| (r, UserId::new())).collect();

    for &(role, user) in &users {
        let scope = if role == Role::PlatformAdmin { EntityRef::platform() } else { workspace("a") };
        bind(&engine, user, role, scope);
    }

    for &(role, user) in &users {
        let holds = matches!(role, Role::PlatformAdmin | Role::WorkspaceAdmin | Role::Operator);
        assert_eq!(allowed(&engine, user, Action::ProcessManage, &srv, true), holds, "{role} with MFA");
        assert!(!allowed(&engine, user, Action::ProcessManage, &srv, false), "{role} without MFA");

        // Same holders as server.command.
        assert_eq!(
            allowed(&engine, user, Action::ProcessManage, &srv, true),
            allowed(&engine, user, Action::ServerCommand, &srv, true),
            "{role}: process.manage and server.command differ"
        );

        if role != Role::PlatformAdmin {
            assert!(!allowed(&engine, user, Action::ProcessManage, &other, true), "{role} in another workspace");
        }
    }

    // An environment-scoped operator reaches only that environment's nodes.
    let ns_op = UserId::new();
    bind(&engine, ns_op, Role::Operator, EntityRef::new(EntityKind::Environment, "dev"));
    assert!(allowed(&engine, ns_op, Action::ProcessManage, &server_in("a", "dev"), true));
    assert!(!allowed(&engine, ns_op, Action::ProcessManage, &server_in("a", "prod"), true));

    // Not declared on a workspace: the request does not fit the schema and fails closed.
    let (_, admin) = users[0];
    let ws = ResourceRef::new(workspace("a"), vec![]);
    assert!(!allowed(&engine, admin, Action::ProcessManage, &ws, true));
}

/// Containers (D-100): admins and operators open a shell inside a container
/// (`container.exec`) with or without MFA, by the operator's choice; reading
/// environment values (`container.inspect_env`) always needs MFA. Both are
/// server-only and never reach another workspace.
#[test]
fn container_exec_and_inspect_env_roles() {
    let engine = PolicyEngine::new().unwrap();
    let srv = server_in("a", "e1");
    let other = server_in("b", "e2");
    let users: Vec<(Role, UserId)> = Role::ALL.into_iter().map(|r| (r, UserId::new())).collect();

    for &(role, user) in &users {
        let scope = if role == Role::PlatformAdmin { EntityRef::platform() } else { workspace("a") };
        bind(&engine, user, role, scope);
    }

    for &(role, user) in &users {
        let holds = matches!(role, Role::PlatformAdmin | Role::WorkspaceAdmin | Role::Operator);
        assert_eq!(allowed(&engine, user, Action::ContainerExec, &srv, false), holds, "{role} exec without MFA");
        assert_eq!(allowed(&engine, user, Action::ContainerExec, &srv, true), holds, "{role} exec with MFA");
        assert_eq!(allowed(&engine, user, Action::ContainerInspectEnv, &srv, true), holds, "{role} env with MFA");
        assert!(!allowed(&engine, user, Action::ContainerInspectEnv, &srv, false), "{role} env without MFA");

        // Same holders as container.manage.
        assert_eq!(
            allowed(&engine, user, Action::ContainerExec, &srv, true),
            allowed(&engine, user, Action::ContainerManage, &srv, true),
            "{role}: container.exec and container.manage differ"
        );

        if role != Role::PlatformAdmin {
            assert!(!allowed(&engine, user, Action::ContainerExec, &other, true), "{role} in another workspace");
            assert!(!allowed(&engine, user, Action::ContainerInspectEnv, &other, true), "{role} in another workspace");
        }
    }

    // Not declared on a workspace: fails closed.
    let (_, admin) = users[0];
    let ws = ResourceRef::new(workspace("a"), vec![]);
    assert!(!allowed(&engine, admin, Action::ContainerExec, &ws, true));
    assert!(!allowed(&engine, admin, Action::ContainerInspectEnv, &ws, true));
}

/// Alerts (#11, D-087): every role but customer sees alarms of its
/// workspace; admins and operators manage rules and acknowledge. Neither
/// needs MFA, and neither reaches another workspace.
#[test]
fn alert_roles_per_workspace() {
    let engine = PolicyEngine::new().unwrap();
    let ws = ResourceRef::new(workspace("a"), vec![]);
    let other = ResourceRef::new(workspace("b"), vec![]);

    for role in Role::ALL {
        let user = UserId::new();
        let scope = if role == Role::PlatformAdmin { EntityRef::platform() } else { workspace("a") };
        bind(&engine, user, role, scope);
        let sees = role != Role::Customer;
        let manages = matches!(role, Role::PlatformAdmin | Role::WorkspaceAdmin | Role::Operator);
        assert_eq!(allowed(&engine, user, Action::AlertView, &ws, false), sees, "{role} alert.view");
        assert_eq!(allowed(&engine, user, Action::AlertManage, &ws, false), manages, "{role} alert.manage");

        if role != Role::PlatformAdmin {
            assert!(!allowed(&engine, user, Action::AlertView, &other, true), "{role} in another workspace");
            assert!(!allowed(&engine, user, Action::AlertManage, &other, true), "{role} in another workspace");
        }
    }
}

/// Port tunnels (D-091, D-100): admins, operators and developers, on
/// environments, services and servers, only with MFA, never in another
/// workspace; an environment binding reaches that environment's services only.
#[test]
fn network_tunnel_roles_and_mfa() {
    let engine = PolicyEngine::new().unwrap();
    let srv = server_in("a", "e1");
    let other = server_in("b", "e2");
    let ns = ResourceRef::new(EntityRef::new(EntityKind::Environment, "e1"), vec![workspace("a")]);
    let app = ResourceRef::new(
        EntityRef::new(EntityKind::Service, "app1"),
        vec![EntityRef::new(EntityKind::Environment, "e1"), workspace("a")],
    );

    for role in Role::ALL {
        let user = UserId::new();
        let scope = if role == Role::PlatformAdmin { EntityRef::platform() } else { workspace("a") };
        bind(&engine, user, role, scope);
        let holds = matches!(role, Role::PlatformAdmin | Role::WorkspaceAdmin | Role::Operator | Role::Developer);

        for r in [&srv, &ns, &app] {
            assert_eq!(allowed(&engine, user, Action::NetworkTunnel, r, true), holds, "{role} with MFA");
            assert!(!allowed(&engine, user, Action::NetworkTunnel, r, false), "{role} without MFA");
        }

        if role != Role::PlatformAdmin {
            assert!(!allowed(&engine, user, Action::NetworkTunnel, &other, true), "{role} in another workspace");
        }
    }

    // A developer on environment `dev` tunnels to dev's services, not prod's.
    let dev = UserId::new();
    bind(&engine, dev, Role::Developer, EntityRef::new(EntityKind::Environment, "dev"));
    let dev_ns = ResourceRef::new(EntityRef::new(EntityKind::Environment, "dev"), vec![workspace("a")]);
    let prod_ns = ResourceRef::new(EntityRef::new(EntityKind::Environment, "prod"), vec![workspace("a")]);
    assert!(allowed(&engine, dev, Action::NetworkTunnel, &dev_ns, true));
    assert!(!allowed(&engine, dev, Action::NetworkTunnel, &prod_ns, true));

    // Auditors never tunnel; a grant (D-100) is checked outside Cedar.
    let auditor = UserId::new();
    bind(&engine, auditor, Role::Auditor, workspace("a"));
    assert!(!allowed(&engine, auditor, Action::NetworkTunnel, &ns, true));
}

/// Notification channels (#11, D-110): only administrators manage them, only
/// with MFA, and never in another workspace.
#[test]
fn notification_channels_need_an_administrator_with_mfa() {
    let engine = PolicyEngine::new().unwrap();
    let ws = ResourceRef::new(workspace("a"), vec![]);
    let other = ResourceRef::new(workspace("b"), vec![]);

    for role in Role::ALL {
        let user = UserId::new();
        let scope = if role == Role::PlatformAdmin { EntityRef::platform() } else { workspace("a") };
        bind(&engine, user, role, scope);
        let admin = matches!(role, Role::PlatformAdmin | Role::WorkspaceAdmin);
        assert_eq!(allowed(&engine, user, Action::NotificationManage, &ws, true), admin, "{role} with MFA");
        assert!(!allowed(&engine, user, Action::NotificationManage, &ws, false), "{role} without MFA");

        if role != Role::PlatformAdmin {
            assert!(!allowed(&engine, user, Action::NotificationManage, &other, true), "{role} in another workspace");
        }
    }
}

/// Issue #73: approving a waiting deployment is for operators and above,
/// not developers, with no MFA step, and never across workspaces.
#[test]
fn deploy_approve_roles() {
    let engine = PolicyEngine::new().unwrap();
    let app = ResourceRef::new(
        EntityRef::new(EntityKind::Service, "app1"),
        vec![EntityRef::new(EntityKind::Environment, "prod"), workspace("a")],
    );

    for (role, ok) in [
        (Role::PlatformAdmin, true),
        (Role::WorkspaceAdmin, true),
        (Role::Operator, true),
        (Role::Developer, false),
        (Role::Dba, false),
        (Role::Auditor, false),
        (Role::Customer, false),
    ] {
        let u = UserId::new();
        bind(&engine, u, role, workspace("a"));
        assert_eq!(allowed(&engine, u, Action::DeployApprove, &app, false), ok, "{role:?}");
    }

    let dev = UserId::new();
    bind(&engine, dev, Role::Developer, workspace("a"));
    assert!(allowed(&engine, dev, Action::Deploy, &app, false), "developers still deploy");
    let other = UserId::new();
    bind(&engine, other, Role::Operator, workspace("b"));
    assert!(!allowed(&engine, other, Action::DeployApprove, &app, true), "cross-workspace");
}

/// Swarm membership and nodes (D-154): administrators only, always with MFA.
#[test]
fn swarm_manage_is_admin_only_with_mfa() {
    let engine = PolicyEngine::new().unwrap();
    let cl = ResourceRef::new(EntityRef::new(EntityKind::Cluster, "swarm-eu"), vec![workspace("a")]);
    let (admin, op, dev, auditor) = (UserId::new(), UserId::new(), UserId::new(), UserId::new());
    bind(&engine, admin, Role::WorkspaceAdmin, workspace("a"));
    bind(&engine, op, Role::Operator, workspace("a"));
    bind(&engine, dev, Role::Developer, workspace("a"));
    bind(&engine, auditor, Role::Auditor, workspace("a"));
    assert!(!allowed(&engine, admin, Action::SwarmManage, &cl, false));
    assert!(allowed(&engine, admin, Action::SwarmManage, &cl, true));

    for u in [op, dev, auditor] {
        assert!(!allowed(&engine, u, Action::SwarmManage, &cl, true));
    }
}

/// Docker resources on a node (D-157): administrators and operators, no MFA;
/// developers and auditors cannot.
#[test]
fn docker_resources_are_for_operators() {
    let engine = PolicyEngine::new().unwrap();
    let node = ResourceRef::new(EntityRef::new(EntityKind::Server, "lab-a"), vec![workspace("a")]);
    let (admin, op, dev, auditor) = (UserId::new(), UserId::new(), UserId::new(), UserId::new());
    bind(&engine, admin, Role::WorkspaceAdmin, workspace("a"));
    bind(&engine, op, Role::Operator, workspace("a"));
    bind(&engine, dev, Role::Developer, workspace("a"));
    bind(&engine, auditor, Role::Auditor, workspace("a"));

    for a in [Action::ImageManage, Action::VolumeManage, Action::NetworkManage] {
        assert!(allowed(&engine, admin, a, &node, false), "{a:?}");
        assert!(allowed(&engine, op, a, &node, false), "{a:?}");
        assert!(!allowed(&engine, dev, a, &node, true), "{a:?}");
        assert!(!allowed(&engine, auditor, a, &node, true), "{a:?}");
    }
}

/// Registries hold credentials (D-158): administrators only, with MFA.
#[test]
fn registry_manage_is_admin_only_with_mfa() {
    let engine = PolicyEngine::new().unwrap();
    let ws = ResourceRef::new(EntityRef::new(EntityKind::Workspace, "a"), vec![]);
    let (admin, op) = (UserId::new(), UserId::new());
    bind(&engine, admin, Role::WorkspaceAdmin, workspace("a"));
    bind(&engine, op, Role::Operator, workspace("a"));
    assert!(!allowed(&engine, admin, Action::RegistryManage, &ws, false));
    assert!(allowed(&engine, admin, Action::RegistryManage, &ws, true));
    assert!(!allowed(&engine, op, Action::RegistryManage, &ws, true));
}

/// Deploy webhooks (D-159): whoever deploys may create them, always with MFA;
/// auditors cannot.
#[test]
fn webhook_manage_needs_mfa() {
    let engine = PolicyEngine::new().unwrap();
    let ws = ResourceRef::new(EntityRef::new(EntityKind::Workspace, "a"), vec![]);
    let (dev, auditor) = (UserId::new(), UserId::new());
    bind(&engine, dev, Role::Developer, workspace("a"));
    bind(&engine, auditor, Role::Auditor, workspace("a"));
    assert!(!allowed(&engine, dev, Action::WebhookManage, &ws, false));
    assert!(allowed(&engine, dev, Action::WebhookManage, &ws, true));
    assert!(!allowed(&engine, auditor, Action::WebhookManage, &ws, true));
}
