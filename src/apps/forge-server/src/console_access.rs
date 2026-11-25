//! What the signed-in caller may open in the web console: one boolean per
//! gated navigation entry, so the console hides what the server would
//! refuse. The server stays the authority; every page's endpoints still
//! authorize each request.
//!
//! Two kinds of checks, both batched:
//!
//! * **Exact**: the action the page's main endpoint authorizes, on the
//!   resource it authorizes (the workspace or the platform), asked of the
//!   real authorizer. Asked as if the session were MFA-verified: the MFA
//!   guardrail decides a request, not whether the console shows the page,
//!   so a session without MFA still sees the entry and is asked to step up
//!   when it acts.
//! * **Anywhere**: pages whose lists the server filters per row (nodes,
//!   environments, stores…). True when one of the caller's role bindings
//!   grants the action at any scope, read from the Cedar role groups. No
//!   entity is scanned, so a user with a role on one cluster or one
//!   environment sees the section and the list shows what they can see.

use std::collections::HashSet;

use forge_authz::PolicyEngine;
use forge_core::ports::{Authorizer, RoleBindingRepository};
use forge_core::{
    AccessRequest, Action, AuthzContext, DomainError, DomainResult, EntityKind, EntityRef, Principal, Role, UserId,
    WorkspaceId,
};
use serde::Serialize;

/// Navigation capabilities of one user. Each field names the Cedar action
/// it stands for; see `reference/rest-api.md` (`GET /me/capabilities`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, schemars::JsonSchema)]
pub struct ConsoleCapabilities {
    /// `view` anywhere: clusters, nodes, services, environments,
    /// services, stacks, DNS, ingress, installed packages, object storage, jobs.
    pub fleet_view: bool,
    /// `terminal.open` anywhere: the Terminal page.
    pub terminal: bool,
    /// `package.manage` anywhere: the App Store.
    pub package_install: bool,
    /// `server.manage` anywhere: package updates.
    pub software_updates: bool,
    /// `file.read` anywhere: SFTP access.
    pub sftp_access: bool,
    /// `network.tunnel` anywhere, or `access.manage` on the workspace: Tunnels.
    pub tunnels: bool,
    /// `alert.view` on the workspace: Alerts.
    pub alerts_view: bool,
    /// `alert.manage` on the workspace.
    pub alerts_manage: bool,
    /// `notification.manage` on the workspace.
    pub notifications_manage: bool,
    /// `audit.read` on the platform: Audit log (the console reads the whole log).
    pub audit_view: bool,
    /// `user.manage` on the workspace: Members & roles.
    pub users_manage: bool,
    /// `access.manage` on the workspace: access profiles, service accounts.
    pub access_manage: bool,
    /// A `platform_admin` binding: Credential vault.
    pub vault: bool,
    /// `system.update` on the platform: Forge updates.
    pub forge_updates: bool,
    /// `analytics.configure` on the platform: change analytics settings.
    pub analytics_configure: bool,
    /// `view` on the platform: Plugins.
    pub plugins: bool,
    /// `view` on the platform: server Settings.
    pub platform_settings: bool,
}

/// Computes [`ConsoleCapabilities`] from ports.
#[derive(Debug)]
pub struct ConsoleAccess<'a> {
    pub bindings: &'a dyn RoleBindingRepository,
    pub policy: &'a PolicyEngine,
    pub authz: &'a dyn Authorizer,
}

/// Exact probes, in the order [`ConsoleAccess::for_user`] reads them.
const EXACT: [(Action, Scope); 9] = [
    (Action::AlertView, Scope::Workspace),
    (Action::AlertManage, Scope::Workspace),
    (Action::NotificationManage, Scope::Workspace),
    (Action::AuditRead, Scope::Platform),
    (Action::UserManage, Scope::Workspace),
    (Action::AccessManage, Scope::Workspace),
    (Action::SystemUpdate, Scope::Platform),
    (Action::AnalyticsConfigure, Scope::Platform),
    (Action::View, Scope::Platform),
];

#[derive(Debug, Clone, Copy)]
enum Scope {
    Platform,
    Workspace,
}

/// Whether `user` holds any role binding on the platform, on workspace `ws`,
/// or on something inside it (a cluster, environment, node…). Such a member
/// may read the workspace's own record even without `view` on the whole
/// workspace: the console needs it to load at all. A binding whose scope no
/// longer resolves does not count.
pub async fn is_member(
    bindings: &dyn RoleBindingRepository,
    resolver: &dyn forge_core::ports::ResourceResolver,
    user: UserId,
    ws: WorkspaceId,
) -> DomainResult<bool> {
    let ws_ref = EntityRef::new(EntityKind::Workspace, ws);

    for b in bindings.list_bindings_for_user(user).await? {
        if b.scope.kind == EntityKind::Platform || b.scope == ws_ref {
            return Ok(true);
        }

        match resolver.resolve(&b.scope).await {
            Ok(r) if r.ancestors.contains(&ws_ref) => return Ok(true),
            Ok(_) | Err(DomainError::NotFound(_) | DomainError::Invalid(_)) => {}
            Err(e) => return Err(e),
        }
    }

    Ok(false)
}

impl ConsoleAccess<'_> {
    /// The capabilities of `user` in the deployment's workspace `ws`.
    pub async fn for_user(&self, user: UserId, ws: WorkspaceId) -> DomainResult<ConsoleCapabilities> {
        let bindings = self.bindings.list_bindings_for_user(user).await?;
        let roles: HashSet<Role> = bindings.iter().map(|b| b.role).collect();
        let mut anywhere: HashSet<Action> = HashSet::new();

        for role in &roles {
            anywhere.extend(self.policy.role_actions(*role).map_err(DomainError::internal)?);
        }

        // MFA-verified context on purpose: see the module documentation.
        let ctx = AuthzContext { mfa: true };
        let ws_ref = EntityRef::new(EntityKind::Workspace, ws);
        let reqs: Vec<AccessRequest> = EXACT
            .iter()
            .map(|(action, scope)| {
                let resource = match scope {
                    Scope::Platform => EntityRef::platform(),
                    Scope::Workspace => ws_ref.clone(),
                };

                AccessRequest::new(Principal::User(user), *action, resource, ctx)
            })
            .collect();
        let exact = self.authz.is_allowed_many(&reqs).await;
        let at = |i: usize| exact.get(i).copied().unwrap_or(false);
        let any = |a: Action| anywhere.contains(&a);

        let access_manage = at(5);

        Ok(ConsoleCapabilities {
            fleet_view: any(Action::View),
            terminal: any(Action::TerminalOpen),
            package_install: any(Action::PackageManage),
            software_updates: any(Action::ServerManage),
            sftp_access: any(Action::FileRead),
            tunnels: any(Action::NetworkTunnel) || access_manage,
            alerts_view: at(0),
            alerts_manage: at(1),
            notifications_manage: at(2),
            audit_view: at(3),
            users_manage: at(4),
            access_manage,
            // The vault endpoints check for a platform_admin binding.
            vault: roles.contains(&Role::PlatformAdmin),
            forge_updates: at(6),
            analytics_configure: at(7),
            plugins: at(8),
            platform_settings: at(8),
        })
    }
}
