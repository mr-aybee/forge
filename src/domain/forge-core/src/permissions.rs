//! Access profiles and service accounts (D-086, issue #13).
//!
//! A [`PermissionProfile`] is a named, explicit list of [`Action`]s. Each
//! change publishes a new immutable [`ProfileRevision`]; a
//! [`ProfileAssignment`] grants the profile's latest revision to one user
//! (person or service account) at one scope. Cedar stays the only decision
//! engine: each revision becomes a policy template whose action list comes
//! from [`Action`] alone, and each assignment links that template with the
//! principal and the scope as slots, so no user-supplied text is ever part
//! of policy source.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{
    Action, DomainError, DomainResult, EntityKind, EntityRef, GroupId, PermissionProfileId, ProfileAssignmentId,
    Timestamp, UserId,
};

/// Scope kinds an assignment may name: places inside the one workspace.
pub const ASSIGNABLE_SCOPES: [EntityKind; 6] = [
    EntityKind::Workspace,
    EntityKind::Cluster,
    EntityKind::Project,
    EntityKind::Environment,
    EntityKind::Server,
    EntityKind::Service,
];

/// The name of the built-in profile that mirrors `role`.
#[must_use]
pub const fn builtin_name(role: crate::Role) -> &'static str {
    match role {
        crate::Role::PlatformAdmin => "Platform admin",
        crate::Role::WorkspaceAdmin => "Admin",
        crate::Role::Operator => "Operator",
        crate::Role::Developer => "Developer",
        crate::Role::Dba => "DBA",
        crate::Role::Auditor => "Auditor",
        crate::Role::Customer => "Customer",
    }
}

/// Longest profile name and description.
pub const MAX_NAME: usize = 80;
pub const MAX_DESCRIPTION: usize = 500;

/// A named set of actions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionProfile {
    pub id: PermissionProfileId,
    pub name: String,
    pub description: String,
    /// Mirrors a built-in role; read-only.
    pub builtin: bool,
    /// The latest published revision (1, 2, …).
    pub revision: u32,
    /// Archived profiles take no new assignments and grant nothing.
    pub archived_at: Option<Timestamp>,
    pub created_by: String,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

/// One published definition of a profile. Never changed after publishing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileRevision {
    pub profile_id: PermissionProfileId,
    pub revision: u32,
    /// Sorted, unique, every one a known action.
    pub actions: Vec<Action>,
    pub created_by: String,
    pub created_at: Timestamp,
}

/// Who an assignment grants to: one user, or every member of a group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Assignee {
    User(UserId),
    Group(GroupId),
}

/// A profile granted to one user, or to a group, at one scope. Exactly one
/// of `user_id` and `group_id` is set ([`ProfileAssignment::assignee`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileAssignment {
    pub id: ProfileAssignmentId,
    #[serde(default)]
    pub user_id: Option<UserId>,
    #[serde(default)]
    pub group_id: Option<GroupId>,
    pub profile_id: PermissionProfileId,
    pub scope: EntityRef,
    /// After this, the assignment grants nothing (removed within 30 s).
    pub expires_at: Option<Timestamp>,
    pub created_by: String,
    pub created_at: Timestamp,
}

impl ProfileAssignment {
    /// Who this assignment grants to. A row with neither or both set is
    /// refused by the store, so `None` never reaches the engine.
    #[must_use]
    pub fn assignee(&self) -> Option<Assignee> {
        match (self.user_id, self.group_id) {
            (Some(u), None) => Some(Assignee::User(u)),
            (None, Some(g)) => Some(Assignee::Group(g)),
            _ => None,
        }
    }

    #[must_use]
    pub fn expired(&self, now: Timestamp) -> bool {
        self.expires_at.is_some_and(|e| e <= now)
    }
}

/// A named set of users (#13). Assigning a profile to a group grants it to
/// every member; a user can be in many groups and a profile can be
/// assigned to many groups. Grants combine as a union.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Group {
    pub id: GroupId,
    pub name: String,
    pub description: String,
    pub created_by: String,
    pub created_at: Timestamp,
}

/// A group with its member ids.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupView {
    #[serde(flatten)]
    pub group: Group,
    pub members: Vec<UserId>,
}

/// A non-person account: tokens only, owned by a person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceAccount {
    pub user_id: UserId,
    pub owner_id: UserId,
    pub description: String,
    pub created_by: String,
    pub created_at: Timestamp,
}

/// One entry of a token's ceiling (#13): these actions, at this scope and
/// everything beneath it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenGrant {
    pub scope: EntityRef,
    pub actions: Vec<Action>,
}

/// The most entries a token's ceiling may have.
pub const MAX_TOKEN_GRANTS: usize = 50;

