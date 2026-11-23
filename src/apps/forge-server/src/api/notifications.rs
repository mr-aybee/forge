//! Alarm notifications (#11, D-110): channels, policies and delivery history.
//!
//! * Channels are where messages leave Forge, so creating, changing,
//!   deleting and testing one needs `notification.manage` (administrators,
//!   MFA). A channel's URL is write-only: answers show its scheme, host and
//!   port. An email channel's SMTP password is write-only the same way
//!   (D-113). Signing with a vault item needs the right to use that item.
//! * Policies choose among existing channels: `alert.manage`. A policy can
//!   also email people by user and by role through an email channel
//!   (D-117); answers list the named users without an address in
//!   `warnings`.
//! * Reading channels, policies and deliveries: `alert.view`. Deliveries of
//!   alarms on resources the caller cannot `view` are left out. The
//!   addresses a delivery to people went to are shown only to callers with
//!   `notification.manage`; others see the count.
//! * A channel, policy or delivery looked up by id that the caller may not
//!   use answers the same `404` as an id of nothing; the denial is still
//!   recorded. Only a caller who has the role and lacks the second factor
//!   is told so (`step_up_required`), so the console can ask for it.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use forge_alerts::notify::{ChannelInput, EmailInput, PolicyInput, TestError};
use forge_core::alerts::{RuleScope, Severity};
use forge_core::notifications::{
    Channel, ChannelKind, Delivery, DeliveryAttempt, DeliveryFilter, DeliveryStatus, NotificationPolicy, PolicyEvent,
    SmtpTls,
};
use forge_core::{
    Action, AlarmId, AlertRuleId, ChannelId, DeliveryId, DomainError, EntityKind, EntityRef, NotificationPolicyId,
    Role, UserId, VaultItemId, WorkspaceId,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::auth::{Caller, Ws};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// Audit action names (one definition).
pub const AUDIT_CHANNEL_CREATE: &str = "notification.channel.create";
pub const AUDIT_CHANNEL_UPDATE: &str = "notification.channel.update";
pub const AUDIT_CHANNEL_DELETE: &str = "notification.channel.delete";
pub const AUDIT_CHANNEL_TEST: &str = "notification.channel.test";
pub const AUDIT_POLICY_CREATE: &str = "notification.policy.create";
pub const AUDIT_POLICY_UPDATE: &str = "notification.policy.update";
pub const AUDIT_POLICY_DELETE: &str = "notification.policy.delete";

/// `404` with a fixed text for a missing row; any other failure as it is.
fn or_not_found(e: DomainError, what: &'static str) -> ApiError {
    match e {
        DomainError::NotFound(_) => ApiError::not_found(what),
        other => other.into(),
    }
}

/// Authorizes `action` on the workspace of something found by its id. A
/// refusal is recorded by the authorization layer and answered as `what`
/// not found, so an id of another workspace cannot be told from an id of
/// nothing. The step-up answer passes: it goes only to a caller whose role
/// allows the action.
async fn authorize_found(caller: &Caller, action: Action, ws: WorkspaceId, what: &'static str) -> ApiResult<()> {
    match caller.authorize(action, ws_ref(ws)).await {
        Ok(()) => Ok(()),
        Err(e) if e.code == "step_up_required" => Err(e),
        Err(_) => Err(ApiError::not_found(what)),
    }
}

fn ws_ref(ws: WorkspaceId) -> EntityRef {
    EntityRef::new(EntityKind::Workspace, ws)
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct WsQuery {
    /// The workspace (default: this deployment's workspace).
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub workspace: Option<WorkspaceId>,
}

// ---- channels --------------------------------------------------------------

/// A channel as written. The URL never comes back in an answer.
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChannelBody {
    /// Default: this deployment's workspace. Fixed after creation.
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub workspace_id: Option<WorkspaceId>,
    /// 1 to 80 characters.
    pub name: String,
    /// `webhook`, `slack` or `email`.
    #[schemars(with = "String")]
    pub kind: ChannelKind,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// `https://host[:port]/path`. Required when creating; leave it out of
    /// an update to keep the stored URL.
    #[serde(default)]
    pub url: Option<String>,
    /// Vault item whose password signs webhook bodies (HMAC-SHA256).
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub secret_item: Option<VaultItemId>,
    /// The SMTP settings of an `email` channel (required for it, refused for
    /// the other kinds).
    #[serde(default)]
    pub email: Option<EmailBody>,
}

/// An email channel's SMTP settings as written. The password is write-only.
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EmailBody {
    /// SMTP server host name or IP address; the same address rules as a
    /// webhook URL apply.
    pub smtp_host: String,
    /// Default: 587 for `starttls`, 465 for `tls`, 25 for `none`.
    #[serde(default)]
    pub smtp_port: Option<u16>,
    /// `starttls` (default, required), `tls` (implicit TLS) or `none`
    /// (only where the operator allows unencrypted targets, and never with
    /// a user name).
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub tls: SmtpTls,
    /// SMTP user name; leave it out to send without authentication.
    #[serde(default)]
    pub username: Option<String>,
    /// SMTP password: required with a new user name and when the host, port
    /// or TLS mode changes; leave it out of an update to keep the stored one
    /// for the same server and user. Never returned.
    #[serde(default)]
    pub password: Option<String>,
    /// Sender: `forge@example.com` or `Forge <forge@example.com>`.
    pub from: String,
    /// 1 to 20 recipients.
    pub to: Vec<String>,
    /// Put before every subject, up to 40 characters (`[prod]`).
    #[serde(default)]
    pub subject_prefix: Option<String>,
}

impl EmailBody {
    fn into_input(self) -> EmailInput {
        EmailInput {
            smtp_host: self.smtp_host,
            smtp_port: self.smtp_port,
            tls: self.tls,
            username: self.username,
            password: self.password.filter(|p| !p.is_empty()),
            from: self.from,
            to: self.to,
            subject_prefix: self.subject_prefix,
        }
    }
}

// Written by hand: the derived form would print the URL.
impl std::fmt::Debug for ChannelBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChannelBody").field("name", &self.name).field("kind", &self.kind).finish_non_exhaustive()
    }
}

