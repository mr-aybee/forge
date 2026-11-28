//! Containers on a node's Docker engine.
//!
//! `POST /api/v1/servers/{id}/containers` with `{op, id?, image?, tail?,
//! reveal_env?}` runs one operation as an `agent.command` job (authorized and
//! audited by the job engine: reads need `server.manage`, changes
//! `container.manage`; `inspect` with `reveal_env` needs
//! `container.inspect_env`, MFA). The call waits briefly for the result so
//! the console stays snappy; slow operations (image pulls) return the job to
//! follow instead.
//!
//! Revealed environment values are returned in this call only: they never
//! reach the job table (the job keeps `env_keys`), and the reveal is audited
//! as `container.env.reveal` with the container id, never the values.

use std::time::Duration;

use axum::Json;
use axum::extract::{Path, State};
use forge_core::model::{JobStatus, NewJob};
use forge_core::ports::{JobRepository, ServerRepository};
use forge_core::{EntityKind, EntityRef, ServerId};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::jobs::agent_command::{parse_command, required_action};
use crate::state::AppState;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ContainerRequest {
    pub op: String,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub image: String,
    #[serde(default)]
    pub tail: u32,
    /// `logs`: lines after this Unix time (decimal seconds), for live follow.
    #[serde(default)]
    pub since: String,
    /// `inspect`: also return environment values (`result.env`, `[{key,
    /// value}]`). Needs `container.inspect_env` (MFA) and an agent that
    /// supports it; the values are not stored with the job.
    #[serde(default)]
    pub reveal_env: bool,
}

/// Container ids and names, as the node's helper accepts them:
/// `[A-Za-z0-9][A-Za-z0-9_.-]{0,127}`.
#[must_use]
pub fn valid_container_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.as_bytes()[0].is_ascii_alphanumeric()
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}

const QUICK_WAIT: Duration = Duration::from_secs(25);

pub async fn run(
    State(state): State<AppState>,
    caller: Caller,
    Path(server_id): Path<ServerId>,
    Json(req): Json<ContainerRequest>,
) -> ApiResult<Json<Value>> {
    if !req.since.is_empty() && !forge_agent_since_ok(&req.since) {
        return Err(ApiError::bad_request("`since` must be a Unix time in seconds"));
    }

    if req.reveal_env && req.op != "inspect" {
        return Err(ApiError::bad_request("`reveal_env` applies to `inspect` only"));
    }

    let command = json!({
        "type": "container", "op": req.op, "id": req.id, "image": req.image, "tail": req.tail, "since": req.since,
        "reveal_env": req.reveal_env,
    });
    parse_command(&command).map_err(ApiError::bad_request)?;

    // Same rules as the node helper, so bad input fails fast with a 400.
    let id_ok = req.id.is_empty() || valid_container_id(&req.id);
    let image_ok = req.image.is_empty()
        || (req.image.len() <= 255
            && req.image.as_bytes()[0].is_ascii_alphanumeric()
            && req
                .image
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b':' | b'.' | b'_' | b'-' | b'@')));
    let needs_id = !matches!(req.op.as_str(), "info" | "list" | "images" | "pull");
    if !id_ok || !image_ok || (needs_id && req.id.is_empty()) || (req.op == "pull" && req.image.is_empty()) {
        return Err(ApiError::bad_request("invalid container id or image reference"));
    }

    let server = state.store.get_server(server_id).await.map_err(|_| ApiError::not_found("server not found"))?;
    let resource = EntityRef::new(EntityKind::Server, server_id);
    let payload = json!({ "server_id": server_id, "command": command });

    // The job runner authorizes again before every step; checking here first
    // turns a denial (or a missing MFA step-up) into a plain 403 instead of
    // a failed job.
    caller.authorize(required_action(&payload), resource.clone()).await?;
    let job = state
        .jobs
        .submit(NewJob {
            kind: "agent.command".into(),
            workspace_id: Some(server.workspace_id),
            resource: resource.clone(),
            principal: caller.principal(),
            context: caller.ctx(),
            idempotency_key: None,
            payload,
            max_attempts: 1,
        })
        .await?;
    let deadline = tokio::time::Instant::now() + if req.op == "pull" { Duration::ZERO } else { QUICK_WAIT };

    loop {
        let j = state.store.get_job(job.id).await?;
        match j.status {
            JobStatus::Succeeded => {
                let mut result = j.state.get("output").cloned().unwrap_or(Value::Null);

                if req.reveal_env {
                    // The values were kept in memory for this call only.
                    let env =
                        state.hub.take_transient(&j.id.to_string()).and_then(|mut t| t.get_mut("env").map(Value::take));

                    let Some(env) = env else {
                        return Err(ApiError::new(
                            axum::http::StatusCode::CONFLICT,
                            "agent_update_required",
                            "this node's agent does not return environment values; update the agent (Software → Updates → Agents)",
                        ));
                    };

                    let count = env.as_array().map_or(0, Vec::len);

                    if let Some(o) = result.as_object_mut() {
                        o.insert("env".into(), env);
                    }

                    caller
                        .audit(
                            "container.env.reveal",
                            &resource,
                            Some(server.workspace_id),
                            json!({ "container": req.id, "variables": count, "job_id": j.id }),
                        )
                        .await;
                }

                return Ok(Json(json!({ "job_id": j.id, "status": "succeeded", "result": result })));
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

/// A decimal Unix time such as `1731234567.123456789` (same rule as the agent).
fn forge_agent_since_ok(s: &str) -> bool {
    let (secs, frac) = s.split_once('.').unwrap_or((s, ""));
    !secs.is_empty()
        && s.len() <= 24
        && secs.bytes().all(|b| b.is_ascii_digit())
        && frac.bytes().all(|b| b.is_ascii_digit())
}
