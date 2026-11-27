//! Services: what runs (service, job or container), its environment and
//! the nodes it runs on.
//!
//! * `GET/POST /api/v1/services`, `GET/PUT/DELETE /api/v1/services/{id}`
//! * `PUT /api/v1/services/{id}/instances` — placements `[{server_id, role}]`
//! * `POST /api/v1/services/{id}/deploy` — deploy to every placement
//! * `POST /api/v1/services/{id}/instances/{server_id}/actions` — `{action, tail?}`
//!
//! `deploy` on the environment is required to change or run a service;
//! placing it on a node also needs `server.manage` there. Secret variables
//! are sealed at rest and never returned.

use std::collections::HashSet;
use std::time::Duration;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use forge_core::model::{Job, JobStatus, Service, ServiceInstance};
use forge_core::ports::{
    InstallationRepository, JobRepository, ServerRepository, ServiceRepository, TenancyRepository,
};
use forge_core::{Action, EntityKind, EntityRef, EnvironmentId, ServerId, ServiceId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::jobs::app::build_spec;
use crate::state::AppState;

/// A service as an authorization resource. Its parents are its environment
/// and the workspace, so grants there cover it too (#13).
/// Audit name of a service change.
pub(super) const AUDIT_UPDATE: &str = "service.update";

pub(super) fn svc_ref(id: ServiceId) -> EntityRef {
    EntityRef::new(forge_core::EntityKind::Service, id)
}

pub(super) fn ns_ref(id: EnvironmentId) -> EntityRef {
    EntityRef::new(EntityKind::Environment, id)
}

#[derive(Debug, Serialize)]
pub struct AppView {
    #[serde(flatten)]
    pub app: Service,
    pub instances: Vec<ServiceInstance>,
    /// The caller may deploy, start, stop and restart it (`deploy` on its
    /// environment). A hint for the console; every action is still authorized.
    pub can_deploy: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListQuery {
    #[schemars(with = "Option<String>")]
    pub environment_id: Option<EnvironmentId>,
    /// Only services in this cluster's environments.
    #[schemars(with = "Option<String>")]
    pub cluster_id: Option<forge_core::ClusterId>,
}

pub async fn list(
    State(state): State<AppState>,
    caller: Caller,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Vec<AppView>>> {
    let (apps, instances) = match (q.cluster_id, q.environment_id) {
        (Some(cluster), None) => state.store.list_cluster_services(cluster).await?,
        (_, ns) => (state.store.list_services(ns).await?, state.store.list_all_instances(ns).await?),
    };

    // Authorize once per environment, not once per service.
    let mut environments: Vec<EnvironmentId> = apps.iter().map(|a| a.environment_id).collect();
    environments.sort_unstable();
    environments.dedup();
    let refs: Vec<EntityRef> = environments.iter().map(|n| ns_ref(*n)).collect();
    let views = caller.can_many(Action::View, refs.clone()).await;
    let deploys = caller.can_many(Action::Deploy, refs).await;
    let allowed: std::collections::HashSet<EnvironmentId> =
        environments.iter().zip(&views).filter(|(_, ok)| **ok).map(|(n, _)| *n).collect();
    let deployable: std::collections::HashSet<EnvironmentId> =
        environments.iter().zip(&deploys).filter(|(_, ok)| **ok).map(|(n, _)| *n).collect();

    // A grant on a single service (#13): services in environments the caller
    // cannot see are checked one by one, in one batch.
    let single: Vec<ServiceId> = apps.iter().filter(|a| !allowed.contains(&a.environment_id)).map(|a| a.id).collect();
    let single_refs: Vec<EntityRef> = single.iter().map(|id| svc_ref(*id)).collect();
    let single_views = caller.can_many(Action::View, single_refs.clone()).await;
    let single_deploys = caller.can_many(Action::Deploy, single_refs).await;
    let app_allowed: std::collections::HashSet<ServiceId> =
        single.iter().zip(&single_views).filter(|(_, ok)| **ok).map(|(a, _)| *a).collect();
    let app_deployable: std::collections::HashSet<ServiceId> =
        single.iter().zip(&single_deploys).filter(|(_, ok)| **ok).map(|(a, _)| *a).collect();

    // Placements in one query (or one per app for a single environment view).
    let mut by_app: std::collections::HashMap<ServiceId, Vec<forge_core::model::ServiceInstance>> =
        std::collections::HashMap::new();

    for i in instances {
        by_app.entry(i.service_id).or_default().push(i);
    }

    Ok(Json(
        apps.into_iter()
            .filter(|a| allowed.contains(&a.environment_id) || app_allowed.contains(&a.id))
            .map(|app| {
                let instances = by_app.remove(&app.id).unwrap_or_default();
                let can_deploy = deployable.contains(&app.environment_id) || app_deployable.contains(&app.id);
                AppView { app, instances, can_deploy }
            })
            .collect(),
    ))
}

async fn load(state: &AppState, id: ServiceId) -> ApiResult<Service> {
    state.store.get_service(id).await?.ok_or_else(|| ApiError::not_found("service not found"))
}

/// Runtime of services registered by a compose stack (issue #7).
pub use forge_catalog::stacks::STACK_RUNTIME;

/// Like [`load`], for changes: services that belong to a compose stack are
/// managed through the stack, never directly.
pub(super) async fn load_own(state: &AppState, id: ServiceId) -> ApiResult<Service> {
    let app = load(state, id).await?;
    if app.runtime == STACK_RUNTIME {
        let stack = app.spec.get("stack").and_then(Value::as_str).unwrap_or_default();
        return Err(ApiError::conflict(format!(
            "`{}` belongs to the compose stack `{stack}`; change it from the stack",
            app.name
        )));
    }

    Ok(app)
}

pub async fn get(State(state): State<AppState>, caller: Caller, Path(id): Path<ServiceId>) -> ApiResult<Json<AppView>> {
    let app = load(&state, id).await?;
    caller.authorize(Action::View, svc_ref(app.id)).await?;
    let instances = state.store.list_instances(id).await?;
    let can_deploy = caller.can(Action::Deploy, svc_ref(app.id)).await;

    Ok(Json(AppView { app, instances, can_deploy }))
}

/// A service's editable fields (see [`crate::service_spec::AppInput`]).
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(transparent)]
pub struct AppInput(pub crate::service_spec::AppInput);

/// The 409 message when a service changed after the editor loaded it.
const CHANGED_SINCE_OPENED: &str = "this service changed since you opened it; reload to see the latest version";

/// Applies `input` to `app` (new or existing), sealing secrets, and validates.
fn apply(state: &AppState, app: &mut Service, ns_slug: &str, input: crate::service_spec::AppInput) -> ApiResult<()> {
    crate::service_spec::apply_input(&state.envelope, state.clock.now(), app, ns_slug, input).map_err(|e| match e {
        forge_core::DomainError::Invalid(m) => ApiError::bad_request(m),
        other => other.into(),
    })
}

pub async fn create(
    State(state): State<AppState>,
    caller: Caller,
    Json(AppInput(input)): Json<AppInput>,
) -> ApiResult<(StatusCode, Json<Service>)> {
    let ns_id = input.environment_id.ok_or_else(|| ApiError::bad_request("environment_id is required"))?;
    caller.authorize(Action::Deploy, ns_ref(ns_id)).await?;
    let ns = state.store.get_environment(ns_id).await.map_err(|_| ApiError::not_found("environment not found"))?;
    let name = input.name.clone().unwrap_or_default().trim().to_lowercase();
    if !forge_catalog::apps::valid_slug(&name, 40) {
        return Err(ApiError::bad_request("name: lowercase letters, digits and dashes (max 40)"));
    }

    if state.store.list_services(Some(ns_id)).await?.iter().any(|a| a.name == name) {
        return Err(ApiError::conflict(format!("`{name}` already exists in this environment")));
    }

    let now = state.clock.now();
    let mut app = Service {
        id: ServiceId::new(),
        workspace_id: ns.workspace_id,
        environment_id: ns_id,
        name,
        runtime: String::new(),
        spec: Value::Null,
        env: vec![],
        created_by: caller.principal(),
        created_at: now,
        updated_at: now,
        version: 0,
        source: String::new(),
    };
    apply(&state, &mut app, &crate::names::environment_slug(&ns.name), input)?;
    state.swarm.check_runtime(ns.id, &app.runtime).await?;

    if state.swarm.swarm_of_environment(app.environment_id).await?.is_some() {
        // The whole Swarm definition is checked now, not first at deploy.
        crate::jobs::swarm_service::binds_of(&app).map_err(ApiError::bad_request)?;
    }

    state.store.put_service(&app).await?;
    caller
        .audit(
            "service.create",
            &ns_ref(ns_id),
            Some(app.workspace_id),
            json!({ "service": app.name, "runtime": app.runtime }),
        )
        .await;

    Ok((StatusCode::CREATED, Json(app)))
}

pub async fn update(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServiceId>,
    Json(AppInput(input)): Json<AppInput>,
) -> ApiResult<Json<Service>> {
    let mut app = load_own(&state, id).await?;
    caller.authorize(Action::Deploy, ns_ref(app.environment_id)).await?;
    let ns_slug = app.spec.get("environment").and_then(Value::as_str).unwrap_or_default().to_owned();

    // The editor sends the version it loaded; a mismatch means someone
    // saved in between, and their change must not be lost silently.
    let expected = input.version;

    if expected.is_some_and(|v| v != app.version) {
        return Err(ApiError::conflict(CHANGED_SINCE_OPENED));
    }

    apply(&state, &mut app, &ns_slug, input)?;
    state.swarm.check_runtime(app.environment_id, &app.runtime).await?;

    if state.swarm.swarm_of_environment(app.environment_id).await?.is_some() {
        // The whole Swarm definition is checked now, not first at deploy.
        crate::jobs::swarm_service::binds_of(&app).map_err(ApiError::bad_request)?;
    }

    match expected {
        Some(v) => {
            if !state.store.update_service_if(&app, v).await? {
                return Err(ApiError::conflict(CHANGED_SINCE_OPENED));
            }

            app.version = v + 1;
        }
        None => {
            state.store.put_service(&app).await?;
            app.version += 1;
        }
    }

    let keys: Vec<&str> = app.env.iter().map(|e| e.key.as_str()).collect();
    caller
        .audit(
            AUDIT_UPDATE,
            &ns_ref(app.environment_id),
            Some(app.workspace_id),
            json!({ "service": app.name, "env_keys": keys }),
        )
        .await;

    Ok(Json(app))
}

pub async fn delete(State(state): State<AppState>, caller: Caller, Path(id): Path<ServiceId>) -> ApiResult<StatusCode> {
    let app = load_own(&state, id).await?;
    caller.authorize(Action::Deploy, ns_ref(app.environment_id)).await?;

    // A Swarm service must be removed from the swarm first.
    if let Some(cluster) = state.swarm.swarm_of_environment(app.environment_id).await? {
        for slot in ["", "a", "b"] {
            let op = forge_catalog::swarm::SwarmOp::ServiceStatus { service_id: app.id.to_string(), slot: slot.into() };
            let status: forge_catalog::swarm::SwarmServiceStatus =
                serde_json::from_value(state.swarm.run(cluster, &caller.principal().to_string(), &op).await?)
                    .map_err(forge_core::DomainError::internal)?;

            if status.exists {
                return Err(ApiError::conflict("remove the service from the swarm first"));
            }
        }
    }

    let instances = state.store.list_instances(id).await?;
    if instances.iter().any(|i| i.status != "pending") {
        return Err(ApiError::conflict("remove the service from its nodes first"));
    }

    // Prepared placements may hold an automatic file root (F-05).
    let actor = caller.principal().to_string();

    for i in &instances {
        crate::app_roots::remove(&state.store, state.audit.as_ref(), &actor, &app, i.server_id).await?;
    }

    state.store.delete_service(id).await?;
    state.health_secrets.forget(id);
    caller
        .audit("service.delete", &ns_ref(app.environment_id), Some(app.workspace_id), json!({ "service": app.name }))
        .await;

    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct Placement {
    #[schemars(with = "String")]
    pub server_id: ServerId,
    #[serde(default = "active")]
    pub role: String,
}
fn active() -> String {
    "active".into()
}

pub async fn place(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServiceId>,
    Json(placements): Json<Vec<Placement>>,
) -> ApiResult<Json<Vec<ServiceInstance>>> {
    let app = load_own(&state, id).await?;
    caller.authorize(Action::Deploy, ns_ref(app.environment_id)).await?;
    let existing = state.store.list_instances(id).await?;
    let wanted: HashSet<ServerId> = placements.iter().map(|p| p.server_id).collect();
    let actor = caller.principal().to_string();

    // A drained node takes no new placements (D-177); checked before
    // anything changes.
    for p in placements.iter().filter(|p| !existing.iter().any(|i| i.server_id == p.server_id)) {
        let server = state.store.get_server(p.server_id).await.map_err(|_| ApiError::not_found("server not found"))?;

        if server.drained {
            return Err(ApiError::conflict("this node is drained: undrain it before placing services on it"));
        }
    }

    for i in &existing {
        if !wanted.contains(&i.server_id) {
            if i.status == "pending" {
                // A prepared (never deployed) placement: its automatic root goes.
                crate::app_roots::remove(&state.store, state.audit.as_ref(), &actor, &app, i.server_id).await?;
                state.store.delete_instance(id, i.server_id).await?;
            } else {
                return Err(ApiError::conflict("use Remove to take a deployed instance off its node"));
            }
        }
    }

    let mut new_nodes = Vec::new();

    for p in placements {
        if !matches!(p.role.as_str(), "active" | "standby") {
            return Err(ApiError::bad_request("role must be active or standby"));
        }

        caller.authorize(Action::ServerManage, EntityRef::new(EntityKind::Server, p.server_id)).await?;
        let server = state.store.get_server(p.server_id).await.map_err(|_| ApiError::not_found("server not found"))?;
        if !existing.iter().any(|i| i.server_id == p.server_id) && crate::app_roots::supported(&server) {
            new_nodes.push(p.server_id);
        }

        let mut inst = existing.iter().find(|i| i.server_id == p.server_id).cloned().unwrap_or(ServiceInstance {
            service_id: id,
            server_id: p.server_id,
            role: p.role.clone(),
            status: "pending".into(),
            state: None,
            last_job_id: None,
            last_error: None,
            updated_at: state.clock.now(),
            desired: forge_core::model::InstanceDesired::Running,
            deployed_release: None,
            exit_code: None,
        });
        inst.role = p.role;
        state.store.put_instance(&inst).await?;
    }

    // F-03: a native app's account, folder and file root are provisioned as
    // soon as it is placed, so code can be uploaded before the first deploy.
    let mut prepare_jobs = Vec::new();

    if crate::app_roots::spec_for(&app).is_some() {
        for server in new_nodes {
            prepare_jobs.push(submit(&state, &caller, Submit::plain(&app, server, "prepare")).await?.id);
        }
    }

    caller
        .audit(
            "service.place",
            &ns_ref(app.environment_id),
            Some(app.workspace_id),
            json!({ "service": app.name, "prepare_jobs": prepare_jobs }),
        )
        .await;

    Ok(Json(state.store.list_instances(id).await?))
}

/// Container volumes of `app` on `server` outside its own folder and the
/// node's approved file roots. When there are any, the caller needs
/// `stack.privileged` (administrators, MFA), the same rule as compose stacks;
/// the job re-authorizes it at every step and refuses mounts added later.
async fn privileged_mounts(
    state: &AppState,
    caller: &Caller,
    app: &Service,
    server: ServerId,
) -> ApiResult<Vec<String>> {
    let spec = build_spec(app, None).map_err(ApiError::bad_request)?;
    let roots = crate::stacks::bind_roots(&state.store, server).await;
    let mounts = crate::jobs::app::privileged_mounts(&spec, &roots);
    if !mounts.is_empty() {
        caller.authorize(Action::StackPrivileged, ns_ref(app.environment_id)).await?;
    }

    Ok(mounts)
}

/// Submits one action on `app`'s instance on `server` as the caller.
async fn submit(state: &AppState, caller: &Caller, req: Submit<'_>) -> ApiResult<Job> {
    let job = crate::service_jobs::app_action_job(crate::service_jobs::AppActionRequest {
        app: req.app,
        server: req.server,
        action: req.action,
        tail: req.tail,
        cursor: req.cursor,
        privileged_mounts: req.privileged_mounts,
        principal: caller.principal(),
        context: caller.ctx(),
        idempotency_key: None,
    });

    Ok(state.jobs.submit(job).await?)
}

/// What [`submit`] runs.
struct Submit<'a> {
    app: &'a Service,
    server: ServerId,
    action: &'a str,
    tail: u32,
    cursor: &'a str,
    privileged_mounts: &'a [String],
}

impl<'a> Submit<'a> {
    /// `action` on `server` with no log tail and no privileged mounts.
    const fn plain(app: &'a Service, server: ServerId, action: &'a str) -> Self {
        Self { app, server, action, tail: 0, cursor: "", privileged_mounts: &[] }
    }
}

pub async fn deploy(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServiceId>,
) -> ApiResult<(StatusCode, Json<Vec<Job>>)> {
    let app = load_own(&state, id).await?;
    caller.authorize(Action::Deploy, svc_ref(app.id)).await?;

    // In a Swarm cluster the swarm places the tasks: one job on a manager.
    if state.swarm.swarm_of_environment(app.environment_id).await?.is_some() {
        let job = super::swarm::start_service_action(
            &state,
            &caller,
            &app,
            crate::jobs::swarm_service::ServiceAction::Deploy,
        )
        .await?;
        return Ok((StatusCode::ACCEPTED, Json(vec![job])));
    }

    let instances = state.store.list_instances(id).await?;
    if instances.is_empty() {
        return Err(ApiError::conflict("choose the nodes this service runs on first"));
    }

    // Mounts outside the app folder and approved roots: checked for every node
    // before anything starts.
    let mut mounts = Vec::new();

    for i in &instances {
        mounts.push(privileged_mounts(&state, &caller, &app, i.server_id).await?);
    }

    let nodes: Vec<ServerId> = instances.iter().map(|i| i.server_id).collect();
    let installs = install_runtimes(&state, &caller, &app, &nodes).await?;
    let runtime_installs = installs.len();
    let mut jobs = installs;

    for (i, m) in instances.iter().zip(&mounts) {
        jobs.push(
            submit(&state, &caller, Submit { privileged_mounts: m, ..Submit::plain(&app, i.server_id, "deploy") })
                .await?,
        );
    }

    let privileged: Vec<&String> =
        mounts.iter().flatten().collect::<std::collections::BTreeSet<_>>().into_iter().collect();
    caller
        .audit(
            "service.deploy",
            &ns_ref(app.environment_id),
            Some(app.workspace_id),
            json!({
                "service": app.name, "nodes": instances.len(), "privileged_mounts": privileged,
                "runtime_installs": runtime_installs,
            }),
        )
        .await;

    Ok((StatusCode::ACCEPTED, Json(jobs)))
}

/// Starts installs of the runtimes `app` needs on `nodes` where they are
/// missing (`package.manage`, MFA-guarded): Docker Engine for a container,
/// the App Store runtimes it depends on. A deploy waits for them.
pub(super) async fn install_runtimes(
    state: &AppState,
    caller: &Caller,
    app: &Service,
    nodes: &[ServerId],
) -> ApiResult<Vec<Job>> {
    let spec = build_spec(app, None).map_err(ApiError::bad_request)?;
    let mut installs = Vec::new();

    // A container needs Docker Engine: installed first on the nodes that
    // say they have none (see `container_runtime`).
    if spec.runtime == forge_catalog::Runtime::Container {
        let principal = caller.principal().to_string();
        let missing = crate::container_runtime::nodes_without_engine(state, &principal, nodes).await?;
        if !missing.is_empty() {
            let req = super::packages::InstallRequest {
                package: forge_catalog::apps::DOCKER_PACKAGE.to_owned(),
                version: crate::container_runtime::package_version()?.to_owned(),
                params: std::collections::BTreeMap::new(),
                server_ids: missing,
            };

            for s in super::packages::start_install(state, caller, req).await? {
                installs.push(s.job);
            }
        }
    }

    for d in &spec.dependencies {
        let mut missing = Vec::new();

        for server in nodes {
            let have = state.store.list_installations(Some(*server)).await?;
            let ok = have.iter().any(|x| {
                x.package == d.package
                    && x.version == d.version
                    && (x.status == "installed" || x.status.ends_with("ing"))
            });

            if !ok {
                missing.push(*server);
            }
        }

        if !missing.is_empty() {
            let req = super::packages::InstallRequest {
                package: d.package.clone(),
                version: d.version.clone(),
                params: d.params.clone(),
                server_ids: missing,
            };

            for s in super::packages::start_install(state, caller, req).await? {
                installs.push(s.job);
            }
        }
    }

    Ok(installs)
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ActionInput {
    pub action: String,
    #[serde(default)]
    pub tail: u32,
    /// `follow`: the cursor returned by the previous segment.
    #[serde(default)]
    pub cursor: String,
}

pub async fn action(
    State(state): State<AppState>,
    caller: Caller,
    Path((id, server)): Path<(ServiceId, ServerId)>,
    Json(req): Json<ActionInput>,
) -> ApiResult<Json<Value>> {
    if !forge_catalog::apps::APP_ACTIONS.contains(&req.action.as_str()) {
        return Err(ApiError::bad_request("unknown action"));
    }

    let app = load_own(&state, id).await?;
    let needed =
        if matches!(req.action.as_str(), "status" | "logs" | "follow") { Action::View } else { Action::Deploy };

    if req.cursor.len() > 512 || req.cursor.contains(['\n', '\0']) {
        return Err(ApiError::bad_request("invalid cursor"));
    }

    caller.authorize(needed, svc_ref(app.id)).await?;

    if !state.store.list_instances(id).await?.iter().any(|i| i.server_id == server) {
        return Err(ApiError::not_found("the service is not placed on this node"));
    }

    let mounts =
        if req.action == "deploy" { privileged_mounts(&state, &caller, &app, server).await? } else { Vec::new() };
    let job = submit(
        &state,
        &caller,
        Submit {
            app: &app,
            server,
            action: &req.action,
            tail: req.tail.clamp(0, 5000),
            cursor: &req.cursor,
            privileged_mounts: &mounts,
        },
    )
    .await?;

    if needed == Action::Deploy {
        caller
            .audit(
                &format!("service.{}", req.action),
                &ns_ref(app.environment_id),
                Some(app.workspace_id),
                json!({ "service": app.name, "server_id": server, "job_id": job.id, "privileged_mounts": mounts }),
            )
            .await;
    }

    // Quick actions answer inline; deploys and job runs are followed via Jobs.
    // `follow` segments stream through `GET /jobs/{id}/live`.
    let wait =
        if matches!(req.action.as_str(), "deploy" | "follow") { Duration::ZERO } else { Duration::from_secs(25) };
    let deadline = tokio::time::Instant::now() + wait;

    loop {
        let j = state.store.get_job(job.id).await?;
        match j.status {
            JobStatus::Succeeded => {
                return Ok(Json(json!({ "job_id": j.id, "status": "succeeded", "result": j.state.get("output") })));
            }
            JobStatus::Failed | JobStatus::Cancelled => {
                return Ok(Json(json!({ "job_id": j.id, "status": "failed", "error": j.last_error })));
            }
            _ if tokio::time::Instant::now() >= deadline => {
                return Ok(Json(json!({ "job_id": j.id, "status": "running" })));
            }
            _ => tokio::time::sleep(Duration::from_millis(200)).await,
        }
    }
}