const fn yes() -> bool {
    true
}

impl ChannelBody {
    fn into_input(self) -> ChannelInput {
        ChannelInput {
            name: self.name,
            kind: self.kind,
            enabled: self.enabled,
            url: self.url.filter(|u| !u.trim().is_empty()),
            secret_item: self.secret_item,
            email: self.email.map(EmailBody::into_input),
        }
    }
}

/// What the audit log keeps of a channel: never the URL beyond its host,
/// never an SMTP password (only whether one is set).
fn channel_audit(c: &Channel) -> serde_json::Value {
    json!({
        "channel": c.id,
        "name": c.name,
        "kind": c.kind,
        "enabled": c.enabled,
        "target": c.target,
        "secret_item": c.secret_item,
        "email": c.email,
    })
}

/// `GET /api/v1/notification-channels`: the workspace's channels.
pub async fn list_channels(
    State(state): State<AppState>,
    caller: Caller,
    Ws(default): Ws,
    Query(q): Query<WsQuery>,
) -> ApiResult<Json<Vec<Channel>>> {
    let ws = q.workspace.unwrap_or(default);
    caller.authorize(Action::AlertView, ws_ref(ws)).await?;
    Ok(Json(state.notifier.channels(ws).await?))
}

/// A channel the caller may use for `action`. An unknown id and a channel
/// the caller may not use both answer `404`.
async fn channel_for(state: &AppState, caller: &Caller, id: ChannelId, action: Action) -> ApiResult<Channel> {
    const MISSING: &str = "notification channel not found";
    let c = state.notifier.channel(id).await.map_err(|e| or_not_found(e, MISSING))?;
    authorize_found(caller, action, c.workspace_id, MISSING).await?;
    Ok(c)
}

/// `GET /api/v1/notification-channels/{id}`.
pub async fn get_channel(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ChannelId>,
) -> ApiResult<Json<Channel>> {
    Ok(Json(channel_for(&state, &caller, id, Action::AlertView).await?))
}