/// Whether a ceiling lets a token do `action` on a resource whose ancestry
/// is `resource`: one entry lists the action at a scope that covers it.
/// The owner's own access is checked separately; this only narrows it.
#[must_use]
pub fn ceiling_allows(ceiling: &[TokenGrant], action: Action, resource: &crate::ResourceRef) -> bool {
    ceiling.iter().any(|g| g.actions.contains(&action) && resource.is_within(&g.scope))
}

/// Checks a profile name: 1 to [`MAX_NAME`] characters, no control characters.
pub fn check_name(name: &str) -> DomainResult<String> {
    let n = name.trim();
    if n.is_empty() || n.chars().count() > MAX_NAME || n.chars().any(char::is_control) {
        return Err(DomainError::Invalid(format!("name: 1 to {MAX_NAME} characters, no control characters")));
    }

    Ok(n.to_owned())
}

/// Parses action names into a sorted, unique list of known actions. Unknown
/// names are refused, never dropped, and the list may not be empty.
pub fn parse_actions<S: AsRef<str>>(names: &[S]) -> DomainResult<Vec<Action>> {
    let mut out = Vec::new();

    for n in names {
        let n = n.as_ref();
        let a = Action::ALL
            .iter()
            .copied()
            .find(|a| a.as_str() == n)
            .ok_or_else(|| DomainError::Invalid(format!("unknown action `{}`", n.escape_default())))?;
        out.push(a);
    }

    if out.is_empty() {
        return Err(DomainError::Invalid("a profile needs at least one action".into()));
    }

    out.sort_by_key(|a| a.as_str());
    out.dedup();

    Ok(out)
}

/// Action names that once existed and were removed. A stored profile that
/// still lists one must load (the name grants nothing: no policy mentions
/// it), so an upgrade never refuses to start over a retired name.
pub const RETIRED_ACTIONS: &[&str] = &[
    // WireGuard remote access, removed with D-100.
    "network.access",
    "network.connect",
    "network.manage",
    // Object Storage, removed with D-180.
    "storage.manage",
    "storage.data",
];

/// Parses a stored action list: like [`parse_actions`], but names in
/// [`RETIRED_ACTIONS`] are dropped instead of refused. Truly unknown names
/// are still an error, so a hand-edited row can never grant one. A list
/// left empty by the drop is an error too: such a profile grants nothing and
/// should be archived rather than silently kept.
pub fn load_actions<S: AsRef<str>>(names: &[S]) -> DomainResult<Vec<Action>> {
    let kept: Vec<&str> = names.iter().map(AsRef::as_ref).filter(|n| !RETIRED_ACTIONS.contains(n)).collect();
    parse_actions(&kept)
}

/// Parses an assignment scope (`<kind>:<uuid>`) from the allow-list.
pub fn parse_scope(s: &str) -> DomainResult<EntityRef> {
    let r: EntityRef = s.parse().map_err(DomainError::Invalid)?;
    if !ASSIGNABLE_SCOPES.contains(&r.kind) {
        return Err(DomainError::Invalid(
            "scope must be a workspace, cluster, project, environment, server or service".into(),
        ));
    }

    if uuid::Uuid::parse_str(&r.id).is_err() {
        return Err(DomainError::Invalid("scope id must be an id".into()));
    }

    Ok(r)
}

/// Storage for profiles, assignments and service accounts.
#[async_trait]
pub trait PermissionRepository: Send + Sync + std::fmt::Debug {
    async fn list_profiles(&self) -> DomainResult<Vec<PermissionProfile>>;
    async fn get_profile(&self, id: PermissionProfileId) -> DomainResult<Option<PermissionProfile>>;
    /// Creates a profile with its first revision.
    async fn create_profile(&self, p: &PermissionProfile, first: &ProfileRevision) -> DomainResult<()>;
    /// Publishes `rev` (its number must be the profile's revision + 1).
    async fn publish_revision(&self, rev: &ProfileRevision, at: Timestamp) -> DomainResult<()>;
    async fn get_revision(&self, id: PermissionProfileId, revision: u32) -> DomainResult<Option<ProfileRevision>>;
    async fn list_revisions(&self, id: PermissionProfileId) -> DomainResult<Vec<ProfileRevision>>;
    async fn archive_profile(&self, id: PermissionProfileId, at: Timestamp) -> DomainResult<()>;
    /// Keeps a built-in profile equal to its role (start-up only): creates it,
    /// or publishes `rev` when the actions differ from the latest revision.
    /// Returns the profile as stored.
    async fn sync_builtin(&self, p: &PermissionProfile, rev: &ProfileRevision) -> DomainResult<PermissionProfile>;

    async fn list_assignments(&self) -> DomainResult<Vec<ProfileAssignment>>;
    /// Stores `a` only while its profile is not archived and still at
    /// `revision` (fails closed with `Conflict` otherwise), so an assignment
    /// can never outlive a concurrent publish or archive.
    async fn create_assignment(&self, a: &ProfileAssignment, revision: u32) -> DomainResult<()>;
    async fn delete_assignment(&self, id: ProfileAssignmentId) -> DomainResult<bool>;

