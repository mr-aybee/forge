//! App Store: catalog, multi-node installs and package lifecycle.
//!
//! * `GET  /api/v1/catalog` — packages and the roadmap.
//! * `POST /api/v1/packages/install` — `{package, version, params, server_ids}`;
//!   one `package.action` job per node.
//! * `GET  /api/v1/installations?server_id=` — what is installed where.
//! * `POST /api/v1/installations/{id}/actions` — `{action}`: start, stop,
//!   restart, status, uninstall (or install again to repair).
//!
//! `package.manage` (MFA) is checked here for clear errors and again by the
//! job engine on every step; the node helper re-validates against its own
//! compiled-in catalog.

use std::collections::BTreeMap;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use forge_core::model::{Installation, Job, NewJob};
use forge_core::ports::{InstallationRepository, ServerRepository};
use forge_core::{Action, EntityKind, EntityRef, InstallationId, ServerId};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::jobs::package::busy_status;
use crate::state::AppState;

pub(crate) fn server_ref(id: ServerId) -> EntityRef {
    EntityRef::new(EntityKind::Server, id)
}

#[derive(Debug, Serialize)]
pub struct RoadmapItem {
    pub name: &'static str,
    pub description: &'static str,
}

#[derive(Debug, Serialize)]
pub struct Catalog {
    pub packages: Vec<PackageView>,
    pub roadmap: Vec<RoadmapItem>,
}

/// A catalog package with how it integrates with Forge.
#[derive(Debug, Serialize)]
pub struct PackageView {
    #[serde(flatten)]
    pub package: &'static forge_catalog::Package,
    pub integration: Option<forge_catalog::integration::Integration>,
}

