//! Docker images, volumes and networks on a node (D-157): parse, authorize,
//! call, audit, respond. Reads need `server.manage` on the node; changes
//! need `docker.image.manage`, `docker.volume.manage` or
//! `docker.network.manage`. Pulls, pushes and prunes start a `docker.op` job.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use forge_catalog::docker_ops::DockerOp;
use forge_core::model::NewJob;
use forge_core::{Action, EntityKind, EntityRef, ServerId};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::auth::Caller;
use crate::docker_resources::{action_for, is_slow};
use crate::error::{ApiError, ApiResult};
use crate::jobs::docker_op::{KIND, Payload};
use crate::state::AppState;

fn server_ref(id: ServerId) -> EntityRef {
    EntityRef::new(EntityKind::Server, id)
}

async fn read(state: &AppState, caller: &Caller, id: ServerId, op: DockerOp) -> ApiResult<Json<Value>> {
    caller.authorize(action_for(&op), server_ref(id)).await?;
    Ok(Json(state.docker_resources.run(id, &caller.principal().to_string(), &op).await?))
}

pub async fn images(State(state): State<AppState>, caller: Caller, Path(id): Path<ServerId>) -> ApiResult<Json<Value>> {
    read(&state, &caller, id, DockerOp::Images {}).await
}

pub async fn volumes(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServerId>,
) -> ApiResult<Json<Value>> {
    read(&state, &caller, id, DockerOp::Volumes {}).await
}

pub async fn networks(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServerId>,
) -> ApiResult<Json<Value>> {
    read(&state, &caller, id, DockerOp::Networks {}).await
}

pub async fn disk_usage(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServerId>,
) -> ApiResult<Json<Value>> {
    read(&state, &caller, id, DockerOp::DiskUsage {}).await
}

#[derive(Debug, Deserialize)]
pub struct EventsQuery {
    #[serde(default = "five_minutes")]
    pub since_secs: u32,
}

const fn five_minutes() -> u32 {
    300
}

