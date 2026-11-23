//! Docker Swarm in Swarm clusters (D-154): the swarm, its members, and node
//! changes. Joins and leaves are `swarm.member` jobs; node changes and token
//! rotation run through a manager at once. Logic lives in
//! [`crate::swarm::SwarmService`]; these handlers parse, authorize, call,
//! audit and respond.

use std::collections::BTreeMap;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use forge_catalog::swarm::{Availability, SwarmNode};
use forge_core::model::{Job, NewJob};
use forge_core::swarm::MemberRole;
use forge_core::{Action, ClusterId, ServerId, ServiceId};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::clusters::cluster_ref;
use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::jobs::swarm::{InitSettings, KIND, MemberAction, Payload};
use crate::jobs::swarm_service::ServiceAction;
use crate::state::AppState;
use crate::swarm::{NodeChange, SwarmView};

pub const AUDIT_JOIN: &str = "swarm.member.join";
pub const AUDIT_LEAVE: &str = "swarm.member.leave";
pub const AUDIT_NODE_UPDATE: &str = "swarm.node.update";
pub const AUDIT_TOKENS_ROTATE: &str = "swarm.tokens.rotate";

pub async fn get(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ClusterId>,
) -> ApiResult<Json<SwarmView>> {
    caller.authorize(Action::View, cluster_ref(id)).await?;
    Ok(Json(state.swarm.view(id, &caller.principal().to_string()).await?))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct JoinInput {
    #[schemars(with = "String")]
    pub server_id: ServerId,
    /// `manager` or `worker`; the first node is always a manager.
    #[schemars(with = "String")]
    pub role: MemberRole,
    /// IP address or interface other nodes reach this one on.
    #[serde(default)]
    pub advertise_addr: Option<String>,
    /// Overlay address pools (CIDR), first node only.
    #[serde(default)]
    pub default_addr_pool: Vec<String>,
    /// Lock manager keys at rest, first node only.
    #[serde(default)]
    pub autolock: bool,
}

#[derive(Debug, Serialize)]
pub struct JoinStarted {
    pub job: Job,
    /// Docker Engine installs started first on a node that had none.
    pub installs: Vec<Job>,
}

async fn submit(state: &AppState, caller: &Caller, cluster: ClusterId, p: &Payload) -> ApiResult<Job> {
    let ws = state.swarm.swarm_cluster(cluster).await?.workspace_id;
    Ok(state
        .jobs
        .submit(NewJob {
            kind: KIND.into(),
            workspace_id: Some(ws),
            resource: cluster_ref(cluster),
            principal: caller.principal(),
            context: caller.ctx(),
            idempotency_key: None,
            payload: serde_json::to_value(p).map_err(forge_core::DomainError::internal)?,
            max_attempts: 20,
        })
        .await?)
}

pub async fn join(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ClusterId>,
    Json(req): Json<JoinInput>,
) -> ApiResult<(StatusCode, Json<JoinStarted>)> {
    Ok((StatusCode::ACCEPTED, Json(start_join(&state, &caller, id, req).await?)))
}

/// Checks, authorizes and starts a join (also used when a node is assigned
/// to a Swarm cluster with a swarm role).
pub(super) async fn start_join(
    state: &AppState,
    caller: &Caller,
    id: ClusterId,
    req: JoinInput,
) -> ApiResult<JoinStarted> {
    caller.authorize(Action::SwarmManage, cluster_ref(id)).await?;
    // Validated like the node will: address and pools.
    forge_catalog::swarm::SwarmOp::Init {
        advertise_addr: req.advertise_addr.clone(),
        port: forge_catalog::swarm::DEFAULT_PORT,
        default_addr_pool: req.default_addr_pool.clone(),
        autolock: req.autolock,
    }
    .validate()
    .map_err(ApiError::bad_request)?;
    state.swarm.begin_join(id, req.server_id, req.role, req.advertise_addr.as_deref()).await?;
    // Docker Engine first on a node that says it has none (package.manage).
    let principal = caller.principal().to_string();
    let missing = crate::container_runtime::nodes_without_engine(state, &principal, &[req.server_id]).await?;
    let mut installs = Vec::new();
    if !missing.is_empty() {
        let install = super::packages::InstallRequest {
            package: forge_catalog::apps::DOCKER_PACKAGE.to_owned(),
            version: crate::container_runtime::package_version()?.to_owned(),
            params: BTreeMap::new(),
            server_ids: missing,
        };
        installs = super::packages::start_install(state, caller, install).await?.into_iter().map(|s| s.job).collect();
    }
    let payload = Payload {
        cluster_id: id,
        server_id: req.server_id,
        action: MemberAction::Join,
        role: req.role,
        advertise_addr: req.advertise_addr,
        init: InitSettings { default_addr_pool: req.default_addr_pool, autolock: req.autolock },
        force: false,
    };
    let job = submit(state, caller, id, &payload).await?;
    caller
        .audit(
            AUDIT_JOIN,
            &cluster_ref(id),
            None,
            json!({ "server_id": req.server_id, "role": req.role.as_str(), "job_id": job.id }),
        )
        .await;
    Ok(JoinStarted { job, installs })
}

#[derive(Debug, Deserialize)]
pub struct LeaveQuery {
    /// Remove a node that cannot answer any more.
    #[serde(default)]
    pub force: bool,
}

pub async fn leave(
    State(state): State<AppState>,
    caller: Caller,
    Path((id, server)): Path<(ClusterId, ServerId)>,
    Query(q): Query<LeaveQuery>,
) -> ApiResult<(StatusCode, Json<Job>)> {
    caller.authorize(Action::SwarmManage, cluster_ref(id)).await?;
    let m = state.swarm.check_leave(id, server, q.force).await?;
    let payload = Payload {
        cluster_id: id,
        server_id: server,
        action: MemberAction::Leave,
        role: m.role,
        advertise_addr: None,
        init: InitSettings::default(),
        force: q.force,
    };
    let job = submit(&state, &caller, id, &payload).await?;
    caller
        .audit(AUDIT_LEAVE, &cluster_ref(id), None, json!({ "server_id": server, "force": q.force, "job_id": job.id }))
        .await;
    Ok((StatusCode::ACCEPTED, Json(job)))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct NodeInput {
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub role: Option<MemberRole>,
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub availability: Option<Availability>,
    /// Replaces the node's labels when present.
    #[serde(default)]
    pub labels: Option<BTreeMap<String, String>>,
}

pub async fn update_node(
    State(state): State<AppState>,
    caller: Caller,
    Path((id, server)): Path<(ClusterId, ServerId)>,
    Json(req): Json<NodeInput>,
) -> ApiResult<Json<SwarmNode>> {
    caller.authorize(Action::SwarmManage, cluster_ref(id)).await?;
    let details = json!({
        "server_id": server,
        "role": req.role.map(MemberRole::as_str),
        "availability": req.availability.map(Availability::as_str),
        "labels": req.labels,
    });
    let change = NodeChange { role: req.role, availability: req.availability, labels: req.labels };
    match state.swarm.update_node(id, server, &caller.principal().to_string(), change).await {
        Ok(node) => {
            caller.audit(AUDIT_NODE_UPDATE, &cluster_ref(id), None, details).await;
            Ok(Json(node))
        }
        Err(e) => {
            caller.audit_failed(AUDIT_NODE_UPDATE, &cluster_ref(id), None, details, &e.to_string()).await;
            Err(e.into())
        }
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RotateInput {
    #[serde(default)]
    pub worker: bool,
    #[serde(default)]
    pub manager: bool,
}

pub async fn rotate_tokens(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ClusterId>,
    Json(req): Json<RotateInput>,
) -> ApiResult<StatusCode> {
    caller.authorize(Action::SwarmManage, cluster_ref(id)).await?;
    let details = json!({ "worker": req.worker, "manager": req.manager });
    if let Err(e) = state.swarm.rotate_tokens(id, &caller.principal().to_string(), req.worker, req.manager).await {
        caller.audit_failed(AUDIT_TOKENS_ROTATE, &cluster_ref(id), None, details, &e.to_string()).await;
        return Err(e.into());
    }
    caller
        .audit(AUDIT_TOKENS_ROTATE, &cluster_ref(id), None, json!({ "worker": req.worker, "manager": req.manager }))
        .await;
    Ok(StatusCode::NO_CONTENT)
}

// ------------------------------------------------------------ services

pub const AUDIT_SERVICE: &str = "swarm.service";

/// The Swarm cluster of a service, or `422` when it is not in one.
async fn service_cluster(state: &AppState, app: &forge_core::model::Service) -> ApiResult<ClusterId> {
    state.swarm.swarm_of_environment(app.environment_id).await?.ok_or_else(|| {
        ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, "invalid", "this service is not in a Docker Swarm cluster")
    })
}

#[derive(Debug, Serialize)]
pub struct ServiceSwarmView {
    pub cluster_id: ClusterId,
    /// The serving Swarm service: `forge-<id>` or `forge-<id>-<slot>`.
    pub name: String,
    pub settings: forge_catalog::swarm_service::SwarmSettings,
    pub status: Option<forge_catalog::swarm::SwarmServiceStatus>,
    pub status_error: Option<String>,
    /// High-availability readiness of the service's settings (D-156).
    pub ha: forge_catalog::swarm_service::HaReport,
}

pub async fn service_get(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServiceId>,
) -> ApiResult<Json<ServiceSwarmView>> {
    let app = super::services::load_own(&state, id).await?;
    caller.authorize(Action::View, super::services::svc_ref(id)).await?;
    let cluster = service_cluster(&state, &app).await?;
    let spec = crate::jobs::app::build_spec(&app, None).map_err(ApiError::bad_request)?;
    let slot = state.swarm.serving_slot(app.id).await?;
    let name = forge_catalog::swarm_service::service_name(&app.id.to_string(), &slot);
    let op = forge_catalog::swarm::SwarmOp::ServiceStatus { service_id: app.id.to_string(), slot };
    let (status, status_error) = match state.swarm.run(cluster, &caller.principal().to_string(), &op).await {
        Ok(v) => (Some(serde_json::from_value(v).map_err(forge_core::DomainError::internal)?), None),
        Err(e) => (None, Some(e.to_string())),
    };
    let settings = spec.swarm.clone().unwrap_or_default();
    let managers = state.swarm.view_managers(cluster).await?;
    let ha = forge_catalog::swarm_service::ha_report(&settings, spec.healthcheck.is_some(), managers);
    Ok(Json(ServiceSwarmView { cluster_id: cluster, name, settings, status, status_error, ha }))
}

/// Starts a `swarm.service` job for `app` (also used by Deploy).
pub(super) async fn start_service_action(
    state: &AppState,
    caller: &Caller,
    app: &forge_core::model::Service,
    action: ServiceAction,
) -> ApiResult<Job> {
    let cluster = service_cluster(state, app).await?;
    // Fails early (409) when the swarm has no manager yet.
    state.swarm.manager(cluster).await?;
    // Host folders outside the approved roots of every swarm node need
    // stack.privileged on the environment, as they do on Default clusters.
    let privileged_binds = if action == ServiceAction::Deploy {
        let binds = crate::jobs::swarm_service::binds_of(app).map_err(ApiError::bad_request)?;
        let needed = state.swarm.privileged_binds(cluster, &binds).await?;
        if !needed.is_empty() {
            caller.authorize(Action::StackPrivileged, super::services::ns_ref(app.environment_id)).await?;
        }
        needed
    } else {
        Vec::new()
    };
    let slot = state.swarm.serving_slot(app.id).await?;
    if matches!(action, ServiceAction::Deploy | ServiceAction::Rollback) && !slot.is_empty() {
        return Err(ApiError::conflict(
            "this service runs in slots (Blue-Green or Canary): deploy or roll back a release from Deployments",
        ));
    }
    let resource = if privileged_binds.is_empty() {
        super::services::svc_ref(app.id)
    } else {
        super::services::ns_ref(app.environment_id)
    };
    let payload =
        crate::jobs::swarm_service::Payload { service_id: app.id, cluster_id: cluster, action, privileged_binds, slot };
    let job = state
        .jobs
        .submit(NewJob {
            kind: crate::jobs::swarm_service::KIND.into(),
            workspace_id: Some(app.workspace_id),
            // stack.privileged is held per environment (as for app.action).
            resource,
            principal: caller.principal(),
            context: caller.ctx(),
            idempotency_key: None,
            payload: serde_json::to_value(&payload).map_err(forge_core::DomainError::internal)?,
            max_attempts: 6,
        })
        .await?;
    caller
        .audit(
            AUDIT_SERVICE,
            &super::services::ns_ref(app.environment_id),
            Some(app.workspace_id),
            json!({
                "service": app.name, "action": action.name(), "job_id": job.id,
                "privileged_binds": payload.privileged_binds,
            }),
        )
        .await;
    Ok(job)
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ServiceActionInput {
    /// `deploy`, `scale`, `restart`, `rollback` or `remove`.
    #[serde(flatten)]
    #[schemars(with = "Value")]
    pub action: ServiceAction,
}

pub async fn service_action(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServiceId>,
    Json(req): Json<ServiceActionInput>,
) -> ApiResult<(StatusCode, Json<Job>)> {
    let app = super::services::load_own(&state, id).await?;
    caller.authorize(Action::Deploy, super::services::svc_ref(id)).await?;
    if let ServiceAction::Scale { replicas } = req.action
        && replicas > 1000
    {
        return Err(ApiError::bad_request("at most 1000 replicas"));
    }
    let job = start_service_action(&state, &caller, &app, req.action).await?;
    // Recorded once the job is accepted, so a refused scale (no manager, a
    // slotted service) leaves the definition alone. If this write fails the
    // swarm still scales; the error says so and a retry is harmless.
    if let ServiceAction::Scale { replicas } = req.action {
        state.swarm.set_replicas(&app, replicas).await?;
    }
    Ok((StatusCode::ACCEPTED, Json(job)))
}

#[derive(Debug, Deserialize)]
pub struct LogsQuery {
    #[serde(default = "tail")]
    pub tail: u32,
}

const fn tail() -> u32 {
    200
}

pub async fn service_logs(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServiceId>,
    Query(q): Query<LogsQuery>,
) -> ApiResult<Json<Value>> {
    let app = super::services::load_own(&state, id).await?;
    caller.authorize(Action::View, super::services::svc_ref(id)).await?;
    let cluster = service_cluster(&state, &app).await?;
    let slot = state.swarm.serving_slot(app.id).await?;
    let op =
        forge_catalog::swarm::SwarmOp::ServiceLogs { service_id: app.id.to_string(), slot, tail: q.tail.min(10_000) };
    Ok(Json(state.swarm.run(cluster, &caller.principal().to_string(), &op).await?))
}