pub async fn catalog(_caller: Caller) -> Json<Catalog> {
    Json(Catalog {
        packages: forge_catalog::CATALOG
            .iter()
            .map(|p| PackageView { package: p, integration: forge_catalog::integration::integration(p.id) })
            .collect(),
        roadmap: forge_catalog::ROADMAP.iter().map(|(name, description)| RoadmapItem { name, description }).collect(),
    })
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListQuery {
    #[schemars(with = "Option<String>")]
    pub server_id: Option<ServerId>,
}

pub async fn list(
    State(state): State<AppState>,
    caller: Caller,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Vec<Installation>>> {
    let all = state.store.list_installations(q.server_id).await?;
    let allowed = caller.can_many(Action::View, all.iter().map(|i| server_ref(i.server_id)).collect()).await;

    Ok(Json(all.into_iter().zip(allowed).filter_map(|(i, ok)| ok.then_some(i)).collect()))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct InstallRequest {
    pub package: String,
    pub version: String,
    #[serde(default)]
    pub params: BTreeMap<String, String>,
    #[schemars(with = "Vec<String>")]
    pub server_ids: Vec<ServerId>,
}

#[derive(Debug, Serialize)]
pub struct Started {
    pub server_id: ServerId,
    pub installation: Installation,
    pub job: Job,
}

async fn submit(state: &AppState, caller: &Caller, i: &Installation, action: &str) -> ApiResult<Job> {
    submit_as(state, caller.principal(), caller.ctx(), i, action, None).await
}

/// Submits a `package.action` job for an installation as `principal`
/// (re-authorized by the job engine on every step). `build` pins the exact
/// OS package build for install/upgrade.
pub(crate) async fn submit_as(
    state: &AppState,
    principal: forge_core::Principal,
    context: forge_core::AuthzContext,
    i: &Installation,
    action: &str,
    build: Option<&str>,
) -> ApiResult<Job> {
    let mut params = i.params.clone();

    if let Some(b) = build {
        params.insert(forge_catalog::BUILD_PARAM.to_owned(), b.to_owned());
    }

    Ok(state
        .jobs
        .submit(NewJob {
            kind: "package.action".into(),
            workspace_id: Some(i.workspace_id),
            resource: server_ref(i.server_id),
            principal,
            context,
            idempotency_key: None,
            payload: json!({
                "server_id": i.server_id,
                "installation_id": i.id,
                "package": i.package,
                "version": i.version,
                "action": action,
                "params": params,
            }),
            max_attempts: 2,
        })
        .await?)
}

pub async fn install(
    State(state): State<AppState>,
    caller: Caller,
    Json(req): Json<InstallRequest>,
) -> ApiResult<(StatusCode, Json<Vec<Started>>)> {
    Ok((StatusCode::ACCEPTED, Json(start_install(&state, &caller, req).await?)))
}

/// Validates and starts an install on each node (`package.manage` on each,
/// checked before anything starts). Also used for service runtimes.
pub(crate) async fn start_install(state: &AppState, caller: &Caller, req: InstallRequest) -> ApiResult<Vec<Started>> {
    let v =
        forge_catalog::validate(&req.package, &req.version, "install", &req.params).map_err(ApiError::bad_request)?;

    if req.server_ids.is_empty() || req.server_ids.len() > 100 {
        return Err(ApiError::bad_request("choose between 1 and 100 nodes"));
    }

    // Check every node before starting anything, so a request either starts
    // everywhere or nowhere.
    let mut targets = Vec::new();

    for &id in &req.server_ids {
        caller.authorize(Action::PackageManage, server_ref(id)).await?;
        let server = state.store.get_server(id).await.map_err(|_| ApiError::not_found("server not found"))?;
        if !v.package.os.contains(&server.os.as_str()) {
            return Err(ApiError::bad_request(format!(
                "{} does not support {} ({})",
                v.package.name, server.os, server.hostname
            )));
        }

        let installed = state.store.list_installations(Some(id)).await?;

        for dep in v.package.requires {
            if !installed.iter().any(|i| i.package == *dep && i.status == "installed") {
                let dep_name = forge_catalog::find(dep).map_or(*dep, |p| p.name);
                return Err(ApiError::conflict(format!(
                    "{} needs {dep_name} on {} first",
                    v.package.name, server.hostname
                )));
            }
        }

        if let Some(busy) =
            installed.iter().find(|i| i.package == v.package.id && i.version == v.version && i.status.ends_with("ing"))
        {
            return Err(ApiError::conflict(format!(
                "{} is already {} on {}",
                v.package.name, busy.status, server.hostname
            )));
        }

        targets.push((server, installed));
    }

    // Only non-secret, validated parameters are stored (packages generate
    // their passwords on the node).
    let params: BTreeMap<String, String> = v
        .package
        .params
        .iter()
        .map(|p| (p.key.to_owned(), v.env[&format!("FORGE_P_{}", p.key.to_ascii_uppercase())].clone()))
        .collect();
    let now = state.clock.now();
    let mut started = Vec::new();

    for (server, installed) in targets {
        let mut inst = installed
            .into_iter()
            .find(|i| i.package == v.package.id && i.version == v.version)
            .unwrap_or_else(|| Installation {
                id: InstallationId::new(),
                workspace_id: server.workspace_id,
                server_id: server.id,
                package: v.package.id.to_owned(),
                version: v.version.to_owned(),
                params: BTreeMap::new(),
                status: "pending".into(),
                state: None,
                env: BTreeMap::new(),
                last_job_id: None,
                last_error: None,
                versions: None,
                update_policy: "notify".into(),
                created_at: now,
                updated_at: now,
            });
        inst.params.clone_from(&params);
        inst.status = busy_status("install").into();
        inst.updated_at = now;
        state.store.put_installation(&inst).await?;
        let job = submit(state, caller, &inst, "install").await?;
        caller
            .audit(
                "package.install",
                &server_ref(server.id),
                Some(server.workspace_id),
                json!({ "package": inst.package, "version": inst.version, "params": inst.params, "job_id": job.id }),
            )
            .await;
        started.push(Started { server_id: server.id, installation: inst, job });
    }

    Ok(started)
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ActionRequest {
    pub action: String,
}

pub async fn action(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<InstallationId>,
    Json(req): Json<ActionRequest>,
) -> ApiResult<(StatusCode, Json<Job>)> {
    let inst = state.store.get_installation(id).await?.ok_or_else(|| ApiError::not_found("installation not found"))?;
    forge_catalog::validate(&inst.package, &inst.version, &req.action, &inst.params).map_err(ApiError::bad_request)?;

    if req.action == "upgrade" {
        return Err(ApiError::bad_request("use POST /installations/{id}/update to update a package"));
    }

    let read_only = forge_catalog::READ_ONLY_ACTIONS.contains(&req.action.as_str());
    let needed = if read_only { Action::ServerManage } else { Action::PackageManage };
    caller.authorize(needed, server_ref(inst.server_id)).await?;

    if inst.status.ends_with("ing") && !read_only {
        return Err(ApiError::conflict(format!("{} is {}; wait for it to finish", inst.package, inst.status)));
    }

    // Show the action as in progress right away (the job confirms it). A
    // status check keeps the current status, e.g. `failed`.
    if !read_only {
        let mut busy = inst.clone();
        busy.status = busy_status(&req.action).into();
        busy.updated_at = state.clock.now();
        state.store.put_installation(&busy).await?;
    }

    let job = submit(&state, &caller, &inst, &req.action).await?;

    if !read_only {
        caller
            .audit(
                &format!("package.{}", req.action),
                &server_ref(inst.server_id),
                Some(inst.workspace_id),
                json!({ "package": inst.package, "version": inst.version, "job_id": job.id }),
            )
            .await;
    }

    Ok((StatusCode::ACCEPTED, Json(job)))
}
