//! `forge.stack.yaml` (`plans/native-mesh.md` 3.2, D-174).
//!
//! * `POST /api/v1/stacks/plan` — what applying the document would do; no
//!   side effects besides the `stack.plan` audit entry.
//! * `POST /api/v1/stacks/apply` — create or update the stack (a revision
//!   noted `apply <sha256 of the document>[:12]`), then start a
//!   `stack.rollout` job.
//!
//! Both need `stack.manage` on the environment and `server.manage` on every
//! node the stack runs on, as the stack editor does; host-access settings
//! also need `stack.privileged`. The server never reads files: the CLI sends
//! the compose file's and the `.env` file's text.

use std::collections::BTreeMap;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use forge_catalog::stack_doc::{self, MAX_HEALTH_TIMEOUT, MIN_HEALTH_TIMEOUT, RolloutOrder, StackDoc};
use forge_catalog::stacks as cat;
use forge_core::model::{Environment, JobFilter, JobStatus, NewJob};
use forge_core::ports::{ClusterRepository, JobRepository, ServerRepository, TenancyRepository};
use forge_core::stacks::{Stack, StackKind, StackRepository};
use forge_core::{Action, EntityKind, EntityRef, ServerId, StackId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::stacks::{
    CreateInput, UpdateInput, check_dir, create_stack, deps, env_input, ns_ref, plan_for, sha256, update_stack,
};
use crate::auth::{Caller, Ws};
use crate::error::{ApiError, ApiResult};
use crate::jobs::stack_rollout::{KIND as ROLLOUT_KIND, RolloutPayload};
use crate::stacks::{Plan, open_env, preview};
use crate::state::AppState;

/// Audit action of a plan.
const AUDIT_PLAN: &str = "stack.plan";
/// Audit action of the rollout start (the existing deploy action).
const AUDIT_DEPLOY: &str = "stack.deploy";
/// Most active jobs looked at when checking for a running rollout.
const ACTIVE_SCAN: u32 = 500;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanInput {
    /// The `forge.stack.yaml` text.
    pub document: String,
    /// The compose file's text (`spec.compose.file`); omitted with
    /// `spec.compose.inline`.
    #[serde(default)]
    pub compose: Option<String>,
    /// The `.env` file's text (`spec.compose.envFile`); omitted keeps the
    /// stored variables of an existing stack.
    #[serde(default)]
    pub env: Option<String>,
    /// Apply to this environment instead of `metadata.environment`.
    #[serde(default)]
    pub environment: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplyInput {
    pub document: String,
    #[serde(default)]
    pub compose: Option<String>,
    #[serde(default)]
    pub env: Option<String>,
    #[serde(default)]
    pub environment: Option<String>,
    /// Overrides `spec.rollout.order` (`node-by-node` or `all-at-once`).
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub order: Option<RolloutOrder>,
    /// Overrides `spec.rollout.healthTimeout`, in seconds (10 to 3600).
    #[serde(default)]
    pub health_timeout_s: Option<u64>,
}

impl ApplyInput {
    fn plan_input(&self) -> PlanInput {
        PlanInput {
            document: self.document.clone(),
            compose: self.compose.clone(),
            env: self.env.clone(),
            environment: self.environment.clone(),
        }
    }
}

/// Whether the plan creates or updates the stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanAction {
    Create,
    Update,
}

/// The cluster container network across the plan's nodes.
#[derive(Debug, Serialize)]
pub struct MeshView {
    pub enabled: bool,
    pub nodes_in_sync: Vec<String>,
    pub nodes_not_in_sync: Vec<String>,
}

/// What `apply` would do.
#[derive(Debug, Serialize)]
pub struct PlanView {
    pub action: PlanAction,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stack_id: Option<StackId>,
    pub stack: String,
    pub environment_id: forge_core::EnvironmentId,
    pub environment: String,
    pub order: RolloutOrder,
    pub health_timeout_s: u64,
    pub revision_next: i64,
    /// The nodes in rollout order.
    pub nodes: Vec<crate::stacks::NodePlan>,
    /// Nodes of the current revision that would be taken down.
    pub removes: Vec<ServerId>,
    pub mesh: MeshView,
    pub findings: Vec<cat::Finding>,
    pub privileged: bool,
    pub blocked: bool,
    /// The stack editor's preview (services, ports, mesh per service).
    pub preview: Value,
}

/// A document read and planned, nothing saved.
struct Prepared {
    doc: StackDoc,
    environment: Environment,
    existing: Option<Stack>,
    /// The compose text with the document's top-level `x-forge` added.
    compose: String,
    /// `.env` variables from the request; `None` keeps the stored ones.
    env_text: Option<String>,
    env: BTreeMap<String, String>,
    default_server: ServerId,
    plan: Plan,
    /// Node ids in rollout order.
    order: Vec<ServerId>,
    removes: Vec<ServerId>,
}

fn invalid(e: impl std::fmt::Display) -> ApiError {
    ApiError::bad_request(e.to_string())
}

async fn find_environment(state: &AppState, ws: forge_core::WorkspaceId, slug: &str) -> ApiResult<Environment> {
    state
        .store
        .list_environments(ws, None)
        .await?
        .into_iter()
        .find(|n| crate::names::environment_slug(&n.name) == slug)
        .ok_or_else(|| ApiError::not_found(format!("metadata.environment: no environment `{slug}`")))
}

/// The stack's default node: the first of `placement.nodes`, the first
/// node of `placement.cluster`, or the existing stack's.
async fn default_server(
    state: &AppState,
    ws: forge_core::WorkspaceId,
    doc: &StackDoc,
    existing: Option<&Stack>,
) -> ApiResult<ServerId> {
    let p = &doc.spec.placement;

    if let Some(first) = p.nodes.first() {
        let all = state.store.list_servers(Some(ws)).await?;
        let s = crate::stacks::resolve_node(&deps(state), ws, &all, first)
            .await
            .map_err(|e| ApiError::bad_request(format!("spec.placement.nodes: {e}")))?;

        return Ok(s.id);
    }

    if let Some(c) = &p.cluster {
        let cluster = state
            .store
            .list_clusters(ws)
            .await?
            .into_iter()
            .find(|x| x.name == *c)
            .ok_or_else(|| ApiError::bad_request(format!("spec.placement.cluster: no cluster named `{c}`")))?;
        let mut nodes: Vec<_> = state
            .store
            .list_servers(Some(ws))
            .await?
            .into_iter()
            .filter(|s| s.cluster_id == Some(cluster.id))
            .collect();
        nodes.sort_by(|a, b| a.hostname.cmp(&b.hostname));

        return nodes
            .first()
            .map(|s| s.id)
            .ok_or_else(|| ApiError::bad_request(format!("spec.placement.cluster: cluster `{c}` has no nodes")));
    }

    existing
        .map(|s| s.default_server)
        .ok_or_else(|| ApiError::bad_request("spec.placement: name the nodes or the cluster the stack runs on"))
}

/// Node ids in rollout order: the default node first, then the others in
/// `placement.nodes` order, then the rest by name.
fn rollout_order(plan: &Plan, default: ServerId, doc: &StackDoc) -> Vec<ServerId> {
    let rank = |n: &crate::stacks::NodePlan| {
        let listed = doc.spec.placement.nodes.iter().position(|x| *x == n.hostname || *x == n.server_id.to_string());
        (n.server_id != default, listed.unwrap_or(usize::MAX), n.hostname.clone())
    };
    let mut nodes: Vec<&crate::stacks::NodePlan> = plan.nodes.iter().collect();
    nodes.sort_by_key(|n| rank(n));

    nodes.into_iter().map(|n| n.server_id).collect()
}

/// Reads the document, resolves where it runs and plans it. Authorizes
/// `stack.manage` on the environment and `server.manage` on every node.
async fn prepare(
    state: &AppState,
    caller: &Caller,
    ws: forge_core::WorkspaceId,
    input: PlanInput,
) -> ApiResult<Prepared> {
    let yaml = crate::deploy::parse_yaml(&input.document).map_err(ApiError::bad_request)?;
    let doc = stack_doc::parse(yaml, input.environment.as_deref()).map_err(invalid)?;
    let environment = find_environment(state, ws, &doc.metadata.environment).await?;
    caller.authorize(Action::StackManage, ns_ref(environment.id)).await?;

    // A Swarm cluster's stack is one `docker stack deploy` (D-171), not a
    // per-node rollout.
    if state.swarm.swarm_of_environment(environment.id).await?.is_some() {
        return Err(ApiError::bad_request(
            "this environment runs on a Swarm cluster; deploy its stacks with `forge swarm stacks`",
        ));
    }

    let mut compose = input
        .compose
        .or_else(|| doc.spec.compose.inline.clone())
        .ok_or_else(|| ApiError::bad_request("spec.compose.file: send the compose file's text as `compose`"))?;
    let tree = crate::stacks::parse_compose(&compose).map_err(|e| ApiError::bad_request(format!("compose: {e}")))?;
    doc.check_compose(&tree).map_err(invalid)?;

    if let Some(extra) = doc.compose_overlay(&tree).map_err(invalid)? {
        compose.push_str(&extra);
    }

    let existing = state.store.stack_by_name(environment.id, &doc.metadata.name).await?;
    let default = default_server(state, ws, &doc, existing.as_ref()).await?;

    let env = match (env_input(input.env.as_deref())?, &existing) {
        (Some(e), _) => e,
        (None, Some(s)) => open_env(&state.envelope, s.id, s.env_sealed.as_deref())
            .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", e))?,
        (None, None) => BTreeMap::new(),
    };

    let ns = super::stacks::ns_name(state, environment.id).await?;
    let dir = check_dir(existing.as_ref().and_then(|s| s.dir.as_deref()))?;
    let plan = plan_for(state, ws, default, &ns, &doc.metadata.name, dir.as_deref(), &compose, &env).await?;

    for n in &plan.nodes {
        caller.authorize(Action::ServerManage, EntityRef::new(EntityKind::Server, n.server_id)).await?;
    }

    let removes = match &existing {
        Some(s) => state
            .store
            .list_nodes(s.id)
            .await?
            .into_iter()
            .map(|n| n.server_id)
            .filter(|id| plan.node(*id).is_none())
            .collect(),
        None => Vec::new(),
    };

    let order = rollout_order(&plan, default, &doc);

    Ok(Prepared {
        doc,
        environment,
        existing,
        compose,
        env_text: input.env,
        env,
        default_server: default,
        plan,
        order,
        removes,
    })
}

/// The revision an apply would save: the next one when the content changes.
async fn revision_next(state: &AppState, p: &Prepared) -> ApiResult<i64> {
    let Some(s) = &p.existing else { return Ok(1) };
    let current = state.store.get_revision(s.id, s.revision).await?;

    Ok(if current.is_some_and(|r| r.content == p.compose) { s.revision } else { s.revision + 1 })
}

fn view(p: &Prepared, revision_next: i64, order: RolloutOrder, health_timeout_s: u64) -> PlanView {
    let ordered: Vec<crate::stacks::NodePlan> = p.order.iter().filter_map(|id| p.plan.node(*id).cloned()).collect();
    let (in_sync, not): (Vec<_>, Vec<_>) = ordered.iter().partition(|n| n.mesh_network.is_some());

    PlanView {
        action: if p.existing.is_some() { PlanAction::Update } else { PlanAction::Create },
        stack_id: p.existing.as_ref().map(|s| s.id),
        stack: p.doc.metadata.name.clone(),
        environment_id: p.environment.id,
        environment: p.doc.metadata.environment.clone(),
        order,
        health_timeout_s,
        revision_next,
        mesh: MeshView {
            enabled: p.doc.spec.network.mesh,
            nodes_in_sync: in_sync.iter().map(|n| n.hostname.clone()).collect(),
            nodes_not_in_sync: not.iter().map(|n| n.hostname.clone()).collect(),
        },
        nodes: ordered,
        removes: p.removes.clone(),
        findings: p.plan.report.findings.clone(),
        privileged: p.plan.report.privileged,
        blocked: p.plan.report.findings.iter().any(|f| f.severity == cat::Severity::Blocked),
        preview: preview(&p.plan),
    }
}

/// `POST /api/v1/stacks/plan`: what `apply` would do. Saves nothing.
pub async fn plan(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Json(input): Json<PlanInput>,
) -> ApiResult<Json<PlanView>> {
    let p = prepare(&state, &caller, ws, input).await?;
    let next = revision_next(&state, &p).await?;
    let v = view(&p, next, p.doc.spec.rollout.order, p.doc.health_timeout_s());
    caller
        .audit(
            AUDIT_PLAN,
            &ns_ref(p.environment.id),
            Some(p.environment.workspace_id),
            json!({
                "stack": v.stack, "action": v.action, "revision_next": next,
                "nodes": v.nodes.iter().map(|n| &n.hostname).collect::<Vec<_>>(),
                "blocked": v.blocked, "privileged": v.privileged,
            }),
        )
        .await;

    Ok(Json(v))
}

/// What `apply` started.
#[derive(Debug, Serialize)]
pub struct Applied {
    pub job_id: forge_core::JobId,
    pub stack_id: StackId,
    pub revision: i64,
    pub plan: PlanView,
}

/// A queued or running rollout of `stack`, if any.
async fn active_rollout(state: &AppState, stack: StackId) -> ApiResult<Option<forge_core::JobId>> {
    for status in [JobStatus::Queued, JobStatus::Running] {
        let filter = JobFilter { workspace_id: None, status: Some(status), limit: ACTIVE_SCAN };
        let found = state
            .store
            .list_jobs(&filter)
            .await?
            .into_iter()
            .find(|j| j.kind == ROLLOUT_KIND && j.payload["stack_id"] == json!(stack));

        if let Some(j) = found {
            return Ok(Some(j.id));
        }
    }

    Ok(None)
}

/// `POST /api/v1/stacks/apply`: create or update the stack from the
/// document and start its rollout.
pub async fn apply(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Json(input): Json<ApplyInput>,
) -> ApiResult<(StatusCode, Json<Applied>)> {
    let p = prepare(&state, &caller, ws, input.plan_input()).await?;
    let order = input.order.unwrap_or(p.doc.spec.rollout.order);
    let timeout = input.health_timeout_s.unwrap_or_else(|| p.doc.health_timeout_s());

    if !(MIN_HEALTH_TIMEOUT..=MAX_HEALTH_TIMEOUT).contains(&timeout) {
        return Err(ApiError::bad_request("health_timeout_s: between 10 and 3600"));
    }

    p.plan.report.ensure(true).map_err(ApiError::bad_request)?;

    if p.plan.report.privileged {
        caller.authorize(Action::StackPrivileged, ns_ref(p.environment.id)).await?;
    }

    if let Some(s) = &p.existing
        && let Some(job) = active_rollout(&state, s.id).await?
    {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "rollout_running",
            format!("a rollout of this stack is already running (job {job})"),
        ));
    }

    // Saved first, then the rollout: a failed submit leaves a saved
    // revision that `forge stacks deploy` or another apply rolls out (the
    // save and the start are not one transaction; nothing to undo).
    let next = revision_next(&state, &p).await?;
    let note = format!("apply {}", &sha256(&input.document)[..12]);

    let stack = match &p.existing {
        Some(s) => {
            let u = UpdateInput {
                content: Some(p.compose.clone()),
                env: p.env_text.clone(),
                dir: None,
                note: Some(note),
                version: None,
            };
            update_stack(&state, &caller, s.id, u, Some(p.default_server)).await?.0
        }
        None => {
            let c = CreateInput {
                environment_id: p.environment.id,
                server_id: p.default_server,
                name: p.doc.metadata.name.clone(),
                content: p.compose.clone(),
                env: None,
                dir: None,
                note: Some(note),
            };
            create_stack(&state, &caller, c, StackKind::Compose, p.env.clone()).await?.0
        }
    };

    let payload = RolloutPayload {
        stack_id: stack.id,
        revision: stack.revision,
        nodes: p.order.clone(),
        remove: p.removes.clone(),
        order,
        health_timeout_s: timeout,
        privileged: p.plan.report.privileged,
        index: 0,
    };
    let job = state
        .jobs
        .submit(NewJob {
            kind: ROLLOUT_KIND.into(),
            workspace_id: Some(stack.workspace_id),
            resource: ns_ref(stack.environment_id),
            principal: caller.principal(),
            context: caller.ctx(),
            idempotency_key: None,
            payload: serde_json::to_value(&payload).map_err(|e| ApiError::bad_request(e.to_string()))?,
            max_attempts: 3,
        })
        .await?;
    let v = view(&p, next, order, timeout);
    caller
        .audit(
            AUDIT_DEPLOY,
            &ns_ref(stack.environment_id),
            Some(stack.workspace_id),
            json!({
                "stack": stack.name, "revision": stack.revision, "rollout_job": job.id, "order": order,
                "health_timeout_s": timeout, "document_sha256": sha256(&input.document),
                "nodes": v.nodes.iter().map(|n| &n.hostname).collect::<Vec<_>>(),
                "removes": p.removes, "privileged": p.plan.report.privileged,
            }),
        )
        .await;

    Ok((StatusCode::ACCEPTED, Json(Applied { job_id: job.id, stack_id: stack.id, revision: stack.revision, plan: v })))
}
