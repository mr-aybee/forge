//! Workspaces, tenants, environments, users and role bindings.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use forge_core::model::{Environment, RoleBinding, Tenant, Workspace};
use forge_core::ports::{
    ClusterRepository, ProjectRepository, RoleBindingRepository, ServiceRepository, TenancyRepository, UserRepository,
};
use forge_core::stacks::StackRepository;
use forge_core::*;
use serde::Deserialize;
use serde_json::json;

use crate::auth::Caller;
use crate::auth::Ws;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

fn workspace_ref(id: WorkspaceId) -> EntityRef {
    EntityRef::new(EntityKind::Workspace, id)
}

fn valid_name(name: &str) -> ApiResult<String> {
    let n = name.trim();
    if n.is_empty() || n.len() > 100 {
        return Err(ApiError::bad_request("name must be 1-100 characters"));
    }

    Ok(n.to_owned())
}

// ------------------------------------------------------------ workspaces

/// The deployment's single workspace (D-016).
pub async fn current_workspace(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
) -> ApiResult<Json<Workspace>> {
    // `Action::View` on the workspace, or any role inside it: a person with
    // a role on one cluster or environment must be able to load the console.
    // The record holds only the workspace's id and name.
    let member = crate::console_access::is_member(&state.store, &state.store, caller.session.user.id, ws).await?;
    if !member {
        caller.authorize(Action::View, workspace_ref(ws)).await?;
    }

    let all = state.store.list_workspaces().await?;

    all.into_iter().find(|w| w.id == ws).map(Json).ok_or_else(|| ApiError::not_found("workspace"))
}

// ------------------------------------------------------------ tenants

