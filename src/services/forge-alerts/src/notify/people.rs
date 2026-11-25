//! Policy recipients (#11, D-117): the people a policy emails by user and by
//! role, resolved per alarm when a delivery is planned.
//!
//! * A **named user** is emailed only while they may see the alarm
//!   (`alert.view` on its resource): naming someone in a policy never shows
//!   them an alarm they could not open in the console.
//! * A **role** stands for the people responsible for the alarm's resource:
//!   everyone holding that role on the resource itself or on any of its
//!   ancestors (environment, cluster, workspace, platform). A binding on a
//!   sibling resource does not count. Role holders pass the same
//!   `alert.view` check.
//! * Disabled accounts and accounts without an address are skipped. Skipped
//!   people are recorded by id and reason, never by address.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use forge_core::alerts::Alarm;
use forge_core::model::User;
use forge_core::notifications::{
    DeliveryRecipients, MAX_RESOLVED_RECIPIENTS, NotificationPolicy, RecipientSkip, SkippedRecipient,
};
use forge_core::ports::{Authorizer, ResourceResolver, RoleBindingRepository, UserRepository};
use forge_core::{
    AccessRequest, Action, AuthzContext, DomainError, DomainResult, EntityKind, EntityRef, Principal, ResourceRef,
    UserId, WorkspaceId,
};

/// The ports recipient resolution reads through.
#[derive(Debug, Clone)]
pub struct People {
    pub users: Arc<dyn UserRepository>,
    pub bindings: Arc<dyn RoleBindingRepository>,
    pub authorizer: Arc<dyn Authorizer>,
    pub resolver: Arc<dyn ResourceResolver>,
}

fn ws_ref(ws: WorkspaceId) -> EntityRef {
    EntityRef::new(EntityKind::Workspace, ws)
}

impl People {
    /// Checks that every id is a user of the workspace: an account that
    /// holds a role on the platform, the workspace or anything in it.
    pub async fn check_members(&self, workspace: WorkspaceId, ids: &[UserId]) -> DomainResult<()> {
        let found: HashSet<UserId> = self.users.get_users(ids).await?.into_iter().map(|u| u.id).collect();
        let ws = ws_ref(workspace);

        for id in ids {
            if !found.contains(id) {
                return Err(DomainError::Invalid(format!("email_users: no user {id}")));
            }

            let mut member = false;

            for b in self.bindings.list_bindings_for_user(*id).await? {
                member = match b.scope.kind {
                    EntityKind::Platform => true,
                    EntityKind::Workspace => b.scope == ws,
                    _ => match self.resolver.resolve(&b.scope).await {
                        Ok(r) => r.is_within(&ws),
                        // A binding on something deleted grants nothing.
                        Err(DomainError::NotFound(_)) => false,
                        Err(e) => return Err(e),
                    },
                };

                if member {
                    break;
                }
            }

            if !member {
                return Err(DomainError::Invalid(format!("email_users: user {id} has no role in this workspace")));
            }
        }

        Ok(())
    }

    /// The named users without an email address (for a warning in the
    /// policy editor).
    pub async fn without_email(&self, ids: &[UserId]) -> DomainResult<Vec<UserId>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }

        let users = self.users.get_users(ids).await?;
        let mut out: Vec<UserId> = users
            .into_iter()
            .filter(|u| u.email.as_deref().is_none_or(|e| e.trim().is_empty()))
            .map(|u| u.id)
            .collect();
        out.sort();

        Ok(out)
    }

    /// The alarm's resource with its ancestors. A resource that is gone
    /// keeps its workspace as its only ancestor.
    async fn resource(&self, alarm: &Alarm) -> DomainResult<ResourceRef> {
        match self.resolver.resolve(&alarm.resource).await {
            Ok(r) => Ok(r),
            Err(DomainError::NotFound(_)) => {
                Ok(ResourceRef::new(alarm.resource.clone(), vec![ws_ref(alarm.workspace_id)]))
            }
            Err(e) => Err(e),
        }
    }

    /// The people of `policy` for `alarm`: named users and role holders,
    /// deduplicated by address. `mailbox` checks an address and returns it
    /// as it is sent (`None`: not an address).
    pub async fn resolve(
        &self,
        policy: &NotificationPolicy,
        alarm: &Alarm,
        mailbox: impl Fn(&str) -> Option<String>,
    ) -> DomainResult<DeliveryRecipients> {
        let mut candidates: Vec<UserId> = policy.email_users.clone();

        if !policy.email_roles.is_empty() {
            let resource = self.resource(alarm).await?;

            for b in self.bindings.list_bindings().await? {
                if policy.email_roles.contains(&b.role) && resource.is_within(&b.scope) {
                    candidates.push(b.user_id);
                }
            }
        }

        candidates.sort();
        candidates.dedup();
        let users: HashMap<UserId, User> =
            self.users.get_users(&candidates).await?.into_iter().map(|u| (u.id, u)).collect();
        let asked: Vec<UserId> =
            candidates.iter().copied().filter(|id| users.get(id).is_some_and(|u| !u.disabled)).collect();
        let requests: Vec<AccessRequest> = asked
            .iter()
            .map(|id| {
                AccessRequest::new(
                    Principal::User(*id),
                    Action::AlertView,
                    alarm.resource.clone(),
                    AuthzContext::default(),
                )
            })
            .collect();
        let allowed: HashSet<UserId> = asked
            .into_iter()
            .zip(self.authorizer.is_allowed_many(&requests).await)
            .filter(|(_, ok)| *ok)
            .map(|(id, _)| id)
            .collect();

        let mut out = DeliveryRecipients::default();
        let mut seen: HashSet<String> = HashSet::new();

        for id in candidates {
            let skip = |reason| SkippedRecipient { user_id: id, reason };

            let Some(user) = users.get(&id) else {
                out.skipped.push(skip(RecipientSkip::NoAccess));
                continue;
            };

            if user.disabled {
                out.skipped.push(skip(RecipientSkip::Disabled));
                continue;
            }

            if !allowed.contains(&id) {
                out.skipped.push(skip(RecipientSkip::NoAccess));
                continue;
            }

            let Some(address) = user.email.as_deref().map(str::trim).filter(|e| !e.is_empty()).and_then(&mailbox)
            else {
                out.skipped.push(skip(RecipientSkip::NoEmail));
                continue;
            };

            if !seen.insert(address.to_lowercase()) {
                continue;
            }

            if out.to.len() >= MAX_RESOLVED_RECIPIENTS {
                out.skipped.push(skip(RecipientSkip::TooMany));
                continue;
            }

            out.to.push(address);
        }

        out.count = out.to.len();

        Ok(out)
    }
}