/// `POST /api/v1/notification-channels`.
pub async fn create_channel(
    State(state): State<AppState>,
    caller: Caller,
    Ws(default): Ws,
    Json(body): Json<ChannelBody>,
) -> ApiResult<(StatusCode, Json<Channel>)> {
    let ws = body.workspace_id.unwrap_or(default);
    caller.authorize(Action::NotificationManage, ws_ref(ws)).await?;
    if ws != default {
        return Err(ApiError::bad_request("no such workspace"));
    }
    // Signing sends something derived from the vault item to the target:
    // the same right as using the item anywhere else.
    if let Some(item) = body.secret_item {
        super::vault::authorize_item_use(&state, &caller, ws, item, AUDIT_CHANNEL_CREATE).await?;
    }
    let channel = state.notifier.create_channel(ws, body.into_input(), caller.principal()).await?;
    caller.audit(AUDIT_CHANNEL_CREATE, &ws_ref(ws), Some(ws), channel_audit(&channel)).await;
    Ok((StatusCode::CREATED, Json(channel)))
}

/// `PUT /api/v1/notification-channels/{id}`.
pub async fn update_channel(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ChannelId>,
    Json(body): Json<ChannelBody>,
) -> ApiResult<Json<Channel>> {
    let old = channel_for(&state, &caller, id, Action::NotificationManage).await?;
    let ws = old.workspace_id;
    if body.workspace_id.is_some_and(|w| w != ws) {
        return Err(ApiError::bad_request("a channel cannot move to another workspace"));
    }
    let input = body.into_input();
    // A signed channel pointed at a new URL, or a channel given another
    // secret, uses the vault item anew.
    if let Some(item) = input.secret_item
        && (old.secret_item != Some(item) || input.url.is_some())
    {
        super::vault::authorize_item_use(&state, &caller, ws, item, AUDIT_CHANNEL_UPDATE).await?;
    }
    let url_changed = input.url.is_some();
    let password_changed = input.email.as_ref().is_some_and(|e| e.password.is_some());
    let channel = state.notifier.update_channel(id, input).await?;
    let mut details = channel_audit(&channel);
    if let Some(d) = details.as_object_mut() {
        d.insert("url_changed".into(), json!(url_changed));
        d.insert("password_changed".into(), json!(password_changed));
    }
    caller.audit(AUDIT_CHANNEL_UPDATE, &ws_ref(ws), Some(ws), details).await;
    Ok(Json(channel))
}

/// `DELETE /api/v1/notification-channels/{id}`. Refused while a policy uses
/// the channel. Its delivery history stays.
pub async fn delete_channel(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ChannelId>,
) -> ApiResult<StatusCode> {
    let old = channel_for(&state, &caller, id, Action::NotificationManage).await?;
    let channel = state.notifier.delete_channel(id).await?;
    caller
        .audit(AUDIT_CHANNEL_DELETE, &ws_ref(old.workspace_id), Some(old.workspace_id), channel_audit(&channel))
        .await;
    Ok(StatusCode::NO_CONTENT)
}

/// A delivery with its attempts.
#[derive(Debug, Serialize)]
pub struct DeliveryView {
    #[serde(flatten)]
    pub delivery: Delivery,
    pub attempts: Vec<DeliveryAttempt>,
}

