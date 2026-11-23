//! Agent self-update (GitHub issue #4).
//!
//! * `GET  /agents/versions`: every visible node's agent version vs. the
//!   control plane's, with an `outdated` flag.
//! * `POST /servers/{id}/agent/update`: an `agent.update` job for one node.
//! * `POST /agents/update` `{server_ids?: [...]}`: one job per listed (or
//!   every outdated, online Linux) node. Jobs run through the durable engine
//!   (bounded concurrency), each re-authorized as `server.command` (MFA).
//!   A node with an update already queued or running is refused (409).
//! * `POST /agents/rollouts`: one `agent.rollout` on the platform, batch by
//!   batch; refused (409) while another rollout is queued or running.
//! * `GET|PUT /agents/auto-update`: the platform-wide automatic update
//!   setting; saving needs `system.update` on the platform and MFA.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use forge_core::model::{AgentStatus, Job, NewJob};
use forge_core::ports::{JobRepository, ServerRepository};
use forge_core::{Action, EntityKind, EntityRef, ServerId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::agent_update_policy::{is_ahead, is_outdated};
use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::jobs::agent_update::{SUBMIT_LOCK, active_jobs, active_update};
use crate::state::AppState;

/// The agent version this control plane hands out (`MANIFEST.json` in the
/// agent folder, else this server's version).
fn target_version(state: &AppState) -> &str {
    state.dist.version()
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/agents/versions", get(versions))
        .route("/agents/update", post(update_many))
        .route("/agents/rollouts", post(rollout))
        .route("/agents/auto-update", get(auto_get).put(auto_put))
        .route("/servers/{id}/agent/update", post(update_one))
}

/// The nodes "every outdated node" covers: outdated, online, and able to
/// update themselves (or not saying; see [`crate::agent_update_policy`]).
async fn outdated_ids(state: &AppState) -> ApiResult<Vec<ServerId>> {
    Ok(state.agent_updates.eligible_ids().await?)
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RolloutRequest {
    /// Nodes to update; empty: every outdated online Linux node.
    #[serde(default)]
    #[schemars(with = "Vec<String>")]
    pub server_ids: Vec<ServerId>,
    /// Nodes per batch; the first batch is the canary.
    pub batch: usize,
}

/// Submits an `agent.rollout` job as `principal` with `ctx` (shared by the
/// API and the auto-update task). Every node is checked for `server.command`
/// up front, so a rollout never stops halfway on a permission it lacked; on
/// the automatic path each refused node is audited. Only one rollout is
/// queued or running at a time: a waiting rollout holds a worker slot, so
/// several could stall the job engine.
async fn submit_rollout(
    state: &AppState,
    ids: Vec<ServerId>,
    batch: usize,
    principal: forge_core::Principal,
    ctx: forge_core::AuthzContext,
    origin: &str,
) -> ApiResult<Job> {
    if ids.is_empty() {
        return Err(ApiError::conflict("no outdated online Linux node to update"));
    }
    if ids.len() > 500 {
        return Err(ApiError::bad_request("at most 500 nodes per rollout"));
    }
    if batch == 0 || batch > crate::jobs::agent_rollout::MAX_BATCH {
        return Err(ApiError::bad_request("batch: 1 to 100 nodes"));
    }
    if state.config.jobs.concurrency < 2 {
        return Err(ApiError::conflict("rollouts need [jobs] concurrency of at least 2"));
    }
    let mut refused: Option<(forge_core::DomainError, Vec<ServerId>)> = None;
    for id in &ids {
        let req = forge_core::AccessRequest::new(principal.clone(), Action::ServerCommand, server_ref(*id), ctx);
        if let Err(e) = state.authz.authorize(&req).await {
            refused.get_or_insert_with(|| (e, Vec::new())).1.push(*id);
        }
    }
    if let Some((e, nodes)) = refused {
        if origin == "auto" {
            for id in &nodes {
                state
                    .audit_system_outcome(
                        "agent.auto_update.node_refused",
                        forge_core::model::AuditOutcome::Denied,
                        json!({ "as": principal.to_string(), "server_id": id, "target": target_version(state) }),
                    )
                    .await;
            }
        }
        let mut err = ApiError::from(e);
        err.message = format!("{} ({} of {} nodes refused)", err.message, nodes.len(), ids.len());
        return Err(err);
    }
    // Each child job is filed under its node's own workspace.
    let workspaces: serde_json::Map<String, Value> = state
        .store
        .list_servers(None)
        .await?
        .into_iter()
        .filter(|s| ids.contains(&s.id))
        .map(|s| (s.id.to_string(), json!(s.workspace_id)))
        .collect();
    let _guard = SUBMIT_LOCK.lock().await;
    if let Some(j) = active_jobs(&state.store, "agent.rollout").await?.first() {
        return Err(ApiError::conflict(format!(
            "rollout {} is still queued or running: wait for it to finish or cancel it",
            j.id
        )));
    }
    // On the platform, not a workspace: the rollout lists nodes (and their
    // errors) from every workspace, so only platform-wide roles see or
    // cancel it.
    Ok(state
        .jobs
        .submit(NewJob {
            kind: "agent.rollout".into(),
            workspace_id: None,
            resource: EntityRef::platform(),
            principal,
            context: ctx,
            idempotency_key: None,
            payload: json!({
                "server_ids": ids, "workspaces": workspaces, "batch": batch, "origin": origin,
                "target": target_version(state),
            }),
            max_attempts: 1,
        })
        .await?)
}

/// `POST /agents/rollouts`: updates nodes in batches, canary first; stops at
/// the first failure.
pub async fn rollout(
    State(state): State<AppState>,
    caller: Caller,
    Json(req): Json<RolloutRequest>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let ids = if req.server_ids.is_empty() { outdated_ids(&state).await? } else { req.server_ids };
    let batches = ids.len().div_ceil(req.batch.max(1));
    let n = ids.len();
    let job = match submit_rollout(&state, ids, req.batch, caller.principal(), caller.ctx(), "manual").await {
        Ok(j) => j,
        Err(e) => {
            caller.audit_denied("agent.rollout", &EntityRef::platform(), None, &e.message).await;
            return Err(e);
        }
    };
    caller
        .audit(
            "agent.rollout",
            &EntityRef::new(EntityKind::Job, job.id),
            job.workspace_id,
            json!({ "nodes": n, "batch": req.batch, "batches": batches, "target": target_version(&state) }),
        )
        .await;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({ "job": job, "target": target_version(&state), "nodes": n, "batches": batches })),
    ))
}

