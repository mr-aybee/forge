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
use forge_core::{Action, ClusterId, ServerId};

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::clusters::cluster_ref;
use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::jobs::swarm::{InitSettings, KIND, MemberAction, Payload};
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
    let missing = crate::container_runtime::nodes_without_engine(&state, &principal, &[req.server_id]).await?;
    let mut installs = Vec::new();
    if !missing.is_empty() {
        let install = super::packages::InstallRequest {
            package: forge_catalog::apps::DOCKER_PACKAGE.to_owned(),
            version: crate::container_runtime::package_version()?.to_owned(),
            params: BTreeMap::new(),
            server_ids: missing,
        };
        installs = super::packages::start_install(&state, &caller, install).await?.into_iter().map(|s| s.job).collect();
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
    let job = submit(&state, &caller, id, &payload).await?;
    caller
        .audit(
            AUDIT_JOIN,
            &cluster_ref(id),
            None,
            json!({ "server_id": req.server_id, "role": req.role.as_str(), "job_id": job.id }),
        )
        .await;
    Ok((StatusCode::ACCEPTED, Json(JoinStarted { job, installs })))
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
    let m = state.swarm.check_leave(id, server).await?;
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
    let node = state.swarm.update_node(id, server, &caller.principal().to_string(), change).await?;
    caller.audit(AUDIT_NODE_UPDATE, &cluster_ref(id), None, details).await;
    Ok(Json(node))
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
    state.swarm.rotate_tokens(id, &caller.principal().to_string(), req.worker, req.manager).await?;
    caller
        .audit(AUDIT_TOKENS_ROTATE, &cluster_ref(id), None, json!({ "worker": req.worker, "manager": req.manager }))
        .await;
    Ok(StatusCode::NO_CONTENT)
}
