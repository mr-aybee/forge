//! Access profiles and assignments (D-086).
//!
//! * `GET  /api/v1/access/catalog`: every action, whether it needs MFA and
//!   which built-in roles hold it.
//! * `GET  /api/v1/access/profiles`, `POST` (create, `access.manage`).
//! * `GET  /api/v1/access/profiles/{id}` with its revisions; `PUT` publishes a
//!   new revision (`?dry_run=true` returns the impact only); `DELETE`
//!   archives (its assignments stop granting at once).
//! * `POST /api/v1/access/profiles/{id}/duplicate`: an editable copy.
//! * `GET  /api/v1/access/assignments`, `POST` (assign to a user or a
//!   group), `DELETE /{id}`.
//! * `GET  /api/v1/access/groups`, `POST`; `PATCH`/`DELETE /{id}`;
//!   `PUT`/`DELETE /{id}/members/{user_id}` (#13).
//! * `GET  /api/v1/access/effective?user_id=&scope=`: every action, allowed
//!   or not, for one user at one scope, with the assignments behind it.
//!
//! Escalation guard: assigning a profile, or publishing a revision of a
//! profile that is assigned, needs every action of it on each affected
//! scope, held by the caller right now, with an MFA-verified session.
//! Removing an assignment never needs MFA. Adding a member to a group is
//! an assignment of everything the group holds, so it is guarded the same
//! way at every scope the group is assigned.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use forge_core::permissions::{
    Group, GroupView, PermissionProfile, PermissionRepository, ProfileAssignment, ProfileRevision, check_name,
    parse_actions, parse_scope,
};
use forge_core::ports::UserRepository;
use forge_core::{
    AccessRequest, Action, AuthzContext, DomainError, EntityKind, EntityRef, GroupId, PermissionProfileId, Principal,
    ProfileAssignmentId, Role, UserId, WorkspaceId,
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::auth::{Caller, Ws};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

fn ws_ref(ws: WorkspaceId) -> EntityRef {
    EntityRef::new(EntityKind::Workspace, ws)
}

fn profile_ref(id: PermissionProfileId) -> EntityRef {
    // Profiles have no Cedar entity; audit names them under the platform.
    EntityRef::new(EntityKind::Platform, format!("profile:{id}"))
}

fn group_ref(id: GroupId) -> EntityRef {
    // Groups have no Cedar resource entity; audit names them under the platform.
    EntityRef::new(EntityKind::Platform, format!("group:{id}"))
}

fn names(actions: &[Action]) -> Vec<&'static str> {
    actions.iter().map(|a| a.as_str()).collect()
}

async fn view(state: &AppState, p: &PermissionProfile) -> ApiResult<Value> {
    let actions = crate::access::current_actions(state, p).await?;
    let mut v = serde_json::to_value(p).map_err(DomainError::internal)?;
    v["actions"] = json!(names(&actions));
    Ok(v)
}

async fn load(state: &AppState, id: PermissionProfileId) -> ApiResult<PermissionProfile> {
    state.store.get_profile(id).await?.ok_or_else(|| ApiError::not_found("profile not found"))
}

/// `GET /access/catalog`.
pub async fn catalog(State(state): State<AppState>, caller: Caller, Ws(ws): Ws) -> ApiResult<Json<Value>> {
    caller.authorize(Action::View, ws_ref(ws)).await?;
    let mfa = super::openapi::mfa_actions();
    let mut roles_of: std::collections::HashMap<&str, Vec<&str>> = std::collections::HashMap::new();
    for role in Role::ALL {
        for a in state.policy.role_actions(role).map_err(DomainError::internal)? {
            roles_of.entry(a.as_str()).or_default().push(role.as_str());
        }
    }
    let actions: Vec<Value> = Action::ALL
        .iter()
        .map(|a| {
            let info = a.info();
            json!({
                "action": a.as_str(),
                "area": info.area,
                "label": info.label,
                "description": info.description,
                "level": info.level,
                "mfa": mfa.contains(a.as_str()),
                "roles": roles_of.get(a.as_str()).cloned().unwrap_or_default(),
            })
        })
        .collect();
    let scopes: Vec<&str> = forge_core::permissions::ASSIGNABLE_SCOPES.iter().map(|k| k.as_str()).collect();
    Ok(Json(json!({ "actions": actions, "scopes": scopes })))
}

