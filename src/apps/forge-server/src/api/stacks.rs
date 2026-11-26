//! Compose stacks and **Create container** (issue #7).
//!
//! * `GET /api/v1/stacks?environment_id=` · `POST /api/v1/stacks` (create)
//! * `POST /api/v1/stacks/preview` — parse, place and check without saving
//! * `POST /api/v1/stacks/containers` — the **Create container** form (a one-service stack)
//! * `GET/PUT/DELETE /api/v1/stacks/{id}` · `GET /api/v1/stacks/{id}/revisions[/{n}]`
//! * `POST /api/v1/stacks/{id}/deploy` `{revision?, services?, pull?}` (also rollback)
//! * `POST /api/v1/stacks/{id}/services` `{server_id, action, services?}`
//! * `POST /api/v1/stacks/{id}/refresh` · `GET /api/v1/stacks/{id}/logs?server_id=&service=&tail=`
//! * `POST /api/v1/stacks/{id}/down` `{volumes}` · `POST /api/v1/stacks/{id}/expose`
//!
//! `stack.manage` on the environment changes stacks; stacks with host access
//! (privileged settings, host network, Docker socket, binds outside approved
//! roots) also need `stack.privileged` (administrators, MFA). `.env` values
//! are sealed at rest and never returned; reads need `view`.

use std::collections::BTreeMap;
use std::time::Duration;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use forge_catalog::stacks::{self as cat, parse_env, valid_stack_name};
use forge_core::model::{Job, JobStatus, NewJob};
use forge_core::ports::{JobRepository, ServerRepository, TenancyRepository};
use forge_core::stacks::{Stack, StackKind, StackNode, StackRepository, StackRevision};
use forge_core::{Action, EntityKind, EntityRef, EnvironmentId, ServerId, StackId, Timestamp};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::auth::{Caller, Ws};
use crate::error::{ApiError, ApiResult};
use crate::stacks::{ContainerForm, Deps, Plan, ns_slug, open_env, plan, preview, seal_env};
use crate::state::AppState;

fn internal(e: impl std::fmt::Display) -> ApiError {
    ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", e.to_string())
}

pub(super) fn ns_ref(id: EnvironmentId) -> EntityRef {
    EntityRef::new(EntityKind::Environment, id)
}

fn sha256(s: &str) -> String {
    hex::encode(ring::digest::digest(&ring::digest::SHA256, s.as_bytes()))
}

fn deps(state: &AppState) -> Deps<'_> {
    Deps {
        servers: &state.store,
        clusters: &state.store,
        fabrics: &state.store,
        files: &state.store,
        facts: &state.store,
    }
}

pub(super) async fn load(state: &AppState, id: StackId) -> ApiResult<Stack> {
    state.store.get_stack(id).await?.ok_or_else(|| ApiError::not_found("stack not found"))
}

async fn ns_name(state: &AppState, id: EnvironmentId) -> ApiResult<String> {
    let ns = state.store.get_environment(id).await?;
    let slug = ns_slug(&ns.name);
    if !valid_stack_name(&slug) {
        return Err(ApiError::bad_request(format!("environment `{}` has no usable short name", ns.name)));
    }

    Ok(slug)
}

/// `.env` text → variables (or `None` to keep the stored ones).
fn env_input(text: Option<&str>) -> ApiResult<Option<BTreeMap<String, String>>> {
    text.map(|t| parse_env(t).map_err(ApiError::bad_request)).transpose()
}

#[allow(clippy::too_many_arguments)]
async fn plan_for(
    state: &AppState,
    ws: forge_core::WorkspaceId,
    server: ServerId,
    ns: &str,
    name: &str,
    dir: Option<&str>,
    content: &str,
    env: &BTreeMap<String, String>,
) -> ApiResult<Plan> {
    plan(&deps(state), ws, server, ns, name, dir, content, env, true).await.map_err(ApiError::bad_request)
}

/// The plan of a stack in a Swarm cluster's environment: no per-node split,
/// the swarm policy (D-171). `None` for other environments.
async fn swarm_plan(
    state: &AppState,
    caller: &Caller,
    environment: EnvironmentId,
    name: &str,
    content: &str,
    env: &BTreeMap<String, String>,
) -> ApiResult<Option<Plan>> {
    if state.swarm.swarm_of_environment(environment).await?.is_none() {
        return Ok(None);
    }

    let draft = crate::swarm_stacks::Draft { environment, name, content, env: env.clone() };
    let p = state.swarm_stacks.prepare_draft(draft, true, &caller.principal().to_string()).await?;
    let tree = crate::stacks::parse_compose(content).map_err(ApiError::bad_request)?;

    Ok(Some(Plan { tree, interpolated: p.part.compose, nodes: Vec::new(), report: p.report }))
}

