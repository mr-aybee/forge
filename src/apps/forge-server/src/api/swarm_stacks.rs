//! Docker Swarm beyond membership (D-171): stacks, secrets and configs,
//! networks, advertise addresses and converting a cluster's mode. Logic lives
//! in [`crate::swarm_stacks::SwarmStacks`]; these handlers parse, authorize,
//! call, audit and respond.
//!
//! * `GET  /clusters/{id}/swarm/stacks` · `GET /clusters/{id}/swarm/networks`
//! * `POST /clusters/{id}/swarm/networks/shared`
//! * `GET  /clusters/{id}/swarm/objects?kind=` · `POST /clusters/{id}/swarm/objects`
//! * `POST /clusters/{id}/swarm/objects/prune` · `DELETE /clusters/{id}/swarm/objects/{kind}/{name}`
//! * `GET  /clusters/{id}/swarm/addresses?server_id=`
//! * `GET  /clusters/{id}/convert?to=` · `POST /clusters/{id}/convert`
//! * `GET  /stacks/{id}/swarm` · `POST /stacks/{id}/swarm/deploy` · `POST /stacks/{id}/swarm/remove`
//! * `POST /stacks/{id}/swarm/services/{service}` · `GET /stacks/{id}/swarm/services/{service}/logs`

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use forge_catalog::swarm::{StackServiceAction, SwarmNetwork, SwarmOp};
use forge_catalog::swarm_stack::{ObjectKind, StackStatus, SwarmObject};
use forge_core::model::{ClusterMode, Job, NewJob};
use forge_core::stacks::Stack;
use forge_core::{Action, ClusterId, ServerId, StackId, VaultItemId};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::clusters::cluster_ref;
use super::stacks::ns_ref;
use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::jobs::swarm_stack::{KIND, Payload, StackAction};
use crate::state::AppState;
use crate::swarm_stacks::{AddressChoice, ConvertCheck};

pub const AUDIT_STACK: &str = "swarm.stack";
pub const AUDIT_STACK_SERVICE: &str = "swarm.stack.service";
pub const AUDIT_OBJECT_PUT: &str = "swarm.object.put";
pub const AUDIT_OBJECT_REMOVE: &str = "swarm.object.remove";
pub const AUDIT_OBJECT_PRUNE: &str = "swarm.object.prune";
pub const AUDIT_NETWORK_SHARED: &str = "swarm.network.shared";
pub const AUDIT_CONVERT: &str = "cluster.convert";

fn principal(caller: &Caller) -> String {
    caller.principal().to_string()
}

// ------------------------------------------------------------ cluster reads

/// A stack in one of the cluster's environments.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct ClusterStack {
    #[schemars(with = "String")]
    pub id: StackId,
    pub name: String,
    #[schemars(with = "String")]
    pub environment_id: forge_core::EnvironmentId,
    pub environment: String,
    pub revision: i64,
    /// 0: not deployed to the swarm.
    pub deployed_revision: i64,
}

/// Stacks of the cluster's environments; only environments the caller can view.
pub async fn stacks(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ClusterId>,
) -> ApiResult<Json<Vec<ClusterStack>>> {
    caller.authorize(Action::View, cluster_ref(id)).await?;
    let mut out = Vec::new();

    for (e, s) in state.swarm_stacks.cluster_stacks(id).await? {
        if !caller.can(Action::View, ns_ref(e.id)).await {
            continue;
        }

        out.push(ClusterStack {
            id: s.id,
            name: s.name,
            environment_id: e.id,
            environment: e.name,
            revision: s.revision,
            deployed_revision: s.deployed_revision,
        });
    }

    Ok(Json(out))
}

pub async fn networks(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ClusterId>,
) -> ApiResult<Json<Vec<SwarmNetwork>>> {
    caller.authorize(Action::View, cluster_ref(id)).await?;
    state.swarm.swarm_cluster(id).await?;

    Ok(Json(state.swarm_stacks.networks(id, &principal(&caller)).await?))
}