// ------------------------------------------------- after a server update

/// How long the follow-up waits for agents to reconnect after the restart.
const FOLLOW_UP_WAIT: std::time::Duration = std::time::Duration::from_secs(180);
const FOLLOW_UP_POLL: std::time::Duration = std::time::Duration::from_secs(15);

/// The agent follow-up of a server update (D-098): once the new server
/// serves and the agents have reconnected, one canary rollout of every
/// outdated online Linux agent to this version, as the person who applied
/// the update (every node re-authorized as `server.command`). Audited as
/// `system.update.agents` with the outcome.
pub(crate) async fn update_follow_up(state: AppState, f: crate::update::AgentFollowUp) {
    use forge_core::model::AuditOutcome;
    if f.mode != "rollout" {
        return;
    }
    let who = f.principal.to_string();
    // Wait until the reconnecting fleet settles: the same outdated set on two
    // polls in a row, or the deadline.
    let deadline = tokio::time::Instant::now() + FOLLOW_UP_WAIT;
    let mut last: Option<Vec<ServerId>> = None;
    let ids = loop {
        tokio::time::sleep(FOLLOW_UP_POLL).await;
        let now = match outdated_ids(&state).await {
            Ok(ids) => ids,
            Err(e) => {
                tracing::warn!(error = %e.message, "agent follow-up: listing nodes failed");
                Vec::new()
            }
        };
        if (!now.is_empty() && last.as_ref() == Some(&now)) || tokio::time::Instant::now() >= deadline {
            break now;
        }
        last = Some(now);
    };
    if ids.is_empty() {
        state
            .audit_system(
                "system.update.agents",
                json!({ "as": who, "target": target_version(&state), "nodes": 0, "result": "nothing to update" }),
            )
            .await;
        return;
    }
    let n = ids.len();
    match submit_rollout(&state, ids, f.batch, f.principal, f.context, "update").await {
        Ok(job) => {
            state
                .audit_system(
                    "system.update.agents",
                    json!({ "as": who, "job": job.id, "nodes": n, "batch": f.batch, "target": target_version(&state) }),
                )
                .await;
        }
        Err(e) => {
            let outcome = if matches!(e.status, StatusCode::FORBIDDEN | StatusCode::UNAUTHORIZED) {
                AuditOutcome::Denied
            } else {
                AuditOutcome::Failure
            };
            tracing::warn!(error = %e.message, "agent follow-up of the update was not started");
            state
                .audit_system_outcome(
                    "system.update.agents",
                    outcome,
                    json!({ "as": who, "target": target_version(&state), "nodes": n, "error": e.message }),
                )
                .await;
        }
    }
}