/// `POST /api/v1/notification-channels/{id}/test`: sends one test message
/// now and answers with what happened. Every outcome is audited: accepted,
/// refused by the receiver, failed on the way, or not sent at all. `429`
/// while the last test to the channel is a few seconds old.
pub async fn test_channel(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ChannelId>,
) -> ApiResult<Json<DeliveryView>> {
    let channel = channel_for(&state, &caller, id, Action::NotificationManage).await?;
    let ws = channel.workspace_id;
    let about = json!({ "channel": id, "name": channel.name, "target": channel.target });
    let (delivery, attempts) = match state.notifier.test_channel(id).await {
        Ok(done) => done,
        Err(TestError::Cooldown(secs)) => {
            let e = TestError::Cooldown(secs).to_string();
            caller.audit_denied(AUDIT_CHANNEL_TEST, &ws_ref(ws), Some(ws), &e).await;
            return Err(ApiError::new(StatusCode::TOO_MANY_REQUESTS, "rate_limited", e));
        }
        // A request may have left before this failure: it is on record
        // either way.
        Err(TestError::Domain(e)) => {
            caller.audit_failed(AUDIT_CHANNEL_TEST, &ws_ref(ws), Some(ws), about, &e.to_string()).await;
            return Err(e.into());
        }
    };
    let details = json!({
        "channel": id,
        "name": channel.name,
        "target": channel.target,
        "delivery": delivery.id,
        "status": delivery.status,
        "http_status": delivery.last_http_status,
        "error": delivery.last_error,
    });
    caller.audit(AUDIT_CHANNEL_TEST, &ws_ref(ws), Some(ws), details).await;
    Ok(Json(DeliveryView { delivery: shown(&caller, ws, vec![delivery]).await.remove(0), attempts }))
}

// ---- policies --------------------------------------------------------------

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PolicyBody {
    /// Default: this deployment's workspace. Fixed after creation.
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub workspace_id: Option<WorkspaceId>,
    /// 1 to 80 characters.
    pub name: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Alarms of this severity and worse: `info`, `warning`, `major`, `critical`.
    #[schemars(with = "String")]
    pub min_severity: Severity,
    /// Only alarms of these rules (empty: every rule).
    #[serde(default)]
    #[schemars(with = "Vec<String>")]
    pub rules: Vec<AlertRuleId>,
    /// Only alarms on these resources (`{"kind": "servers", "ids": […]}`).
    #[serde(default)]
    #[schemars(with = "Option<serde_json::Value>")]
    pub scope: Option<RuleScope>,
    /// `fired`, `resolved`.
    #[schemars(with = "Vec<String>")]
    pub events: Vec<PolicyEvent>,
    /// Remind this often while firing and not acknowledged: 300 to 604800.
    #[serde(default)]
    pub repeat_interval_secs: Option<u32>,
    /// 0 to 10 channel ids (at least one unless the policy emails users or
    /// roles).
    #[serde(default)]
    #[schemars(with = "Vec<String>")]
    pub channels: Vec<ChannelId>,
    /// The email channel that sends to `email_users` and `email_roles`.
    /// Required when either is set; must be an `email` channel.
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub email_channel: Option<ChannelId>,
    /// Up to 100 user ids, emailed when they may see the alarm.
    #[serde(default)]
    #[schemars(with = "Vec<String>")]
    pub email_users: Vec<UserId>,
    /// Up to 20 roles: everyone holding one on the alarm's resource or an
    /// ancestor of it is emailed.
    #[serde(default)]
    #[schemars(with = "Vec<String>")]
    pub email_roles: Vec<Role>,
}

impl PolicyBody {
    fn into_input(self) -> PolicyInput {
        PolicyInput {
            name: self.name,
            enabled: self.enabled,
            min_severity: self.min_severity,
            rules: self.rules,
            scope: self.scope,
            events: self.events,
            repeat_interval_secs: self.repeat_interval_secs,
            channels: self.channels,
            email_channel: self.email_channel,
            email_users: self.email_users,
            email_roles: self.email_roles,
        }
    }
}

/// What a policy editor should know about a policy.
#[derive(Debug, Default, Serialize)]
pub struct PolicyWarnings {
    /// Named users without an email address: skipped until they set one.
    /// Left out when there are none.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub users_without_email: Vec<UserId>,
}

/// A policy as answered: the stored policy and its warnings.
#[derive(Debug, Serialize)]
pub struct PolicyView {
    #[serde(flatten)]
    pub policy: NotificationPolicy,
    pub warnings: PolicyWarnings,
}

async fn views(state: &AppState, policies: Vec<NotificationPolicy>) -> ApiResult<Vec<PolicyView>> {
    let warnings = state.notifier.policies_warnings(&policies).await?;
    Ok(policies
        .into_iter()
        .zip(warnings)
        .map(|(policy, users_without_email)| PolicyView { policy, warnings: PolicyWarnings { users_without_email } })
        .collect())
}

