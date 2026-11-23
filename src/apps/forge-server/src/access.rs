//! Access profiles in the running server (D-086): loading them into the
//! policy engine at start, keeping built-in profiles equal to their roles,
//! user groups (#13), and removing expired assignments.
//!
//! Every change goes through these functions so the store and the engine
//! never disagree: a template is added before an assignment links to it,
//! and an assignment is unlinked as soon as it is deleted, archived away or
//! expired.

use std::collections::HashMap;
use std::time::Duration;

use forge_core::permissions::{
    Assignee, PermissionProfile, PermissionRepository, ProfileAssignment, ProfileRevision, builtin_name,
};
use forge_core::{
    Action, DomainError, DomainResult, EntityRef, GroupId, PermissionProfileId, Principal, Role, Timestamp, UserId,
};
use tokio::sync::watch;

use crate::state::AppState;

/// Refuses an interactive sign-in for a service account (D-086). They have
/// no password, passkey, authenticator or SSO link, so this is belt and
/// braces against a future credential path.
pub async fn refuse_service_account(state: &AppState, user: forge_core::UserId) -> Result<(), crate::error::ApiError> {
    match state.store.get_service_account(user).await {
        Ok(None) => Ok(()),
        Ok(Some(_)) => Err(crate::error::ApiError::forbidden("service accounts don't sign in; they use tokens")),
        Err(e) => Err(e.into()),
    }
}

/// How often expired assignments are removed.
pub const SWEEP: Duration = Duration::from_secs(30);

/// Creates or updates the built-in profiles, adds the latest revision of
/// every active profile to the engine and links every unexpired
/// assignment. Run once at start.
pub async fn load(
    store: &forge_store::Store,
    policy: &forge_authz::PolicyEngine,
    now: Timestamp,
) -> anyhow::Result<()> {
    for role in Role::ALL {
        let actions = policy.role_actions(role)?;
        let id = PermissionProfileId::new();
        let p = PermissionProfile {
            id,
            name: builtin_name(role).to_owned(),
            description: format!("Built-in: the {role} role, read-only. Duplicate it to change it."),
            builtin: true,
            revision: 1,
            archived_at: None,
            created_by: "system:forge".into(),
            created_at: now,
            updated_at: now,
        };
        let rev = ProfileRevision {
            profile_id: id,
            revision: 1,
            actions,
            created_by: "system:forge".into(),
            created_at: now,
        };
        store.sync_builtin(&p, &rev).await?;
    }
    let mut revisions: HashMap<PermissionProfileId, u32> = HashMap::new();
    for p in store.list_profiles().await? {
        if p.archived_at.is_some() {
            continue;
        }
        let Some(rev) = store.get_revision(p.id, p.revision).await? else { continue };
        policy.add_profile_revision(p.id, rev.revision, &rev.actions)?;
        revisions.insert(p.id, rev.revision);
    }
    for a in store.list_assignments().await? {
        if a.expired(now) {
            continue;
        }
        if let Some(rev) = revisions.get(&a.profile_id) {
            policy.link_assignment(&a, *rev)?;
        }
    }
    policy.set_memberships(&store.list_memberships().await?);
    Ok(())
}

fn internal(e: impl std::fmt::Display) -> DomainError {
    DomainError::internal(e.to_string())
}

/// The actions of `profile`'s latest revision.
pub async fn current_actions(state: &AppState, profile: &PermissionProfile) -> DomainResult<Vec<Action>> {
    Ok(state.store.get_revision(profile.id, profile.revision).await?.map(|r| r.actions).unwrap_or_default())
}

/// Which of `actions` the caller does not hold over `scope` (the scope and
/// everything beneath it) right now, with their session's MFA state. The
/// escalation guard: a person can only hand out, or widen into, what they
/// hold. "Over" lets a server-only action such as `container.manage` be
/// granted at an environment or cluster by someone who holds it there (#13).
pub async fn missing(caller: &crate::auth::Caller, actions: &[Action], scope: &EntityRef) -> Vec<Action> {
    let held = caller.holds_over(actions, scope).await;
    actions.iter().zip(held).filter(|(_, ok)| !ok).map(|(a, _)| *a).collect()
}

/// Stores and links a new assignment. The caller holds `access_lock`
/// across its own checks and this call.
pub async fn assign(state: &AppState, a: &ProfileAssignment) -> DomainResult<()> {
    let p = state
        .store
        .get_profile(a.profile_id)
        .await?
        .ok_or_else(|| DomainError::NotFound("profile not found".into()))?;
    if p.archived_at.is_some() {
        return Err(DomainError::Conflict("the profile is archived".into()));
    }
    let actions = current_actions(state, &p).await?;
    state.policy.add_profile_revision(p.id, p.revision, &actions).map_err(internal)?;
    // Fails closed if the profile was archived or published meanwhile.
    state.store.create_assignment(a, p.revision).await?;
    state.policy.link_assignment(a, p.revision).map_err(internal)
}