// ------------------------------------------------------------ auto-update

/// Setting key of the auto-update configuration (managed here only).
pub const AUTO_KEY: &str = "agents.auto_update";
const DAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

/// Automatic agent updates (off by default), inside a weekly UTC window.
#[derive(Debug, Clone, Default, serde::Serialize, Deserialize)]
pub struct AutoUpdate {
    pub enabled: bool,
    /// Weekdays (`mon`…`sun`); empty: every day.
    #[serde(default)]
    pub days: Vec<String>,
    /// Window start and end, `HH:MM` UTC; the end may be past midnight.
    #[serde(default = "default_start")]
    pub start: String,
    #[serde(default = "default_end")]
    pub end: String,
    /// Nodes per batch (the first is the canary).
    #[serde(default = "default_batch")]
    pub batch: usize,
    /// Who turned it on: rollouts run as this person (re-authorized).
    #[serde(default)]
    pub enabled_by: Option<super::fabric_reconcile::AppliedBy>,
    /// The last version an automatic rollout was started for (one try per version).
    #[serde(default)]
    pub last_target: Option<String>,
    #[serde(default)]
    pub last_job: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
}

fn default_start() -> String {
    "02:00".into()
}
fn default_end() -> String {
    "05:00".into()
}
const fn default_batch() -> usize {
    1
}

fn minutes(hhmm: &str) -> Option<u32> {
    let (h, m) = hhmm.split_once(':')?;
    let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60 && hhmm.len() == 5).then_some(h * 60 + m)
}

impl AutoUpdate {
    fn validate(&self) -> Result<(), String> {
        let (Some(s), Some(e)) = (minutes(&self.start), minutes(&self.end)) else {
            return Err("start and end: HH:MM (UTC)".into());
        };
        if s == e {
            return Err("the window needs a length: start and end differ".into());
        }
        if let Some(d) = self.days.iter().find(|d| !DAYS.contains(&d.as_str())) {
            return Err(format!("unknown day `{d}`: mon, tue, wed, thu, fri, sat or sun"));
        }
        if self.batch == 0 || self.batch > crate::jobs::agent_rollout::MAX_BATCH {
            return Err("batch: 1 to 100 nodes".into());
        }
        Ok(())
    }

    /// Whether `now_ms` (epoch ms) is inside the window (UTC).
    #[must_use]
    pub fn in_window(&self, now_ms: i64) -> bool {
        let (Some(s), Some(e)) = (minutes(&self.start), minutes(&self.end)) else { return false };
        let mins_of_week = |ms: i64| {
            let days = ms.div_euclid(86_400_000);
            // 1970-01-01 was a Thursday (index 3 from Monday).
            let weekday = usize::try_from((days + 3).rem_euclid(7)).unwrap_or(0);
            let m = u32::try_from(ms.rem_euclid(86_400_000) / 60_000).unwrap_or(0);
            (weekday, m)
        };
        let (wd, m) = mins_of_week(now_ms);
        let day_ok = |d: usize| self.days.is_empty() || self.days.iter().any(|x| x == DAYS[d]);
        if s < e {
            day_ok(wd) && (s..e).contains(&m)
        } else {
            // Past midnight: the window belongs to the day it started.
            (m >= s && day_ok(wd)) || (m < e && day_ok((wd + 6) % 7))
        }
    }
}

async fn load_auto(state: &AppState) -> ApiResult<AutoUpdate> {
    use forge_core::ports::SettingsRepository;
    Ok(state
        .store
        .all_settings()
        .await?
        .into_iter()
        .find(|(k, _)| k == AUTO_KEY)
        .and_then(|(_, v)| serde_json::from_str(&v).ok())
        .unwrap_or_default())
}

async fn save_auto(state: &AppState, a: &AutoUpdate) -> ApiResult<()> {
    use forge_core::ports::SettingsRepository;
    let v = serde_json::to_string(a).map_err(forge_core::DomainError::internal)?;
    state.store.put_setting(AUTO_KEY, &v, state.clock.now()).await?;
    Ok(())
}

