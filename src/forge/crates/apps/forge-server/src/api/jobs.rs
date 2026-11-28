//! Jobs: list, inspect, submit, cancel.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use forge_core::model::{Job, JobFilter, JobStatus, JobStepRecord, NewJob};
use forge_core::ports::JobRepository;
use forge_core::*;
use serde::Deserialize;
use serde_json::Value;

use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

fn job_ref(id: JobId) -> EntityRef {
    EntityRef::new(EntityKind::Job, id)
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct JobQuery {
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub workspace: Option<WorkspaceId>,
    #[serde(default)]
    #[schemars(with = "Option<crate::api::schema_types::JobStatus>")]
    pub status: Option<JobStatus>,
    #[serde(default)]
    pub limit: Option<u32>,
}

pub async fn list(
    State(state): State<AppState>,
    caller: Caller,
    Query(q): Query<JobQuery>,
) -> ApiResult<Json<Vec<Job>>> {
    let filter = JobFilter { workspace_id: q.workspace, status: q.status, limit: q.limit.unwrap_or(100) };
    let mut visible = Vec::new();

    for j in state.store.list_jobs(&filter).await? {
        if caller.can(Action::View, job_ref(j.id)).await {
            visible.push(redact_job(j));
        }
    }

    Ok(Json(visible))
}

pub async fn get(State(state): State<AppState>, caller: Caller, Path(id): Path<JobId>) -> ApiResult<Json<Job>> {
    caller.authorize(Action::View, job_ref(id)).await?;
    Ok(Json(redact_job(state.store.get_job(id).await?)))
}

/// Basic-auth hashes of an endpoint's `agent.command` never leave the server.
fn redact_job(mut j: Job) -> Job {
    if let Some(spec) = j.payload.pointer_mut("/command/spec") {
        let mut wrapper = serde_json::json!({ "spec": spec.take() });
        crate::endpoints::redact(&mut wrapper);
        *spec = wrapper["spec"].take();
    }

    j
}

/// `GET /jobs/{id}/live`: output streamed by the agent while the job's
/// command runs (`null` when nothing is streaming).
pub async fn live(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<JobId>,
) -> ApiResult<Json<serde_json::Value>> {
    caller.authorize(Action::View, job_ref(id)).await?;
    let job = state.store.get_job(id).await?;
    let output =
        job.state.get("operation_id").and_then(serde_json::Value::as_str).and_then(|op| state.hub.live_output(op));

    Ok(Json(serde_json::json!({ "output": output, "status": job.status })))
}

pub async fn steps(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<JobId>,
) -> ApiResult<Json<Vec<JobStepRecord>>> {
    caller.authorize(Action::View, job_ref(id)).await?;
    Ok(Json(state.store.list_steps(id).await?))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SubmitJob {
    pub kind: String,
    /// Entity reference the job acts on, e.g. `workspace:<id>` or `server:<id>`.
    pub resource: String,
    #[serde(default)]
    pub payload: Value,
    #[serde(default)]
    pub idempotency_key: Option<String>,
    #[serde(default)]
    pub max_attempts: Option<u32>,
}

/// Generic submission for job kinds that accept direct submission.
const DIRECT_KINDS: [&str; 1] = ["diagnostics.sleep"];

pub async fn submit(
    State(state): State<AppState>,
    caller: Caller,
    Json(req): Json<SubmitJob>,
) -> ApiResult<(StatusCode, Json<Job>)> {
    if !DIRECT_KINDS.contains(&req.kind.as_str()) {
        return Err(ApiError::bad_request("this job kind cannot be submitted directly"));
    }

    let resource: EntityRef = req.resource.parse().map_err(ApiError::bad_request)?;
    let resolved = state.authz_resolve(&resource).await.map_err(|_| ApiError::not_found("resource not found"))?;
    let workspace_id = std::iter::once(&resolved.entity)
        .chain(resolved.ancestors.iter())
        .find(|e| e.kind == EntityKind::Workspace)
        .and_then(|e| e.id.parse().ok());
    let job = state
        .jobs
        .submit(NewJob {
            kind: req.kind,
            workspace_id,
            resource,
            principal: caller.principal(),
            context: caller.ctx(),
            idempotency_key: req.idempotency_key,
            payload: if req.payload.is_null() { Value::Object(serde_json::Map::new()) } else { req.payload },
            max_attempts: req.max_attempts.unwrap_or(3).clamp(1, 10),
        })
        .await?;

    Ok((StatusCode::ACCEPTED, Json(job)))
}

/// Cancels a queued or running job: the running step stops at once, and an
/// operation still running on a node is stopped there too.
pub async fn cancel(State(state): State<AppState>, caller: Caller, Path(id): Path<JobId>) -> ApiResult<StatusCode> {
    if state.jobs.cancel(id, caller.principal(), caller.ctx()).await? {
        // The node keeps running a recipe or install unless told: send it a
        // signed cancel for the job's operation (best effort).
        if let Ok(job) = state.store.get_job(id).await
            && let Some(op) = job.state.get("operation_id").and_then(Value::as_str)
            && let Some(server) =
                job.payload.get("server_id").and_then(Value::as_str).and_then(|s| s.parse::<ServerId>().ok())
        {
            let (body, signed) = state.signer.issue(
                &server.to_string(),
                &caller.principal().to_string(),
                forge_proto::Command::CancelAction(forge_proto::messages::CancelAction { operation_id: op.to_owned() }),
                state.clock.now(),
                std::time::Duration::from_secs(120),
            );

            if let Err(e) = state.hub.enqueue(server, &body.operation_id, signed) {
                tracing::warn!(error = %e, job = %id, "could not send the cancel to the node");
            }
        }

        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::new(StatusCode::CONFLICT, "conflict", "job already finished"))
    }
}
