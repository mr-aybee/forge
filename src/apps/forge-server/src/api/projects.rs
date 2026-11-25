//! Projects (D-102): the products whose environments are environments. The
//! console calls a project an *Service*; the API keeps `projects`
//! because `/services` already names the services.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use forge_core::model::{Environment, Project};
use forge_core::ports::{ProjectRepository, ServiceRepository, TenancyRepository};
use forge_core::*;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::auth::Caller;
use crate::auth::Ws;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

pub(crate) fn group_ref(id: ProjectId) -> EntityRef {
    EntityRef::new(EntityKind::Project, id)
}

/// Lower-case letters, digits and dashes, 1-63 characters.
fn valid_name(name: &str) -> ApiResult<String> {
    let n = name.trim();
    let ok =
        !n.is_empty() && n.len() <= 63 && n.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');

    if ok {
        Ok(n.to_owned())
    } else {
        Err(ApiError::bad_request("project name: 1-63 lower-case letters, digits and dashes"))
    }
}

fn valid_description(d: &str) -> String {
    d.trim().chars().take(500).collect()
}

fn name_taken(e: DomainError) -> ApiError {
    match e {
        DomainError::Conflict(_) => ApiError::conflict("this workspace already has a project with that name"),
        e => e.into(),
    }
}

/// One environment of a project, as listed with the group.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct EnvironmentSummary {
    #[schemars(with = "String")]
    pub id: EnvironmentId,
    pub name: String,
    #[schemars(with = "Option<String>")]
    pub cluster_id: Option<ClusterId>,
    pub protected: bool,
}

/// A project with its environments and the number of services in them.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct ProjectView {
    #[schemars(with = "String")]
    pub id: ProjectId,
    #[schemars(with = "String")]
    pub workspace_id: WorkspaceId,
    pub name: String,
    pub description: String,
    #[schemars(with = "i64")]
    pub created_at: Timestamp,
    pub environments: Vec<EnvironmentSummary>,
    /// Services (services) across the group's environments.
    pub service_count: usize,
}

fn summary(ns: &Environment) -> EnvironmentSummary {
    EnvironmentSummary { id: ns.id, name: ns.name.clone(), cluster_id: ns.cluster_id, protected: ns.protected }
}

/// The group with the environments the caller may view and its service count.
async fn view(state: &AppState, caller: &Caller, g: Project) -> ApiResult<ProjectView> {
    let mut environments = Vec::new();
    let mut service_count = 0;

    for ns in state.store.list_environments_in_group(g.workspace_id, g.id).await? {
        if !caller.can(Action::View, EntityRef::new(EntityKind::Environment, ns.id)).await {
            continue;
        }

        service_count += state.store.list_services(Some(ns.id)).await?.len();
        environments.push(summary(&ns));
    }

    Ok(ProjectView {
        id: g.id,
        workspace_id: g.workspace_id,
        name: g.name,
        description: g.description,
        created_at: g.created_at,
        environments,
        service_count,
    })
}

/// `GET /projects`: the projects the caller may view, with their environments.
pub async fn list(State(state): State<AppState>, caller: Caller, Ws(ws): Ws) -> ApiResult<Json<Vec<ProjectView>>> {
    // Filtered per group, like `GET /environments`: a developer bound to one
    // project sees that group without `view` on the workspace.
    let mut out = Vec::new();

    for g in state.store.list_projects(ws).await? {
        if caller.can(Action::View, group_ref(g.id)).await {
            out.push(view(&state, &caller, g).await?);
        }
    }

    Ok(Json(out))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CreateProject {
    /// Lower-case letters, digits and dashes, 1-63 characters; unique per workspace.
    pub name: String,
    #[serde(default)]
    pub description: String,
}

/// `POST /projects`: a new project in the workspace.
pub async fn create(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Json(req): Json<CreateProject>,
) -> ApiResult<(StatusCode, Json<ProjectView>)> {
    caller.authorize(Action::WorkspaceManage, EntityRef::new(EntityKind::Workspace, ws)).await?;
    let g = Project {
        id: ProjectId::new(),
        workspace_id: ws,
        name: valid_name(&req.name)?,
        description: valid_description(&req.description),
        created_at: state.clock.now(),
    };
    state.store.create_project(&g).await.map_err(name_taken)?;
    caller.audit("project.create", &group_ref(g.id), Some(ws), json!({ "name": g.name })).await;

    Ok((StatusCode::CREATED, Json(view(&state, &caller, g).await?)))
}

/// `GET /projects/{id}`: one project with its environments.
pub async fn get(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ProjectId>,
) -> ApiResult<Json<ProjectView>> {
    caller.authorize(Action::View, group_ref(id)).await?;
    let g = state.store.get_project(id).await?;

    Ok(Json(view(&state, &caller, g).await?))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UpdateProject {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

/// `PUT /projects/{id}`: rename or describe a project.
pub async fn update(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ProjectId>,
    Json(req): Json<UpdateProject>,
) -> ApiResult<Json<ProjectView>> {
    caller.authorize(Action::WorkspaceManage, group_ref(id)).await?;
    let mut g = state.store.get_project(id).await?;
    let from = g.name.clone();

    if let Some(n) = &req.name {
        g.name = valid_name(n)?;
    }

    if let Some(d) = &req.description {
        g.description = valid_description(d);
    }

    state.store.update_project(id, &g.name, &g.description).await.map_err(name_taken)?;
    caller.audit("project.update", &group_ref(id), Some(g.workspace_id), json!({ "from": from, "name": g.name })).await;

    Ok(Json(view(&state, &caller, g).await?))
}

/// `DELETE /projects/{id}`: remove a project that has no environments.
pub async fn delete(State(state): State<AppState>, caller: Caller, Path(id): Path<ProjectId>) -> ApiResult<StatusCode> {
    caller.authorize(Action::WorkspaceManage, group_ref(id)).await?;
    let g = state.store.get_project(id).await?;
    let envs = state.store.list_environments_in_group(g.workspace_id, id).await?;
    if !envs.is_empty() {
        let names: Vec<&str> = envs.iter().map(|n| n.name.as_str()).collect();
        return Err(ApiError::conflict(format!(
            "delete or detach this project's environments first: {}",
            names.join(", ")
        )));
    }

    state.store.delete_project(id).await?;
    caller.audit("project.delete", &group_ref(id), Some(g.workspace_id), json!({ "name": g.name })).await;

    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_lower_case_slugs() {
        assert_eq!(valid_name(" shop-2 ").ok().as_deref(), Some("shop-2"));

        for bad in ["", "Shop", "shop_2", "shop 2", "a.b", &"x".repeat(64)] {
            assert!(valid_name(bad).is_err(), "{bad:?}");
        }
    }
}