pub async fn events(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServerId>,
    Query(q): Query<EventsQuery>,
) -> ApiResult<Json<Value>> {
    read(&state, &caller, id, DockerOp::Events { since_secs: q.since_secs.min(86_400) }).await
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DockerOpInput {
    /// One `forge_catalog::docker_ops::DockerOp`, e.g.
    /// `{"op": "image_pull", "image": "nginx:1.27"}`.
    #[serde(flatten)]
    #[schemars(with = "Value")]
    pub op: DockerOp,
}

/// `POST /servers/{id}/docker/ops`: one operation. Reads and quick changes
/// answer `200` with the node's result; slow ones `202` with the job.
pub async fn op(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServerId>,
    Json(DockerOpInput { op }): Json<DockerOpInput>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    // Authorized first, so a refused request is audited as a denial too.
    caller.authorize(action_for(&op), server_ref(id)).await?;
    if let DockerOp::ImagePull { registry_auth: Some(_), .. } | DockerOp::ImagePush { registry_auth: Some(_), .. } = &op
    {
        return Err(ApiError::bad_request("registry credentials are not accepted here; add the registry in Forge"));
    }
    op.validate().map_err(ApiError::bad_request)?;
    let ws = state.docker_resources.workspace(id).await?;
    // Pushes go only to registries added in Forge: an image never leaves for
    // a registry nobody approved.
    // Pushes use the workspace's stored credentials: only administrators
    // (registry.manage, MFA), and only to a registry that allows pushes.
    if let DockerOp::ImagePush { image, .. } = &op
        && !state.registries.for_image(ws, image).await?.is_some_and(|r| r.allow_push)
    {
        return Err(ApiError::bad_request(format!(
            "push only to a registry added in Forge with pushes allowed (Settings → Registries); `{}` is not one",
            forge_core::registries::image_host(image)
        )));
    }
    let audit = format!("docker.{}", op.name());
    let details = json!({ "op": op.name(), "request": redacted(&op) });
    let principal = caller.principal().to_string();
    if op.read_only() {
        let mut out = state.docker_resources.run(id, &principal, &op).await?;
        if matches!(op, DockerOp::ImageInspect { .. } | DockerOp::ImageHistory { .. })
            && !caller.can(Action::ContainerInspectEnv, server_ref(id)).await
        {
            // Environment values and build arguments often hold build-time
            // secrets: keys only.
            hide_env_values(&mut out);
        }
        if op.is_inspect() {
            caller.audit(&audit, &server_ref(id), Some(ws), details).await;
        }
        return Ok((StatusCode::OK, Json(out)));
    }
    if is_slow(&op) {
        let job = state
            .jobs
            .submit(NewJob {
                kind: KIND.into(),
                workspace_id: Some(ws),
                resource: server_ref(id),
                principal: caller.principal(),
                context: caller.ctx(),
                idempotency_key: None,
                payload: serde_json::to_value(Payload { server_id: id, op })
                    .map_err(forge_core::DomainError::internal)?,
                max_attempts: 2,
            })
            .await;
        return match job {
            Ok(job) => {
                caller.audit(&audit, &server_ref(id), Some(ws), json!({ "details": details, "job_id": job.id })).await;
                Ok((StatusCode::ACCEPTED, Json(json!(job))))
            }
            Err(e) => {
                caller.audit_failed(&audit, &server_ref(id), Some(ws), details, &e.to_string()).await;
                Err(e.into())
            }
        };
    }
    match state.docker_resources.run(id, &principal, &op).await {
        Ok(out) => {
            caller.audit(&audit, &server_ref(id), Some(ws), details).await;
            Ok((StatusCode::OK, Json(out)))
        }
        Err(e) => {
            caller.audit_failed(&audit, &server_ref(id), Some(ws), details, &e.to_string()).await;
            Err(e.into())
        }
    }
}

/// Hides environment values (an image's details) and `KEY=value` words in
/// build commands (its history), keeping the keys.
fn hide_env_values(detail: &mut Value) {
    if let Some(env) = detail.as_object_mut().and_then(|d| d.get_mut("env")).and_then(Value::as_array_mut) {
        for e in env {
            e["value"] = Value::Null;
        }
    }
    if let Some(layers) = detail.as_array_mut() {
        for l in layers {
            if let Some(cmd) = l["created_by"].as_str() {
                l["created_by"] = Value::String(mask_assignments(cmd));
            }
        }
    }
}

/// `ENV TOKEN=abc` → `ENV TOKEN=***`; build arguments (`|1 NPM_TOKEN=abc
/// /bin/sh -c …`) the same. Any `KEY=value` word is masked: a harmless one
/// costs a little readability, a missed secret costs more.
fn mask_assignments(cmd: &str) -> String {
    cmd.split(' ')
        .map(|w| match w.split_once('=') {
            Some((k, _))
                if k.bytes().next().is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
                    && k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') =>
            {
                format!("{k}=***")
            }
            _ => w.to_owned(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The request for the audit log: never credentials.
fn redacted(op: &DockerOp) -> Value {
    // A typed enum always serializes; the fallback only keeps audit going.
    let mut v = serde_json::to_value(op).unwrap_or(Value::Null);
    if let Some(o) = v.as_object_mut() {
        o.remove("registry_auth");
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_secrets_are_masked() {
        assert_eq!(mask_assignments("ENV TOKEN=abc PATH=/bin"), "ENV TOKEN=*** PATH=***");
        assert_eq!(mask_assignments("|1 NPM_TOKEN=s3cret /bin/sh -c npm ci"), "|1 NPM_TOKEN=*** /bin/sh -c npm ci");
        assert_eq!(mask_assignments("RUN make --flag"), "RUN make --flag");
        let mut history = serde_json::json!([{"created_by": "ENV KEY=v"}]);
        hide_env_values(&mut history);
        assert_eq!(history[0]["created_by"], "ENV KEY=***");
    }
}