/// How long automatic rollouts may rely on the MFA-verified session that
/// saved the setting (90 days). After that the setting is saved again.
pub const AUTO_MFA_MAX_AGE_MS: i64 = 90 * 86_400_000;

fn auto_view(a: &AutoUpdate) -> Value {
    json!({
        "enabled": a.enabled, "days": a.days, "start": a.start, "end": a.end, "batch": a.batch,
        "enabled_by": a.enabled_by.as_ref().map(|b| b.principal.to_string()),
        "enabled_at": a.enabled_by.as_ref().map(|b| b.at.0),
        "expires_at": a.enabled_by.as_ref().map(|b| b.at.0.saturating_add(AUTO_MFA_MAX_AGE_MS)),
        "last_target": a.last_target, "last_job": a.last_job, "last_error": a.last_error,
        "timezone": "UTC",
    })
}

/// `GET /agents/auto-update`.
pub async fn auto_get(
    State(state): State<AppState>,
    caller: Caller,
    crate::auth::Ws(ws): crate::auth::Ws,
) -> ApiResult<Json<Value>> {
    caller.authorize(Action::View, EntityRef::new(EntityKind::Workspace, ws)).await?;
    Ok(Json(auto_view(&load_auto(&state).await?)))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AutoUpdateInput {
    pub enabled: bool,
    #[serde(default)]
    pub days: Vec<String>,
    #[serde(default = "default_start")]
    pub start: String,
    #[serde(default = "default_end")]
    pub end: String,
    #[serde(default = "default_batch")]
    pub batch: usize,
}

/// `PUT /agents/auto-update`: `system.update` on the platform (the setting
/// covers every node), and an MFA-verified session. Rollouts then run as
/// this person for at most [`AUTO_MFA_MAX_AGE_MS`]; saving again renews it.
pub async fn auto_put(
    State(state): State<AppState>,
    caller: Caller,
    Json(req): Json<AutoUpdateInput>,
) -> ApiResult<Json<Value>> {
    let platform = EntityRef::platform();
    if state.config.mfa_required() && !caller.ctx().mfa {
        caller.audit_denied("agent.auto_update.update", &platform, None, "MFA required").await;
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "step_up_required",
            "changing automatic agent updates needs an MFA-verified session",
        ));
    }
    caller.authorize(Action::SystemUpdate, platform.clone()).await?;
    let mut a = load_auto(&state).await?;
    a.enabled = req.enabled;
    a.days = req.days.iter().map(|d| d.trim().to_ascii_lowercase()).collect();
    a.start = req.start;
    a.end = req.end;
    a.batch = req.batch;
    a.validate().map_err(ApiError::bad_request)?;
    a.enabled_by = req.enabled.then(|| super::fabric_reconcile::AppliedBy {
        principal: caller.principal(),
        context: caller.ctx(),
        at: state.clock.now(),
    });
    // Saving again (with fresh MFA) clears the last refusal.
    if a.enabled {
        a.last_error = None;
    }
    save_auto(&state, &a).await?;
    caller
        .audit(
            "agent.auto_update.update",
            &platform,
            None,
            json!({ "enabled": a.enabled, "days": a.days, "start": a.start, "end": a.end, "batch": a.batch }),
        )
        .await;
    Ok(Json(auto_view(&a)))
}

/// Starts an automatic rollout when it is on, inside its window, and the
/// nodes are behind this control plane's version. One rollout per version:
/// once one started, a person looks before the next try. A refused or failed
/// submit is not counted and is tried again on the next tick in the window.
/// Rollouts run as the person who saved the setting, and only while that
/// person is enabled and their MFA is at most 90 days old.
pub(crate) async fn auto_run(state: AppState, mut shutdown: tokio::sync::watch::Receiver<bool>) {
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(300));
    loop {
        tokio::select! {
            _ = tick.tick() => {}
            _ = shutdown.changed() => return,
        }
        if let Err(e) = auto_tick(&state).await {
            tracing::warn!(error = %e.message, "automatic agent update");
        }
    }
}

