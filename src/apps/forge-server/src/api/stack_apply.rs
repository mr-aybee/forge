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
use forge_core::model::{Environment, NewJob};
use forge_core::stacks::{Stack, StackKind};
use forge_core::{Action, DomainError, EntityKind, EntityRef, ServerId, StackId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::stacks::{
    CreateInput, UpdateInput, check_dir, create_stack, deps, env_input, ns_ref, plan_for, sha256, update_stack,
};
use crate::auth::{Caller, Ws};
use crate::error::{ApiError, ApiResult};
use crate::jobs::stack_rollout::{KIND as ROLLOUT_KIND, RolloutPayload};
use crate::stack_apply::StackApplyPorts;
use crate::stacks::{Plan, open_env, preview};
use crate::state::AppState;

/// Audit action of a plan.
const AUDIT_PLAN: &str = "stack.plan";
/// Audit action of the rollout start (the existing deploy action).
const AUDIT_DEPLOY: &str = "stack.deploy";

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

/// The service over this server's repositories.
fn ports(state: &AppState) -> StackApplyPorts<'_> {
    StackApplyPorts { tenancy: &state.store, stacks: &state.store, jobs: &state.store, deps: deps(state) }
}

/// A placement problem is the caller's (400), like the document's other
/// errors; anything else maps as usual.
fn placement(e: DomainError) -> ApiError {
    match e {
        DomainError::Invalid(m) => ApiError::bad_request(m),
        other => other.into(),
    }
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
    let svc = ports(state);
    let environment = svc.environment(ws, &doc.metadata.environment).await?;
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

    let existing = svc.existing(&environment, &doc.metadata.name).await?;
    let default = svc.default_server(ws, &doc, existing.as_ref()).await.map_err(placement)?;

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

    let removes = svc.removes(existing.as_ref(), &plan).await?;

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
    let next = ports(&state).revision_next(&state.envelope, p.existing.as_ref(), &p.compose, &p.env).await?;
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
        && let Some(job) = ports(&state).active_rollout(ROLLOUT_KIND, s.id).await?
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
    let next = ports(&state).revision_next(&state.envelope, p.existing.as_ref(), &p.compose, &p.env).await?;
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