    async fn list_service_accounts(&self) -> DomainResult<Vec<ServiceAccount>>;
    async fn get_service_account(&self, user: UserId) -> DomainResult<Option<ServiceAccount>>;
    /// Creates the service account's user (`kind = 'service'`) and records
    /// `sa`, in one transaction.
    async fn create_service_account(&self, user: &crate::model::User, sa: &ServiceAccount) -> DomainResult<()>;
    /// Disables or enables a service account's user. Only service accounts:
    /// returns `false` for any other user.
    async fn set_service_account_disabled(&self, user: UserId, disabled: bool) -> DomainResult<bool>;

    async fn list_groups(&self) -> DomainResult<Vec<Group>>;
    async fn get_group(&self, id: GroupId) -> DomainResult<Option<Group>>;
    /// `Conflict` when the name is taken (case-insensitive).
    async fn create_group(&self, g: &Group) -> DomainResult<()>;
    async fn update_group(&self, g: &Group) -> DomainResult<()>;
    /// Deletes the group, its memberships and its assignments in one
    /// transaction. Returns the ids of the assignments removed.
    async fn delete_group(&self, id: GroupId) -> DomainResult<Vec<ProfileAssignmentId>>;
    /// Every (group, member) pair.
    async fn list_memberships(&self) -> DomainResult<Vec<(GroupId, UserId)>>;
    /// Idempotent; returns whether the user was added now.
    async fn add_member(&self, group: GroupId, user: UserId, at: Timestamp) -> DomainResult<bool>;
    /// Idempotent; returns whether the user was a member.
    async fn remove_member(&self, group: GroupId, user: UserId) -> DomainResult<bool>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actions_are_known_sorted_unique_and_never_empty() {
        let a = parse_actions(&["view", "server.manage", "view"]).unwrap_or_default();
        assert_eq!(a, vec![Action::ServerManage, Action::View], "sorted by name, unique");
        assert!(parse_actions::<&str>(&[]).is_err());

        for bad in ["View", "view\"); permit(", "role:operator", "", "server.manage "] {
            assert!(parse_actions(&[bad]).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn stored_lists_drop_retired_actions_but_refuse_unknown_ones() {
        // An upgrade must start over a profile that still names a removed action.
        let a = load_actions(&["network.access", "view", "network.connect"]).unwrap_or_default();
        assert_eq!(a, vec![Action::View]);

        // Retired only: the profile grants nothing, so loading it is an error.
        assert!(load_actions(&["network.manage"]).is_err());

        // A name that never existed is still refused on the way out.
        assert!(load_actions(&["view", "made.up"]).is_err());

        // Input stays strict: retired names are not accepted on the way in.
        assert!(parse_actions(&["view", "network.access"]).is_err());

        for r in RETIRED_ACTIONS {
            assert!(Action::ALL.iter().all(|a| a.as_str() != *r), "{r} is still a live action");
        }
    }

    #[test]
    fn scopes_come_from_the_allow_list_with_real_ids() {
        let id = uuid::Uuid::now_v7();
        assert!(parse_scope(&format!("cluster:{id}")).is_ok());
        assert!(parse_scope(&format!("service:{id}")).is_ok());

        for bad in [
            format!("platform:{id}"),
            "platform:root".to_owned(),
            format!("job:{id}"),
            "cluster:x\") || true || (\"".to_owned(),
            "cluster:".to_owned(),
        ] {
            assert!(parse_scope(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_token_ceiling_covers_only_listed_actions_beneath_its_scopes() {
        let ws = EntityRef::new(EntityKind::Workspace, "w");
        let (ns_a, ns_b) = (EntityRef::new(EntityKind::Environment, "a"), EntityRef::new(EntityKind::Environment, "b"));
        let app = crate::ResourceRef::new(EntityRef::new(EntityKind::Service, "x"), vec![ns_a.clone(), ws.clone()]);
        let other = crate::ResourceRef::new(EntityRef::new(EntityKind::Service, "y"), vec![ns_b, ws]);
        let c = vec![TokenGrant { scope: ns_a, actions: vec![Action::View, Action::Deploy] }];
        assert!(ceiling_allows(&c, Action::View, &app));
        assert!(ceiling_allows(&c, Action::Deploy, &app));
        assert!(!ceiling_allows(&c, Action::SecretRead, &app), "not listed");
        assert!(!ceiling_allows(&c, Action::View, &other), "another environment");
        assert!(!ceiling_allows(&[], Action::View, &app), "an empty ceiling allows nothing");
    }

    #[test]
    fn names_are_bounded_text() {
        assert_eq!(check_name("  UAT Support ").ok().as_deref(), Some("UAT Support"));
        assert!(check_name("").is_err());
        assert!(check_name("a\nb").is_err());
        assert!(check_name(&"x".repeat(81)).is_err());
    }
}
