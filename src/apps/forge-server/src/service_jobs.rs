//! The `app.action` job for one service instance, built in one place for
//! the API and the service reconciler (D-177).

use forge_core::model::{NewJob, Service};
use forge_core::{AuthzContext, EntityKind, EntityRef, Principal, ServerId};
use serde_json::json;

/// Job kind of an action on a service instance.
pub const APP_ACTION_KIND: &str = "app.action";

/// One action on one instance of a service.
#[derive(Debug, Clone)]
pub struct AppActionRequest<'a> {
    pub app: &'a Service,
    pub server: ServerId,
    /// `deploy`, `start`, `stop`, `restart`, `remove`, `prepare`, `status`,
    /// `logs` or `follow`.
    pub action: &'a str,
    pub tail: u32,
    /// `follow`: where the previous segment stopped.
    pub cursor: &'a str,
    /// Volumes outside the app folder and approved roots the submitter was
    /// authorized for (`stack.privileged`).
    pub privileged_mounts: &'a [String],
    pub principal: Principal,
    pub context: AuthzContext,
    pub idempotency_key: Option<String>,
}

/// The job for `req`. Its resource is the service itself, so a grant on one
/// service is enough (#13); privileged mounts need `stack.privileged`, held
/// per environment, so those jobs stay on the environment.
#[must_use]
pub fn app_action_job(req: AppActionRequest<'_>) -> NewJob {
    let resource = if req.privileged_mounts.is_empty() {
        EntityRef::new(EntityKind::Service, req.app.id)
    } else {
        EntityRef::new(EntityKind::Environment, req.app.environment_id)
    };

    NewJob {
        kind: APP_ACTION_KIND.into(),
        workspace_id: Some(req.app.workspace_id),
        resource,
        principal: req.principal,
        context: req.context,
        idempotency_key: req.idempotency_key,
        payload: json!({
            "service_id": req.app.id,
            "server_id": req.server,
            "action": req.action,
            "tail": req.tail,
            "cursor": req.cursor,
            "privileged_mounts": req.privileged_mounts,
        }),
        max_attempts: 2,
    }
}