/// `GET /access/profiles`.
pub async fn list(State(state): State<AppState>, caller: Caller, Ws(ws): Ws) -> ApiResult<Json<Vec<Value>>> {
    caller.authorize(Action::View, ws_ref(ws)).await?;
    let mut out = Vec::new();
    for p in state.store.list_profiles().await? {
        out.push(view(&state, &p).await?);
    }
    Ok(Json(out))
}

/// `GET /access/profiles/{id}`: the profile and every revision.
pub async fn get(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Path(id): Path<PermissionProfileId>,
) -> ApiResult<Json<Value>> {
    caller.authorize(Action::View, ws_ref(ws)).await?;
    let p = load(&state, id).await?;
    let mut v = view(&state, &p).await?;
    let revs: Vec<Value> = state
        .store
        .list_revisions(id)
        .await?
        .iter()
        .map(|r| json!({ "revision": r.revision, "actions": names(&r.actions), "created_by": r.created_by, "created_at": r.created_at }))
        .collect();
    v["revisions"] = json!(revs);
    Ok(Json(v))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NewProfile {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Action names from `GET /access/catalog`.
    pub actions: Vec<String>,
}

fn check_description(d: &str) -> ApiResult<String> {
    let d = d.trim();
    if d.chars().count() > forge_core::permissions::MAX_DESCRIPTION || d.chars().any(|c| c.is_control() && c != '\n') {
        return Err(ApiError::bad_request("description: at most 500 characters"));
    }
    Ok(d.to_owned())
}

async fn create_profile(
    state: &AppState,
    caller: &Caller,
    name: &str,
    description: &str,
    actions: Vec<Action>,
    copied_from: Option<(PermissionProfileId, u32)>,
) -> ApiResult<PermissionProfile> {
    let now = state.clock.now();
    let id = PermissionProfileId::new();
    let by = caller.principal().to_string();
    let p = PermissionProfile {
        id,
        name: check_name(name)?,
        description: check_description(description)?,
        builtin: false,
        revision: 1,
        archived_at: None,
        created_by: by.clone(),
        created_at: now,
        updated_at: now,
    };
    let rev = ProfileRevision { profile_id: id, revision: 1, actions, created_by: by, created_at: now };
    let taken = |name: &str| ApiError::conflict(format!("a profile named `{name}` exists"));
    if state.store.list_profiles().await?.iter().any(|x| x.name.eq_ignore_ascii_case(&p.name)) {
        return Err(taken(&p.name));
    }
    match state.store.create_profile(&p, &rev).await {
        Err(DomainError::Conflict(_)) => return Err(taken(&p.name)),
        r => r?,
    }
    caller
        .audit(
            "access.profile.create",
            &profile_ref(id),
            None,
            json!({ "name": p.name, "revision": 1, "actions": names(&rev.actions), "copied_from": copied_from.map(|(i, r)| json!({ "profile": i, "revision": r })) }),
        )
        .await;
    Ok(p)
}

/// `POST /access/profiles`.
pub async fn create(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Json(req): Json<NewProfile>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    caller.authorize(Action::AccessManage, ws_ref(ws)).await?;
    let actions = parse_actions(&req.actions)?;
    let p = create_profile(&state, &caller, &req.name, &req.description, actions, None).await?;
    Ok((StatusCode::CREATED, Json(view(&state, &p).await?)))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Duplicate {
    pub name: String,
}

/// `POST /access/profiles/{id}/duplicate`.
pub async fn duplicate(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Path(id): Path<PermissionProfileId>,
    Json(req): Json<Duplicate>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    caller.authorize(Action::AccessManage, ws_ref(ws)).await?;
    let src = load(&state, id).await?;
    let actions = crate::access::current_actions(&state, &src).await?;
    let p = create_profile(&state, &caller, &req.name, &src.description, actions, Some((src.id, src.revision))).await?;
    Ok((StatusCode::CREATED, Json(view(&state, &p).await?)))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Publish {
    pub actions: Vec<String>,
    /// The revision the change is based on; another publish meanwhile gives
    /// `409`.
    pub expected_revision: u32,
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct DryRun {
    #[serde(default)]
    pub dry_run: bool,
}

/// `PUT /access/profiles/{id}`: publishes a new revision. Every assignment
/// moves to it, so the caller needs every new action at every assigned
/// scope. `?dry_run=true` returns the impact without publishing.
pub async fn publish(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Path(id): Path<PermissionProfileId>,
    Query(q): Query<DryRun>,
    Json(req): Json<Publish>,
) -> ApiResult<Json<Value>> {
    caller.authorize(Action::AccessManage, ws_ref(ws)).await?;
    if !q.dry_run {
        caller.require_mfa("access.profile.publish", &profile_ref(id)).await?;
    }
    let _serial = state.access_lock.lock().await;
    let p = load(&state, id).await?;
    if p.builtin {
        caller.audit_denied("access.profile.publish", &profile_ref(id), None, "built-in profiles are read-only").await;
        return Err(ApiError::conflict("built-in profiles are read-only; duplicate it to change it"));
    }
    if p.archived_at.is_some() {
        caller.audit_denied("access.profile.publish", &profile_ref(id), None, "the profile is archived").await;
        return Err(ApiError::conflict("the profile is archived"));
    }
    if req.expected_revision != p.revision {
        return Err(ApiError::conflict(format!("the profile is at revision {} now", p.revision)));
    }
    let before = crate::access::current_actions(&state, &p).await?;
    let after = parse_actions(&req.actions)?;
    let added: Vec<Action> = after.iter().copied().filter(|a| !before.contains(a)).collect();
    let removed: Vec<Action> = before.iter().copied().filter(|a| !after.contains(a)).collect();
    let now = state.clock.now();
    let assignments: Vec<ProfileAssignment> =
        state.store.list_assignments().await?.into_iter().filter(|a| a.profile_id == id && !a.expired(now)).collect();
    // Only what the revision adds can widen anyone's access.
    let mut blocked = Vec::new();
    for a in &assignments {
        let missing = crate::access::missing(&caller, &added, &a.scope).await;
        if !missing.is_empty() {
            blocked.push(json!({ "assignment": a.id, "scope": a.scope.to_string(), "missing": names(&missing) }));
        }
    }
    let memberships = state.store.list_memberships().await?;
    let users: std::collections::BTreeSet<UserId> =
        assignments.iter().flat_map(|a| crate::access::reached_users(a, &memberships)).collect();
    let impact = json!({
        "profile": id,
        "from_revision": p.revision,
        "added": names(&added),
        "removed": names(&removed),
        "assignments": assignments.len(),
        "users": users.len(),
        "blocked": blocked,
    });
    if q.dry_run {
        return Ok(Json(impact));
    }
    if !blocked.is_empty() {
        caller
            .audit_denied(
                "access.profile.publish",
                &profile_ref(id),
                None,
                "the caller lacks added actions at assigned scopes",
            )
            .await;
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "escalation",
            format!("you would grant actions you don't hold at {} assigned scope(s); see ?dry_run=true", blocked.len()),
        ));
    }
    let rev = crate::access::publish(&state, &p, after, &caller.principal()).await?;
    caller
        .audit(
            "access.profile.publish",
            &profile_ref(id),
            None,
            json!({ "name": p.name, "revision": rev.revision, "added": names(&added), "removed": names(&removed), "assignments": assignments.len() }),
        )
        .await;
    Ok(Json(view(&state, &load(&state, id).await?).await?))
}

/// `DELETE /access/profiles/{id}`: archives; its assignments stop granting
/// at once and it takes no new ones.
pub async fn archive(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Path(id): Path<PermissionProfileId>,
) -> ApiResult<Json<Value>> {
    caller.authorize(Action::AccessManage, ws_ref(ws)).await?;
    let _serial = state.access_lock.lock().await;
    let p = load(&state, id).await?;
    if p.builtin {
        caller.audit_denied("access.profile.archive", &profile_ref(id), None, "built-in profiles are read-only").await;
        return Err(ApiError::conflict("built-in profiles cannot be archived"));
    }
    let n = crate::access::archive(&state, &p).await?;
    caller
        .audit("access.profile.archive", &profile_ref(id), None, json!({ "name": p.name, "assignments_unlinked": n }))
        .await;
    Ok(Json(json!({ "archived": id, "assignments_unlinked": n })))
}

// ------------------------------------------------------------ assignments

/// `GET /access/assignments`: assignments at scopes where the caller has
/// `user.manage`, and the caller's own.
pub async fn list_assignments(
    State(state): State<AppState>,
    caller: Caller,
) -> ApiResult<Json<Vec<ProfileAssignment>>> {
    let me = caller.session.user.id;
    let mine = state.policy.groups_of(me);
    let all = state.store.list_assignments().await?;
    let allowed = caller.can_many(Action::UserManage, all.iter().map(|a| a.scope.clone()).collect()).await;
    let out = all
        .into_iter()
        .zip(allowed)
        .filter(|(a, ok)| *ok || a.user_id == Some(me) || a.group_id.is_some_and(|g| mine.contains(&g)))
        .map(|(a, _)| a)
        .collect();
    Ok(Json(out))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NewAssignment {
    #[schemars(with = "Option<String>")]
    #[serde(default)]
    pub user_id: Option<UserId>,
    /// Username or email (alternative to `user_id`).
    #[serde(default)]
    pub login: Option<String>,
    /// A group instead of a user: every member gets the profile.
    #[schemars(with = "Option<String>")]
    #[serde(default)]
    pub group_id: Option<GroupId>,
    #[schemars(with = "String")]
    pub profile_id: PermissionProfileId,
    /// `<kind>:<id>`: workspace, cluster, project, environment, server or service.
    pub scope: String,
    /// Minutes until it stops granting (1 to 525 600); default: no expiry.
    #[serde(default)]
    pub expires_in_minutes: Option<u32>,
}

/// `POST /access/assignments`.
pub async fn assign(
    State(state): State<AppState>,
    caller: Caller,
    Json(req): Json<NewAssignment>,
) -> ApiResult<(StatusCode, Json<ProfileAssignment>)> {
    let scope = parse_scope(&req.scope)?;
    caller.authorize(Action::UserManage, scope.clone()).await?;
    caller.require_mfa("access.assign", &scope).await?;
    // The scope must exist (fails closed for forged ids).
    state.authz_resolve(&scope).await?;
    let (user_id, group_id) = match (req.group_id, req.user_id, req.login.as_deref()) {
        (Some(g), None, None) => {
            state.store.get_group(g).await?.ok_or_else(|| ApiError::not_found("no such group"))?;
            (None, Some(g))
        }
        (Some(_), _, _) => return Err(ApiError::bad_request("give a group_id or a user, not both")),
        (None, Some(id), _) => {
            (Some(state.store.get_user(id).await.map_err(|_| ApiError::not_found("no such user"))?.id), None)
        }
        (None, None, Some(login)) => {
            let login = login.trim().to_lowercase();
            let found = if login.contains('@') {
                state.store.find_user_by_email(&login).await?
            } else {
                state.store.find_user_by_username(&login).await?
            };
            (Some(found.ok_or_else(|| ApiError::not_found("no such user"))?.id), None)
        }
        _ => return Err(ApiError::bad_request("user_id, login or group_id required")),
    };
    // One assignment, publish or archive at a time: what is checked below
    // is what gets stored and linked.
    let _serial = state.access_lock.lock().await;
    let p = load(&state, req.profile_id).await?;
    if p.archived_at.is_some() {
        return Err(ApiError::conflict("the profile is archived"));
    }
    let actions = crate::access::current_actions(&state, &p).await?;
    let missing = crate::access::missing(&caller, &actions, &scope).await;
    if !missing.is_empty() {
        caller
            .audit_denied("access.assign", &scope, None, "the caller lacks actions of the profile at this scope")
            .await;
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "escalation",
            format!("you can only hand out what you hold here; missing: {}", names(&missing).join(", ")),
        ));
    }
    let now = state.clock.now();
    let expires_at = match req.expires_in_minutes {
        None => None,
        Some(m) if (1..=525_600).contains(&m) => Some(now.plus(std::time::Duration::from_secs(u64::from(m) * 60))),
        Some(_) => return Err(ApiError::bad_request("expires_in_minutes: 1 to 525600")),
    };
    let a = ProfileAssignment {
        id: ProfileAssignmentId::new(),
        user_id,
        group_id,
        profile_id: p.id,
        scope: scope.clone(),
        expires_at,
        created_by: caller.principal().to_string(),
        created_at: now,
    };
    crate::access::assign(&state, &a).await?;
    caller
        .audit(
            "access.assign",
            &scope,
            None,
            json!({ "assignment": a.id, "user": user_id, "group": group_id, "profile": p.id, "profile_name": p.name, "revision": p.revision, "expires_at": a.expires_at }),
        )
        .await;
    Ok((StatusCode::CREATED, Json(a)))
}

/// `DELETE /access/assignments/{id}`: never needs MFA.
pub async fn unassign(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ProfileAssignmentId>,
) -> ApiResult<StatusCode> {
    let a = state
        .store
        .list_assignments()
        .await?
        .into_iter()
        .find(|a| a.id == id)
        .ok_or_else(|| ApiError::not_found("assignment not found"))?;
    caller.authorize(Action::UserManage, a.scope.clone()).await?;
    let _serial = state.access_lock.lock().await;
    crate::access::unassign(&state, id).await?;
    caller
        .audit(
            "access.unassign",
            &a.scope,
            None,
            json!({ "assignment": id, "user": a.user_id, "group": a.group_id, "profile": a.profile_id }),
        )
        .await;
    Ok(StatusCode::NO_CONTENT)
}

// ----------------------------------------------------------------- groups

async fn load_group(state: &AppState, id: GroupId) -> ApiResult<Group> {
    state.store.get_group(id).await?.ok_or_else(|| ApiError::not_found("group not found"))
}

fn group_view(g: Group, memberships: &[(GroupId, UserId)]) -> GroupView {
    let members = memberships.iter().filter(|(x, _)| *x == g.id).map(|(_, u)| *u).collect();
    GroupView { group: g, members }
}

/// `GET /access/groups`: every group with its members.
pub async fn list_groups(State(state): State<AppState>, caller: Caller, Ws(ws): Ws) -> ApiResult<Json<Vec<GroupView>>> {
    caller.authorize(Action::View, ws_ref(ws)).await?;
    let memberships = state.store.list_memberships().await?;
    Ok(Json(state.store.list_groups().await?.into_iter().map(|g| group_view(g, &memberships)).collect()))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NewGroup {
    pub name: String,
    #[serde(default)]
    pub description: String,
}

/// `POST /access/groups` (`access.manage`). A new group has no members and
/// grants nothing.
pub async fn create_group(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Json(req): Json<NewGroup>,
) -> ApiResult<(StatusCode, Json<GroupView>)> {
    caller.authorize(Action::AccessManage, ws_ref(ws)).await?;
    let g = Group {
        id: GroupId::new(),
        name: check_name(&req.name)?,
        description: check_description(&req.description)?,
        created_by: caller.principal().to_string(),
        created_at: state.clock.now(),
    };
    match state.store.create_group(&g).await {
        Err(DomainError::Conflict(_)) => return Err(ApiError::conflict(format!("a group named `{}` exists", g.name))),
        r => r?,
    }
    caller.audit("access.group.create", &group_ref(g.id), None, json!({ "name": g.name })).await;
    Ok((StatusCode::CREATED, Json(GroupView { group: g, members: vec![] })))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GroupPatch {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

/// `PATCH /access/groups/{id}`: name and description only.
pub async fn update_group(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Path(id): Path<GroupId>,
    Json(req): Json<GroupPatch>,
) -> ApiResult<Json<GroupView>> {
    caller.authorize(Action::AccessManage, ws_ref(ws)).await?;
    let mut g = load_group(&state, id).await?;
    if let Some(n) = req.name.as_deref() {
        g.name = check_name(n)?;
    }
    if let Some(d) = req.description.as_deref() {
        g.description = check_description(d)?;
    }
    match state.store.update_group(&g).await {
        Err(DomainError::Conflict(_)) => return Err(ApiError::conflict(format!("a group named `{}` exists", g.name))),
        r => r?,
    }
    caller.audit("access.group.update", &group_ref(id), None, json!({ "name": g.name })).await;
    let memberships = state.store.list_memberships().await?;
    Ok(Json(group_view(g, &memberships)))
}

/// `DELETE /access/groups/{id}`: the group, its memberships and its
/// assignments; every grant through it ends at once.
pub async fn delete_group(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Path(id): Path<GroupId>,
) -> ApiResult<StatusCode> {
    caller.authorize(Action::AccessManage, ws_ref(ws)).await?;
    let g = load_group(&state, id).await?;
    let _serial = state.access_lock.lock().await;
    let n = crate::access::delete_group(&state, id).await?;
    caller
        .audit("access.group.delete", &group_ref(id), None, json!({ "name": g.name, "assignments_removed": n }))
        .await;
    Ok(StatusCode::NO_CONTENT)
}

/// `PUT /access/groups/{id}/members/{user_id}`: needs `user.manage` on the
/// workspace, an MFA-verified session, and every action the group holds at
/// every scope it is assigned (the escalation guard).
pub async fn add_member(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Path((id, user_id)): Path<(GroupId, UserId)>,
) -> ApiResult<StatusCode> {
    caller.authorize(Action::UserManage, ws_ref(ws)).await?;
    caller.require_mfa("access.group.member.add", &group_ref(id)).await?;
    let g = load_group(&state, id).await?;
    let user = state.store.get_user(user_id).await.map_err(|_| ApiError::not_found("no such user"))?;
    let _serial = state.access_lock.lock().await;
    let now = state.clock.now();
    for a in state.store.list_assignments().await?.into_iter().filter(|a| a.group_id == Some(id) && !a.expired(now)) {
        let Some(p) = state.store.get_profile(a.profile_id).await? else { continue };
        if p.archived_at.is_some() {
            continue;
        }
        let actions = crate::access::current_actions(&state, &p).await?;
        let missing = crate::access::missing(&caller, &actions, &a.scope).await;
        if !missing.is_empty() {
            caller
                .audit_denied(
                    "access.group.member.add",
                    &group_ref(id),
                    None,
                    "the caller lacks actions the group holds at an assigned scope",
                )
                .await;
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "escalation",
                format!("the group holds actions you don't at {}; missing: {}", a.scope, names(&missing).join(", ")),
            ));
        }
    }
    let added = crate::access::add_member(&state, id, user.id).await?;
    if added {
        caller
            .audit("access.group.member.add", &group_ref(id), None, json!({ "group": g.name, "user": user.id }))
            .await;
    }
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /access/groups/{id}/members/{user_id}`: never needs MFA; the
/// user's grants through the group end at once.
pub async fn remove_member(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Path((id, user_id)): Path<(GroupId, UserId)>,
) -> ApiResult<StatusCode> {
    caller.authorize(Action::UserManage, ws_ref(ws)).await?;
    let g = load_group(&state, id).await?;
    let _serial = state.access_lock.lock().await;
    if crate::access::remove_member(&state, id, user_id).await? {
        caller
            .audit("access.group.member.remove", &group_ref(id), None, json!({ "group": g.name, "user": user_id }))
            .await;
    }
    Ok(StatusCode::NO_CONTENT)
}

// -------------------------------------------------------- effective access

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct EffectiveQuery {
    #[schemars(with = "String")]
    pub user_id: UserId,
    /// `<kind>:<id>` from the assignable scopes.
    pub scope: String,
}

#[derive(Debug, serde::Serialize)]
pub struct EffectiveAction {
    pub action: Action,
    pub allowed: bool,
}

#[derive(Debug, serde::Serialize)]
pub struct EffectiveSource {
    pub assignment_id: ProfileAssignmentId,
    pub profile_id: PermissionProfileId,
    pub profile_name: String,
    pub scope: EntityRef,
    /// `direct` or `group`.
    pub via: &'static str,
    pub group_id: Option<GroupId>,
    pub group_name: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct Effective {
    pub user_id: UserId,
    pub scope: EntityRef,
    pub actions: Vec<EffectiveAction>,
    pub sources: Vec<EffectiveSource>,
}

/// `GET /access/effective`: which actions `user_id` holds over `scope` and
/// everything beneath it, judged by the same policies as real requests (as
/// if signed in with MFA, so step-up is not shown as a denial), and the
/// assignments that reach it. Needs
/// `user.manage` on the scope, or asking about oneself.
pub async fn effective(
    State(state): State<AppState>,
    caller: Caller,
    Query(q): Query<EffectiveQuery>,
) -> ApiResult<Json<Effective>> {
    let scope = parse_scope(&q.scope)?;
    if q.user_id != caller.session.user.id {
        caller.authorize(Action::UserManage, scope.clone()).await?;
    }
    let resolved = state.authz_resolve(&scope).await?;
    let requests: Vec<AccessRequest> = Action::ALL
        .iter()
        .map(|a| AccessRequest::new(Principal::User(q.user_id), *a, scope.clone(), AuthzContext { mfa: true }))
        .collect();
    let allowed = state.authz.holds_over(&requests).await;
    let actions = Action::ALL.iter().zip(allowed).map(|(a, ok)| EffectiveAction { action: *a, allowed: ok }).collect();

    // Assignments whose scope is the target or one of its ancestors.
    let groups = state.policy.groups_of(q.user_id);
    let group_names: std::collections::HashMap<GroupId, String> =
        state.store.list_groups().await?.into_iter().map(|g| (g.id, g.name)).collect();
    let now = state.clock.now();
    let mut sources = Vec::new();
    for a in state.store.list_assignments().await? {
        if a.expired(now) || !resolved.is_within(&a.scope) {
            continue;
        }
        let via = match (a.user_id, a.group_id) {
            (Some(u), _) if u == q.user_id => "direct",
            (_, Some(g)) if groups.contains(&g) => "group",
            _ => continue,
        };
        let Some(p) = state.store.get_profile(a.profile_id).await? else { continue };
        if p.archived_at.is_some() {
            continue;
        }
        sources.push(EffectiveSource {
            assignment_id: a.id,
            profile_id: p.id,
            profile_name: p.name,
            scope: a.scope,
            via,
            group_id: a.group_id,
            group_name: a.group_id.and_then(|g| group_names.get(&g).cloned()),
        });
    }
    Ok(Json(Effective { user_id: q.user_id, scope, actions, sources }))
}