/// Why automatic rollouts may not run as `by` at `now`: the person is gone
/// or disabled, or the MFA-verified session that saved the setting is older
/// than [`AUTO_MFA_MAX_AGE_MS`].
fn auto_refusal(
    user: Option<&forge_core::model::User>,
    by: &super::fabric_reconcile::AppliedBy,
    now: forge_core::Timestamp,
) -> Option<String> {
    match user {
        None => Some(format!("{} no longer exists: save the setting again as someone else", by.principal)),
        Some(u) if u.disabled => Some(format!("{} is disabled: save the setting again as someone else", by.principal)),
        _ if now.0.saturating_sub(by.at.0) > AUTO_MFA_MAX_AGE_MS => {
            Some("the setting was saved more than 90 days ago: save it again from an MFA-verified session".into())
        }
        _ => None,
    }
}

async fn auto_tick(state: &AppState) -> ApiResult<()> {
    use forge_core::model::AuditOutcome;
    use forge_core::ports::UserRepository;
    let a = load_auto(state).await?;
    let target = target_version(state);
    let now = state.clock.now();
    if !a.enabled || a.last_target.as_deref() == Some(target) || !a.in_window(now.0) {
        return Ok(());
    }
    let Some(by) = a.enabled_by.clone() else { return Ok(()) };
    let ids = outdated_ids(state).await?;
    if ids.is_empty() || !active_jobs(&state.store, "agent.rollout").await?.is_empty() {
        return Ok(());
    }
    let who = by.principal.to_string();
    let forge_core::Principal::User(uid) = &by.principal;
    let user = match state.store.get_user(*uid).await {
        Ok(u) => Some(u),
        Err(forge_core::DomainError::NotFound(_)) => None,
        Err(e) => return Err(e.into()),
    };
    let (last_target, last_job, last_error) = if let Some(why) = auto_refusal(user.as_ref(), &by, now) {
        state
            .audit_system_outcome(
                "agent.auto_update.refused",
                AuditOutcome::Denied,
                json!({ "as": who, "target": target, "reason": why }),
            )
            .await;
        (a.last_target.clone(), a.last_job.clone(), Some(why))
    } else {
        match submit_rollout(state, ids.clone(), a.batch, by.principal, by.context, "auto").await {
            Ok(job) => {
                state
                    .audit_system(
                        "agent.auto_update.start",
                        json!({ "as": who, "job": job.id, "nodes": ids.len(), "batch": a.batch, "target": target }),
                    )
                    .await;
                // One rollout per version: after it, a person looks before the next try.
                (Some(target.to_owned()), Some(job.id.to_string()), None)
            }
            Err(e) => {
                let outcome = if matches!(e.status, StatusCode::FORBIDDEN | StatusCode::UNAUTHORIZED) {
                    AuditOutcome::Denied
                } else {
                    AuditOutcome::Failure
                };
                state
                    .audit_system_outcome(
                        "agent.auto_update.failed",
                        outcome,
                        json!({ "as": who, "target": target, "error": e.message }),
                    )
                    .await;
                // Not recorded as tried: the next tick in the window tries again.
                (a.last_target.clone(), a.last_job.clone(), Some(e.message))
            }
        }
    };
    // Re-read so a setting saved meanwhile is kept; only the outcome is ours.
    let mut fresh = load_auto(state).await?;
    fresh.last_target = last_target;
    fresh.last_job = last_job;
    if fresh.enabled_by.as_ref().map(|b| b.at) == Some(by.at) {
        fresh.last_error = last_error;
    }
    save_auto(state, &fresh).await
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn at(day: i64, h: i64, m: i64) -> i64 {
        // 2026-11-16 is a Monday.
        let monday = 1_794_787_200_000_i64;
        monday + day * 86_400_000 + h * 3_600_000 + m * 60_000
    }

    #[test]
    fn windows_follow_days_and_midnight() {
        let w = AutoUpdate {
            start: "02:00".into(),
            end: "05:00".into(),
            days: vec!["tue".into()],
            batch: 1,
            ..AutoUpdate::default()
        };
        assert!(w.validate().is_ok());
        assert!(w.in_window(at(1, 3, 0)), "Tuesday 03:00");
        assert!(!w.in_window(at(0, 3, 0)), "Monday");
        assert!(!w.in_window(at(1, 5, 0)), "end is exclusive");
        let night = AutoUpdate {
            start: "23:00".into(),
            end: "01:00".into(),
            days: vec!["sat".into()],
            batch: 1,
            ..AutoUpdate::default()
        };
        assert!(night.in_window(at(5, 23, 30)), "Saturday 23:30");
        assert!(night.in_window(at(6, 0, 30)), "Sunday 00:30 belongs to Saturday's window");
        assert!(!night.in_window(at(6, 23, 30)), "Sunday night");
        let bad = AutoUpdate { start: "25:00".into(), end: "01:00".into(), batch: 1, ..AutoUpdate::default() };
        assert!(bad.validate().is_err());
    }

    fn person(disabled: bool) -> forge_core::model::User {
        forge_core::model::User {
            id: forge_core::UserId::new(),
            username: None,
            email: Some(format!("p{}@example.com", forge_core::UserId::new())),
            display_name: "P".into(),
            password_hash: None,
            totp_secret_sealed: None,
            oidc_subject: None,
            disabled,
            created_at: forge_core::Timestamp(0),
        }
    }

    fn applied(u: &forge_core::model::User, at: i64) -> super::super::fabric_reconcile::AppliedBy {
        super::super::fabric_reconcile::AppliedBy {
            principal: forge_core::Principal::User(u.id),
            context: forge_core::AuthzContext { mfa: true },
            at: forge_core::Timestamp(at),
        }
    }

    #[test]
    fn automatic_rollouts_need_an_enabled_person_with_recent_mfa() {
        let now = forge_core::Timestamp(200 * 86_400_000);
        let ok = person(false);
        let fresh = now.0 - 89 * 86_400_000;
        assert_eq!(auto_refusal(Some(&ok), &applied(&ok, fresh), now), None);
        let stale = auto_refusal(Some(&ok), &applied(&ok, now.0 - 91 * 86_400_000), now).unwrap();
        assert!(stale.contains("90 days"), "{stale}");
        let off = person(true);
        assert!(auto_refusal(Some(&off), &applied(&off, fresh), now).unwrap().contains("disabled"));
        assert!(auto_refusal(None, &applied(&ok, fresh), now).unwrap().contains("no longer exists"));
    }

    /// A window around `now` (UTC), every day.
    fn window_now(now_ms: i64) -> (String, String) {
        let m = now_ms.rem_euclid(86_400_000) / 60_000;
        let f = |m: i64| format!("{:02}:{:02}", m.rem_euclid(1440) / 60, m.rem_euclid(1440) % 60);
        (f(m - 60), f(m + 60))
    }

    async fn last_audit(state: &AppState) -> forge_core::model::AuditEntry {
        use forge_core::ports::AuditRepository;
        state.store.last_entry().await.unwrap().unwrap()
    }

    #[tokio::test]
    async fn auto_tick_refuses_and_audits_as_the_person_and_retries_a_failed_submit() {
        use forge_core::ports::{ServerRepository, UserRepository};
        let dir = tempfile::tempdir().unwrap();
        // Agent builds to hand out: without them no node is eligible.
        let dist = dir.path().join("dist");
        std::fs::create_dir_all(&dist).unwrap();
        for bin in ["forge-agent-linux-x86_64", "forge-agent-helper-linux-x86_64"] {
            std::fs::write(dist.join(bin), bin).unwrap();
        }
        let config = crate::config::ServerConfig {
            data_dir: dir.path().to_path_buf(),
            agent_dist_dir: Some(dist),
            cookie_secure: false,
            jobs: crate::config::JobSettings { lease_secs: 5, concurrency: 4, poll_ms: 50 },
            ..crate::config::ServerConfig::default()
        };
        let state = crate::state::Services::build(config).await.unwrap();
        let init = crate::bootstrap::init_admin(
            &state,
            None,
            Some("admin@example.com"),
            "Admin",
            "a-long-pass-phrase-1",
            false,
        )
        .await
        .unwrap();
        let now = state.clock.now();
        // One outdated, online Linux node.
        let node = forge_core::model::Server {
            id: ServerId::new(),
            workspace_id: init.workspace_id,
            environment_id: None,
            cluster_id: None,
            role: forge_core::model::NodeRole::default(),
            display_name: None,
            hostname: "auto-a".into(),
            os: "linux".into(),
            arch: "x86_64".into(),
            agent_version: "0.0.1-old".into(),
            status: AgentStatus::Online,
            last_seen_at: Some(now),
            cert_serial: None,
            cert_not_after: None,
            created_at: now,
        };
        state.store.create_server(&node).await.unwrap();
        let (start, end) = window_now(now.0);
        let setting = |by| AutoUpdate {
            enabled: true,
            start: start.clone(),
            end: end.clone(),
            batch: 1,
            enabled_by: Some(by),
            ..AutoUpdate::default()
        };

        // A disabled person: refused, audited as them, nothing recorded as tried.
        let off = person(true);
        state.store.create_user(&off).await.unwrap();
        save_auto(&state, &setting(applied(&off, now.0))).await.unwrap();
        auto_tick(&state).await.unwrap();
        let a = load_auto(&state).await.unwrap();
        assert!(a.last_error.as_deref().unwrap().contains("disabled"), "{a:?}");
        assert_eq!(a.last_target, None);
        let e = last_audit(&state).await;
        assert_eq!(
            (e.action.as_str(), e.outcome),
            ("agent.auto_update.refused", forge_core::model::AuditOutcome::Denied)
        );
        assert_eq!(e.details["as"], format!("user:{}", off.id));

        // MFA captured more than 90 days ago: refused.
        let on = person(false);
        state.store.create_user(&on).await.unwrap();
        save_auto(&state, &setting(applied(&on, now.0 - 91 * 86_400_000))).await.unwrap();
        auto_tick(&state).await.unwrap();
        let a = load_auto(&state).await.unwrap();
        assert!(a.last_error.as_deref().unwrap().contains("90 days"), "{a:?}");

        // Recent MFA, but no right to command the node: each refused node and
        // the failed attempt are audited as Denied, naming the person; the
        // version is not marked as tried, so the next tick tries again.
        save_auto(&state, &setting(applied(&on, now.0))).await.unwrap();
        auto_tick(&state).await.unwrap();
        let a = load_auto(&state).await.unwrap();
        assert!(a.last_error.is_some(), "{a:?}");
        assert_eq!(a.last_target, None, "a refused submit is retried");
        let e = last_audit(&state).await;
        assert_eq!(
            (e.action.as_str(), e.outcome),
            ("agent.auto_update.failed", forge_core::model::AuditOutcome::Denied)
        );
        assert_eq!(e.details["as"], format!("user:{}", on.id));
        use forge_core::ports::AuditRepository;
        let recent = state.store.recent_entries(None, 20).await.unwrap();
        assert!(
            recent.iter().any(|e| e.action == "agent.auto_update.node_refused"
                && e.details["server_id"] == node.id.to_string()
                && e.outcome == forge_core::model::AuditOutcome::Denied),
            "{recent:?}"
        );
    }
}

