//! Releases and deployments of services (#73).
//!
//! * `POST /api/v1/deployments/validate` — `{yaml, environment?}`: checks a
//!   `forge.deploy.yaml` without anything else. Signed in.
//! * `POST /api/v1/deployments/plan` — the same, plus the release, nodes and
//!   steps the rollout would take, without starting it. `deploy`.
//! * `POST /api/v1/deployments` — applies the document: a release, then the
//!   rollout (`202`, the deployment). `deploy` on the service (or its
//!   environment for a new one); `server.manage` on nodes it adds;
//!   `stack.privileged` for host mounts outside the app folder.
//! * `GET /api/v1/services/{id}/releases`, `…/releases/{number}`
//! * `GET /api/v1/services/{id}/deployments`, `GET …/slots`
//! * `POST /api/v1/services/{id}/deployments` — `{release}`: roll out an
//!   earlier release again. `POST …/rollback` — `{release?}`.
//! * `GET /api/v1/deployments/{id}` (with events), `…/events?after=`
//! * `POST /api/v1/deployments/{id}/{pause|resume|abort}` — `deploy`;
//!   `…/{approve|reject}` — `deploy.approve`, never on one's own deployment.

use std::sync::{Arc, Weak};

use async_trait::async_trait;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use forge_catalog::deploy as cat;
use forge_core::deploy::{
    Deployment, DeploymentEvent, DeploymentKind, DeploymentRepository, DeploymentRequest, Release, ReleaseRepository,
    SlotRecord,
};
use forge_core::model::{Job, Service};
use forge_core::ports::{ServerRepository, ServiceRepository};
use forge_core::{Action, DeploymentId, DomainResult, EntityKind, EntityRef, ServerId, ServiceId, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::auth::{Caller, Ws};
use crate::deploy::{MAX_LISTED, PlanView, Prepared, RolloutEffects, Starter, TrafficSample};
use crate::error::{ApiError, ApiResult};
use crate::state::{AppState, Services};

fn svc_ref(id: ServiceId) -> EntityRef {
    EntityRef::new(EntityKind::Service, id)
}

fn ns_ref(id: forge_core::EnvironmentId) -> EntityRef {
    EntityRef::new(EntityKind::Environment, id)
}

/// Audit action names (one definition).
mod audit {
    pub const APPLY: &str = "deployment.apply";
    pub const REDEPLOY: &str = "deployment.redeploy";
    pub const ROLLBACK: &str = "deployment.rollback";
    pub const REQUEST: &str = "deployment.request";
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DocInput {
    /// The `forge.deploy.yaml` text.
    pub yaml: String,
    /// Apply to this environment instead of `metadata.environment` (its
    /// `overrides` entry is merged in).
    #[serde(default)]
    pub environment: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Validated {
    pub valid: bool,
    pub service: String,
    pub environment: String,
    pub runtime: forge_catalog::Runtime,
    pub artifact: String,
    pub policy: cat::RolloutPolicy,
}

pub async fn validate(_caller: Caller, Json(input): Json<DocInput>) -> ApiResult<Json<Validated>> {
    let doc = crate::deploy::read_document(&input.yaml, input.environment.as_deref())?;
    let artifact = doc.spec.artifact.resolve(doc.spec.runtime).map_err(|e| ApiError::bad_request(e.to_string()))?;
    let policy = doc.policy().map_err(|e| ApiError::bad_request(e.to_string()))?;
    Ok(Json(Validated {
        valid: true,
        service: doc.metadata.name,
        environment: doc.metadata.environment,
        runtime: doc.spec.runtime,
        artifact: artifact.label(),
        policy,
    }))
}

/// Authorizes what applying `p` does: `deploy` on the service (or its
/// environment, for a new one), `server.manage` on nodes it adds, and
/// `stack.privileged` for mounts outside the app folder. Returns those mounts.
async fn authorize(state: &AppState, caller: &Caller, p: &Prepared) -> ApiResult<Vec<String>> {
    let target = if p.is_new { ns_ref(p.app.environment_id) } else { svc_ref(p.app.id) };
    caller.authorize(Action::Deploy, target).await?;
    for n in &p.added {
        caller.authorize(Action::ServerManage, EntityRef::new(EntityKind::Server, *n)).await?;
    }
    if !p.removed.is_empty() {
        for n in &p.removed {
            caller.authorize(Action::ServerManage, EntityRef::new(EntityKind::Server, *n)).await?;
        }
    }
    let spec = crate::jobs::app::build_spec(&crate::deploy::release_app(&p.app, &p.release), None)
        .map_err(ApiError::bad_request)?;
    let mut mounts = std::collections::BTreeSet::new();
    for n in &p.nodes {
        let roots = crate::stacks::bind_roots(&state.store, *n).await;
        mounts.extend(crate::jobs::app::privileged_mounts(&spec, &roots));
    }
    if !mounts.is_empty() {
        caller.authorize(Action::StackPrivileged, ns_ref(p.app.environment_id)).await?;
    }
    Ok(mounts.into_iter().collect())
}

async fn prepare(
    state: &AppState,
    caller: &Caller,
    ws: forge_core::WorkspaceId,
    input: &DocInput,
) -> ApiResult<Prepared> {
    let doc = crate::deploy::read_document(&input.yaml, input.environment.as_deref())?;
    let p = state.deploy.prepare(ws, &doc, &input.yaml, &caller.principal()).await?;
    state.swarm.check_runtime(p.app.environment_id, &p.app.runtime).await?;
    if state.swarm.swarm_of_environment(p.app.environment_id).await?.is_some() {
        return Err(ApiError::conflict(
            "release rollouts are not available in Docker Swarm clusters yet; use Deploy on the service",
        ));
    }
    Ok(p)
}

async fn next_number(state: &AppState, app: ServiceId) -> DomainResult<i64> {
    Ok(state.store.list_releases(app, 1).await?.first().map_or(1, |r| r.number + 1))
}

pub async fn plan(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Json(input): Json<DocInput>,
) -> ApiResult<Json<PlanView>> {
    let p = prepare(&state, &caller, ws, &input).await?;
    authorize(&state, &caller, &p).await?;
    let n = next_number(&state, p.app.id).await?;
    let env = p.release.spec["environment"].as_str().unwrap_or_default().to_owned();
    Ok(Json(p.view(&env, n)))
}

#[derive(Debug, Serialize)]
pub struct Started {
    pub deployment: Deployment,
    pub plan: PlanView,
    /// Runtime installs started first (Docker Engine, App Store runtimes).
    pub installs: Vec<Job>,
}

pub async fn apply(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Json(input): Json<DocInput>,
) -> ApiResult<(StatusCode, Json<Started>)> {
    let p = prepare(&state, &caller, ws, &input).await?;
    let mounts = authorize(&state, &caller, &p).await?;
    let env = p.release.spec["environment"].as_str().unwrap_or_default().to_owned();
    let view = p.view(&env, next_number(&state, p.app.id).await?);
    let rel_app = crate::deploy::release_app(&p.app, &p.release);
    let installs = super::services::install_runtimes(&state, &caller, &rel_app, &p.nodes).await?;
    let who = Starter { principal: caller.principal(), context: caller.ctx(), privileged_mounts: mounts.clone() };
    let (app, is_new) = (p.app.clone(), p.is_new);
    let d = state.deploy.start(p, DeploymentKind::Apply, &who, &state.jobs).await?;
    caller
        .audit(
            audit::APPLY,
            &svc_ref(app.id),
            Some(app.workspace_id),
            json!({
                "service": app.name, "environment": env, "created": is_new, "release": d.release_number,
                "deployment_id": d.id, "strategy": d.strategy, "privileged_mounts": mounts,
            }),
        )
        .await;
    Ok((StatusCode::ACCEPTED, Json(Started { deployment: d, plan: view, installs })))
}

async fn load_app(state: &AppState, id: ServiceId) -> ApiResult<Service> {
    super::services::load_own(state, id).await
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListQuery {
    /// Most entries (default and maximum 200).
    #[serde(default)]
    pub limit: Option<u32>,
}

pub async fn releases(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServiceId>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Vec<Release>>> {
    let app = load_app(&state, id).await?;
    caller.authorize(Action::View, svc_ref(app.id)).await?;
    Ok(Json(state.store.list_releases(app.id, q.limit.unwrap_or(MAX_LISTED).clamp(1, MAX_LISTED)).await?))
}

pub async fn release(
    State(state): State<AppState>,
    caller: Caller,
    Path((id, number)): Path<(ServiceId, i64)>,
) -> ApiResult<Json<Release>> {
    let app = load_app(&state, id).await?;
    caller.authorize(Action::View, svc_ref(app.id)).await?;
    let r =
        state.store.release_by_number(app.id, number).await?.ok_or_else(|| ApiError::not_found("release not found"))?;
    Ok(Json(r))
}

pub async fn deployments(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServiceId>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Vec<Deployment>>> {
    let app = load_app(&state, id).await?;
    caller.authorize(Action::View, svc_ref(app.id)).await?;
    Ok(Json(state.store.list_deployments(app.id, q.limit.unwrap_or(MAX_LISTED).clamp(1, MAX_LISTED)).await?))
}

#[derive(Debug, Serialize)]
pub struct SlotsView {
    /// The release that served last (0: none yet).
    pub current_release: i64,
    pub slots: Vec<SlotRecord>,
}

pub async fn slots(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServiceId>,
) -> ApiResult<Json<SlotsView>> {
    let app = load_app(&state, id).await?;
    caller.authorize(Action::View, svc_ref(app.id)).await?;
    Ok(Json(SlotsView {
        current_release: state.store.current_release(app.id).await?,
        slots: state.store.list_slots(app.id).await?,
    }))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ReleaseInput {
    /// The release to roll out (rollback: default the one before the current).
    #[serde(default)]
    pub release: Option<i64>,
}

async fn redeploy(
    state: &AppState,
    caller: &Caller,
    app: &Service,
    number: i64,
    kind: DeploymentKind,
) -> ApiResult<(StatusCode, Json<Started>)> {
    caller.authorize(Action::Deploy, svc_ref(app.id)).await?;
    let p = state.deploy.prepare_release(app, number).await?;
    let mounts = authorize(state, caller, &p).await?;
    let env = p.release.spec["environment"].as_str().unwrap_or_default().to_owned();
    let view = p.view(&env, number);
    let who = Starter { principal: caller.principal(), context: caller.ctx(), privileged_mounts: mounts };
    let d = state.deploy.start(p, kind, &who, &state.jobs).await?;
    let action = if kind == DeploymentKind::Rollback { audit::ROLLBACK } else { audit::REDEPLOY };
    caller
        .audit(
            action,
            &svc_ref(app.id),
            Some(app.workspace_id),
            json!({ "service": app.name, "release": number, "from": d.from_release, "deployment_id": d.id }),
        )
        .await;
    Ok((StatusCode::ACCEPTED, Json(Started { deployment: d, plan: view, installs: Vec::new() })))
}

pub async fn deploy_release(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServiceId>,
    Json(input): Json<ReleaseInput>,
) -> ApiResult<(StatusCode, Json<Started>)> {
    let app = load_app(&state, id).await?;
    let number = input.release.ok_or_else(|| ApiError::bad_request("release is required"))?;
    redeploy(&state, &caller, &app, number, DeploymentKind::Apply).await
}

pub async fn rollback(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServiceId>,
    Json(input): Json<ReleaseInput>,
) -> ApiResult<(StatusCode, Json<Started>)> {
    let app = load_app(&state, id).await?;
    caller.authorize(Action::Deploy, svc_ref(app.id)).await?;
    let number = state.deploy.rollback_target(&app, input.release).await?;
    redeploy(&state, &caller, &app, number, DeploymentKind::Rollback).await
}

#[derive(Debug, Serialize)]
pub struct DeploymentView {
    #[serde(flatten)]
    pub deployment: Deployment,
    pub service: String,
    /// The plan's steps for people, in order.
    pub steps: Vec<String>,
    pub events: Vec<DeploymentEvent>,
    /// The caller may pause, resume and abort it.
    pub can_control: bool,
    /// The caller may approve or reject it (`deploy.approve`, not their own).
    pub can_approve: bool,
}

async fn load_deployment(state: &AppState, caller: &Caller, id: DeploymentId) -> ApiResult<(Deployment, Service)> {
    let d = state.store.get_deployment(id).await?.ok_or_else(|| ApiError::not_found("deployment not found"))?;
    let app = load_app(state, d.service_id).await?;
    caller.authorize(Action::View, svc_ref(app.id)).await?;
    Ok((d, app))
}

pub async fn get(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<DeploymentId>,
) -> ApiResult<Json<DeploymentView>> {
    let (d, app) = load_deployment(&state, &caller, id).await?;
    let plan: cat::Plan = serde_json::from_value(d.plan.clone()).map_err(forge_core::DomainError::internal)?;
    let names = node_names(&state, &plan).await?;
    let events = state.store.list_events(d.id, 0, 1_000).await?;
    let can_control = caller.can(Action::Deploy, svc_ref(app.id)).await;
    let can_approve = d.created_by != caller.principal() && caller.can(Action::DeployApprove, svc_ref(app.id)).await;
    Ok(Json(DeploymentView {
        steps: plan.steps.iter().map(|s| s.describe(&names)).collect(),
        service: app.name,
        deployment: d,
        events,
        can_control,
        can_approve,
    }))
}

async fn node_names(state: &AppState, plan: &cat::Plan) -> DomainResult<std::collections::BTreeMap<String, String>> {
    let mut names = std::collections::BTreeMap::new();
    for t in &plan.targets {
        if let Ok(id) = t.server_id.parse::<ServerId>()
            && let Ok(s) = state.store.get_server(id).await
        {
            names.insert(t.server_id.clone(), s.display_name.unwrap_or(s.hostname));
        }
    }
    Ok(names)
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct EventsQuery {
    /// Only events after this sequence number.
    #[serde(default)]
    pub after: i64,
}

pub async fn events(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<DeploymentId>,
    Query(q): Query<EventsQuery>,
) -> ApiResult<Json<Vec<DeploymentEvent>>> {
    let (d, _) = load_deployment(&state, &caller, id).await?;
    Ok(Json(state.store.list_events(d.id, q.after.max(0), 1_000).await?))
}

pub async fn request(
    State(state): State<AppState>,
    caller: Caller,
    Path((id, what)): Path<(DeploymentId, String)>,
) -> ApiResult<Json<Deployment>> {
    let req = DeploymentRequest::parse(&what).ok_or_else(|| ApiError::not_found("unknown deployment action"))?;
    let (d, app) = load_deployment(&state, &caller, id).await?;
    match req {
        DeploymentRequest::Approve | DeploymentRequest::Reject => {
            caller.authorize(Action::DeployApprove, svc_ref(app.id)).await?;
            if d.created_by == caller.principal() {
                return Err(ApiError::forbidden("someone other than its author approves a deployment"));
            }
            if d.state != forge_core::deploy::DeploymentState::AwaitingApproval {
                return Err(ApiError::conflict("the deployment is not waiting for an approval"));
            }
        }
        DeploymentRequest::Pause | DeploymentRequest::Resume | DeploymentRequest::Abort => {
            caller.authorize(Action::Deploy, svc_ref(app.id)).await?;
        }
    }
    if !d.state.active() {
        return Err(ApiError::conflict("the deployment has finished"));
    }
    state.store.request_deployment(d.id, req, state.clock.now()).await?;
    let who = caller.principal().to_string();
    let e = DeploymentEvent {
        deployment_id: d.id,
        seq: 0,
        at: state.clock.now(),
        level: "info".into(),
        server_id: None,
        message: format!("{} requested by {who}", req.as_str()),
    };
    state.store.add_event(&e).await?;
    caller
        .audit(
            audit::REQUEST,
            &svc_ref(app.id),
            Some(app.workspace_id),
            json!({ "service": app.name, "deployment_id": d.id, "request": req.as_str() }),
        )
        .await;
    let d = state.store.get_deployment(d.id).await?.ok_or_else(|| ApiError::not_found("deployment not found"))?;
    Ok(Json(d))
}

/// What a rollout changes outside its job, through the flows the API owns:
/// endpoints (re-applied as whoever applied them last), health policies and
/// the edges' traffic figures. Holds the services weakly (they hold the job).
#[derive(Debug)]
pub struct ApiEffects(pub Weak<Services>);

impl ApiEffects {
    fn state(&self) -> DomainResult<AppState> {
        self.0.upgrade().ok_or_else(|| forge_core::DomainError::Internal("the control plane is stopping".into()))
    }
}

#[async_trait]
impl RolloutEffects for ApiEffects {
    async fn reapply_endpoints(&self, app: ServiceId) -> DomainResult<Vec<String>> {
        super::sites::reapply_for_app(&self.state()?, app).await
    }

    async fn push_health(&self, app: ServiceId, server: ServerId) {
        let Ok(state) = self.state() else { return };
        match state.store.get_service(app).await {
            Ok(Some(a)) => crate::health_ingest::push(&state, &a, server, true),
            Ok(None) => {}
            Err(e) => tracing::warn!(%app, error = %e, "reading the service for its health policy failed"),
        }
    }

    async fn traffic(&self, app: ServiceId, copies: &[cat::Target], since: Timestamp) -> DomainResult<TrafficSample> {
        let state = self.state()?;
        let resolver =
            super::routing::Resolver::new(&state).await.map_err(|e| forge_core::DomainError::Internal(e.message))?;
        let slots = state.store.list_slots(app).await?;
        let mut wanted = std::collections::HashSet::new();
        for c in copies {
            let Ok(server) = c.server_id.parse::<ServerId>() else { continue };
            let name = cat::Slot::name(c.slot);
            let Some(row) = slots.iter().find(|s| s.server_id == server && s.slot == name) else { continue };
            for port in crate::deploy::port_map(row).values() {
                for host in resolver.addresses_of(&c.server_id) {
                    wanted.insert(if host.contains(':') {
                        format!("[{host}]:{port}")
                    } else {
                        format!("{host}:{port}")
                    });
                }
            }
        }
        let mut out = TrafficSample::default();
        for (_, sites) in state.telemetry.traffic(since.0) {
            for site in sites {
                for u in site.upstreams.iter().filter(|u| wanted.contains(&u.address)) {
                    out.requests += u.requests;
                    out.errors += u.errors;
                    if u.p95_ms > 0 {
                        out.p95_ms = Some(out.p95_ms.unwrap_or(0).max(u.p95_ms));
                    }
                }
            }
        }
        Ok(out)
    }
}

/// The effects for `state` (wired in `state.rs`).
#[must_use]
pub fn effects(state: &AppState) -> Arc<dyn RolloutEffects> {
    Arc::new(ApiEffects(Arc::downgrade(state)))
}