pub async fn ensure_shared(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ClusterId>,
) -> ApiResult<StatusCode> {
    caller.authorize(Action::SwarmManage, cluster_ref(id)).await?;
    state.swarm.swarm_cluster(id).await?;

    if let Err(e) = state.swarm_stacks.ensure_shared(id, &principal(&caller)).await {
        caller.audit_failed(AUDIT_NETWORK_SHARED, &cluster_ref(id), None, json!({}), &e.to_string()).await;
        return Err(e.into());
    }

    caller.audit(AUDIT_NETWORK_SHARED, &cluster_ref(id), None, json!({})).await;

    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct KindQuery {
    #[schemars(with = "String")]
    pub kind: ObjectKind,
}

pub async fn objects(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ClusterId>,
    Query(q): Query<KindQuery>,
) -> ApiResult<Json<Vec<SwarmObject>>> {
    caller.authorize(Action::View, cluster_ref(id)).await?;
    state.swarm.swarm_cluster(id).await?;

    Ok(Json(state.swarm_stacks.objects(id, q.kind, &principal(&caller)).await?))
}

/// A new secret or config, or the next version of one. Give either `value`
/// or `vault_item_id` (a credential vault item: administrators, with MFA).
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObjectInput {
    #[schemars(with = "String")]
    pub kind: ObjectKind,
    pub name: String,
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub vault_item_id: Option<VaultItemId>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct ObjectPut {
    pub docker_name: String,
    pub version: u32,
}

pub async fn put_object(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ClusterId>,
    Json(req): Json<ObjectInput>,
) -> ApiResult<(StatusCode, Json<ObjectPut>)> {
    caller.authorize(Action::SwarmManage, cluster_ref(id)).await?;
    let c = state.swarm.swarm_cluster(id).await?;

    let value = match (req.value, req.vault_item_id) {
        (Some(v), None) => v.into_bytes(),
        (None, Some(item)) => {
            super::vault::authorize_item_attach(&state, &caller, c.workspace_id, item, AUDIT_OBJECT_PUT).await?;
            super::vault::open_item(&state, c.workspace_id, item).await?
        }
        _ => return Err(ApiError::bad_request("give either `value` or `vault_item_id`")),
    };

    // The value is never part of the audit record.
    let details = json!({ "kind": req.kind.as_str(), "name": req.name, "from_vault": req.vault_item_id });

    match state.swarm_stacks.put_object(id, req.kind, &req.name, &value, &principal(&caller)).await {
        Ok((docker_name, version)) => {
            caller.audit(AUDIT_OBJECT_PUT, &cluster_ref(id), Some(c.workspace_id), details).await;
            Ok((StatusCode::CREATED, Json(ObjectPut { docker_name, version })))
        }
        Err(e) => {
            caller
                .audit_failed(AUDIT_OBJECT_PUT, &cluster_ref(id), Some(c.workspace_id), details, &e.to_string())
                .await;
            Err(e.into())
        }
    }
}

pub async fn remove_object(
    State(state): State<AppState>,
    caller: Caller,
    Path((id, kind, name)): Path<(ClusterId, ObjectKind, String)>,
) -> ApiResult<StatusCode> {
    caller.authorize(Action::SwarmManage, cluster_ref(id)).await?;
    let c = state.swarm.swarm_cluster(id).await?;
    let details = json!({ "kind": kind.as_str(), "docker_name": name });

    if let Err(e) = state.swarm_stacks.remove_object(id, kind, &name, &principal(&caller)).await {
        caller.audit_failed(AUDIT_OBJECT_REMOVE, &cluster_ref(id), Some(c.workspace_id), details, &e.to_string()).await;
        return Err(e.into());
    }

    caller.audit(AUDIT_OBJECT_REMOVE, &cluster_ref(id), Some(c.workspace_id), details).await;

    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PruneInput {
    #[schemars(with = "String")]
    pub kind: ObjectKind,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct Pruned {
    pub removed: Vec<String>,
}

pub async fn prune_objects(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ClusterId>,
    Json(req): Json<PruneInput>,
) -> ApiResult<Json<Pruned>> {
    caller.authorize(Action::SwarmManage, cluster_ref(id)).await?;
    let c = state.swarm.swarm_cluster(id).await?;

    match state.swarm_stacks.prune_objects(id, req.kind, &principal(&caller)).await {
        Ok(removed) => {
            let details = json!({ "kind": req.kind.as_str(), "removed": removed });
            caller.audit(AUDIT_OBJECT_PRUNE, &cluster_ref(id), Some(c.workspace_id), details).await;
            Ok(Json(Pruned { removed }))
        }
        Err(e) => {
            let details = json!({ "kind": req.kind.as_str() });
            caller
                .audit_failed(AUDIT_OBJECT_PRUNE, &cluster_ref(id), Some(c.workspace_id), details, &e.to_string())
                .await;
            Err(e.into())
        }
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AddressQuery {
    #[schemars(with = "String")]
    pub server_id: ServerId,
}

/// Addresses the node could advertise to the swarm, the mesh address first.
pub async fn addresses(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ClusterId>,
    Query(q): Query<AddressQuery>,
) -> ApiResult<Json<Vec<AddressChoice>>> {
    caller.authorize(Action::View, cluster_ref(id)).await?;

    Ok(Json(state.swarm_stacks.addresses(id, q.server_id).await?))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct StatsQuery {
    /// Docker node ids, comma-separated (the nodes the viewed tasks run on).
    pub nodes: String,
}

/// CPU and memory of the Swarm tasks on these nodes, sampled live (reused
/// for ten seconds per node).
pub async fn task_stats(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ClusterId>,
    Query(q): Query<StatsQuery>,
) -> ApiResult<Json<Vec<forge_catalog::swarm::TaskStat>>> {
    caller.authorize(Action::View, cluster_ref(id)).await?;
    state.swarm.swarm_cluster(id).await?;
    let nodes: Vec<String> = q
        .nodes
        .split(',')
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .take(crate::swarm_stacks::MAX_STATS_NODES)
        .map(str::to_owned)
        .collect();

    Ok(Json(state.swarm_stacks.task_stats(id, &nodes, &principal(&caller)).await?))
}

// ------------------------------------------------------------ conversion

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConvertInput {
    /// `swarm` or `default`.
    #[schemars(with = "String")]
    pub to: ClusterMode,
}

pub async fn convert_check(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ClusterId>,
    Query(q): Query<ConvertInput>,
) -> ApiResult<Json<ConvertCheck>> {
    caller.authorize(Action::View, cluster_ref(id)).await?;

    Ok(Json(state.swarm_stacks.convert_check(id, q.to).await?))
}

pub async fn convert(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ClusterId>,
    Json(req): Json<ConvertInput>,
) -> ApiResult<Json<ConvertCheck>> {
    caller.authorize(Action::WorkspaceManage, cluster_ref(id)).await?;
    caller.authorize(Action::SwarmManage, cluster_ref(id)).await?;
    let details = json!({ "to": req.to.as_str() });

    match state.swarm_stacks.convert(id, req.to, &principal(&caller)).await {
        Ok(check) => {
            caller.audit(AUDIT_CONVERT, &cluster_ref(id), None, json!({ "from": check.from, "to": check.to })).await;
            Ok(Json(check))
        }
        Err(e) => {
            caller.audit_failed(AUDIT_CONVERT, &cluster_ref(id), None, details, &e.to_string()).await;
            Err(e.into())
        }
    }
}

// ------------------------------------------------------------ stacks

/// A stack in a swarm: where it runs and how it is doing.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct StackSwarmView {
    #[schemars(with = "String")]
    pub cluster_id: ClusterId,
    pub namespace: String,
    pub revision: i64,
    pub deployed_revision: i64,
    #[schemars(with = "Option<serde_json::Value>")]
    pub status: Option<StackStatus>,
    pub status_error: Option<String>,
    /// What the policy says about the latest revision.
    #[schemars(with = "Vec<serde_json::Value>")]
    pub findings: Vec<forge_catalog::stacks::Finding>,
    pub findings_error: Option<String>,
    /// The latest revision needs `stack.privileged`.
    pub privileged: bool,
}

async fn swarm_stack(state: &AppState, id: StackId) -> ApiResult<(Stack, ClusterId)> {
    let stack = super::stacks::load(state, id).await?;
    let cluster = state.swarm_stacks.cluster_of(&stack).await?;

    Ok((stack, cluster))
}

pub async fn stack_get(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<StackId>,
) -> ApiResult<Json<StackSwarmView>> {
    let (stack, cluster) = swarm_stack(&state, id).await?;
    caller.authorize(Action::View, ns_ref(stack.environment_id)).await?;
    let who = principal(&caller);

    let (status, status_error) = match state.swarm_stacks.status(&stack, &who).await {
        Ok(s) => (Some(s), None),
        Err(e) => (None, Some(e.to_string())),
    };

    let (findings, privileged, findings_error) =
        match state.swarm_stacks.prepare(&stack, stack.revision, true, &who).await {
            Ok(p) => (p.report.findings, p.report.privileged, None),
            Err(e) => (Vec::new(), false, Some(e.to_string())),
        };

    Ok(Json(StackSwarmView {
        cluster_id: cluster,
        namespace: state.swarm_stacks.namespace(&stack).await?,
        revision: stack.revision,
        deployed_revision: stack.deployed_revision,
        status,
        status_error,
        findings,
        findings_error,
        privileged,
    }))
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SwarmDeployInput {
    /// Revision to deploy (default: the latest). An older one rolls back.
    #[serde(default)]
    pub revision: Option<i64>,
}

/// Starts a `swarm.stack` job (also used by `POST /stacks/{id}/deploy` and
/// `/down` for stacks in a Swarm cluster).
pub(super) async fn start(state: &AppState, caller: &Caller, stack: &Stack, action: StackAction) -> ApiResult<Job> {
    caller.authorize(Action::StackManage, ns_ref(stack.environment_id)).await?;
    let cluster = state.swarm_stacks.cluster_of(stack).await?;

    // Fails early (409) when the swarm has no manager yet.
    state.swarm.manager(cluster).await?;
    let mut privileged = false;
    let mut details = json!({ "stack": stack.name, "action": action.name() });

    if let StackAction::Deploy { revision } = action {
        let p = state.swarm_stacks.prepare(stack, revision, true, &principal(caller)).await?;
        p.report.ensure(true).map_err(ApiError::bad_request)?;

        if p.report.privileged {
            // Host-access settings: administrators with MFA only.
            caller.authorize(Action::StackPrivileged, ns_ref(stack.environment_id)).await?;
            privileged = true;
        }

        details["revision"] = json!(revision);
        details["rollback"] = json!(revision < stack.revision);
        details["privileged"] = json!(privileged);
    }

    let payload =
        Payload { stack_id: stack.id, cluster_id: cluster, action, privileged, stack_name: stack.name.clone() };
    let job = state
        .jobs
        .submit(NewJob {
            kind: KIND.into(),
            workspace_id: Some(stack.workspace_id),
            resource: ns_ref(stack.environment_id),
            principal: caller.principal(),
            context: caller.ctx(),
            idempotency_key: None,
            payload: serde_json::to_value(&payload).map_err(forge_core::DomainError::internal)?,
            max_attempts: 6,
        })
        .await?;
    details["job_id"] = json!(job.id);
    caller.audit(AUDIT_STACK, &ns_ref(stack.environment_id), Some(stack.workspace_id), details).await;

    Ok(job)
}

pub async fn stack_deploy(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<StackId>,
    Json(req): Json<SwarmDeployInput>,
) -> ApiResult<(StatusCode, Json<Job>)> {
    let (stack, _) = swarm_stack(&state, id).await?;
    let revision = req.revision.unwrap_or(stack.revision);
    let job = start(&state, &caller, &stack, StackAction::Deploy { revision }).await?;

    Ok((StatusCode::ACCEPTED, Json(job)))
}

pub async fn stack_remove(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<StackId>,
) -> ApiResult<(StatusCode, Json<Job>)> {
    let (stack, _) = swarm_stack(&state, id).await?;
    let job = start(&state, &caller, &stack, StackAction::Remove).await?;

    Ok((StatusCode::ACCEPTED, Json(job)))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct StackServiceInput {
    /// `{"kind": "scale", "replicas": 3}`, `{"kind": "restart"}` or `{"kind": "rollback"}`.
    #[serde(flatten)]
    #[schemars(with = "serde_json::Value")]
    pub action: StackServiceAction,
}

pub async fn stack_service(
    State(state): State<AppState>,
    caller: Caller,
    Path((id, service)): Path<(StackId, String)>,
    Json(req): Json<StackServiceInput>,
) -> ApiResult<StatusCode> {
    let (stack, _) = swarm_stack(&state, id).await?;
    caller.authorize(Action::StackManage, ns_ref(stack.environment_id)).await?;
    let namespace = state.swarm_stacks.namespace(&stack).await?;
    let details = json!({ "stack": stack.name, "service": service, "action": req.action });
    let op = SwarmOp::StackServiceAction { namespace, service, action: req.action };

    if let Err(e) = state.swarm_stacks.service_op(&stack, &principal(&caller), op).await {
        caller
            .audit_failed(
                AUDIT_STACK_SERVICE,
                &ns_ref(stack.environment_id),
                Some(stack.workspace_id),
                details,
                &e.to_string(),
            )
            .await;
        return Err(e.into());
    }

    caller.audit(AUDIT_STACK_SERVICE, &ns_ref(stack.environment_id), Some(stack.workspace_id), details).await;

    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct LogsQuery {
    #[serde(default)]
    pub tail: Option<u32>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct Logs {
    pub logs: String,
}

pub async fn stack_service_logs(
    State(state): State<AppState>,
    caller: Caller,
    Path((id, service)): Path<(StackId, String)>,
    Query(q): Query<LogsQuery>,
) -> ApiResult<Json<Logs>> {
    let (stack, _) = swarm_stack(&state, id).await?;
    caller.authorize(Action::View, ns_ref(stack.environment_id)).await?;
    let namespace = state.swarm_stacks.namespace(&stack).await?;
    let tail = q.tail.unwrap_or(200).clamp(1, 10_000);
    let v = state
        .swarm_stacks
        .service_op(&stack, &principal(&caller), SwarmOp::StackServiceLogs { namespace, service, tail })
        .await?;

    Ok(Json(serde_json::from_value(v).map_err(forge_core::DomainError::internal)?))
}

pub fn routes() -> axum::Router<AppState> {
    use axum::routing::{delete, get, post};

    axum::Router::new()
        .route("/clusters/{id}/swarm/stacks", get(stacks))
        .route("/clusters/{id}/swarm/networks", get(networks))
        .route("/clusters/{id}/swarm/networks/shared", post(ensure_shared))
        .route("/clusters/{id}/swarm/objects", get(objects).post(put_object))
        .route("/clusters/{id}/swarm/objects/prune", post(prune_objects))
        .route("/clusters/{id}/swarm/objects/{kind}/{name}", delete(remove_object))
        .route("/clusters/{id}/swarm/addresses", get(addresses))
        .route("/clusters/{id}/swarm/task-stats", get(task_stats))
        .route("/clusters/{id}/convert", get(convert_check).post(convert))
        .route("/stacks/{id}/swarm", get(stack_get))
        .route("/stacks/{id}/swarm/deploy", post(stack_deploy))
        .route("/stacks/{id}/swarm/remove", post(stack_remove))
        .route("/stacks/{id}/swarm/services/{service}", post(stack_service))
        .route("/stacks/{id}/swarm/services/{service}/logs", get(stack_service_logs))
}