fn server_ref(id: ServerId) -> EntityRef {
    EntityRef::new(EntityKind::Server, id)
}

/// One node in `GET /agents/versions`.
#[derive(Debug, Serialize)]
pub struct AgentVersionRow {
    pub id: ServerId,
    pub hostname: String,
    pub display_name: Option<String>,
    pub status: AgentStatus,
    pub os: String,
    pub arch: String,
    pub agent_version: String,
    /// Older than `target`.
    pub outdated: bool,
    /// Newer than `target`: never updated or rolled back.
    pub ahead: bool,
    /// Outdated Linux node (the per-node Update button).
    pub updatable: bool,
    #[serde(flatten)]
    pub verdict: crate::agent_update_policy::NodeVerdict,
}

/// `GET /agents/versions`.
#[derive(Debug, Serialize)]
pub struct VersionsView {
    /// The agent version on offer.
    pub target: String,
    /// This server's own version.
    pub server: &'static str,
    pub outdated: usize,
    pub nodes: Vec<AgentVersionRow>,
}

/// Agent versions of the nodes you can see, with what an update would do.
pub async fn versions(State(state): State<AppState>, caller: Caller) -> ApiResult<Json<VersionsView>> {
    let all = state.agent_updates.nodes().await?;
    let allowed = caller.can_many(Action::View, all.iter().map(|n| server_ref(n.server.id)).collect()).await;
    let target = state.agent_updates.target().to_owned();
    let nodes: Vec<AgentVersionRow> = all
        .into_iter()
        .zip(allowed)
        .filter(|(n, ok)| *ok && n.server.status != AgentStatus::Revoked)
        .map(|(n, _)| {
            let s = n.server;
            let outdated = is_outdated(&s.agent_version, &target);
            AgentVersionRow {
                ahead: is_ahead(&s.agent_version, &target),
                updatable: outdated && s.os.eq_ignore_ascii_case("linux"),
                outdated,
                id: s.id,
                hostname: s.hostname,
                display_name: s.display_name,
                status: s.status,
                os: s.os,
                arch: s.arch,
                agent_version: s.agent_version,
                verdict: n.verdict,
            }
        })
        .collect();
    let outdated = nodes.iter().filter(|n| n.outdated).count();
    Ok(Json(VersionsView { target, server: crate::VERSION, outdated, nodes }))
}

