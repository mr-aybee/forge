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

/// What a masked value is shown as.
const MASK: &str = "***";

/// Flags whose next word is a credential (`--password x`).
const SECRET_FLAGS: &[&str] = &[
    "--password",
    "--passwd",
    "--pass",
    "--token",
    "--auth-token",
    "--access-token",
    "--secret",
    "--client-secret",
    "--api-key",
    "--apikey",
    "--access-key",
    "--secret-key",
    "--auth",
];

/// Splits `cmd` into words and the whitespace between them (kept verbatim).
/// A quoted run (`"a b"`, `'a b'`) or a backslash escape stays in its word.
fn words(cmd: &str) -> Vec<(String, bool)> {
    let mut out: Vec<(String, bool)> = Vec::new();
    let mut quote: Option<char> = None;
    let mut chars = cmd.chars();

    while let Some(c) = chars.next() {
        let space = quote.is_none() && c.is_whitespace();

        match out.last_mut() {
            Some((w, s)) if *s == space => w.push(c),
            _ => out.push((c.to_string(), space)),
        }

        if space {
            continue;
        }

        match (c, quote) {
            ('\\', _) => {
                if let (Some(n), Some((w, _))) = (chars.next(), out.last_mut()) {
                    w.push(n);
                }
            }
            ('"' | '\'', None) => quote = Some(c),
            (q, Some(open)) if q == open => quote = None,
            _ => {}
        }
    }

    out
}

/// Whether `k` (before `=`) looks like a variable or a `--flag`.
fn is_key(k: &str) -> bool {
    let k = k.trim_start_matches('-');
    k.bytes().next().is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// `ENV TOKEN=abc` → `ENV TOKEN=***`; build arguments (`|1 NPM_TOKEN=abc
/// /bin/sh -c …`) the same, quoted values (`KEY="a b"`) whole. Words are
/// split on any whitespace. The word after a secret flag (`--password x`)
/// and an `Authorization:` header value (`Bearer x`) are masked too. Any
/// `KEY=value` word is masked: a harmless one costs a little readability, a
/// missed secret costs more.
fn mask_assignments(cmd: &str) -> String {
    let mut out = String::with_capacity(cmd.len());
    let mut mask_next = false;

    for (w, space) in words(cmd) {
        if space {
            out.push_str(&w);
            continue;
        }

        let bare = w.trim_matches(['"', '\'']).to_ascii_lowercase();

        if mask_next {
            // `Authorization: Bearer x`: keep the scheme, mask what follows.
            if matches!(bare.as_str(), "bearer" | "basic" | "token") {
                out.push_str(&w);
            } else {
                out.push_str(MASK);
                mask_next = false;
            }

            continue;
        }

        if SECRET_FLAGS.contains(&bare.as_str()) || bare == "bearer" {
            out.push_str(&w);
            mask_next = true;
        } else if let Some(i) = w.to_ascii_lowercase().find("authorization:") {
            let head = &w[..i + "authorization:".len()];
            let rest = w[head.len()..].trim_matches(['"', '\'']);
            if rest.is_empty() {
                out.push_str(&w);
                mask_next = true;
            } else {
                out.push_str(head);
                out.push(' ');
                out.push_str(MASK);

                if let Some(q) = w.chars().last().filter(|c| *c == '"' || *c == '\'') {
                    out.push(q);
                }
            }
        } else if let Some((k, _)) = w.split_once('=')
            && is_key(k.trim_start_matches(['"', '\'']))
        {
            out.push_str(k);
            out.push('=');
            out.push_str(MASK);
        } else {
            out.push_str(&w);
        }
    }

    out
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
        assert_eq!(mask_assignments("ENV KEY=v"), "ENV KEY=***");
    }

    #[test]
    fn quoted_values_are_masked_whole() {
        assert_eq!(mask_assignments(r#"ENV KEY="a b c" OTHER=x"#), "ENV KEY=*** OTHER=***");
        assert_eq!(mask_assignments("ENV KEY='s3 cr3t'"), "ENV KEY=***");
        assert_eq!(mask_assignments(r#"ENV KEY="a \" b""#), "ENV KEY=***");
    }

    #[test]
    fn any_whitespace_separates_words() {
        assert_eq!(
            mask_assignments(
                "ENV	TOKEN=abc
PASS=x"
            ),
            "ENV	TOKEN=***
PASS=***"
        );
    }

    #[test]
    fn secret_flags_and_auth_headers_are_masked() {
        assert_eq!(mask_assignments("RUN login --password hunter2 --user bob"), "RUN login --password *** --user bob");
        assert_eq!(mask_assignments("RUN tool --token abc"), "RUN tool --token ***");
        assert_eq!(mask_assignments("RUN tool --api-key=abc"), "RUN tool --api-key=***");
        assert_eq!(
            mask_assignments("RUN curl -H Authorization: Bearer abc https://x"),
            "RUN curl -H Authorization: Bearer *** https://x"
        );
        assert_eq!(
            mask_assignments(r#"RUN curl -H "Authorization: Bearer abc" https://x"#),
            r#"RUN curl -H "Authorization: ***" https://x"#
        );
        assert_eq!(mask_assignments("RUN echo bearer tok"), "RUN echo bearer ***");
        let mut history = serde_json::json!([{"created_by": "ENV KEY=v"}]);
        hide_env_values(&mut history);
        assert_eq!(history[0]["created_by"], "ENV KEY=***");
    }
}