pub async fn list_tenants(State(state): State<AppState>, caller: Caller, Ws(org): Ws) -> ApiResult<Json<Vec<Tenant>>> {
    caller.authorize(Action::View, workspace_ref(org)).await?;
    Ok(Json(state.store.list_tenants(org).await?))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CreateNamed {
    pub name: String,
}

pub async fn create_tenant(
    State(state): State<AppState>,
    caller: Caller,
    Ws(org): Ws,
    Json(req): Json<CreateNamed>,
) -> ApiResult<(StatusCode, Json<Tenant>)> {
    caller.authorize(Action::WorkspaceManage, workspace_ref(org)).await?;
    let t =
        Tenant { id: TenantId::new(), workspace_id: org, name: valid_name(&req.name)?, created_at: state.clock.now() };
    state.store.create_tenant(&t).await?;
    caller
        .audit("tenant.create", &EntityRef::new(EntityKind::Tenant, t.id), Some(org), json!({ "name": t.name }))
        .await;

    Ok((StatusCode::CREATED, Json(t)))
}

// ------------------------------------------------------------ environments

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct EnvironmentQuery {
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub cluster_id: Option<ClusterId>,
    /// Only the environments of this project (D-102).
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub project_id: Option<ProjectId>,
}

/// `GET /environments[?cluster_id=][&project_id=]`: the environments the caller may view.
pub async fn list_environments(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Query(q): Query<EnvironmentQuery>,
) -> ApiResult<Json<Vec<Environment>>> {
    let mut visible = Vec::new();

    for ns in state.store.list_environments(ws, q.cluster_id).await? {
        if q.project_id.is_some_and(|g| ns.project_id != Some(g)) {
            continue;
        }

        if caller.can(Action::View, EntityRef::new(EntityKind::Environment, ns.id)).await {
            visible.push(ns);
        }
    }

    Ok(Json(visible))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CreateEnvironment {
    pub name: String,
    #[serde(default)]
    pub protected: bool,
    /// Required: an environment always lives in a cluster (D-056).
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub cluster_id: Option<ClusterId>,
    /// Required: a new environment is an environment of a project (D-102).
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub project_id: Option<ProjectId>,
}

/// A project an environment may join: same workspace, and the caller may
/// manage it.
async fn checked_project(
    state: &AppState,
    caller: &Caller,
    ws: WorkspaceId,
    id: Option<ProjectId>,
) -> ApiResult<ProjectId> {
    let Some(id) = id else { return Err(ApiError::bad_request("project_id is required")) };
    let g = state.store.get_project(id).await.map_err(|_| ApiError::bad_request("unknown project"))?;
    if g.workspace_id != ws {
        return Err(ApiError::bad_request("project belongs to another workspace"));
    }

    caller.authorize(Action::WorkspaceManage, EntityRef::new(EntityKind::Project, id)).await?;

    Ok(id)
}

/// A cluster an environment may join: same workspace, and the caller may manage it.
async fn checked_cluster(
    state: &AppState,
    caller: &Caller,
    ws: WorkspaceId,
    id: Option<ClusterId>,
) -> ApiResult<ClusterId> {
    let Some(id) = id else { return Err(ApiError::bad_request("cluster_id is required")) };
    caller.authorize(Action::WorkspaceManage, EntityRef::new(EntityKind::Cluster, id)).await?;
    let c = state.store.get_cluster(id).await.map_err(|_| ApiError::bad_request("unknown cluster"))?;
    if c.workspace_id != ws {
        return Err(ApiError::bad_request("cluster belongs to another workspace"));
    }

    Ok(id)
}

fn name_taken(e: DomainError) -> ApiError {
    match e {
        DomainError::Conflict(_) => ApiError::conflict("this cluster already has an environment with that name"),
        e => e.into(),
    }
}

/// `POST /environments`: a new environment in a cluster.
pub async fn create_environment(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Json(req): Json<CreateEnvironment>,
) -> ApiResult<(StatusCode, Json<Environment>)> {
    let cluster_id = checked_cluster(&state, &caller, ws, req.cluster_id).await?;
    let project_id = checked_project(&state, &caller, ws, req.project_id).await?;
    let e = Environment {
        id: EnvironmentId::new(),
        workspace_id: ws,
        project_id: Some(project_id),
        cluster_id: Some(cluster_id),
        name: valid_name(&req.name)?,
        protected: req.protected,
        created_at: state.clock.now(),
    };
    state.store.create_environment(&e).await.map_err(name_taken)?;
    caller
        .audit(
            "environment.create",
            &EntityRef::new(EntityKind::Environment, e.id),
            Some(ws),
            json!({ "name": e.name, "protected": e.protected, "cluster_id": e.cluster_id, "project_id": e.project_id }),
        )
        .await;

    Ok((StatusCode::CREATED, Json(e)))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RenameEnvironment {
    pub name: String,
}

/// `PUT /environments/{id}`: rename an environment (unique in its cluster).
pub async fn rename_environment(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<EnvironmentId>,
    Json(req): Json<RenameEnvironment>,
) -> ApiResult<Json<Environment>> {
    let target = EntityRef::new(EntityKind::Environment, id);
    caller.authorize(Action::WorkspaceManage, target.clone()).await?;
    let mut e = state.store.get_environment(id).await?;
    let name = valid_name(&req.name)?;
    if name != e.name {
        state.store.rename_environment(id, &name).await.map_err(name_taken)?;
        caller.audit("environment.rename", &target, Some(e.workspace_id), json!({ "from": e.name, "to": name })).await;
        e.name = name;
    }

    Ok(Json(e))
}

/// `DELETE /environments/{id}`: remove an empty environment (409 while
/// services or stacks are still in it, or it owns an object store).
pub async fn delete_environment(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<EnvironmentId>,
) -> ApiResult<StatusCode> {
    let target = EntityRef::new(EntityKind::Environment, id);
    caller.authorize(Action::WorkspaceManage, target.clone()).await?;
    let e = state.store.get_environment(id).await?;
    let apps = state.store.list_services(Some(id)).await?.len();
    let stacks = state.store.list_stacks(Some(id)).await?.len();
    if apps + stacks > 0 {
        return Err(ApiError::conflict(format!(
            "remove this environment's {apps} service(s) and {stacks} stack(s) first"
        )));
    }

    // Refused with `409` by the store while the environment owns an object
    // store (D-107): that check is part of the delete's transaction.
    state.store.delete_environment(id).await?;
    caller
        .audit(
            "environment.delete",
            &target,
            Some(e.workspace_id),
            json!({ "name": e.name, "cluster_id": e.cluster_id, "project_id": e.project_id }),
        )
        .await;

    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SetProject {
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub project_id: Option<ProjectId>,
}

/// `PUT /environments/{id}/project`: attach an environment (for example one
/// created before projects existed) to a project.
pub async fn set_environment_project(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<EnvironmentId>,
    Json(req): Json<SetProject>,
) -> ApiResult<Json<Environment>> {
    let target = EntityRef::new(EntityKind::Environment, id);
    caller.authorize(Action::WorkspaceManage, target.clone()).await?;
    let mut e = state.store.get_environment(id).await?;
    let group = checked_project(&state, &caller, e.workspace_id, req.project_id).await?;
    state.store.set_environment_project(id, Some(group)).await?;
    caller
        .audit("environment.set_project", &target, Some(e.workspace_id), json!({ "from": e.project_id, "to": group }))
        .await;
    e.project_id = Some(group);

    Ok(Json(e))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct MoveEnvironment {
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub cluster_id: Option<ClusterId>,
}

/// `PUT /environments/{id}/cluster`: move an environment (and its services)
/// into another cluster.
pub async fn set_environment_cluster(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<EnvironmentId>,
    Json(req): Json<MoveEnvironment>,
) -> ApiResult<Json<Environment>> {
    let target = EntityRef::new(EntityKind::Environment, id);
    caller.authorize(Action::WorkspaceManage, target.clone()).await?;
    let mut e = state.store.get_environment(id).await?;
    let cluster_id = checked_cluster(&state, &caller, e.workspace_id, req.cluster_id).await?;
    state.store.set_environment_cluster(id, cluster_id).await.map_err(name_taken)?;
    caller
        .audit("environment.move", &target, Some(e.workspace_id), json!({ "from": e.cluster_id, "to": cluster_id }))
        .await;
    e.cluster_id = Some(cluster_id);

    Ok(Json(e))
}

// ------------------------------------------------------------ users

pub async fn list_users(
    State(state): State<AppState>,
    caller: Caller,
) -> ApiResult<Json<Vec<forge_core::model::User>>> {
    caller.authorize(Action::UserManage, EntityRef::platform()).await?;
    Ok(Json(state.store.list_users().await?))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CreateUser {
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub display_name: String,
    pub password: String,
}

pub async fn create_user(
    State(state): State<AppState>,
    caller: Caller,
    Json(req): Json<CreateUser>,
) -> ApiResult<(StatusCode, Json<forge_core::model::User>)> {
    caller.authorize(Action::UserManage, EntityRef::platform()).await?;
    caller.require_mfa("user.create", &EntityRef::platform()).await?;
    let user = state
        .identity
        .create_account(req.username.as_deref(), req.email.as_deref(), &req.display_name, &req.password)
        .await?;
    caller
        .audit(
            "user.create",
            &EntityRef::new(EntityKind::User, user.id),
            None,
            json!({ "username": user.username, "email": user.email }),
        )
        .await;

    Ok((StatusCode::CREATED, Json(user)))
}

// ------------------------------------------------------------ role bindings

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CreateBinding {
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub user_id: Option<UserId>,
    /// Username or email of the user (alternative to `user_id`).
    #[serde(default, alias = "login")]
    pub email: Option<String>,
    #[schemars(with = "String")]
    pub role: Role,
    /// `platform:root`, `workspace:<id>`, `tenant:<id>`, `cluster:<id>`,
    /// `project:<id>` or `environment:<id>`.
    pub scope: String,
}

const SCOPE_KINDS: [EntityKind; 6] = [
    EntityKind::Platform,
    EntityKind::Workspace,
    EntityKind::Tenant,
    EntityKind::Project,
    EntityKind::Environment,
    EntityKind::Cluster,
];

pub async fn list_bindings(State(state): State<AppState>, caller: Caller) -> ApiResult<Json<Vec<RoleBinding>>> {
    let mut visible = Vec::new();

    for b in state.store.list_bindings().await? {
        if b.user_id == caller.session.user.id || caller.can(Action::UserManage, b.scope.clone()).await {
            visible.push(b);
        }
    }

    Ok(Json(visible))
}

pub async fn create_binding(
    State(state): State<AppState>,
    caller: Caller,
    Json(req): Json<CreateBinding>,
) -> ApiResult<(StatusCode, Json<RoleBinding>)> {
    let scope: EntityRef = req.scope.parse().map_err(ApiError::bad_request)?;
    if !SCOPE_KINDS.contains(&scope.kind) {
        return Err(ApiError::bad_request(
            "scope must be platform, workspace, tenant, cluster, project or environment",
        ));
    }

    // Escalation guard: platform_admin can only be bound at platform scope
    // (an "org-scoped platform admin" would be meaningless and confusing), and
    // granting anything requires user.manage on the scope itself, which only
    // platform admins hold at platform scope.
    if req.role == Role::PlatformAdmin && scope.kind != EntityKind::Platform {
        return Err(ApiError::bad_request("platform_admin can only be bound at platform scope"));
    }

    caller.authorize(Action::UserManage, scope.clone()).await?;
    caller.require_mfa("role.grant", &scope).await?;

    // Validate the scope exists (fails closed for forged ids).
    state.authz_resolve(&scope).await?;

    let user = match (req.user_id, req.email.as_deref()) {
        (Some(id), _) => state.store.get_user(id).await?,
        (None, Some(login)) => {
            let login = login.trim();
            let found = if login.contains('@') {
                state.store.find_user_by_email(login).await?
            } else {
                state.store.find_user_by_username(login).await?
            };

            found.ok_or_else(|| ApiError::not_found("no user with that username or email"))?
        }
        _ => return Err(ApiError::bad_request("user_id, username or email required")),
    };

    let binding = RoleBinding {
        id: RoleBindingId::new(),
        user_id: user.id,
        role: req.role,
        scope: scope.clone(),
        created_at: state.clock.now(),
    };
    state.grant(&binding).await?;
    caller
        .audit(
            "role.grant",
            &scope,
            None,
            json!({ "binding_id": binding.id, "user_id": user.id, "role": binding.role }),
        )
        .await;

    Ok((StatusCode::CREATED, Json(binding)))
}

pub async fn delete_binding(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<RoleBindingId>,
) -> ApiResult<StatusCode> {
    let binding = state
        .store
        .list_bindings()
        .await?
        .into_iter()
        .find(|b| b.id == id)
        .ok_or_else(|| ApiError::not_found("binding not found"))?;
    caller.authorize(Action::UserManage, binding.scope.clone()).await?;

    if binding.role == Role::PlatformAdmin {
        let Principal::User(me) = caller.principal();
        let admins: std::collections::BTreeSet<UserId> = state
            .store
            .list_bindings()
            .await?
            .into_iter()
            .filter(|b| b.role == Role::PlatformAdmin)
            .map(|b| b.user_id)
            .collect();

        if let Some(reason) = admin_revoke_block(me, binding.user_id, &admins) {
            caller.audit_denied("role.revoke", &binding.scope, None, reason).await;
            return Err(ApiError::conflict(reason));
        }
    }

    state.revoke(id).await?;
    caller
        .audit(
            "role.revoke",
            &binding.scope,
            None,
            json!({ "binding_id": id, "user_id": binding.user_id, "role": binding.role }),
        )
        .await;

    Ok(StatusCode::NO_CONTENT)
}

/// Guards on revoking a platform-admin binding: never leave the platform
/// without an administrator, and an administrator cannot drop their own
/// admin access. A second administrator has to do it.
fn admin_revoke_block(
    caller: UserId,
    target: UserId,
    admins: &std::collections::BTreeSet<UserId>,
) -> Option<&'static str> {
    let others = admins.iter().filter(|u| **u != target).count();
    if others == 0 {
        Some("this is the only platform administrator; grant platform admin to another user first")
    } else if caller == target {
        Some("you cannot revoke your own platform admin access; another platform administrator must do it")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_admin_and_self_revoke_are_blocked() {
        let (a, b) = (UserId::new(), UserId::new());
        let one: std::collections::BTreeSet<_> = [a].into();
        let two: std::collections::BTreeSet<_> = [a, b].into();
        assert!(admin_revoke_block(b, a, &one).is_some(), "last admin");
        assert!(admin_revoke_block(a, a, &one).is_some(), "last admin, self");
        assert!(admin_revoke_block(a, a, &two).is_some(), "self with a second admin");
        assert!(admin_revoke_block(b, a, &two).is_none(), "second admin may revoke");
    }
}
