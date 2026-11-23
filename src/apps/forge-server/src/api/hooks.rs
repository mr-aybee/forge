//! The public end of deploy webhooks (D-159): `POST /hooks/{token}` deploys
//! the webhook's target as its creator; `GET /hooks/{token}/jobs/{job}`
//! reports a job this webhook started. No session: the secret URL (and the
//! signature, when set) is the credential. Rate-limited per source address
//! (before any lookup) and per webhook; every delivery is recorded.

use axum::Json;
use axum::body::Bytes;
use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use forge_core::webhooks::{Delivery, DeliveryOutcome, Webhook, WebhookTarget};
use forge_core::{JobId, WebhookDeliveryId};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::state::AppState;
use crate::webhooks::{Rejection, SignatureHeaders};

/// Largest delivery body read (CI payloads are small).
pub const MAX_BODY: usize = 64 * 1024;
pub const AUDIT_DELIVER: &str = "webhook.deliver";

#[derive(Debug, Deserialize)]
pub struct TagQuery {
    #[serde(default)]
    pub tag: Option<String>,
}

fn answer(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}

async fn record(
    state: &AppState,
    w: &Webhook,
    outcome: DeliveryOutcome,
    reason: &str,
    job: Option<JobId>,
    tag: Option<String>,
    ip: &str,
) {
    let d = Delivery {
        id: WebhookDeliveryId::new(),
        webhook_id: w.id,
        at: state.clock.now(),
        outcome,
        reason: reason.chars().take(500).collect(),
        job_id: job,
        tag,
        source_ip: ip.to_owned(),
    };
    if let Err(e) = state.webhooks.repo.add_delivery(&d).await {
        tracing::warn!(webhook = %w.id, error = %e, "recording a webhook delivery failed");
    }
}

/// The tag a delivery asks for: `?tag=`, else a JSON body's `"tag"`.
fn requested_tag(q: &TagQuery, body: &[u8]) -> Option<String> {
    q.tag.clone().or_else(|| {
        serde_json::from_slice::<Value>(body).ok().and_then(|v| v.get("tag").and_then(Value::as_str).map(str::to_owned))
    })
}

pub async fn fire(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>,
    headers: HeaderMap,
    Path(token): Path<String>,
    Query(q): Query<TagQuery>,
    body: Bytes,
) -> Response {
    let ip = crate::throttle::client_ip(&headers, peer, &state.config.trusted_proxies());
    if !state.webhooks.allow_ip(ip) {
        return answer(StatusCode::TOO_MANY_REQUESTS, "too many requests");
    }
    let w = match state.webhooks.by_token(&token).await {
        Ok(Some(w)) => w,
        Ok(None) => return answer(StatusCode::NOT_FOUND, "no such webhook"),
        Err(e) => {
            tracing::error!(error = %e, "webhook lookup failed");
            return answer(StatusCode::SERVICE_UNAVAILABLE, "try again later");
        }
    };
    let ip_text = ip.to_string();
    let tag = requested_tag(&q, &body);
    if !state.webhooks.allow_hook(w.id) {
        record(&state, &w, DeliveryOutcome::RateLimited, "rate limited", None, tag, &ip_text).await;
        return answer(StatusCode::TOO_MANY_REQUESTS, "too many deliveries; slow down");
    }
    let h = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    let sig = SignatureHeaders {
        hub_signature_256: h("x-hub-signature-256"),
        gitlab_token: h("x-gitlab-token"),
        forge_signature: h("x-forge-signature"),
        forge_timestamp: h("x-forge-timestamp"),
    };
    let tag = match state.webhooks.verify(&w, &sig, &body, tag.as_deref()) {
        Ok(t) => t,
        Err(r) => {
            record(&state, &w, DeliveryOutcome::Rejected, &r.to_string(), None, tag, &ip_text).await;
            let status = match r {
                Rejection::BadSignature | Rejection::Stale => StatusCode::UNAUTHORIZED,
                Rejection::Disabled => StatusCode::FORBIDDEN,
                Rejection::RateLimited => StatusCode::TOO_MANY_REQUESTS,
                Rejection::TagNotAllowed | Rejection::TagRefused => StatusCode::UNPROCESSABLE_ENTITY,
            };
            return answer(status, &r.to_string());
        }
    };
    let caller = match crate::auth::webhook_caller(&state, &w.created_by, w.id).await {
        Ok(Some(c)) => c,
        Ok(None) | Err(_) => {
            record(
                &state,
                &w,
                DeliveryOutcome::Failed,
                "the webhook's creator is gone or disabled",
                None,
                tag,
                &ip_text,
            )
            .await;
            return answer(StatusCode::FORBIDDEN, "the webhook's creator can no longer deploy");
        }
    };
    match deploy(&state, &caller, &w, tag.as_deref()).await {
        Ok(jobs) => {
            let first = jobs.first().copied();
            record(&state, &w, DeliveryOutcome::Accepted, "", first, tag.clone(), &ip_text).await;
            let target = EntityRefFor(&w);
            caller
                .audit(
                    AUDIT_DELIVER,
                    &target.entity(),
                    Some(w.workspace_id),
                    json!({ "webhook": w.id, "tag": tag, "jobs": jobs, "source_ip": ip_text }),
                )
                .await;
            let status_url = first.map(|j| format!("/hooks/{token}/jobs/{j}"));
            (StatusCode::ACCEPTED, Json(json!({ "jobs": jobs, "status_url": status_url }))).into_response()
        }
        Err(e) => {
            record(&state, &w, DeliveryOutcome::Failed, &e.message, None, tag, &ip_text).await;
            e.into_response()
        }
    }
}

