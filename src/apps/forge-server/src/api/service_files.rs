//! Services files (D-178): Compose / Swarm format YAML run by Forge.
//!
//! * `POST /api/v1/services/plan` — what applying the file would change
//! * `POST /api/v1/services/apply` — apply it
//! * `POST /api/v1/services/{id}/scale` — `{replicas}`: change the node count
//! * `GET /api/v1/services/{id}/export` — one service as a services file
//! * `GET /api/v1/environments/{id}/services/export` — every service of an environment
//!
//! `deploy` on the environment is needed to plan and apply; applying also
//! needs `server.manage` on every node a service is added to and
//! `stack.privileged` for privileged settings and mounts. The work is in
//! [`crate::service_files::ServiceFiles`].

use axum::Json;
use axum::extract::{Path, State};
use forge_core::{Action, EntityKind, EntityRef, EnvironmentId, ServerId, ServiceId};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::services::{ns_ref, svc_ref};
use crate::auth::Caller;
use crate::error::ApiResult;
use crate::service_files::{AUDIT_APPLY, AUDIT_PLAN, AUDIT_SCALE, Applied, Applier, FileRequest, ServicesPlan, counts};
use crate::state::AppState;

/// A services file and the environment it is for.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct FileInput {
    #[schemars(with = "String")]
    pub environment_id: EnvironmentId,
    /// The file's YAML.
    pub file: String,
    /// Remove the services this file applied before and no longer lists
    /// (default `true`). The console's single-service editor sends `false`.
    #[serde(default = "yes")]
    pub prune: bool,
}

const fn yes() -> bool {
    true
}

/// A new node count.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ScaleInput {
    /// Nodes the service runs on (one copy per node), 1 to 1000.
    pub replicas: u32,
}

/// A services file.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct ExportView {
    pub yaml: String,
}

/// Plans a services file: what would be created, changed, moved and removed.
pub async fn plan(
    State(state): State<AppState>,
    caller: Caller,
    Json(input): Json<FileInput>,
) -> ApiResult<Json<ServicesPlan>> {
    caller.authorize(Action::Deploy, ns_ref(input.environment_id)).await?;
    let req = FileRequest { environment: input.environment_id, text: &input.file, prune: input.prune };
    let plan = state.service_files.plan(req, &caller.principal()).await?;
    let details = json!({ "source": plan.source, "changes": counts(&plan) });
    caller.audit(AUDIT_PLAN, &ns_ref(plan.environment_id), Some(plan.workspace_id), details).await;

    Ok(Json(plan))
}

/// Applies a services file.
pub async fn apply(
    State(state): State<AppState>,
    caller: Caller,
    Json(input): Json<FileInput>,
) -> ApiResult<Json<Applied>> {
    caller.authorize(Action::Deploy, ns_ref(input.environment_id)).await?;
    let req = FileRequest { environment: input.environment_id, text: &input.file, prune: input.prune };
    let plan = state.service_files.plan(req, &caller.principal()).await?;
    authorize_plan(&caller, &plan).await?;
    super::vault::authorize_item_names(&state, &caller, plan.workspace_id, &plan.vault_items, AUDIT_APPLY).await?;

    run(&state, &caller, plan, AUDIT_APPLY).await
}

/// Changes the number of nodes a service runs on and deploys it on the new ones.
pub async fn scale(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServiceId>,
    Json(input): Json<ScaleInput>,
) -> ApiResult<Json<Applied>> {
    caller.authorize(Action::Deploy, svc_ref(id)).await?;
    let plan = state.service_files.plan_scale(id, input.replicas).await?;
    caller.authorize(Action::Deploy, ns_ref(plan.environment_id)).await?;
    authorize_plan(&caller, &plan).await?;

    run(&state, &caller, plan, AUDIT_SCALE).await
}

/// One service as a services file.
pub async fn export_service(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServiceId>,
) -> ApiResult<Json<ExportView>> {
    caller.authorize(Action::View, svc_ref(id)).await?;
    let (_, yaml) = state.service_files.export_service(id).await?;

    Ok(Json(ExportView { yaml }))
}

/// Every container and native service of an environment as a services file.
pub async fn export_environment(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<EnvironmentId>,
) -> ApiResult<Json<ExportView>> {
    caller.authorize(Action::View, ns_ref(id)).await?;
    let yaml = state.service_files.export_environment(id).await?;

    Ok(Json(ExportView { yaml }))
}

/// `server.manage` on every node a service is added to, and
/// `stack.privileged` for privileged settings or mounts.
async fn authorize_plan(caller: &Caller, plan: &ServicesPlan) -> ApiResult<()> {
    let mut nodes: Vec<ServerId> = plan.added_nodes.clone();
    nodes.sort_unstable();
    nodes.dedup();

    for n in nodes {
        caller.authorize(Action::ServerManage, EntityRef::new(EntityKind::Server, n)).await?;
    }

    if plan.needs_privileged {
        caller.authorize(Action::StackPrivileged, ns_ref(plan.environment_id)).await?;
    }

    Ok(())
}

async fn run(state: &AppState, caller: &Caller, plan: ServicesPlan, audit: &str) -> ApiResult<Json<Applied>> {
    let env = plan.environment_id;
    let ws = plan.workspace_id;
    let who = Applier { principal: caller.principal(), context: caller.ctx() };
    let applied = state.service_files.apply(plan, &who).await?;
    let changes: Vec<_> = applied
        .plan
        .changes
        .iter()
        .map(
            |c| json!({ "service": c.service, "action": c.action, "added": c.added_nodes, "removed": c.removed_nodes }),
        )
        .collect();
    let jobs: Vec<_> = applied.jobs.iter().map(|j| j.id).collect();
    let details = json!({ "source": applied.plan.source, "changes": changes, "jobs": jobs });
    caller.audit(audit, &ns_ref(env), Some(ws), details).await;

    Ok(Json(applied))
}