/// Idempotency key of a single update: one queued or running update per node
/// and target version (a double submit finds it), but a failed or cancelled
/// one does not block a retry: each ended attempt moves the key on
/// (`…`, `…:r1`, `…:r2`…). Looked up key by key, so old attempts are found
/// however many jobs ran since.
async fn update_key(state: &AppState, id: ServerId) -> ApiResult<String> {
    use forge_core::model::JobStatus;
    let base = format!("agent.update:{id}:{}", target_version(state));
    for n in 0..MAX_UPDATE_ATTEMPTS {
        let key = if n == 0 { base.clone() } else { format!("{base}:r{n}") };
        match state.store.find_job_by_key("agent.update", &key).await? {
            Some(j) if matches!(j.status, JobStatus::Failed | JobStatus::Cancelled) => {}
            // Free, or queued, running or succeeded: a submit with it finds that job.
            _ => return Ok(key),
        }
    }
    Err(ApiError::conflict(format!(
        "this node's update to {} failed {MAX_UPDATE_ATTEMPTS} times; fix the node before trying again",
        target_version(state)
    )))
}

/// Most attempts of one node's update to one version.
const MAX_UPDATE_ATTEMPTS: u32 = 1000;

async fn submit(state: &AppState, caller: &Caller, id: ServerId) -> ApiResult<Job> {
    caller.authorize(Action::ServerCommand, server_ref(id)).await?;
    let node = state.store.get_server(id).await.map_err(|_| ApiError::not_found("node not found"))?;
    if node.status == AgentStatus::Revoked {
        return Err(ApiError::conflict("the node is revoked"));
    }
    // One update per node at a time: a rollout's update (or another key)
    // already in flight is not doubled. A double submit still finds its job.
    let _guard = SUBMIT_LOCK.lock().await;
    let key = update_key(state, id).await?;
    if let Some(other) = active_update(&state.store, id).await?
        && other.idempotency_key.as_deref() != Some(key.as_str())
    {
        let why = format!("another update of this node is queued or running (job {})", other.id);
        caller.audit_denied("agent.update", &server_ref(id), Some(node.workspace_id), &why).await;
        return Err(ApiError::conflict(why));
    }
    Ok(state
        .jobs
        .submit(NewJob {
            kind: "agent.update".into(),
            workspace_id: Some(node.workspace_id),
            resource: server_ref(id),
            principal: caller.principal(),
            context: caller.ctx(),
            idempotency_key: Some(key),
            payload: json!({ "server_id": id, "target": target_version(state) }),
            max_attempts: 3,
        })
        .await?)
}