fn check_dir(dir: Option<&str>) -> ApiResult<Option<String>> {
    match dir.map(str::trim).filter(|d| !d.is_empty()) {
        None => Ok(None),
        Some(d) => cat::normalize_abs(d)
            .map(Some)
            .ok_or_else(|| ApiError::bad_request("the folder must be an absolute path without `..`")),
    }
}

// ---- reads ----------------------------------------------------------------------

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListQuery {
    #[schemars(with = "Option<String>")]
    pub environment_id: Option<EnvironmentId>,
    #[schemars(with = "Option<String>")]
    pub server_id: Option<ServerId>,
}

/// Stacks with their node parts. Only environments the caller can view.
pub async fn list(
    State(state): State<AppState>,
    caller: Caller,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Vec<Value>>> {
    let stacks = state.store.list_stacks(q.environment_id).await?;
    let mut out = Vec::new();

    for s in stacks {
        if !caller.can(Action::View, ns_ref(s.environment_id)).await {
            continue;
        }

        let nodes = state.store.list_nodes(s.id).await?;

        if let Some(server) = q.server_id
            && !nodes.iter().any(|n| n.server_id == server)
            && s.default_server != server
        {
            continue;
        }

        out.push(json!({ "stack": s, "nodes": nodes }));
    }

    Ok(Json(out))
}

pub async fn get(State(state): State<AppState>, caller: Caller, Path(id): Path<StackId>) -> ApiResult<Json<Value>> {
    let stack = load(&state, id).await?;
    caller.authorize(Action::View, ns_ref(stack.environment_id)).await?;
    let rev =
        state.store.get_revision(id, stack.revision).await?.ok_or_else(|| ApiError::not_found("revision missing"))?;
    let ns = ns_name(&state, stack.environment_id).await?;
    let env = open_env(&state.envelope, id, stack.env_sealed.as_deref()).map_err(internal)?;

    // A stack whose nodes or placement changed still opens; the preview says why.
    let p = plan(
        &deps(&state),
        stack.workspace_id,
        stack.default_server,
        &ns,
        &stack.name,
        stack.dir.as_deref(),
        &rev.content,
        &env,
        true,
    )
    .await
    .map_or_else(|e| json!({ "error": e }), |p| preview(&p));
    let nodes = state.store.list_nodes(id).await?;

    Ok(Json(json!({
        "stack": stack,
        "content": rev.content,
        "dir": crate::stacks::stack_dir(stack.dir.as_deref(), &ns, &stack.name),
        "nodes": nodes,
        "preview": p,
    })))
}

pub async fn revisions(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<StackId>,
) -> ApiResult<Json<Vec<StackRevision>>> {
    let stack = load(&state, id).await?;
    caller.authorize(Action::View, ns_ref(stack.environment_id)).await?;

    Ok(Json(state.store.list_revisions(id).await?))
}

pub async fn revision(
    State(state): State<AppState>,
    caller: Caller,
    Path((id, n)): Path<(StackId, i64)>,
) -> ApiResult<Json<StackRevision>> {
    let stack = load(&state, id).await?;
    caller.authorize(Action::View, ns_ref(stack.environment_id)).await?;

    Ok(Json(state.store.get_revision(id, n).await?.ok_or_else(|| ApiError::not_found("revision not found"))?))
}

// ---- preview and create -----------------------------------------------------------

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PreviewInput {
    #[schemars(with = "String")]
    pub environment_id: EnvironmentId,
    /// The node the stack is created from (services without placement run here).
    #[schemars(with = "String")]
    pub server_id: ServerId,
    #[serde(default)]
    pub name: Option<String>,
    pub content: String,
    /// `.env` file content.
    #[serde(default)]
    pub env: Option<String>,
    #[serde(default)]
    pub dir: Option<String>,
}

pub async fn preview_stack(
    State(state): State<AppState>,
    caller: Caller,
    Json(input): Json<PreviewInput>,
) -> ApiResult<Json<Value>> {
    caller.authorize(Action::StackManage, ns_ref(input.environment_id)).await?;

    // As for create: the node must be in the environment's organization and
    // the caller must manage it (a preview reveals its placement and roots).
    let environment = state.store.get_environment(input.environment_id).await?;
    let server = state.store.get_server(input.server_id).await.map_err(|_| ApiError::not_found("node not found"))?;
    if server.workspace_id != environment.workspace_id {
        return Err(ApiError::bad_request("the node is in another organization"));
    }

    caller.authorize(Action::ServerManage, EntityRef::new(EntityKind::Server, server.id)).await?;
    let ns = ns_name(&state, input.environment_id).await?;
    let env = env_input(input.env.as_deref())?.unwrap_or_default();
    let dir = check_dir(input.dir.as_deref())?;
    let name = input.name.unwrap_or_else(|| "stack".into());
    let p = match swarm_plan(&state, &caller, input.environment_id, &name, &input.content, &env).await? {
        Some(p) => p,
        None => {
            plan_for(&state, server.workspace_id, server.id, &ns, &name, dir.as_deref(), &input.content, &env).await?
        }
    };

    let mut v = preview(&p);
    v["can_privileged"] = json!(caller.can(Action::StackPrivileged, ns_ref(input.environment_id)).await);

    Ok(Json(v))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateInput {
    #[schemars(with = "String")]
    pub environment_id: EnvironmentId,
    #[schemars(with = "String")]
    pub server_id: ServerId,
    pub name: String,
    pub content: String,
    #[serde(default)]
    pub env: Option<String>,
    #[serde(default)]
    pub dir: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

async fn create_stack(
    state: &AppState,
    caller: &Caller,
    input: CreateInput,
    kind: StackKind,
    env: BTreeMap<String, String>,
) -> ApiResult<(Stack, Plan)> {
    caller.authorize(Action::StackManage, ns_ref(input.environment_id)).await?;

    if !valid_stack_name(&input.name) {
        return Err(ApiError::bad_request("stack names use a-z, 0-9, `-` and `_` (up to 63)"));
    }

    let ns = state.store.get_environment(input.environment_id).await?;
    let ns_s = ns_name(state, ns.id).await?;
    let server = state.store.get_server(input.server_id).await.map_err(|_| ApiError::not_found("node not found"))?;
    if server.workspace_id != ns.workspace_id {
        return Err(ApiError::bad_request("the node is in another organization"));
    }

    caller.authorize(Action::ServerManage, EntityRef::new(EntityKind::Server, server.id)).await?;

    if state.store.stack_by_name(ns.id, &input.name).await?.is_some() {
        return Err(ApiError::conflict(format!("a stack named `{}` exists in this environment", input.name)));
    }

    let dir = check_dir(input.dir.as_deref())?;

    let p = match swarm_plan(state, caller, ns.id, &input.name, &input.content, &env).await? {
        Some(p) => p,
        None => {
            plan_for(state, ns.workspace_id, server.id, &ns_s, &input.name, dir.as_deref(), &input.content, &env)
                .await?
        }
    };

    if let Some(f) = p.report.findings.iter().find(|f| f.severity == cat::Severity::Blocked) {
        return Err(ApiError::bad_request(
            cat::Report { findings: vec![f.clone()], privileged: false }.ensure(true).err().unwrap_or_default(),
        ));
    }

    let now = state.clock.now();
    let id = StackId::new();
    let stack = Stack {
        id,
        workspace_id: ns.workspace_id,
        environment_id: ns.id,
        name: input.name.clone(),
        kind,
        default_server: server.id,
        dir,
        revision: 1,
        deployed_revision: 0,
        env_sealed: seal_env(&state.envelope, id, &env).map_err(internal)?,
        env_keys: env.keys().cloned().collect(),
        created_by: caller.principal(),
        created_at: now,
        updated_at: now,
    };
    let rev = StackRevision {
        stack_id: id,
        revision: 1,
        sha256: sha256(&input.content),
        content: input.content,
        note: input.note,
        author: caller.principal(),
        created_at: now,
    };
    state.store.create_stack(&stack, &rev).await?;
    caller
        .audit(
            "stack.create",
            &ns_ref(ns.id),
            Some(ns.workspace_id),
            json!({
                "stack": stack.name, "kind": kind.as_str(), "revision": 1, "sha256": rev.sha256,
                "services": p.services(), "nodes": p.nodes.iter().map(|n| &n.hostname).collect::<Vec<_>>(),
                "privileged": p.report.privileged, "env_keys": stack.env_keys,
            }),
        )
        .await;

    Ok((stack, p))
}

pub async fn create(
    State(state): State<AppState>,
    caller: Caller,
    Json(input): Json<CreateInput>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let env = env_input(input.env.as_deref())?.unwrap_or_default();
    let (stack, p) = create_stack(&state, &caller, input, StackKind::Compose, env).await?;

    Ok((StatusCode::CREATED, Json(json!({ "stack": stack, "preview": preview(&p) }))))
}

/// `GET /stack-templates`: the compose app templates (D-160).
pub async fn templates(
    caller: Caller,
    Ws(ws): Ws,
) -> ApiResult<Json<&'static [forge_catalog::stack_templates::Template]>> {
    caller.authorize(Action::View, EntityRef::new(EntityKind::Workspace, ws)).await?;
    Ok(Json(forge_catalog::stack_templates::TEMPLATES))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FromTemplateInput {
    /// A template id from `GET /stack-templates`.
    pub template: String,
    #[schemars(with = "String")]
    pub environment_id: EnvironmentId,
    #[schemars(with = "String")]
    pub server_id: ServerId,
    pub name: String,
    /// Variable values; empty secrets are generated.
    #[serde(default)]
    pub values: BTreeMap<String, String>,
}

/// `POST /stacks/from-template`: a compose stack from a template, through
/// the same checks as any stack (policy, sealed `.env`, audit).
pub async fn from_template(
    State(state): State<AppState>,
    caller: Caller,
    Json(input): Json<FromTemplateInput>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let t = forge_catalog::stack_templates::find(&input.template)
        .ok_or_else(|| ApiError::bad_request(format!("unknown template `{}`", input.template)))?;
    let env = forge_catalog::stack_templates::render_env(t, &input.values, forge_secrets::crypto::random_token::<24>)
        .map_err(ApiError::bad_request)?;
    let stack_input = CreateInput {
        environment_id: input.environment_id,
        server_id: input.server_id,
        name: input.name,
        content: t.compose.to_owned(),
        env: Some(env),
        dir: None,
        note: Some(format!("from the {} template", t.title)),
    };

    create(State(state), caller, Json(stack_input)).await
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContainerInput {
    #[schemars(with = "String")]
    pub environment_id: EnvironmentId,
    #[schemars(with = "String")]
    pub server_id: ServerId,
    #[serde(default)]
    pub dir: Option<String>,
    #[schemars(with = "serde_json::Value")]
    pub container: ContainerForm,
    /// Start it right away (default).
    #[serde(default = "yes")]
    pub start: bool,
}

const fn yes() -> bool {
    true
}

/// **Create container**: a one-service stack named after the container.
pub async fn create_container(
    State(state): State<AppState>,
    caller: Caller,
    Json(input): Json<ContainerInput>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let (content, env) = input.container.to_compose().map_err(ApiError::bad_request)?;
    let name = input.container.name.to_ascii_lowercase().replace('.', "-");
    let create = CreateInput {
        environment_id: input.environment_id,
        server_id: input.server_id,
        name,
        content,
        env: None,
        dir: input.dir,
        note: Some("Create container".into()),
    };
    let (stack, p) = create_stack(&state, &caller, create, StackKind::Container, env).await?;

    let jobs = if input.start {
        deploy_jobs(&state, &caller, &stack, &p, stack.revision, &[], true).await?
    } else {
        Vec::new()
    };

    Ok((StatusCode::CREATED, Json(json!({ "stack": stack, "preview": preview(&p), "jobs": jobs }))))
}

// ---- update and delete -------------------------------------------------------------

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateInput {
    /// New compose content (a new revision when it differs).
    #[serde(default)]
    pub content: Option<String>,
    /// New `.env` content; omitted keeps the stored variables.
    #[serde(default)]
    pub env: Option<String>,
    #[serde(default)]
    pub dir: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
    /// The stack's `updated_at` when the editor loaded it. A different
    /// value means someone changed the stack since: `409 stale_version`.
    #[serde(default)]
    pub version: Option<i64>,
}

fn stale_version() -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        "stale_version",
        "the stack changed since you loaded it; reload it and apply your change again",
    )
}

pub async fn update(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<StackId>,
    Json(input): Json<UpdateInput>,
) -> ApiResult<Json<Value>> {
    let mut stack = load(&state, id).await?;
    caller.authorize(Action::StackManage, ns_ref(stack.environment_id)).await?;

    if input.version.is_some_and(|v| v != stack.updated_at.0) {
        return Err(stale_version());
    }

    let ns = ns_name(&state, stack.environment_id).await?;
    let current =
        state.store.get_revision(id, stack.revision).await?.ok_or_else(|| ApiError::not_found("revision missing"))?;

    let env = match env_input(input.env.as_deref())? {
        Some(e) => e,
        None => open_env(&state.envelope, id, stack.env_sealed.as_deref()).map_err(internal)?,
    };

    if input.dir.is_some() {
        let deployed = !state.store.list_nodes(id).await?.is_empty();
        let dir = check_dir(input.dir.as_deref())?;
        if deployed && dir != stack.dir {
            return Err(ApiError::conflict("take the stack down before moving its folder"));
        }

        stack.dir = dir;
    }

    let content = input.content.unwrap_or_else(|| current.content.clone());
    let p = match swarm_plan(&state, &caller, stack.environment_id, &stack.name, &content, &env).await? {
        Some(p) => p,
        None => {
            plan_for(
                &state,
                stack.workspace_id,
                stack.default_server,
                &ns,
                &stack.name,
                stack.dir.as_deref(),
                &content,
                &env,
            )
            .await?
        }
    };

    if let Some(f) = p.report.findings.iter().find(|f| f.severity == cat::Severity::Blocked) {
        return Err(ApiError::bad_request(
            cat::Report { findings: vec![f.clone()], privileged: false }.ensure(true).err().unwrap_or_default(),
        ));
    }

    // Claim the edit atomically: of two saves that read the same stack,
    // only one gets past this; the other answers 409. The new timestamp is
    // always later than the one read, so the version changes.
    let now = Timestamp(state.clock.now().0.max(stack.updated_at.0 + 1));
    if !state.store.claim_stack_edit(id, stack.updated_at, now).await? {
        return Err(stale_version());
    }

    let mut new_rev = None;

    if content != current.content {
        let rev = StackRevision {
            stack_id: id,
            revision: 0,
            sha256: sha256(&content),
            content,
            note: input.note.clone(),
            author: caller.principal(),
            created_at: now,
        };
        let n = state.store.add_revision(&rev).await?;
        new_rev = Some((n, rev.sha256));
        stack.revision = n;
    }

    stack.env_sealed = seal_env(&state.envelope, id, &env).map_err(internal)?;
    stack.env_keys = env.keys().cloned().collect();
    stack.updated_at = now;
    state.store.put_stack(&stack).await?;
    caller
        .audit(
            "stack.update",
            &ns_ref(stack.environment_id),
            Some(stack.workspace_id),
            json!({
                "stack": stack.name, "revision": new_rev.as_ref().map(|r| r.0), "sha256": new_rev.as_ref().map(|r| &r.1),
                "env_keys": stack.env_keys, "env_changed": input.env.is_some(), "privileged": p.report.privileged,
            }),
        )
        .await;

    Ok(Json(json!({ "stack": stack, "preview": preview(&p) })))
}

pub async fn delete(State(state): State<AppState>, caller: Caller, Path(id): Path<StackId>) -> ApiResult<StatusCode> {
    let stack = load(&state, id).await?;
    caller.authorize(Action::StackManage, ns_ref(stack.environment_id)).await?;

    if !state.store.list_nodes(id).await?.is_empty() {
        return Err(ApiError::conflict("take the stack down on its nodes first"));
    }

    if stack.deployed_revision > 0 && state.swarm.swarm_of_environment(stack.environment_id).await?.is_some() {
        return Err(ApiError::conflict("remove the stack from the swarm first"));
    }

    state.store.delete_stack(id).await?;
    caller
        .audit("stack.delete", &ns_ref(stack.environment_id), Some(stack.workspace_id), json!({ "stack": stack.name }))
        .await;

    Ok(StatusCode::NO_CONTENT)
}

// ---- operations -------------------------------------------------------------------------

async fn submit(state: &AppState, caller: &Caller, stack: &Stack, payload: Value) -> ApiResult<Job> {
    Ok(state
        .jobs
        .submit(NewJob {
            kind: "stack.action".into(),
            workspace_id: Some(stack.workspace_id),
            resource: ns_ref(stack.environment_id),
            principal: caller.principal(),
            context: caller.ctx(),
            idempotency_key: None,
            payload,
            max_attempts: 2,
        })
        .await?)
}

/// One deploy job per node of the plan that runs a chosen service (all
/// nodes when `services` is empty); nodes that no longer have services get
/// a `down` job.
async fn deploy_jobs(
    state: &AppState,
    caller: &Caller,
    stack: &Stack,
    p: &Plan,
    revision: i64,
    services: &[String],
    pull: bool,
) -> ApiResult<Vec<Job>> {
    if let Some(bad) = services.iter().find(|s| !p.services().contains(*s)) {
        return Err(ApiError::bad_request(format!("unknown service `{bad}`")));
    }

    let privileged = p.report.privileged;
    if privileged {
        // Host-access settings: administrators with MFA only.
        caller.authorize(Action::StackPrivileged, ns_ref(stack.environment_id)).await?;
    }

    let wanted = cat::with_dependencies(&p.interpolated, services);
    let mut jobs = Vec::new();

    for n in &p.nodes {
        if !services.is_empty() && !n.services.iter().any(|s| wanted.contains(s)) {
            continue;
        }

        caller.authorize(Action::ServerManage, EntityRef::new(EntityKind::Server, n.server_id)).await?;
        let payload = json!({
            "stack_id": stack.id, "server_id": n.server_id, "op": "deploy", "revision": revision,
            "services": services, "pull": pull, "privileged": privileged,
        });
        jobs.push(submit(state, caller, stack, payload).await?);
    }

    if services.is_empty() {
        for old in state.store.list_nodes(stack.id).await? {
            if p.node(old.server_id).is_none() {
                let payload =
                    json!({ "stack_id": stack.id, "server_id": old.server_id, "op": "down", "volumes": false });
                jobs.push(submit(state, caller, stack, payload).await?);
            }
        }
    }

    if jobs.is_empty() {
        return Err(ApiError::bad_request("none of the chosen services has a node"));
    }

    Ok(jobs)
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeployInput {
    /// Revision to deploy (default: the latest). An older one rolls back.
    #[serde(default)]
    pub revision: Option<i64>,
    /// Services to start (and their dependencies); empty: all.
    #[serde(default)]
    pub services: Vec<String>,
    /// Pull images even when present (`--pull always`).
    #[serde(default)]
    pub pull: bool,
}

pub async fn deploy(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<StackId>,
    Json(input): Json<DeployInput>,
) -> ApiResult<(StatusCode, Json<Vec<Job>>)> {
    let stack = load(&state, id).await?;
    caller.authorize(Action::StackManage, ns_ref(stack.environment_id)).await?;
    let revision = input.revision.unwrap_or(stack.revision);

    // A stack in a Swarm cluster is one `docker stack deploy` (D-171).
    if state.swarm.swarm_of_environment(stack.environment_id).await?.is_some() {
        let action = crate::jobs::swarm_stack::StackAction::Deploy { revision };
        return Ok((
            StatusCode::ACCEPTED,
            Json(vec![super::swarm_stacks::start(&state, &caller, &stack, action).await?]),
        ));
    }

    let rev = state.store.get_revision(id, revision).await?.ok_or_else(|| ApiError::not_found("revision not found"))?;
    let ns = ns_name(&state, stack.environment_id).await?;
    let env = open_env(&state.envelope, id, stack.env_sealed.as_deref()).map_err(internal)?;
    let p = plan_for(
        &state,
        stack.workspace_id,
        stack.default_server,
        &ns,
        &stack.name,
        stack.dir.as_deref(),
        &rev.content,
        &env,
    )
    .await?;

    if let Err(e) = p.report.ensure(true) {
        return Err(ApiError::bad_request(e));
    }

    let jobs = deploy_jobs(&state, &caller, &stack, &p, revision, &input.services, input.pull).await?;
    caller
        .audit(
            "stack.deploy",
            &ns_ref(stack.environment_id),
            Some(stack.workspace_id),
            json!({
                "stack": stack.name, "revision": revision, "sha256": rev.sha256,
                "rollback": revision < stack.revision, "services": input.services,
                "nodes": p.nodes.iter().map(|n| &n.hostname).collect::<Vec<_>>(),
                "privileged": p.report.privileged,
                "exceptions": p.report.findings.iter().filter(|f| f.severity == cat::Severity::Privileged).map(|f| &f.message).collect::<Vec<_>>(),
            }),
        )
        .await;

    Ok((StatusCode::ACCEPTED, Json(jobs)))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ServiceInput {
    #[schemars(with = "String")]
    pub server_id: ServerId,
    /// `start`, `stop`, `restart` or `pull`.
    pub action: String,
    #[serde(default)]
    pub services: Vec<String>,
}

pub async fn services(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<StackId>,
    Json(input): Json<ServiceInput>,
) -> ApiResult<(StatusCode, Json<Job>)> {
    let stack = load(&state, id).await?;
    caller.authorize(Action::StackManage, ns_ref(stack.environment_id)).await?;

    if !matches!(input.action.as_str(), "start" | "stop" | "restart" | "pull") {
        return Err(ApiError::bad_request("action: start, stop, restart or pull"));
    }

    let node = node_of(&state, id, input.server_id).await?;
    if let Some(bad) = input.services.iter().find(|s| !node.services.contains(*s)) {
        return Err(ApiError::bad_request(format!("`{bad}` does not run on this node")));
    }

    let payload = json!({ "stack_id": id, "server_id": input.server_id, "op": "service", "action": input.action, "services": input.services });
    let job = submit(&state, &caller, &stack, payload).await?;
    caller
        .audit(
            "stack.service",
            &ns_ref(stack.environment_id),
            Some(stack.workspace_id),
            json!({ "stack": stack.name, "server_id": input.server_id, "action": input.action, "services": input.services }),
        )
        .await;

    Ok((StatusCode::ACCEPTED, Json(job)))
}

async fn node_of(state: &AppState, id: StackId, server: ServerId) -> ApiResult<StackNode> {
    state
        .store
        .list_nodes(id)
        .await?
        .into_iter()
        .find(|n| n.server_id == server)
        .ok_or_else(|| ApiError::not_found("the stack is not deployed on this node"))
}

/// Refreshes `docker compose ps` on every node of the stack.
pub async fn refresh(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<StackId>,
) -> ApiResult<(StatusCode, Json<Vec<Job>>)> {
    let stack = load(&state, id).await?;
    caller.authorize(Action::View, ns_ref(stack.environment_id)).await?;
    let mut jobs = Vec::new();

    for n in state.store.list_nodes(id).await? {
        jobs.push(
            submit(&state, &caller, &stack, json!({ "stack_id": id, "server_id": n.server_id, "op": "ps" })).await?,
        );
    }

    Ok((StatusCode::ACCEPTED, Json(jobs)))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct LogsQuery {
    #[schemars(with = "String")]
    pub server_id: ServerId,
    pub service: String,
    #[serde(default)]
    pub tail: Option<u32>,
}

const QUICK_WAIT: Duration = Duration::from_secs(25);

pub async fn logs(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<StackId>,
    Query(q): Query<LogsQuery>,
) -> ApiResult<Json<Value>> {
    let stack = load(&state, id).await?;
    caller.authorize(Action::View, ns_ref(stack.environment_id)).await?;
    let node = node_of(&state, id, q.server_id).await?;
    if !node.services.contains(&q.service) {
        return Err(ApiError::bad_request("this service does not run on this node"));
    }

    let payload = json!({ "stack_id": id, "server_id": q.server_id, "op": "logs", "service": q.service, "tail": q.tail.unwrap_or(200).min(10_000) });
    let job = submit(&state, &caller, &stack, payload).await?;
    let deadline = tokio::time::Instant::now() + QUICK_WAIT;

    loop {
        let j = state.store.get_job(job.id).await?;
        match j.status {
            JobStatus::Succeeded => {
                return Ok(Json(json!({ "job_id": j.id, "logs": j.state.pointer("/output/logs") })));
            }
            JobStatus::Failed | JobStatus::Cancelled => {
                return Err(ApiError::new(StatusCode::BAD_GATEWAY, "node_error", j.last_error.unwrap_or_default()));
            }
            _ if tokio::time::Instant::now() >= deadline => {
                return Ok(Json(json!({ "job_id": j.id, "status": "running" })));
            }
            _ => tokio::time::sleep(Duration::from_millis(200)).await,
        }
    }
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DownInput {
    /// Also remove the stack's named volumes (their data is lost).
    #[serde(default)]
    pub volumes: bool,
    /// Only this node; default all.
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub server_id: Option<ServerId>,
}

pub async fn down(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<StackId>,
    Json(input): Json<DownInput>,
) -> ApiResult<(StatusCode, Json<Vec<Job>>)> {
    let stack = load(&state, id).await?;
    caller.authorize(Action::StackManage, ns_ref(stack.environment_id)).await?;

    if state.swarm.swarm_of_environment(stack.environment_id).await?.is_some() {
        let action = crate::jobs::swarm_stack::StackAction::Remove;
        return Ok((
            StatusCode::ACCEPTED,
            Json(vec![super::swarm_stacks::start(&state, &caller, &stack, action).await?]),
        ));
    }

    let mut jobs = Vec::new();

    for n in state.store.list_nodes(id).await? {
        if input.server_id.is_some_and(|s| s != n.server_id) {
            continue;
        }

        let payload = json!({ "stack_id": id, "server_id": n.server_id, "op": "down", "volumes": input.volumes });
        jobs.push(submit(&state, &caller, &stack, payload).await?);
    }

    if jobs.is_empty() {
        return Err(ApiError::conflict("the stack is not deployed"));
    }

    caller
        .audit(
            "stack.down",
            &ns_ref(stack.environment_id),
            Some(stack.workspace_id),
            json!({ "stack": stack.name, "volumes_removed": input.volumes, "nodes": jobs.len() }),
        )
        .await;

    Ok((StatusCode::ACCEPTED, Json(jobs)))
}

// ---- exposure through Nginx ------------------------------------------------------------

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExposeInput {
    pub service: String,
    /// The published (host) port of the service.
    pub port: u16,
    pub domains: Vec<String>,
    /// `off`, `acme` or `uploaded`.
    #[serde(default)]
    pub tls: Option<String>,
    #[serde(default)]
    pub acme_email: Option<String>,
    #[serde(default)]
    pub cert_id: Option<String>,
    /// Edge nodes with Nginx; or use `cluster_id` for a cluster's ingress profile.
    #[serde(default)]
    #[schemars(with = "Vec<String>")]
    pub edges: Vec<ServerId>,
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub cluster_id: Option<forge_core::ClusterId>,
}

/// Creates a Site (Nginx) for a deployed service. The upstream is the
/// service's registered service: its nodes at their cluster network
/// address. Without a cluster network ports bind to loopback, so the edge
/// must be the service's own node.
pub async fn expose(
    State(state): State<AppState>,
    caller: Caller,
    ws: Ws,
    Path(id): Path<StackId>,
    Json(input): Json<ExposeInput>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let stack = load(&state, id).await?;
    caller.authorize(Action::StackManage, ns_ref(stack.environment_id)).await?;
    let nodes: Vec<StackNode> =
        state.store.list_nodes(id).await?.into_iter().filter(|n| n.services.contains(&input.service)).collect();

    if nodes.is_empty() {
        return Err(ApiError::conflict("deploy the service before exposing it"));
    }

    let app = forge_core::ports::ServiceRepository::list_services(&state.store, Some(stack.environment_id))
        .await?
        .into_iter()
        .find(|a| {
            a.runtime == cat::STACK_RUNTIME
                && a.spec["stack_id"] == json!(stack.id)
                && a.spec["service"] == json!(input.service)
        })
        .ok_or_else(|| ApiError::conflict("the service is not registered yet; refresh the stack"))?;
    let ips = crate::stacks::fabric_ips(&state.store, stack.workspace_id).await;
    let mesh = nodes.iter().all(|n| ips.contains_key(&n.server_id.to_string()));
    let target = if mesh {
        json!({ "kind": "app", "service_id": app.id.to_string(), "port": input.port })
    } else {
        if nodes.len() != 1 || input.cluster_id.is_some() || input.edges != [nodes[0].server_id] {
            return Err(ApiError::bad_request(
                "this node has no cluster network, so the service listens on 127.0.0.1 only: choose the service's own node as the only edge (with Nginx), or set up the cluster network",
            ));
        }

        json!({ "kind": "external", "address": format!("127.0.0.1:{}", input.port) })
    };

    let site = json!({
        "name": format!("{}-{}", stack.name, input.service),
        "domains": input.domains,
        "upstreams": [],
        "tls": input.tls.unwrap_or_else(|| "off".into()),
        "acme_email": input.acme_email.unwrap_or_default(),
        "cert_id": input.cert_id.unwrap_or_default(),
        "edges": input.edges,
        "cluster_id": input.cluster_id,
        "targets": [target],
    });
    let site_input: super::sites::SiteInput =
        serde_json::from_value(site).map_err(|e| ApiError::bad_request(format!("invalid site: {e}")))?;
    let audit = json!({ "stack": stack.name, "service": input.service, "port": input.port, "domains": input.domains });
    let (status, Json(created)) =
        super::sites::create(State(state.clone()), caller.clone(), ws, Json(site_input)).await?;
    caller.audit("stack.expose", &ns_ref(stack.environment_id), Some(stack.workspace_id), audit).await;

    Ok((status, Json(json!({ "site": created, "next": "Apply the site on its edges from Sites" }))))
}