/// Unlinks and deletes an assignment. Idempotent.
pub async fn unassign(state: &AppState, id: forge_core::ProfileAssignmentId) -> DomainResult<bool> {
    state.policy.unlink_assignment(id).map_err(internal)?;
    state.store.delete_assignment(id).await
}

/// Adds `user` to `group` in the store, then in the engine, so the grant
/// only starts once it is recorded. Returns whether the user was added now.
pub async fn add_member(state: &AppState, group: GroupId, user: UserId) -> DomainResult<bool> {
    let added = state.store.add_member(group, user, state.clock.now()).await?;
    if state.store.get_group(group).await?.is_none() {
        return Err(DomainError::NotFound("group not found".into()));
    }
    state.policy.add_member(group, user);
    Ok(added)
}

/// Removes `user` from `group`: the engine first, so the grant ends at
/// once even if the store write fails (fails closed until restart).
pub async fn remove_member(state: &AppState, group: GroupId, user: UserId) -> DomainResult<bool> {
    state.policy.remove_member(group, user);
    state.store.remove_member(group, user).await
}

/// Deletes a group with its memberships and assignments. The engine stops
/// granting first; returns how many assignments were removed.
pub async fn delete_group(state: &AppState, group: GroupId) -> DomainResult<usize> {
    state.policy.drop_group(group);
    for a in state.store.list_assignments().await?.into_iter().filter(|a| a.group_id == Some(group)) {
        state.policy.unlink_assignment(a.id).map_err(internal)?;
    }
    let removed = state.store.delete_group(group).await?;
    for id in &removed {
        state.policy.unlink_assignment(*id).map_err(internal)?;
    }
    Ok(removed.len())
}

/// The users an assignment reaches: its user, or its group's members.
#[must_use]
pub fn reached_users(a: &ProfileAssignment, memberships: &[(GroupId, UserId)]) -> Vec<UserId> {
    match a.assignee() {
        Some(Assignee::User(u)) => vec![u],
        Some(Assignee::Group(g)) => memberships.iter().filter(|(x, _)| *x == g).map(|(_, u)| *u).collect(),
        None => vec![],
    }
}

/// Publishes `actions` as the profile's next revision and moves every
/// assignment to it.
pub async fn publish(
    state: &AppState,
    profile: &PermissionProfile,
    actions: Vec<Action>,
    by: &Principal,
) -> DomainResult<ProfileRevision> {
    let now = state.clock.now();
    let rev = ProfileRevision {
        profile_id: profile.id,
        revision: profile.revision + 1,
        actions,
        created_by: by.to_string(),
        created_at: now,
    };
    state.policy.add_profile_revision(profile.id, rev.revision, &rev.actions).map_err(internal)?;
    state.store.publish_revision(&rev, now).await?;
    for a in state.store.list_assignments().await?.into_iter().filter(|a| a.profile_id == profile.id && !a.expired(now))
    {
        state.policy.link_assignment(&a, rev.revision).map_err(internal)?;
    }
    Ok(rev)
}

/// Archives a profile and unlinks every assignment of it at once.
pub async fn archive(state: &AppState, profile: &PermissionProfile) -> DomainResult<usize> {
    state.store.archive_profile(profile.id, state.clock.now()).await?;
    let mut n = 0;
    for a in state.store.list_assignments().await?.into_iter().filter(|a| a.profile_id == profile.id) {
        state.policy.unlink_assignment(a.id).map_err(internal)?;
        n += 1;
    }
    Ok(n)
}

/// Removes assignments whose expiry passed; audited as the system.
pub async fn sweep(state: &AppState) {
    let now = state.clock.now();
    let all = match state.store.list_assignments().await {
        Ok(all) => all,
        Err(e) => {
            // Expired links stop granting anyway (the engine checks expiry
            // at every decision); the rows stay until the next pass.
            tracing::error!(error = %e, "access assignment sweep could not list assignments");
            return;
        }
    };
    for a in all.into_iter().filter(|a| a.expired(now)) {
        let removed = match unassign(state, a.id).await {
            Ok(r) => r,
            Err(e) => {
                tracing::error!(assignment = %a.id, error = %e, "could not remove an expired access assignment");
                false
            }
        };
        if removed {
            state
                .audit_system(
                    "access.assignment.expire",
                    serde_json::json!({ "assignment": a.id, "user": a.user_id, "group": a.group_id, "profile": a.profile_id, "scope": a.scope.to_string() }),
                )
                .await;
        }
    }
}

pub(crate) async fn run(state: AppState, mut shutdown: watch::Receiver<bool>) {
    loop {
        tokio::select! {
            () = tokio::time::sleep(SWEEP) => sweep(&state).await,
            _ = shutdown.changed() => break,
        }
    }
}