/// Updates one node's agent.
pub async fn update_one(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServerId>,
) -> ApiResult<(StatusCode, Json<Job>)> {
    Ok((StatusCode::ACCEPTED, Json(submit(&state, &caller, id).await?)))
}

#[derive(Debug, Deserialize, Default, schemars::JsonSchema)]
pub struct UpdateMany {
    #[serde(default)]
    #[schemars(with = "Vec<String>")]
    pub server_ids: Vec<ServerId>,
}

/// Updates the listed nodes, or every outdated online Linux node.
pub async fn update_many(
    State(state): State<AppState>,
    caller: Caller,
    Json(req): Json<UpdateMany>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    // Listed nodes are always submitted, so a node's own refusal is recorded;
    // only "every outdated node" leaves out those that cannot take it.
    let ids: Vec<ServerId> = if req.server_ids.is_empty() { outdated_ids(&state).await? } else { req.server_ids };
    if ids.len() > 500 {
        return Err(ApiError::bad_request("at most 500 nodes per request"));
    }
    let mut jobs = Vec::new();
    let mut refused = Vec::new();
    for id in ids {
        match submit(&state, &caller, id).await {
            Ok(j) => jobs.push(json!({ "server_id": id, "job_id": j.id })),
            Err(e) => refused.push(json!({ "server_id": id, "error": e.message })),
        }
    }
    Ok((StatusCode::ACCEPTED, Json(json!({ "target": target_version(&state), "jobs": jobs, "refused": refused }))))
}