/// The audit target of a webhook.
struct EntityRefFor<'a>(&'a Webhook);

impl EntityRefFor<'_> {
    fn entity(&self) -> forge_core::EntityRef {
        match self.0.target {
            WebhookTarget::Service { id } => forge_core::EntityRef::new(forge_core::EntityKind::Service, id),
            // Stacks have no entity of their own: the workspace.
            WebhookTarget::Stack { .. } => {
                forge_core::EntityRef::new(forge_core::EntityKind::Workspace, self.0.workspace_id)
            }
        }
    }
}

/// Deploys the target as `caller` through the same paths as the console.
async fn deploy(
    state: &AppState,
    caller: &crate::auth::Caller,
    w: &Webhook,
    tag: Option<&str>,
) -> Result<Vec<JobId>, crate::error::ApiError> {
    match w.target {
        WebhookTarget::Service { id } => {
            let app = super::services::load_own(state, id).await?;
            if let Some(t) = tag {
                caller.authorize(forge_core::Action::Deploy, super::services::svc_ref(id)).await?;
                state.webhooks.set_tag(&app, t).await?;
                caller
                    .audit(
                        "service.update",
                        &super::services::svc_ref(id),
                        Some(app.workspace_id),
                        json!({ "service": app.name, "tag": t, "via": "webhook" }),
                    )
                    .await;
            }
            let (_, Json(jobs)) = super::services::deploy(State(state.clone()), caller.clone(), Path(id)).await?;
            Ok(jobs.into_iter().map(|j| j.id).collect())
        }
        WebhookTarget::Stack { id } => {
            if tag.is_some() {
                return Err(crate::error::ApiError::bad_request("a stack webhook cannot change a tag"));
            }
            let input = super::stacks::DeployInput { revision: None, services: Vec::new(), pull: true };
            let (_, Json(jobs)) =
                super::stacks::deploy(State(state.clone()), caller.clone(), Path(id), Json(input)).await?;
            Ok(jobs.into_iter().map(|j| j.id).collect())
        }
    }
}

pub async fn job(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>,
    headers: HeaderMap,
    Path((token, job)): Path<(String, JobId)>,
) -> Response {
    let ip = crate::throttle::client_ip(&headers, peer, &state.config.trusted_proxies());
    if !state.webhooks.allow_ip(ip) {
        return answer(StatusCode::TOO_MANY_REQUESTS, "too many requests");
    }
    let Ok(Some(w)) = state.webhooks.by_token(&token).await else {
        return answer(StatusCode::NOT_FOUND, "no such webhook");
    };
    match state.webhooks.job_status(&w, job).await {
        Ok(v) => Json(v).into_response(),
        Err(_) => answer(StatusCode::NOT_FOUND, "no such job for this webhook"),
    }
}
