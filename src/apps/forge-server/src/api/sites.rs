//! Reverse-proxy sites (Nginx on edge nodes) and DNS (PowerDNS on a node).
//!
//! * `GET/POST /api/v1/sites`, `GET/PUT/DELETE /api/v1/sites/{id}`
//! * `POST /api/v1/sites/preview` — renders the managed config without a node
//! * `POST /api/v1/sites/{id}/apply` — every edge: write, `nginx -t`, reload
//!   (rolled back on failure), then publish DNS records if configured
//! * `POST /api/v1/sites/{id}/remove` — take the site off its edges
//! * `POST /api/v1/sites/{id}/cache/clear` — empty the proxy cache on every edge
//! * `GET /api/v1/sites/{id}/logs?server_id=&log=access|error` — the tail of
//!   the endpoint's own access or error log on one edge
//! * `POST /api/v1/servers/{id}/dns` — PowerDNS zones and records on that node
//!
//! Applying, removing and DNS changes need `proxy.manage` (MFA) on each node;
//! every node operation is an audited `agent.command` job. A non-empty custom
//! block also needs `server.command` (MFA) on each edge, to save and to apply:
//! nginx runs it as root. A static directory must be inside an approved
//! file root on every edge and needs `file.read` there. Sites are scoped to
//! the caller's workspace.

use std::time::Duration;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use forge_catalog::SiteSpec;
use forge_catalog::ingress::Target;
use forge_core::model::{AgentStatus, JobStatus, NewJob, Site};
use forge_core::ports::{JobRepository, ServerRepository, SiteRepository};
use forge_core::{Action, EntityKind, EntityRef, ServerId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::auth::{Caller, Ws};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

fn ws_ref(ws: forge_core::WorkspaceId) -> EntityRef {
    EntityRef::new(EntityKind::Workspace, ws)
}
fn server_ref(id: ServerId) -> EntityRef {
    EntityRef::new(EntityKind::Server, id)
}

/// Runs one agent command as a job and waits up to `wait` for its result.
pub async fn run_on_node(
    state: &AppState,
    caller: &Caller,
    server: ServerId,
    command: Value,
    wait: Duration,
) -> ApiResult<Value> {
    run_as(state, caller.principal(), caller.ctx(), server, command, wait).await
}

/// [`run_on_node`] for a stored principal (background reconcilers). The job
/// runner re-authorizes the principal before every step, as for any job.
pub async fn run_as(
    state: &AppState,
    principal: forge_core::Principal,
    context: forge_core::AuthzContext,
    server: ServerId,
    command: Value,
    wait: Duration,
) -> ApiResult<Value> {
    submit_and_wait(state, principal, context, server, command, wait, None).await
}

/// How long [`submit_and_wait`] waits for a node whose agent is offline or
/// holds no link to this server.
const OFFLINE_GRACE: Duration = Duration::from_secs(1);

/// Marks a command sent by the endpoint flow (after its own checks): the
/// site's id, and whether the job adds the uploaded certificate's key to a
/// site `apply` (only after authorizing the certificate's use).
#[derive(Debug, Clone, Copy)]
struct EndpointFlow<'a> {
    site: &'a str,
    attach_certificate: bool,
}

