//! Alerts (#11, D-087): rules and alarms of a workspace.
//!
//! Rules and alarm lists are authorized on the workspace (`alert.view`,
//! `alert.manage`); alarm lists are also filtered to resources the caller
//! may `view`, so an alarm never shows a node or service the caller
//! cannot see. A firing alarm that an active silence covers says so
//! (`silence`, D-112): it is still evaluated, only its notifications wait.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use forge_alerts::silence::SilenceMark;
use forge_core::alerts::{
    Alarm, AlarmEvent, AlarmFilter, AlertRepository, AlertRule, MAX_NOTE, MAX_RULE_NAME, RuleScope, Severity,
};
use forge_core::ports::{ServerRepository, TenancyRepository};
use forge_core::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::auth::{Caller, Ws};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

fn ws_ref(ws: WorkspaceId) -> EntityRef {
    EntityRef::new(EntityKind::Workspace, ws)
}

fn rule_ref(id: AlertRuleId) -> EntityRef {
    // Rules have no entity of their own; audit records name them by id.
    EntityRef::new(EntityKind::Workspace, format!("alert-rule:{id}"))
}

/// A subject: lowercase dotted segments of `[a-z0-9_-]`, at most 128 bytes
/// (the observation subject rule, D-085).
fn valid_subject(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.split('.').all(|seg| {
            !seg.is_empty()
                && seg.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
        })
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct WsQuery {
    /// The workspace (default: this deployment's workspace).
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub workspace: Option<WorkspaceId>,
}

/// `GET /api/v1/alert-rules`: the workspace's rules.
pub async fn list_rules(
    State(state): State<AppState>,
    caller: Caller,
    Ws(default): Ws,
    Query(q): Query<WsQuery>,
) -> ApiResult<Json<Vec<AlertRule>>> {
    let ws = q.workspace.unwrap_or(default);
    caller.authorize(Action::AlertView, ws_ref(ws)).await?;
    Ok(Json(state.store.list_rules(Some(ws)).await?))
}

/// `GET /api/v1/alert-rules/{id}`.
pub async fn get_rule(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<AlertRuleId>,
) -> ApiResult<Json<Value>> {
    let rule = state.store.get_rule(id).await.map_err(|_| ApiError::not_found("alert rule not found"))?;
    caller.authorize(Action::AlertView, ws_ref(rule.workspace_id)).await?;
    let describe = forge_alerts::parse_spec(&rule.spec).map(|s| s.describe()).ok();
    let mut v = serde_json::to_value(&rule).unwrap_or_default();
    v["condition"] = json!(describe);
    Ok(Json(v))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RuleInput {
    /// Default: this deployment's workspace. Fixed after creation.
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub workspace_id: Option<WorkspaceId>,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[schemars(with = "String")]
    pub severity: Severity,
    #[schemars(with = "Value")]
    pub scope: RuleScope,
    /// Observation subject, e.g. `node.agent.status`.
    pub subject: String,
    /// The condition: `{trigger, mode, recovery}` (see the REST reference).
    pub spec: Value,
}

const fn yes() -> bool {
    true
}

/// A validation failure as `400 bad_request` with its own message; other
/// errors keep their mapping.
fn bad_input(e: DomainError) -> ApiError {
    match e {
        DomainError::Invalid(m) => ApiError::bad_request(m),
        e => e.into(),
    }
}

/// Checks a rule's fields; the scope's nodes must be in the workspace.
async fn check(state: &AppState, ws: WorkspaceId, r: &RuleInput) -> ApiResult<()> {
    let name = r.name.trim();
    if name.is_empty() || name.chars().count() > MAX_RULE_NAME || name.chars().any(char::is_control) {
        return Err(ApiError::bad_request(format!(
            "the name is 1 to {MAX_RULE_NAME} characters, without control characters"
        )));
    }
    if r.description.chars().count() > 2000 {
        return Err(ApiError::bad_request("the description is at most 2000 characters"));
    }
    if !valid_subject(&r.subject) {
        return Err(ApiError::bad_request(
            "the subject is lowercase dotted segments of a-z, 0-9, _ and - (at most 128 bytes)",
        ));
    }
    // A typo would never fire: the subject must be one Forge produces, for
    // this scope (D-118).
    forge_alerts::catalog::check_subject(&r.subject, r.scope.kind()).map_err(bad_input)?;
    forge_alerts::parse_spec(&r.spec).map_err(ApiError::bad_request)?;
    check_scope_ids(state, ws, &r.scope).await
}

/// The ids a scope names must exist in the workspace.
async fn check_scope_ids(state: &AppState, ws: WorkspaceId, scope: &RuleScope) -> ApiResult<()> {
    if scope.len() > 500 {
        return Err(ApiError::bad_request("at most 500 ids in a scope"));
    }
    forge_alerts::catalog::check_scope_ids(&state.store, &state.store, ws, scope).await.map_err(bad_input)?;
    if let RuleScope::Servers { ids } = scope {
        for id in ids {
            let s = state.store.get_server(*id).await.map_err(|_| ApiError::bad_request(format!("no node {id}")))?;
            if s.workspace_id != ws {
                return Err(ApiError::bad_request(format!("no node {id}")));
            }
        }
    }
    if let RuleScope::Services { ids } = scope
        && ids.iter().any(|i| i.is_empty() || i.len() > 64)
    {
        return Err(ApiError::bad_request("service ids are 1 to 64 characters"));
    }
    Ok(())
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SubjectsQuery {
    /// The workspace whose observed health checks are listed (default: this
    /// deployment's workspace).
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub workspace: Option<WorkspaceId>,
}

/// What a rule can watch.
#[derive(Debug, Serialize)]
pub struct SubjectList {
    /// Every subject Forge produces, with the `health.<purpose>.{check}`
    /// patterns.
    pub subjects: Vec<forge_alerts::catalog::SubjectInfo>,
    /// Concrete `health.*` subjects observed on the workspace's
    /// services (sorted, at most 200).
    pub seen_health: Vec<String>,
}

/// `GET /api/v1/alert-subjects`: the subjects alert rules can watch.
pub async fn list_subjects(
    State(state): State<AppState>,
    caller: Caller,
    Ws(default): Ws,
    Query(q): Query<SubjectsQuery>,
) -> ApiResult<Json<SubjectList>> {
    let ws = q.workspace.unwrap_or(default);
    caller.authorize(Action::AlertView, ws_ref(ws)).await?;
    let seen_health = forge_alerts::catalog::seen_health_subjects(&state.store, &state.store, ws).await?;
    Ok(Json(SubjectList { subjects: forge_alerts::catalog::subjects(), seen_health }))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TemplatesQuery {
    /// Only templates for `node`, `service`, `database` or `certificate`
    /// (`any` or none: all).
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub place: Option<forge_alerts::catalog::Place>,
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub workspace: Option<WorkspaceId>,
}

/// `GET /api/v1/alert-rule-templates`: ready-made rules (D-118).
pub async fn list_templates(
    caller: Caller,
    Ws(default): Ws,
    Query(q): Query<TemplatesQuery>,
) -> ApiResult<Json<Vec<forge_alerts::catalog::AlertTemplate>>> {
    let ws = q.workspace.unwrap_or(default);
    caller.authorize(Action::AlertView, ws_ref(ws)).await?;
    Ok(Json(forge_alerts::catalog::templates(q.place)))
}

/// `POST /api/v1/alert-rules`.
pub async fn create_rule(
    State(state): State<AppState>,
    caller: Caller,
    Ws(default): Ws,
    Json(input): Json<RuleInput>,
) -> ApiResult<(StatusCode, Json<AlertRule>)> {
    let ws = input.workspace_id.unwrap_or(default);
    caller.authorize(Action::AlertManage, ws_ref(ws)).await?;
    state.store.get_workspace(ws).await.map_err(|_| ApiError::bad_request("no such workspace"))?;
    let max = state.config.alerts.max_rules_per_workspace as usize;
    if state.store.list_rules(Some(ws)).await?.len() >= max {
        return Err(ApiError::conflict(format!(
            "this workspace has the most alert rules allowed ({max}); delete one or raise [alerts] max_rules_per_workspace"
        )));
    }
    check(&state, ws, &input).await?;
    let now = state.clock.now();
    let rule = AlertRule {
        id: AlertRuleId::new(),
        workspace_id: ws,
        name: input.name.trim().to_owned(),
        description: input.description,
        enabled: input.enabled,
        severity: input.severity,
        scope: input.scope,
        subject: input.subject,
        spec: input.spec,
        revision: 1,
        created_by: caller.principal(),
        created_at: now,
        updated_at: now,
    };
    state.store.create_rule(&rule).await?;
    caller
        .audit("alert.rule.create", &rule_ref(rule.id), Some(ws), json!({ "name": rule.name, "subject": rule.subject }))
        .await;
    Ok((StatusCode::CREATED, Json(rule)))
}

/// `PUT /api/v1/alert-rules/{id}`: a new revision. A changed condition,
/// scope or subject ends the rule's firing alarms on the next pass (their
/// history is not mixed with the new condition's).
pub async fn update_rule(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<AlertRuleId>,
    Json(input): Json<RuleInput>,
) -> ApiResult<Json<AlertRule>> {
    let old = state.store.get_rule(id).await.map_err(|_| ApiError::not_found("alert rule not found"))?;
    caller.authorize(Action::AlertManage, ws_ref(old.workspace_id)).await?;
    if input.workspace_id.is_some_and(|w| w != old.workspace_id) {
        return Err(ApiError::bad_request("a rule cannot move to another workspace"));
    }
    check(&state, old.workspace_id, &input).await?;
    // Only a change of what is evaluated starts a new revision; a new name
    // or description keeps the timers.
    let evaluated_changed = old.subject != input.subject
        || old.scope != input.scope
        || old.spec != input.spec
        || old.enabled != input.enabled;
    let rule = AlertRule {
        name: input.name.trim().to_owned(),
        description: input.description,
        enabled: input.enabled,
        severity: input.severity,
        scope: input.scope,
        subject: input.subject,
        spec: input.spec,
        revision: if evaluated_changed { old.revision + 1 } else { old.revision },
        updated_at: state.clock.now(),
        ..old.clone()
    };
    state.store.update_rule(&rule).await?;
    caller
        .audit(
            "alert.rule.update",
            &rule_ref(id),
            Some(rule.workspace_id),
            json!({ "name": rule.name, "revision": rule.revision, "resets_evaluation": evaluated_changed }),
        )
        .await;
    Ok(Json(rule))
}

/// `DELETE /api/v1/alert-rules/{id}`. Its alarms stay as history; firing
/// ones end on the next pass ("the rule was deleted").
pub async fn delete_rule(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<AlertRuleId>,
) -> ApiResult<StatusCode> {
    let rule = state.store.get_rule(id).await.map_err(|_| ApiError::not_found("alert rule not found"))?;
    caller.authorize(Action::AlertManage, ws_ref(rule.workspace_id)).await?;
    state.store.delete_rule(id).await?;
    caller.audit("alert.rule.delete", &rule_ref(id), Some(rule.workspace_id), json!({ "name": rule.name })).await;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AlarmQuery {
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub workspace: Option<WorkspaceId>,
    /// Only firing alarms.
    #[serde(default)]
    pub active: bool,
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub rule: Option<AlertRuleId>,
    /// Only this server's alarms.
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub server: Option<ServerId>,
    /// 1–500, default 100.
    #[serde(default)]
    pub limit: Option<u32>,
}

/// An alarm with the silence that holds its notifications back now.
#[derive(Debug, Serialize)]
pub struct AlarmView {
    #[serde(flatten)]
    pub alarm: Alarm,
    /// Present while the alarm fires and an active silence covers it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub silence: Option<SilenceMark>,
}

/// An alarm, its silence and its timeline.
#[derive(Debug, Serialize)]
pub struct AlarmDetail {
    #[serde(flatten)]
    pub alarm: AlarmView,
    pub events: Vec<AlarmEvent>,
}

/// Adds to each alarm the silence that covers it now.
async fn with_silences(state: &AppState, alarms: Vec<Alarm>) -> ApiResult<Vec<AlarmView>> {
    let marks = state.notifier.silences.marks(&alarms).await?;
    Ok(alarms.into_iter().zip(marks).map(|(alarm, silence)| AlarmView { alarm, silence }).collect())
}

/// What `view` is checked on for an alarm's resource. Installations and
/// ingress certificates are no Cedar entities (D-118): an installation's
/// alarm is visible to whoever may view its node, a certificate's to whoever
/// may view its workspace (as the certificate list is).
fn view_target(a: &Alarm) -> EntityRef {
    match a.resource.kind {
        EntityKind::Installation => {
            a.server.map_or_else(|| ws_ref(a.workspace_id), |s| EntityRef::new(EntityKind::Server, s))
        }
        EntityKind::Certificate => ws_ref(a.workspace_id),
        _ => a.resource.clone(),
    }
}

/// Drops alarms on resources the caller cannot `view`.
async fn visible(caller: &Caller, alarms: Vec<Alarm>) -> Vec<Alarm> {
    let refs = alarms.iter().map(view_target).collect();
    let ok = caller.can_many(Action::View, refs).await;
    alarms.into_iter().zip(ok).filter(|(_, ok)| *ok).map(|(a, _)| a).collect()
}

/// `GET /api/v1/alarms`: newest first.
pub async fn list_alarms(
    State(state): State<AppState>,
    caller: Caller,
    Ws(default): Ws,
    Query(q): Query<AlarmQuery>,
) -> ApiResult<Json<Vec<AlarmView>>> {
    let ws = q.workspace.unwrap_or(default);
    caller.authorize(Action::AlertView, ws_ref(ws)).await?;
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    let mut filter = AlarmFilter {
        workspaces: Some(vec![ws]),
        active: q.active,
        rule: q.rule,
        resource: q.server.map(|s| EntityRef::new(EntityKind::Server, s)),
        before: None,
    };
    // The view filter runs after the query, so page on until the list is
    // full or the alarms run out (review of #23); bounded.
    let mut out: Vec<Alarm> = Vec::new();
    for _ in 0..20 {
        let page = state.store.list_alarms(&filter, limit).await?;
        let full = page.len() == limit as usize;
        filter.before = page.last().map(|a| (a.fired_at, a.id));
        out.extend(visible(&caller, page).await);
        if out.len() >= limit as usize || !full {
            break;
        }
    }
    out.truncate(limit as usize);
    Ok(Json(with_silences(&state, out).await?))
}

/// An alarm the caller may see: `alert.view` on its workspace and `view` on
/// its resource. Hidden ones answer `404` (no existence leak).
/// Operator events (acknowledgements and notes) one alarm may collect.
const MAX_OPERATOR_EVENTS: usize = 500;

/// Refuses another operator event on a full timeline.
async fn room_for_event(state: &AppState, id: AlarmId) -> ApiResult<()> {
    let n = state.store.alarm_events(id).await?.iter().filter(|e| e.actor.is_some()).count();
    if n >= MAX_OPERATOR_EVENTS {
        return Err(ApiError::conflict(format!(
            "this alarm already has {MAX_OPERATOR_EVENTS} notes and acknowledgements"
        )));
    }
    Ok(())
}

async fn alarm_for(state: &AppState, caller: &Caller, id: AlarmId, action: Action) -> ApiResult<Alarm> {
    let a = state.store.get_alarm(id).await.map_err(|_| ApiError::not_found("alarm not found"))?;
    let ok = caller.can_many(Action::View, vec![view_target(&a)]).await;
    if !ok.first().copied().unwrap_or(false) {
        return Err(ApiError::not_found("alarm not found"));
    }
    caller.authorize(action, ws_ref(a.workspace_id)).await?;
    Ok(a)
}

/// `GET /api/v1/alarms/{id}`: the alarm and its timeline.
pub async fn get_alarm(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<AlarmId>,
) -> ApiResult<Json<AlarmDetail>> {
    let a = alarm_for(&state, &caller, id, Action::AlertView).await?;
    let events = state.store.alarm_events(id).await?;
    let alarm = with_silences(&state, vec![a]).await?.pop().ok_or_else(|| ApiError::not_found("alarm not found"))?;
    Ok(Json(AlarmDetail { alarm, events }))
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NoteInput {
    /// Free text, at most 2000 characters.
    #[serde(default)]
    pub note: String,
}

fn clean_note(n: &str) -> ApiResult<String> {
    let n = n.trim();
    if n.chars().count() > MAX_NOTE || n.chars().any(|c| c.is_control() && c != '\n') {
        return Err(ApiError::bad_request(format!("notes are at most {MAX_NOTE} characters")));
    }
    Ok(n.to_owned())
}

/// `POST /api/v1/alarms/{id}/acknowledge`: marks that someone is on it. It
/// does not resolve the alarm.
pub async fn acknowledge(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<AlarmId>,
    body: Option<Json<NoteInput>>,
) -> ApiResult<Json<Alarm>> {
    let a = alarm_for(&state, &caller, id, Action::AlertManage).await?;
    let note = clean_note(&body.map(|b| b.0.note).unwrap_or_default())?;
    room_for_event(&state, id).await?;
    let now = state.clock.now();
    let who = caller.principal();
    let ev = AlarmEvent {
        alarm_id: id,
        at: now,
        kind: "acknowledged".into(),
        actor: Some(who.clone()),
        detail: json!({ "note": note }),
    };
    state.store.acknowledge(id, Some(&who), now, &ev).await?;
    caller.audit("alert.alarm.acknowledge", &a.resource, Some(a.workspace_id), json!({ "alarm": id })).await;
    Ok(Json(state.store.get_alarm(id).await?))
}

/// `POST /api/v1/alarms/{id}/unacknowledge`.
pub async fn unacknowledge(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<AlarmId>,
) -> ApiResult<Json<Alarm>> {
    let a = alarm_for(&state, &caller, id, Action::AlertManage).await?;
    room_for_event(&state, id).await?;
    let now = state.clock.now();
    let ev = AlarmEvent {
        alarm_id: id,
        at: now,
        kind: "unacknowledged".into(),
        actor: Some(caller.principal()),
        detail: json!({}),
    };
    state.store.acknowledge(id, None, now, &ev).await?;
    caller.audit("alert.alarm.unacknowledge", &a.resource, Some(a.workspace_id), json!({ "alarm": id })).await;
    Ok(Json(state.store.get_alarm(id).await?))
}

/// `POST /api/v1/alarms/{id}/notes`.
pub async fn add_note(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<AlarmId>,
    Json(input): Json<NoteInput>,
) -> ApiResult<StatusCode> {
    let a = alarm_for(&state, &caller, id, Action::AlertManage).await?;
    let note = clean_note(&input.note)?;
    if note.is_empty() {
        return Err(ApiError::bad_request("the note is empty"));
    }
    room_for_event(&state, id).await?;
    let ev = AlarmEvent {
        alarm_id: id,
        at: state.clock.now(),
        kind: "note".into(),
        actor: Some(caller.principal()),
        detail: json!({ "note": note }),
    };
    state.store.add_alarm_event(&ev).await?;
    caller.audit("alert.alarm.note", &a.resource, Some(a.workspace_id), json!({ "alarm": id })).await;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subjects_follow_the_observation_rule() {
        for ok in ["node.agent.status", "node.cpu.percent", "health.readiness.http-check", "a_b.c"] {
            assert!(valid_subject(ok), "{ok}");
        }
        for bad in ["", "Node.cpu", "a..b", ".a", "a.", "a b", "a/b", &"a".repeat(129)] {
            assert!(!valid_subject(bad), "{bad:?}");
        }
    }
}
