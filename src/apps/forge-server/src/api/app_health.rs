//! Health checks of a service (issue #12, D-085): the typed policy
//! the console edits as a form or as YAML.
//!
//! - `GET  /services/{id}/health`: the policy, plain-language summaries
//!   and the legacy container `healthcheck` (shown for import). `health.view`.
//! - `POST /services/{id}/health/validate`: checks a draft without saving
//!   (field errors and summaries). `health.view`.
//! - `PUT  /services/{id}/health`: replaces the policy. `health.configure`,
//!   plus `health.command` (MFA) when a check runs code. Optimistic: send
//!   `expected_updated_at` to refuse a stale write. Audited as
//!   `service.health.update`. The saved policy is sent to every node
//!   the service is placed on (`health_ingest::push`); a node that was
//!   offline gets it when it reports under the old revision.
//!
//! Probe targets are checked here as well as in the catalog: besides the
//! workload itself, a remote host must be allowed explicitly, and it may not
//! be another node of the fleet (its host name or fabric address). The node
//! checks the resolved address again when the probe connects.

use std::collections::HashSet;

use axum::Json;
use axum::extract::{Path, State};
use forge_catalog::health::{FieldError, HealthPolicy, Probe, Purpose, summary};
use forge_core::model::Service;
use forge_core::ports::{FabricRepository, ServerRepository, ServiceRepository};
use forge_core::{Action, EntityKind, EntityRef, ServiceId, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::app_health_status::InstanceHealth;
use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

fn app_ref(id: ServiceId) -> EntityRef {
    EntityRef::new(EntityKind::Service, id)
}

/// One line per check, in section order.
#[derive(Debug, Serialize)]
pub struct CheckSummary {
    pub purpose: Purpose,
    pub name: String,
    pub enabled: bool,
    pub summary: String,
    /// Forge's default check (not configured; `name` is `default`).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub default: bool,
}

fn summaries(p: &HealthPolicy) -> Vec<CheckSummary> {
    p.checks()
        .map(|(purpose, c)| CheckSummary {
            purpose,
            name: c.name.clone(),
            enabled: c.enabled,
            summary: summary(purpose, c),
            default: false,
        })
        .collect()
}

/// The effective default checks (D-085), in their resolved form.
fn default_summaries(app: &Service, p: &HealthPolicy) -> Vec<CheckSummary> {
    if !p.default_checks {
        return Vec::new();
    }
    let port = app.spec.get("listen_port").and_then(Value::as_u64);
    let what = match app.runtime.as_str() {
        "systemd" => match port {
            Some(port) => format!("systemd unit active and TCP connect to port {port}"),
            None => "systemd unit active".to_owned(),
        },
        "container" => "container running, plus its image HEALTHCHECK if it has one".to_owned(),
        _ => return Vec::new(),
    };
    let line = |purpose, summary: String| CheckSummary {
        purpose,
        name: "default".into(),
        enabled: true,
        summary,
        default: true,
    };
    let mut out = vec![line(Purpose::Liveness, format!("{what}, every 10 s; unhealthy after 3 failures."))];
    if p.readiness.is_empty() {
        out.push(line(Purpose::Readiness, "Same as the default liveness check.".into()));
    }
    out
}

/// Something the operator should know about the checks.
#[derive(Debug, Serialize)]
pub struct HealthWarning {
    /// `ambiguous_listen_port`: another app on the node declares the same
    /// port, so the default TCP check is off there.
    pub code: &'static str,
    pub server_id: forge_core::ServerId,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct HealthView {
    pub policy: HealthPolicy,
    /// Configured checks, then the effective default checks (`default: true`).
    pub summaries: Vec<CheckSummary>,
    /// The container's Docker `HEALTHCHECK`, if any: run by Docker and not
    /// converted. The console offers to import it as a liveness check.
    pub legacy_healthcheck: Option<Value>,
    pub updated_at: Timestamp,
    /// The caller may save HTTP and TCP checks (`health.configure`).
    pub can_configure: bool,
    /// The caller may save exec and shell checks (`health.command`; with
    /// MFA required, only from an MFA-verified session).
    pub can_command: bool,
    /// Per node the app is placed on: status and check results.
    pub instances: Vec<InstanceHealth>,
    pub warnings: Vec<HealthWarning>,
}

/// Apps on the same node that declare the same listening port (D-085): the
/// default TCP check is off there, and nothing may be wired to that port.
async fn warnings(state: &AppState, app: &Service) -> ApiResult<Vec<HealthWarning>> {
    let Some(port) = app.spec.get("listen_port").and_then(Value::as_u64) else { return Ok(Vec::new()) };
    if app.runtime != "systemd" {
        return Ok(Vec::new());
    }
    let mine: Vec<forge_core::ServerId> =
        state.store.list_instances(app.id).await?.iter().map(|i| i.server_id).collect();
    let mut out = Vec::new();
    let all = state.store.list_all_instances(None).await?;
    for server in mine {
        let mut others = Vec::new();
        for i in all.iter().filter(|i| i.server_id == server && i.service_id != app.id) {
            if let Some(o) = state.store.get_service(i.service_id).await?
                && o.runtime == "systemd"
                && o.spec.get("listen_port").and_then(Value::as_u64) == Some(port)
            {
                others.push(o.name);
            }
        }
        if !others.is_empty() {
            others.sort();
            out.push(HealthWarning {
                code: "ambiguous_listen_port",
                server_id: server,
                message: format!(
                    "port {port} is also declared by {} on this node; the default TCP check is off there",
                    others.join(", ")
                ),
            });
        }
    }
    Ok(out)
}

async fn view(state: &AppState, caller: &Caller, app: &Service, policy: HealthPolicy) -> ApiResult<HealthView> {
    let mut sums = summaries(&policy);
    sums.extend(default_summaries(app, &policy));
    Ok(HealthView {
        summaries: sums,
        legacy_healthcheck: app.spec.get("healthcheck").cloned().filter(|v| !v.is_null()),
        updated_at: app.updated_at,
        can_configure: caller.can(Action::HealthConfigure, app_ref(app.id)).await,
        can_command: caller.can(Action::HealthCommand, app_ref(app.id)).await,
        instances: crate::app_health_status::instances(&state.store, &state.store, state.clock.now(), app, &policy)
            .await?,
        warnings: warnings(state, app).await?,
        policy,
    })
}

pub async fn get(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServiceId>,
) -> ApiResult<Json<HealthView>> {
    let app = super::services::load_own(&state, id).await?;
    caller.authorize(Action::HealthView, app_ref(id)).await?;
    let policy = crate::app_health_status::policy_of(&app);
    Ok(Json(view(&state, &caller, &app, policy).await?))
}
/// Most services one `GET /services/health-summary` covers.
const SUMMARY_MAX_APPS: usize = 500;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SummaryQuery {
    /// Only this environment's services.
    #[schemars(with = "Option<String>")]
    pub environment_id: Option<forge_core::EnvironmentId>,
}

/// Per-node health status of one service, without check details.
#[derive(Debug, Serialize)]
pub struct AppHealthSummary {
    pub service_id: ServiceId,
    pub instances: Vec<InstanceStatusView>,
}

#[derive(Debug, Serialize)]
pub struct InstanceStatusView {
    pub server_id: forge_core::ServerId,
    pub status: forge_health::InstanceStatus,
}

#[derive(Debug, Serialize)]
pub struct HealthSummaryList {
    pub items: Vec<AppHealthSummary>,
    /// More services were visible than one response covers
    /// (`SUMMARY_MAX_APPS`); filter by environment to see the rest.
    pub truncated: bool,
}

/// `GET /services/health-summary`: per-node health status of every
/// service the caller may see health for (`health.view`), so a list or
/// tree needs one request instead of one per service.
pub async fn summary_list(
    State(state): State<AppState>,
    caller: Caller,
    axum::extract::Query(q): axum::extract::Query<SummaryQuery>,
) -> ApiResult<Json<HealthSummaryList>> {
    let apps = state.store.list_services(q.environment_id).await?;
    let allowed = caller.can_many(Action::HealthView, apps.iter().map(|a| app_ref(a.id)).collect()).await;
    let visible: Vec<&Service> = apps.iter().zip(allowed).filter(|(_, ok)| *ok).map(|(a, _)| a).collect();
    let truncated = visible.len() > SUMMARY_MAX_APPS;
    let mut items = Vec::with_capacity(visible.len().min(SUMMARY_MAX_APPS));
    for app in visible.into_iter().take(SUMMARY_MAX_APPS) {
        let policy = crate::app_health_status::policy_of(app);
        let instances =
            crate::app_health_status::instances(&state.store, &state.store, state.clock.now(), app, &policy)
                .await?
                .into_iter()
                .map(|i| InstanceStatusView { server_id: i.server_id, status: i.status })
                .collect();
        items.push(AppHealthSummary { service_id: app.id, instances });
    }
    Ok(Json(HealthSummaryList { items, truncated }))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ValidateInput {
    /// The draft policy (same shape as `policy` in the response of `GET`).
    /// Unknown fields and wrong types come back as errors, not as a 400.
    pub policy: Value,
}

#[derive(Debug, Serialize)]
pub struct ValidateResult {
    pub valid: bool,
    pub errors: Vec<FieldError>,
    pub summaries: Vec<CheckSummary>,
    /// A check runs code on the node: saving needs `health.command` (MFA).
    pub runs_code: bool,
}

/// Host names and fabric addresses of every node: probes may not target them.
async fn fleet_targets(state: &AppState) -> HashSet<String> {
    let mut out = HashSet::new();
    for s in state.store.list_servers(None).await.unwrap_or_default() {
        out.insert(s.hostname.to_ascii_lowercase());
        if let Some(d) = s.display_name {
            out.insert(d.to_ascii_lowercase());
        }
    }
    for f in state.store.list_fabrics(None).await.unwrap_or_default() {
        for lease in f.leases.as_array().into_iter().flatten() {
            if let Some(ip) = lease.get("cidr").and_then(Value::as_str).and_then(|c| c.split('/').next()) {
                out.insert(ip.to_owned());
            }
        }
    }
    out
}

/// Catalog validation plus the fleet rule.
async fn check(state: &AppState, p: &HealthPolicy) -> Vec<FieldError> {
    let mut errs = p.validate();
    let fleet = fleet_targets(state).await;
    for (purpose, list) in
        [(Purpose::Startup, &p.startup), (Purpose::Readiness, &p.readiness), (Purpose::Liveness, &p.liveness)]
    {
        for (i, c) in list.iter().enumerate() {
            let Some(host) = c.probe.remote_host() else { continue };
            let h = host.trim_end_matches('.').to_ascii_lowercase();
            // `::ffff:<node-ip>` and NAT64 forms compare as the node's IPv4.
            let h = h.parse::<std::net::IpAddr>().map_or(h, |ip| forge_catalog::health::canonical(ip).to_string());
            let short = h.strip_suffix(".local").unwrap_or(&h);
            if fleet.contains(&h) || fleet.contains(short) {
                errs.push(FieldError {
                    field: format!("{}[{i}].probe.host", purpose.as_str()),
                    message: "another node of the fleet can't be probed from here".into(),
                });
            }
        }
    }
    errs
}

pub async fn validate(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServiceId>,
    Json(input): Json<ValidateInput>,
) -> ApiResult<Json<ValidateResult>> {
    super::services::load_own(&state, id).await?;
    caller.authorize(Action::HealthView, app_ref(id)).await?;
    let policy = match serde_json::from_value::<HealthPolicy>(input.policy) {
        Ok(p) => p,
        Err(e) => {
            return Ok(Json(ValidateResult {
                valid: false,
                errors: vec![FieldError { field: String::new(), message: format!("not a valid policy: {e}") }],
                summaries: Vec::new(),
                runs_code: false,
            }));
        }
    };
    let input = UpdateInput { policy, expected_updated_at: None };
    let errors = check(&state, &input.policy).await;
    Ok(Json(ValidateResult {
        valid: errors.is_empty(),
        summaries: summaries(&input.policy),
        runs_code: input.policy.runs_code(),
        errors,
    }))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UpdateInput {
    #[schemars(with = "serde_json::Value")]
    pub policy: HealthPolicy,
    /// `updated_at` from the `GET` the edit started from: a newer stored
    /// version refuses the write with `409`.
    #[serde(default)]
    pub expected_updated_at: Option<i64>,
}

/// Every vault reference in the policy's headers (`vault:<id>`).
fn vault_refs(p: &HealthPolicy) -> Vec<String> {
    let mut out: Vec<String> = p
        .checks()
        .filter_map(|(_, c)| match &c.probe {
            Probe::Http(h) => Some(h.headers.iter().filter_map(|hd| hd.secret.clone())),
            _ => None,
        })
        .flatten()
        .collect();
    out.sort();
    out.dedup();
    out
}

/// What the audit log shows of a policy: names, types and targets, never
/// scripts or header values.
fn audit_view(p: &HealthPolicy) -> Value {
    let checks: Vec<Value> = p
        .checks()
        .map(|(purpose, c)| {
            let (kind, target) = match &c.probe {
                Probe::Http(h) => {
                    // The query string can carry tokens: never audited.
                    let path = h.path.split(['?', '#']).next().unwrap_or_default();
                    let secrets: Vec<&str> = h.headers.iter().filter_map(|hd| hd.secret.as_deref()).collect();
                    ("http", json!({ "host": h.host, "port": h.port, "path": path, "vault_refs": secrets }))
                }
                Probe::Tcp(t) => ("tcp", json!({ "host": t.host, "port": t.port })),
                Probe::Exec(e) => ("exec", json!({ "program": e.argv.first() })),
                Probe::Shell(s) => ("shell", json!({ "interpreter": s.interpreter, "script_bytes": s.script.len() })),
            };
            json!({ "purpose": purpose, "name": c.name, "enabled": c.enabled, "type": kind, "target": target })
        })
        .collect();
    json!(checks)
}

pub async fn update(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServiceId>,
    Json(input): Json<UpdateInput>,
) -> ApiResult<Json<HealthView>> {
    let mut app = super::services::load_own(&state, id).await?;
    caller.authorize(Action::HealthConfigure, app_ref(id)).await?;
    if input.policy.runs_code() {
        caller.authorize(Action::HealthCommand, app_ref(id)).await?;
    }
    // Using a vault item in a probe header sends it to the probe target, so
    // it needs the same right as revealing it (and the item must exist). The
    // node checks again when the probe runs.
    for r in vault_refs(&input.policy) {
        let id = r
            .strip_prefix("vault:")
            .and_then(|s| s.parse::<forge_core::VaultItemId>().ok())
            .ok_or_else(|| ApiError::bad_request(format!("`{r}` is not a vault item reference")))?;
        super::vault::authorize_item_use(&state, &caller, app.workspace_id, id, "service.health.update").await?;
    }
    if let Some(expected) = input.expected_updated_at
        && expected != app.updated_at.0
    {
        return Err(ApiError::conflict("the service changed since you opened it; reload and apply again"));
    }
    let errors = check(&state, &input.policy).await;
    if !errors.is_empty() {
        let text: Vec<String> = errors.iter().map(|e| format!("{}: {}", e.field, e.message)).collect();
        return Err(ApiError::new(axum::http::StatusCode::UNPROCESSABLE_ENTITY, "invalid", text.join("; ")));
    }
    let before = crate::app_health_status::policy_of(&app);
    let value = if input.policy.is_empty() {
        Value::Null
    } else {
        serde_json::to_value(&input.policy).map_err(forge_core::DomainError::internal)?
    };
    if let Some(spec) = app.spec.as_object_mut() {
        if value.is_null() {
            spec.remove("health");
        } else {
            spec.insert("health".into(), value);
        }
    }
    app.updated_at = state.clock.now();
    // The policy and who saved it (D-111: a node's vault-header requests
    // are answered on that person's authority, for this revision only) are
    // written atomically, in one transaction. A failure changes nothing
    // and is audited.
    if let Err(e) = state.health_secrets.save_policy(&app, &input.policy, caller.principal()).await {
        caller
            .audit_failed(
                "service.health.update",
                &app_ref(id),
                Some(app.workspace_id),
                json!({ "service": app.name, "after": audit_view(&input.policy) }),
                &e.to_string(),
            )
            .await;
        return Err(e.into());
    }
    caller
        .audit(
            "service.health.update",
            &app_ref(id),
            Some(app.workspace_id),
            json!({ "service": app.name, "before": audit_view(&before), "after": audit_view(&input.policy) }),
        )
        .await;
    // The nodes run the new policy without a redeploy.
    for inst in state.store.list_instances(app.id).await? {
        crate::health_ingest::push(&state, &app, inst.server_id, true);
    }
    let policy = input.policy;
    Ok(Json(view(&state, &caller, &app, policy).await?))
}