/// Runs `command`, optionally marked as sent by the endpoint flow.
async fn submit_and_wait(
    state: &AppState,
    principal: forge_core::Principal,
    context: forge_core::AuthzContext,
    server: ServerId,
    command: Value,
    wait: Duration,
    endpoint: Option<EndpointFlow<'_>>,
) -> ApiResult<Value> {
    crate::jobs::agent_command::parse_command(&command).map_err(ApiError::bad_request)?;
    let node = state.store.get_server(server).await.map_err(|_| ApiError::not_found("server not found"))?;
    let job = state
        .jobs
        .submit(NewJob {
            kind: "agent.command".into(),
            workspace_id: Some(node.workspace_id),
            resource: server_ref(server),
            principal,
            context,
            idempotency_key: None,
            payload: match endpoint {
                Some(EndpointFlow { site, attach_certificate: true }) => json!({
                    "server_id": server, "command": command,
                    crate::jobs::agent_command::ATTACH_CERTIFICATE: site,
                    crate::jobs::agent_command::FROM_ENDPOINT: site,
                }),
                Some(EndpointFlow { site, attach_certificate: false }) => {
                    json!({ "server_id": server, "command": command, crate::jobs::agent_command::FROM_ENDPOINT: site })
                }
                None => json!({ "server_id": server, "command": command }),
            },
            max_attempts: 1,
        })
        .await?;
    // A node whose agent holds no link here cannot answer within `wait`:
    // answer after a short grace with the job still running (it runs when
    // the node reconnects). The grace still reports a job that fails at
    // once (e.g. a denied step), and a link that appears meanwhile restores
    // the full wait.
    let started = tokio::time::Instant::now();
    let (deadline, grace) = (started + wait, started + wait.min(OFFLINE_GRACE));
    loop {
        let j = state.store.get_job(job.id).await?;
        let linked = node.status == AgentStatus::Online && crate::agent_hub::linked(state, server);
        let deadline = if linked { deadline } else { grace };
        match j.status {
            JobStatus::Succeeded => {
                return Ok(json!({ "job_id": j.id, "status": "succeeded", "result": j.state.get("output") }));
            }
            JobStatus::Failed | JobStatus::Cancelled => {
                return Ok(json!({ "job_id": j.id, "status": "failed", "error": j.last_error }));
            }
            _ if tokio::time::Instant::now() >= deadline => return Ok(json!({ "job_id": j.id, "status": "running" })),
            _ => tokio::time::sleep(Duration::from_millis(250)).await,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DnsConfig {
    #[schemars(with = "String")]
    pub server_id: ServerId,
    /// `example.com.`
    pub zone: String,
    #[serde(default = "ttl")]
    pub ttl: u32,
    /// `round_robin` (all addresses) or `active_standby` (the first one).
    #[serde(default = "rr")]
    pub mode: String,
    /// Public addresses of the edge nodes (A / AAAA).
    pub addresses: Vec<String>,
}
const fn ttl() -> u32 {
    300
}
fn rr() -> String {
    "round_robin".into()
}

/// Where an endpoint is served: one or more nodes with Nginx, or a
/// cluster's edge group (its ingress profile decides the edges).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Placement {
    Nodes {
        #[schemars(with = "Vec<String>")]
        server_ids: Vec<ServerId>,
    },
    Cluster {
        #[schemars(with = "String")]
        cluster_id: forge_core::ClusterId,
    },
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SiteInput {
    #[serde(flatten)]
    #[schemars(with = "serde_json::Value")]
    pub spec: SiteSpec,
    /// Explicit edge nodes; ignored when `cluster_id` is set (the cluster's
    /// ingress profile decides).
    #[serde(default)]
    #[schemars(with = "Vec<String>")]
    pub edges: Vec<ServerId>,
    /// Serve this endpoint from a cluster's ingress profile.
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub cluster_id: Option<forge_core::ClusterId>,
    /// Typed upstreams (Forge service, node port, external), resolved to
    /// `host:port` into `upstreams` / `standby` when saved.
    #[serde(default)]
    #[schemars(with = "Vec<serde_json::Value>")]
    pub targets: Vec<Target>,
    #[serde(default)]
    #[schemars(with = "Vec<serde_json::Value>")]
    pub standby_targets: Vec<Target>,
    #[serde(default)]
    pub dns: Option<DnsConfig>,
    /// Where the endpoint is served; replaces `edges` / `cluster_id`, which
    /// older clients still send.
    #[serde(default)]
    pub placement: Option<Placement>,
}

/// The stored `config` of a site row.
#[derive(Debug, Serialize)]
struct StoredConfig<'a> {
    spec: &'a SiteSpec,
    placement: Option<&'a Placement>,
    edges: &'a [ServerId],
    cluster_id: Option<forge_core::ClusterId>,
    targets: &'a [Target],
    standby_targets: &'a [Target],
    dns: Option<&'a DnsConfig>,
}

/// Reads the placement (or the older `edges` / `cluster_id` fields), fills
/// the edges from the cluster profile and resolves typed targets.
async fn resolve(state: &AppState, ws: forge_core::WorkspaceId, input: &mut SiteInput) -> ApiResult<()> {
    match input.placement.clone() {
        Some(Placement::Cluster { cluster_id }) => input.cluster_id = Some(cluster_id),
        Some(Placement::Nodes { server_ids }) => {
            input.cluster_id = None;
            input.edges = server_ids;
        }
        None => {
            input.placement = Some(match input.cluster_id {
                Some(cluster_id) => Placement::Cluster { cluster_id },
                None => Placement::Nodes { server_ids: input.edges.clone() },
            });
        }
    }
    if let Some(c) = input.cluster_id {
        use forge_core::ports::ClusterRepository;
        let cluster = state.store.get_cluster(c).await.map_err(|_| ApiError::bad_request("cluster not found"))?;
        if cluster.workspace_id != ws {
            return Err(ApiError::bad_request("cluster not found"));
        }
        let (profile, _) = super::ingress::load_profile(state, c)
            .await?
            .ok_or_else(|| ApiError::bad_request("this cluster has no ingress profile yet (Ingress → Configure)"))?;
        let ingress = profile.ingress.ok_or_else(|| ApiError::bad_request("this cluster's profile has no ingress"))?;
        input.edges = ingress
            .edges
            .iter()
            .map(|e| e.server_id.parse().map_err(|_| ApiError::bad_request("invalid edge id")))
            .collect::<ApiResult<_>>()?;
        if input.edges.is_empty() {
            return Err(ApiError::bad_request(format!(
                "cluster {}'s edge group has no edge nodes yet: add one on the cluster's ingress profile",
                cluster.name
            )));
        }
    }
    let typed_rules = input.spec.rules.iter().any(|r| !r.targets.is_empty() || !r.standby_targets.is_empty());
    if input.targets.is_empty() && input.standby_targets.is_empty() && !typed_rules {
        return Ok(());
    }
    let r = super::routing::Resolver::new(state).await?.for_edges(&input.edges);
    if !input.targets.is_empty() || !input.standby_targets.is_empty() {
        let (active, standby) = r.upstreams(state, &input.targets, &input.standby_targets).await?;
        input.spec.upstreams = active;
        input.spec.standby = standby;
    }
    for rule in &mut input.spec.rules {
        if !rule.targets.is_empty() || !rule.standby_targets.is_empty() {
            let (active, standby) = r.upstreams(state, &rule.targets, &rule.standby_targets).await?;
            rule.upstreams = active;
            rule.standby = standby;
        }
    }
    Ok(())
}

fn check(input: &SiteInput) -> ApiResult<()> {
    input.spec.validate().map_err(ApiError::bad_request)?;
    if input.edges.is_empty() {
        return Err(ApiError::bad_request("choose the node that serves this endpoint (a node with Nginx installed)"));
    }
    if let Some(d) = &input.dns {
        if !d.zone.ends_with('.') {
            return Err(ApiError::bad_request("DNS zone must end with a dot, e.g. example.com."));
        }
        if d.addresses.is_empty() || d.addresses.iter().any(|a| a.parse::<std::net::IpAddr>().is_err()) {
            return Err(ApiError::bad_request("DNS needs the edge nodes' public IP addresses"));
        }
        if !matches!(d.mode.as_str(), "round_robin" | "active_standby") {
            return Err(ApiError::bad_request("DNS mode: round_robin or active_standby"));
        }
        let zone = d.zone.trim_end_matches('.');
        if let Some(bad) = input.spec.domains.iter().find(|x| *x != zone && !x.ends_with(&format!(".{zone}"))) {
            return Err(ApiError::bad_request(format!("{bad} is not inside zone {}", d.zone)));
        }
    }
    Ok(())
}

/// An uploaded certificate must exist in this workspace and cover every domain.
async fn check_certificate(state: &AppState, ws: forge_core::WorkspaceId, input: &SiteInput) -> ApiResult<()> {
    if input.spec.material.is_some() {
        return Err(ApiError::bad_request("certificate material is added by Forge"));
    }
    if input.spec.tls != forge_catalog::sites::Tls::Uploaded {
        return Ok(());
    }
    let cert = super::certificates::load(state, ws, &input.spec.cert_id).await?;
    if let Some(d) = input.spec.domains.iter().find(|d| !super::certificates::covers(&cert.domains, d)) {
        return Err(ApiError::bad_request(format!("certificate “{}” does not cover {d}", cert.name)));
    }
    if cert.not_after <= state.clock.now() {
        return Err(ApiError::bad_request(format!("certificate “{}” has expired", cert.name)));
    }
    Ok(())
}

/// A non-empty custom block runs as root inside nginx: saving one needs
/// `server.command` (MFA) on every edge it goes to.
async fn authorize_custom(caller: &Caller, custom: &str, edges: &[ServerId]) -> ApiResult<()> {
    if custom.trim().is_empty() {
        return Ok(());
    }
    for e in edges {
        caller.authorize(Action::ServerCommand, server_ref(*e)).await?;
    }
    Ok(())
}

fn config_of(input: &SiteInput) -> ApiResult<Value> {
    let config = StoredConfig {
        spec: &input.spec,
        placement: input.placement.as_ref(),
        edges: &input.edges,
        cluster_id: input.cluster_id,
        targets: &input.targets,
        standby_targets: &input.standby_targets,
        dns: input.dns.as_ref(),
    };
    Ok(serde_json::to_value(config).map_err(forge_core::DomainError::internal)?)
}

/// A static directory is served by nginx (root) from every edge: it must be
/// inside an approved file root there, and the caller needs `file.read`.
async fn authorize_static(state: &AppState, caller: &Caller, spec: &SiteSpec, edges: &[ServerId]) -> ApiResult<()> {
    if spec.static_roots().is_empty() {
        return Ok(());
    }
    for e in edges {
        caller.authorize(Action::FileRead, server_ref(*e)).await?;
    }
    Ok(state.endpoints.check_static_roots(edges, spec).await?)
}

/// A site as the console sees it: basic-auth hashes stay on the server.
fn redacted(mut site: Site) -> Site {
    crate::endpoints::redact(&mut site.config);
    site
}

/// What an endpoint lives on: its cluster (when it has one) and its edge
/// nodes. `view` on any of them shows the endpoint.
fn site_scopes(site: &Site) -> Vec<EntityRef> {
    let cluster = site.config["cluster_id"]
        .as_str()
        .and_then(|c| c.parse::<forge_core::ClusterId>().ok())
        .map(|c| EntityRef::new(EntityKind::Cluster, c));
    let edges = site.config["edges"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| e.as_str()?.parse::<ServerId>().ok())
        .map(server_ref);
    cluster.into_iter().chain(edges).collect()
}

/// Per endpoint, whether the caller may view it (one batched check).
pub(crate) async fn visible_sites(caller: &Caller, sites: &[Site]) -> Vec<bool> {
    let scopes: Vec<Vec<EntityRef>> = sites.iter().map(site_scopes).collect();
    let mut allowed = caller.can_many(Action::View, scopes.iter().flatten().cloned().collect()).await.into_iter();
    // Consume every answer of a site, so the next site starts at its own.
    scopes
        .iter()
        .map(|s| s.iter().map(|_| allowed.next().unwrap_or(false)).collect::<Vec<_>>().contains(&true))
        .collect()
}

/// The endpoint `id` of workspace `ws`, if the caller may view it: `view`
/// on its cluster or one of its edges, else `view` on the workspace (a
/// recorded refusal).
pub(crate) async fn load_visible(
    state: &AppState,
    caller: &Caller,
    ws: forge_core::WorkspaceId,
    id: &str,
) -> ApiResult<Site> {
    let site = load(state, ws, id).await?;
    if !visible_sites(caller, std::slice::from_ref(&site)).await.first().copied().unwrap_or(false) {
        caller.authorize(Action::View, ws_ref(ws)).await?;
    }
    Ok(site)
}

/// `GET /sites`: any role in the workspace may list; each endpoint is shown
/// when the caller may view its cluster or one of its edge nodes. Without
/// any role here the request is refused (and recorded).
pub async fn list(State(state): State<AppState>, caller: Caller, Ws(ws): Ws) -> ApiResult<Json<Vec<Site>>> {
    let sites = state.store.list_workspace_sites(ws).await?;
    // `view` on the workspace shows every endpoint, those on no cluster or
    // edge included.
    if caller.can(Action::View, ws_ref(ws)).await {
        return Ok(Json(sites.into_iter().map(redacted).collect()));
    }
    if !crate::console_access::is_member(&state.store, &state.store, caller.session.user.id, ws).await? {
        caller.authorize(Action::View, ws_ref(ws)).await?;
    }
    let allowed = visible_sites(&caller, &sites).await;
    Ok(Json(sites.into_iter().zip(allowed).filter_map(|(s, ok)| ok.then_some(redacted(s))).collect()))
}

/// A site of workspace `ws`; sites of other workspaces are not found.
pub(crate) async fn load(state: &AppState, ws: forge_core::WorkspaceId, id: &str) -> ApiResult<Site> {
    state
        .store
        .get_site(id)
        .await?
        .filter(|s| s.workspace_id == ws)
        .ok_or_else(|| ApiError::not_found("site not found"))
}

pub async fn get(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Path(id): Path<String>,
) -> ApiResult<Json<Site>> {
    Ok(Json(redacted(load_visible(&state, &caller, ws, &id).await?)))
}

/// Typed targets must belong to the caller's workspace (a node or a service
/// of another workspace is not found).
async fn check_targets_ws(state: &AppState, ws: forge_core::WorkspaceId, input: &SiteInput) -> ApiResult<()> {
    use forge_core::ports::ServiceRepository;
    let mut all: Vec<&Target> = input.targets.iter().chain(&input.standby_targets).collect();
    for r in &input.spec.rules {
        all.extend(r.targets.iter().chain(&r.standby_targets));
    }
    for t in all {
        match t {
            Target::Node { server_id, .. } => {
                let id: ServerId = server_id.parse().map_err(|_| ApiError::bad_request("invalid node id"))?;
                let found = state.store.get_server(id).await.ok().is_some_and(|s| s.workspace_id == ws);
                if !found {
                    return Err(ApiError::bad_request(format!("node {server_id} not found")));
                }
            }
            Target::App { service_id, .. } => {
                let id: forge_core::ServiceId =
                    service_id.parse().map_err(|_| ApiError::bad_request("invalid service id"))?;
                let found = state.store.get_service(id).await.ok().flatten().is_some_and(|a| a.workspace_id == ws);
                if !found {
                    return Err(ApiError::bad_request(format!("service {service_id} not found")));
                }
            }
            Target::External { .. } => {}
        }
    }
    Ok(())
}

/// One typed target of a preview with the addresses it resolves to.
#[derive(Debug, Serialize)]
struct PreviewTarget {
    target: Target,
    standby: bool,
    upstreams: Vec<Value>,
}

/// `POST /sites/preview`.
#[derive(Debug, Serialize)]
pub struct Preview {
    /// `https` with TLS (the variant the edges run), else `http`.
    variant: &'static str,
    /// The main configuration the edges run (read-only).
    config: String,
    http: String,
    https: Option<String>,
    /// The custom configuration file as written to the edges.
    custom: String,
    /// The first refusal in the custom configuration (line and directive).
    custom_error: Option<forge_catalog::sites::CustomError>,
    edges: Vec<ServerId>,
    upstreams: Vec<String>,
    standby: Vec<String>,
    targets: Vec<PreviewTarget>,
}

/// A stand-in hash so a preview renders while passwords stay unknown (the
/// main configuration never contains hashes).
const PREVIEW_HASH: &str =
    "$6$preview$aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

/// `POST /sites/preview`: takes the same body as `POST /sites` and returns
/// the rendered config, the edges and, per typed target, the upstream
/// addresses the edges would use — resolved exactly as on save and apply.
/// A refused custom configuration is reported in `custom_error` (with its
/// line) instead of failing the preview. Read-only: nothing is stored and
/// no node is contacted.
pub async fn preview(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Json(mut input): Json<SiteInput>,
) -> ApiResult<Json<Preview>> {
    caller.authorize(Action::View, ws_ref(ws)).await?;
    check_targets_ws(&state, ws, &input).await?;
    resolve(&state, ws, &mut input).await?;
    if let Some(auth) = input.spec.access.basic_auth.as_mut() {
        for u in &mut auth.users {
            u.password = None;
            PREVIEW_HASH.clone_into(&mut u.hash);
        }
    }
    let custom_error = input.spec.check_custom().err();
    let mut checked = input.spec.clone();
    checked.custom.clear();
    checked.validate().map_err(ApiError::bad_request)?;
    let r = super::routing::Resolver::new(&state).await?.for_edges(&input.edges);
    let mut targets = Vec::new();
    for (list, standby) in [(&input.targets, false), (&input.standby_targets, true)] {
        for t in list {
            let upstreams: Vec<Value> = r
                .expand(&state, t, standby)
                .await?
                .into_iter()
                .map(|u| json!({ "address": u.address, "standby": u.standby, "server_id": u.server_id, "state": u.state }))
                .collect();
            targets.push(PreviewTarget { target: t.clone(), standby, upstreams });
        }
    }
    let spec = if custom_error.is_none() { &input.spec } else { &checked };
    // The variant the edges will run: HTTPS only with TLS (F-10). `https` is
    // null without TLS, since it would point at certificates that never exist.
    let with_tls = spec.tls != forge_catalog::sites::Tls::Off;
    Ok(Json(Preview {
        variant: if with_tls { "https" } else { "http" },
        config: spec.render(with_tls),
        http: spec.render(false),
        https: with_tls.then(|| spec.render(true)),
        custom: input.spec.render_custom(),
        custom_error,
        edges: input.edges.clone(),
        upstreams: spec.upstreams.clone(),
        standby: spec.standby.clone(),
        targets,
    }))
}

pub async fn create(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Json(mut input): Json<SiteInput>,
) -> ApiResult<(StatusCode, Json<Site>)> {
    caller.authorize(Action::WorkspaceManage, ws_ref(ws)).await?;
    check_targets_ws(&state, ws, &input).await?;
    resolve(&state, ws, &mut input).await?;
    crate::endpoints::prepare_basic_auth(&mut input.spec, None)?;
    check(&input)?;
    check_certificate(&state, ws, &input).await?;
    authorize_custom(&caller, &input.spec.custom, &input.edges).await?;
    authorize_static(&state, &caller, &input.spec, &input.edges).await?;
    if state.store.list_sites().await?.iter().any(|s| s.name == input.spec.name) {
        return Err(ApiError::conflict(format!("a site named `{}` exists", input.spec.name)));
    }
    let now = state.clock.now();
    let site = Site {
        id: forge_core::JobId::new().to_string(),
        workspace_id: ws,
        name: input.spec.name.clone(),
        config: config_of(&input)?,
        edge_status: json!({}),
        created_at: now,
        updated_at: now,
    };
    state.store.put_site(&site).await?;
    caller.audit("site.create", &ws_ref(ws), Some(ws), audit_detail(&input)).await;
    Ok((StatusCode::CREATED, Json(redacted(site))))
}

pub async fn update(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Path(id): Path<String>,
    Json(mut input): Json<SiteInput>,
) -> ApiResult<Json<Site>> {
    caller.authorize(Action::WorkspaceManage, ws_ref(ws)).await?;
    let mut site = load(&state, ws, &id).await?;
    input.spec.name.clone_from(&site.name);
    check_targets_ws(&state, ws, &input).await?;
    resolve(&state, ws, &mut input).await?;
    crate::endpoints::prepare_basic_auth(&mut input.spec, spec_of(&site).ok().as_ref())?;
    check(&input)?;
    check_certificate(&state, ws, &input).await?;
    authorize_custom(&caller, &input.spec.custom, &input.edges).await?;
    authorize_static(&state, &caller, &input.spec, &input.edges).await?;
    site.config = config_of(&input)?;
    site.updated_at = state.clock.now();
    state.store.put_site(&site).await?;
    // Custom configurations are versioned in the audit log.
    caller.audit("site.update", &ws_ref(ws), Some(ws), audit_detail(&input)).await;
    Ok(Json(redacted(site)))
}

/// What the audit log records of a saved endpoint: the custom
/// configuration is versioned there; basic-auth users by name only.
fn audit_detail(input: &SiteInput) -> Value {
    let s = &input.spec;
    json!({
        "site": s.name,
        "domains": s.domains,
        "kind": s.kind,
        "rules": s.rules.iter().map(|r| json!({ "path": r.path, "kind": r.kind })).collect::<Vec<_>>(),
        "placement": input.placement,
        "basic_auth_users": s.access.basic_auth.as_ref().map(|b| b.users.iter().map(|u| u.user.clone()).collect::<Vec<_>>()),
        "custom": s.custom,
    })
}

pub async fn delete(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    caller.authorize(Action::WorkspaceManage, ws_ref(ws)).await?;
    let site = load(&state, ws, &id).await?;
    if site.edge_status.as_object().is_some_and(|m| m.values().any(|v| v["status"] == "applied")) {
        return Err(ApiError::conflict("remove the site from its edge nodes first"));
    }
    state.store.delete_site(&id).await?;
    caller.audit("site.delete", &ws_ref(ws), Some(ws), json!({ "site": site.name })).await;
    Ok(StatusCode::NO_CONTENT)
}

fn spec_of(site: &Site) -> ApiResult<SiteSpec> {
    let mut v = site.config["spec"].clone();
    v["name"] = json!(site.name);
    serde_json::from_value(v).map_err(|e| ApiError::bad_request(format!("stored site is invalid: {e}")))
}

fn edges_of(site: &Site) -> Vec<ServerId> {
    serde_json::from_value(site.config["edges"].clone()).unwrap_or_default()
}

/// Current edges: the cluster profile's when the site belongs to a cluster.
async fn current_edges(state: &AppState, site: &Site) -> ApiResult<Vec<ServerId>> {
    let Some(c) = site.config["cluster_id"].as_str().and_then(|c| c.parse().ok()) else { return Ok(edges_of(site)) };
    let Some((profile, _)) = super::ingress::load_profile(state, c).await? else { return Ok(edges_of(site)) };
    Ok(profile.ingress.map(|i| i.edges.iter().filter_map(|e| e.server_id.parse().ok()).collect()).unwrap_or_default())
}

/// Authorizes taking `site` on or off its edges. Nothing is changed before
/// every check passed; a site without edges is refused.
async fn authorize_edges(
    state: &AppState,
    caller: &Caller,
    site: &Site,
    action: &str,
) -> ApiResult<(SiteSpec, Vec<ServerId>)> {
    let spec = spec_of(site)?;
    let edges = current_edges(state, site).await?;
    if edges.is_empty() {
        return Err(ApiError::bad_request("this endpoint has no edge nodes"));
    }
    for e in &edges {
        caller.authorize(Action::ProxyManage, server_ref(*e)).await?;
    }
    if action == "apply" {
        authorize_custom(caller, &spec.custom, &edges).await?;
        // The key of an uploaded certificate leaves the vault for these edges
        // (and only these): its own permission. The job runner checks it again
        // on each edge before the key is unsealed.
        if spec.tls == forge_catalog::sites::Tls::Uploaded {
            caller.authorize(Action::CertificateUse, ws_ref(site.workspace_id)).await?;
        }
        // The edges may have changed with the cluster profile since saving.
        authorize_static(state, caller, &spec, &edges).await?;
    }
    Ok((spec, edges))
}

async fn on_edges(
    state: &AppState,
    caller: &Caller,
    site: &mut Site,
    action: &str,
    (spec, edges): (SiteSpec, Vec<ServerId>),
) -> ApiResult<Vec<Value>> {
    let attach = action == "apply" && spec.tls == forge_catalog::sites::Tls::Uploaded;
    let site_id = site.id.clone();
    let mut results = Vec::new();
    for (e, spec) in per_edge(state, &spec, &edges).await? {
        let r = submit_and_wait(
            state,
            caller.principal(),
            caller.ctx(),
            e,
            json!({ "type": "site", "action": action, "spec": spec }),
            Duration::from_secs(150),
            Some(EndpointFlow { site: &site_id, attach_certificate: attach }),
        )
        .await?;
        let status = match (action, r["status"].as_str()) {
            (_, Some("failed")) => "failed",
            ("apply", Some("succeeded")) => "applied",
            ("remove", Some("succeeded")) => "removed",
            _ => "pending",
        };
        site.edge_status[e.to_string()] =
            json!({ "status": status, "error": r["error"], "job_id": r["job_id"], "updated_at": state.clock.now().0 });
        results.push(json!({ "server_id": e, "result": r }));
    }
    site.updated_at = state.clock.now();
    state.store.put_site(site).await?;
    Ok(results)
}

/// Re-resolves typed targets so the upstreams follow where services run now
/// (placements move after a deploy or a failover), for the default rule and
/// every path rule.
async fn refresh_upstreams(state: &AppState, site: &mut Site) -> ApiResult<()> {
    // Rows written by Forge always hold valid target lists; a missing list is empty.
    let targets: Vec<Target> = serde_json::from_value(site.config["targets"].clone()).unwrap_or_default();
    let standby: Vec<Target> = serde_json::from_value(site.config["standby_targets"].clone()).unwrap_or_default();
    let mut spec = spec_of(site)?;
    let typed_rules = spec.rules.iter().any(|r| !r.targets.is_empty() || !r.standby_targets.is_empty());
    if targets.is_empty() && standby.is_empty() && !typed_rules {
        return Ok(());
    }
    let edges: Vec<String> = serde_json::from_value(site.config["edges"].clone()).unwrap_or_default();
    let r = super::routing::Resolver::new(state).await?.for_edges(edges);
    if !targets.is_empty() || !standby.is_empty() {
        let resolved = r.resolve(state, &targets, &standby).await?;
        spec.upstreams = resolved.active;
        spec.standby = resolved.standby;
        // Weights only while a rollout shifts traffic (#73).
        spec.weights = resolved.weights;
    }
    for rule in &mut spec.rules {
        if !rule.targets.is_empty() || !rule.standby_targets.is_empty() {
            let (active, standby) = r.upstreams(state, &rule.targets, &rule.standby_targets).await?;
            rule.upstreams = active;
            rule.standby = standby;
        }
    }
    spec.validate().map_err(ApiError::bad_request)?;
    site.config["spec"] = serde_json::to_value(&spec).map_err(forge_core::DomainError::internal)?;
    Ok(())
}

/// Re-applies every endpoint that routes to `app` (#73), so the edges follow
/// a rollout: new copies in, drained copies out, canary weights. Each runs
/// as whoever applied the endpoint last (as certificate renewals do), and
/// each edge tests the configuration before it reloads. An endpoint never
/// applied is left alone. Returns the names of the endpoints applied.
pub(crate) async fn reapply_for_app(
    state: &AppState,
    app: forge_core::ServiceId,
) -> forge_core::DomainResult<Vec<String>> {
    use forge_core::DomainError;
    let sites = state.store.list_sites().await?;
    let mut done = Vec::new();
    for mut site in sites.into_iter().filter(|s| crate::deploy::site_targets(&s.config, app)) {
        let Some(by) = site
            .edge_status
            .get("_applied_by")
            .cloned()
            .and_then(|v| serde_json::from_value::<super::fabric_reconcile::AppliedBy>(v).ok())
        else {
            continue;
        };
        refresh_upstreams(state, &mut site)
            .await
            .map_err(|e| DomainError::Invalid(format!("endpoint {}: {}", site.name, e.message)))?;
        let spec =
            spec_of(&site).map_err(|e| DomainError::Invalid(format!("endpoint {}: {}", site.name, e.message)))?;
        let edges = current_edges(state, &site).await.map_err(|e| DomainError::Invalid(e.message))?;
        // The edges may have changed since the endpoint was saved.
        state.endpoints.check_static_roots(&edges, &spec).await?;
        let attach = spec.tls == forge_catalog::sites::Tls::Uploaded;
        let site_id = site.id.clone();
        let mut failed = Vec::new();
        for (e, spec) in per_edge(state, &spec, &edges).await.map_err(|e| DomainError::Invalid(e.message))? {
            let r = submit_and_wait(
                state,
                by.principal.clone(),
                by.context,
                e,
                json!({ "type": "site", "action": "apply", "spec": spec }),
                Duration::from_secs(150),
                Some(EndpointFlow { site: &site_id, attach_certificate: attach }),
            )
            .await
            .map_err(|e| DomainError::Invalid(e.message))?;
            let status = match r["status"].as_str() {
                Some("succeeded") => "applied",
                Some("failed") => "failed",
                _ => "pending",
            };
            if status != "applied" {
                failed.push(format!("{e}: {}", r["error"].as_str().unwrap_or(status)));
            }
            site.edge_status[e.to_string()] = json!({ "status": status, "error": r["error"], "job_id": r["job_id"], "updated_at": state.clock.now().0 });
        }
        site.updated_at = state.clock.now();
        state.store.put_site(&site).await?;
        if !failed.is_empty() {
            return Err(DomainError::Conflict(format!(
                "endpoint {} did not apply on every edge: {}",
                site.name,
                failed.join("; ")
            )));
        }
        done.push(site.name.clone());
    }
    Ok(done)
}

/// The spec each edge gets. With Let's Encrypt on several edges, each issues
/// its own certificate and asks the others for challenges it does not hold.
pub(crate) async fn per_edge(
    state: &AppState,
    spec: &SiteSpec,
    edges: &[ServerId],
) -> ApiResult<Vec<(ServerId, SiteSpec)>> {
    let peers_of = if spec.tls == forge_catalog::sites::Tls::Acme && edges.len() > 1 {
        let r = super::routing::Resolver::new(state).await?.for_edges(edges);
        edges.iter().map(|e| (*e, r.address_of(&e.to_string()))).collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let mut out = Vec::with_capacity(edges.len());
    for e in edges {
        let mut s = spec.clone();
        s.acme_peers = peers_of.iter().filter(|(id, _)| id != e).filter_map(|(_, a)| a.clone()).collect();
        s.access_log = profile_access_log(state, *e).await;
        out.push((*e, s));
    }
    Ok(out)
}

/// The main access log format of the ingress profile that manages edge `e`
/// (its cluster's, when `e` is one of that profile's edges); empty
/// (= `combined`) otherwise. Sites write their logs in their own `server`
/// blocks, which replaces the profile's `http {}`-level `access_log`.
async fn profile_access_log(state: &AppState, e: ServerId) -> String {
    let Ok(server) = state.store.get_server(e).await else { return String::new() };
    let Some(c) = server.cluster_id else { return String::new() };
    let Ok(Some((profile, _))) = super::ingress::load_profile(state, c).await else { return String::new() };
    let id = e.to_string();
    profile
        .ingress
        .filter(|i| i.edges.iter().any(|x| x.server_id == id))
        .map(|i| i.nginx.access_log)
        .filter(|a| a != "combined")
        .unwrap_or_default()
}

pub(crate) fn spec_of_site(site: &Site) -> ApiResult<SiteSpec> {
    spec_of(site)
}

pub(crate) async fn edges_now(state: &AppState, site: &Site) -> ApiResult<Vec<ServerId>> {
    current_edges(state, site).await
}

pub async fn apply(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let mut site = load_visible(&state, &caller, ws, &id).await?;
    refresh_upstreams(&state, &mut site).await?;
    let checked = authorize_edges(&state, &caller, &site, "apply").await?;
    // Certificate renewals run later as whoever applied the endpoint last.
    site.edge_status["_applied_by"] = serde_json::to_value(super::fabric_reconcile::AppliedBy {
        principal: caller.principal(),
        context: caller.ctx(),
        at: state.clock.now(),
    })
    .map_err(forge_core::DomainError::internal)?;
    let edges = on_edges(&state, &caller, &mut site, "apply", checked).await?;
    caller.audit("site.apply", &ws_ref(site.workspace_id), Some(site.workspace_id), json!({ "site": site.name })).await;
    // When the cluster's ingress profile publishes DNS (with health-based
    // failover), it owns the records; per-endpoint records would fight it.
    if profile_dns_zone_server(&state, &site).await.is_some() {
        return Ok(Json(json!({ "edges": edges, "dns": null, "dns_managed_by": "profile" })));
    }
    let dns = match serde_json::from_value::<Option<DnsConfig>>(site.config["dns"].clone()).ok().flatten() {
        Some(d) => Some(publish_dns(&state, &caller, &spec_of(&site)?, &d).await?),
        None => None,
    };
    Ok(Json(json!({ "edges": edges, "dns": dns })))
}

pub async fn remove(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let mut site = load_visible(&state, &caller, ws, &id).await?;
    let checked = authorize_edges(&state, &caller, &site, "remove").await?;
    let edges = on_edges(&state, &caller, &mut site, "remove", checked).await?;
    caller
        .audit("site.remove", &ws_ref(site.workspace_id), Some(site.workspace_id), json!({ "site": site.name }))
        .await;
    Ok(Json(json!({ "edges": edges })))
}

/// `POST /sites/{id}/cache/clear`: empties the endpoint's proxy cache on
/// every edge (`proxy.manage` on each).
pub async fn clear_cache(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let site = load_visible(&state, &caller, ws, &id).await?;
    let (mut spec, edges) = authorize_edges(&state, &caller, &site, "clear_cache").await?;
    // Only `apply` needs the basic-auth users; no hashes in other jobs.
    spec.access.basic_auth = None;
    let mut results = Vec::new();
    for e in edges {
        let r = submit_and_wait(
            &state,
            caller.principal(),
            caller.ctx(),
            e,
            json!({ "type": "site", "action": "clear_cache", "spec": spec }),
            Duration::from_secs(60),
            Some(EndpointFlow { site: &site.id, attach_certificate: false }),
        )
        .await?;
        results.push(json!({ "server_id": e, "result": r }));
    }
    caller.audit("site.cache_clear", &ws_ref(ws), Some(ws), json!({ "site": site.name })).await;
    Ok(Json(json!({ "edges": results })))
}

/// Which of an endpoint's logs to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LogKind {
    Access,
    Error,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct LogQuery {
    /// The edge to read from (one of the endpoint's edges).
    #[schemars(with = "String")]
    pub server_id: ServerId,
    pub log: LogKind,
}

/// `GET /sites/{id}/logs?server_id=&log=access|error`: the last lines (up
/// to 1000, 512 KiB) of the endpoint's own log on one of its edges, and the
/// file's size. Needs `server.manage` on that edge; only this endpoint's log
/// files can be read.
pub async fn logs(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Path(id): Path<String>,
    axum::extract::Query(q): axum::extract::Query<LogQuery>,
) -> ApiResult<Json<Value>> {
    let site = load_visible(&state, &caller, ws, &id).await?;
    let mut spec = spec_of(&site)?;
    spec.access.basic_auth = None;
    if !current_edges(&state, &site).await?.contains(&q.server_id) {
        return Err(ApiError::bad_request("that node is not one of this endpoint's edges"));
    }
    caller.authorize(Action::ServerManage, server_ref(q.server_id)).await?;
    let action = match q.log {
        LogKind::Access => "access_log",
        LogKind::Error => "error_log",
    };
    let r = run_on_node(
        &state,
        &caller,
        q.server_id,
        json!({ "type": "site", "action": action, "spec": spec }),
        Duration::from_secs(20),
    )
    .await?;
    caller
        .audit(
            "site.logs_read",
            &ws_ref(ws),
            Some(ws),
            json!({ "site": site.name, "server_id": q.server_id, "log": action }),
        )
        .await;
    Ok(Json(r))
}

/// The DNS server of the site's cluster profile, if the profile publishes DNS.
async fn profile_dns_zone_server(state: &AppState, site: &Site) -> Option<String> {
    let cluster = site.config["cluster_id"].as_str()?.parse().ok()?;
    let (profile, _) = super::ingress::load_profile(state, cluster).await.ok()??;
    profile.ingress.and_then(|i| i.dns).map(|d| d.server_id)
}

/// Points every domain of the site at the edge addresses.
async fn publish_dns(state: &AppState, caller: &Caller, spec: &SiteSpec, d: &DnsConfig) -> ApiResult<Vec<Value>> {
    caller.authorize(Action::ProxyManage, server_ref(d.server_id)).await?;
    let chosen: Vec<&String> =
        if d.mode == "active_standby" { d.addresses.iter().take(1).collect() } else { d.addresses.iter().collect() };
    let mut out = Vec::new();
    for domain in &spec.domains {
        for (rtype, family) in [("A", false), ("AAAA", true)] {
            let contents: Vec<&String> = chosen.iter().copied().filter(|a| a.contains(':') == family).collect();
            if contents.is_empty() {
                continue;
            }
            let op = json!({ "op": "set_records", "zone": d.zone, "name": format!("{domain}."), "rtype": rtype, "ttl": d.ttl, "contents": contents });
            let r =
                run_on_node(state, caller, d.server_id, json!({ "type": "dns", "op": op }), Duration::from_secs(30))
                    .await?;
            out.push(json!({ "name": domain, "type": rtype, "status": r["status"], "error": r["error"] }));
        }
    }
    Ok(out)
}

pub async fn dns(
    State(state): State<AppState>,
    caller: Caller,
    Path(server): Path<ServerId>,
    Json(op): Json<Value>,
) -> ApiResult<Json<Value>> {
    let r = run_on_node(&state, &caller, server, json!({ "type": "dns", "op": op }), Duration::from_secs(25)).await?;
    if op["op"] != "zones" && op["op"] != "records" {
        let node = state.store.get_server(server).await?;
        caller.audit("dns.change", &server_ref(server), Some(node.workspace_id), json!({ "op": op })).await;
    }
    Ok(Json(r))
}