async fn view(state: &AppState, policy: NotificationPolicy) -> ApiResult<PolicyView> {
    let users_without_email = state.notifier.policy_warnings(&policy).await?;
    Ok(PolicyView { policy, warnings: PolicyWarnings { users_without_email } })
}

fn policy_audit(p: &NotificationPolicy) -> serde_json::Value {
    json!({
        "policy": p.id,
        "name": p.name,
        "enabled": p.enabled,
        "min_severity": p.min_severity,
        "events": p.events,
        "channels": p.channels,
        "rules": p.rules,
        "scope": p.scope,
        "repeat_interval_secs": p.repeat_interval_secs,
        // Who is emailed, by id and role: never addresses.
        "email_channel": p.email_channel,
        "email_users": p.email_users,
        "email_roles": p.email_roles,
    })
}

/// `GET /api/v1/notification-policies`: the workspace's policies.
pub async fn list_policies(
    State(state): State<AppState>,
    caller: Caller,
    Ws(default): Ws,
    Query(q): Query<WsQuery>,
) -> ApiResult<Json<Vec<PolicyView>>> {
    let ws = q.workspace.unwrap_or(default);
    caller.authorize(Action::AlertView, ws_ref(ws)).await?;
    Ok(Json(views(&state, state.notifier.policies(ws).await?).await?))
}

async fn policy_for(
    state: &AppState,
    caller: &Caller,
    id: NotificationPolicyId,
    action: Action,
) -> ApiResult<NotificationPolicy> {
    const MISSING: &str = "notification policy not found";
    let p = state.notifier.policy(id).await.map_err(|e| or_not_found(e, MISSING))?;
    authorize_found(caller, action, p.workspace_id, MISSING).await?;
    Ok(p)
}

/// `GET /api/v1/notification-policies/{id}`.
pub async fn get_policy(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<NotificationPolicyId>,
) -> ApiResult<Json<PolicyView>> {
    let p = policy_for(&state, &caller, id, Action::AlertView).await?;
    Ok(Json(view(&state, p).await?))
}

/// `POST /api/v1/notification-policies`.
pub async fn create_policy(
    State(state): State<AppState>,
    caller: Caller,
    Ws(default): Ws,
    Json(body): Json<PolicyBody>,
) -> ApiResult<(StatusCode, Json<PolicyView>)> {
    let ws = body.workspace_id.unwrap_or(default);
    caller.authorize(Action::AlertManage, ws_ref(ws)).await?;
    if ws != default {
        return Err(ApiError::bad_request("no such workspace"));
    }
    let policy = state.notifier.create_policy(ws, body.into_input(), caller.principal()).await?;
    caller.audit(AUDIT_POLICY_CREATE, &ws_ref(ws), Some(ws), policy_audit(&policy)).await;
    Ok((StatusCode::CREATED, Json(view(&state, policy).await?)))
}

/// `PUT /api/v1/notification-policies/{id}`.
pub async fn update_policy(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<NotificationPolicyId>,
    Json(body): Json<PolicyBody>,
) -> ApiResult<Json<PolicyView>> {
    let old = policy_for(&state, &caller, id, Action::AlertManage).await?;
    if body.workspace_id.is_some_and(|w| w != old.workspace_id) {
        return Err(ApiError::bad_request("a policy cannot move to another workspace"));
    }
    let policy = state.notifier.update_policy(id, body.into_input()).await?;
    // What it matched before and what it matches now: a policy edit can
    // send alarms somewhere new.
    let details = json!({ "policy": id, "before": policy_audit(&old), "after": policy_audit(&policy) });
    caller.audit(AUDIT_POLICY_UPDATE, &ws_ref(old.workspace_id), Some(old.workspace_id), details).await;
    Ok(Json(view(&state, policy).await?))
}

/// `DELETE /api/v1/notification-policies/{id}`. Deliveries already planned
/// are still sent.
pub async fn delete_policy(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<NotificationPolicyId>,
) -> ApiResult<StatusCode> {
    let old = policy_for(&state, &caller, id, Action::AlertManage).await?;
    let policy = state.notifier.delete_policy(id).await?;
    caller.audit(AUDIT_POLICY_DELETE, &ws_ref(old.workspace_id), Some(old.workspace_id), policy_audit(&policy)).await;
    Ok(StatusCode::NO_CONTENT)
}

// ---- deliveries ------------------------------------------------------------

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DeliveryQuery {
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub workspace: Option<WorkspaceId>,
    /// Only this alarm's deliveries.
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub alarm: Option<AlarmId>,
    /// Only deliveries to this channel.
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub channel: Option<ChannelId>,
    /// `pending`, `sending`, `sent`, `failed` or `cancelled`.
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub status: Option<DeliveryStatus>,
    /// 1–500, default 100.
    #[serde(default)]
    pub limit: Option<u32>,
}

/// Drops deliveries of alarms on resources the caller cannot `view`. Test
/// messages name no resource and stay.
async fn visible(caller: &Caller, deliveries: Vec<Delivery>) -> Vec<Delivery> {
    let (with, mut out): (Vec<Delivery>, Vec<Delivery>) = deliveries.into_iter().partition(|d| d.resource.is_some());
    let refs: Vec<EntityRef> = with.iter().filter_map(|d| d.resource.clone()).collect();
    let ok = caller.can_many(Action::View, refs).await;
    out.extend(with.into_iter().zip(ok).filter(|(_, ok)| *ok).map(|(d, _)| d));
    out.sort_by_key(|d| std::cmp::Reverse((d.created_at, d.id)));
    out
}

/// Hides the addresses of deliveries to people from callers who do not
/// manage notification channels; the count stays.
async fn shown(caller: &Caller, ws: WorkspaceId, mut deliveries: Vec<Delivery>) -> Vec<Delivery> {
    if deliveries.iter().any(|d| d.recipients.is_some()) && !caller.can(Action::NotificationManage, ws_ref(ws)).await {
        for r in deliveries.iter_mut().filter_map(|d| d.recipients.as_mut()) {
            r.redact();
        }
    }
    deliveries
}

/// `GET /api/v1/notification-deliveries`: newest first.
pub async fn list_deliveries(
    State(state): State<AppState>,
    caller: Caller,
    Ws(default): Ws,
    Query(q): Query<DeliveryQuery>,
) -> ApiResult<Json<Vec<Delivery>>> {
    let ws = q.workspace.unwrap_or(default);
    caller.authorize(Action::AlertView, ws_ref(ws)).await?;
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    let mut filter =
        DeliveryFilter { workspace: Some(ws), alarm: q.alarm, channel: q.channel, status: q.status, before: None };
    // The view filter runs after the query: page on until the list is full
    // or the deliveries run out; bounded.
    let mut out: Vec<Delivery> = Vec::new();
    for _ in 0..20 {
        let page = state.notifier.deliveries(&filter, limit).await?;
        let full = page.len() == limit as usize;
        filter.before = page.last().map(|d| (d.created_at, d.id));
        out.extend(visible(&caller, page).await);
        if out.len() >= limit as usize || !full {
            break;
        }
    }
    out.truncate(limit as usize);
    Ok(Json(shown(&caller, ws, out).await))
}

/// `GET /api/v1/notification-deliveries/{id}`: the delivery and its
/// attempts. `alert.view` on its workspace is checked first, then `view` on
/// the alarm's resource; an unknown id, a delivery of a workspace the
/// caller may not read and one of a hidden resource all answer `404`.
pub async fn get_delivery(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<DeliveryId>,
) -> ApiResult<Json<DeliveryView>> {
    const MISSING: &str = "delivery not found";
    let (delivery, attempts) = state.notifier.delivery(id).await.map_err(|e| or_not_found(e, MISSING))?;
    authorize_found(&caller, Action::AlertView, delivery.workspace_id, MISSING).await?;
    if visible(&caller, vec![delivery.clone()]).await.is_empty() {
        return Err(ApiError::not_found(MISSING));
    }
    let ws = delivery.workspace_id;
    Ok(Json(DeliveryView { delivery: shown(&caller, ws, vec![delivery]).await.remove(0), attempts }))
}
