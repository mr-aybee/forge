//! Alarm notifications (#11, D-110): channels, policies and the delivery
//! outbox.
//!
//! A **channel** is where a message goes (a webhook, a Slack-compatible
//! incoming webhook, or email over SMTP, D-113). A **policy** says which alarms notify which channels.
//! Every alarm transition is written to an outbox in the transaction that
//! changes the alarm ([`AlarmTransition`]); planning turns a transition into
//! **deliveries**, one per policy and channel, each with a unique
//! [`Delivery::dedupe_key`], so planning the same transition twice adds
//! nothing. Sending never touches the alarm.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::alerts::{RuleScope, Severity};
use crate::{
    AlarmId, AlertRuleId, ChannelId, DeliveryId, DomainResult, EntityRef, NotificationPolicyId, Principal, Role,
    SilenceId, Timestamp, UserId, VaultItemId, WorkspaceId,
};

/// Longest channel or policy name, in characters.
pub const MAX_NAME: usize = 80;
/// Longest target URL, in bytes.
pub const MAX_URL: usize = 2000;
/// Most channels one policy notifies.
pub const MAX_POLICY_CHANNELS: usize = 10;
/// Most rules one policy names.
pub const MAX_POLICY_RULES: usize = 100;
/// Most channels of one workspace.
pub const MAX_CHANNELS: usize = 50;
/// Most policies of one workspace.
pub const MAX_POLICIES: usize = 100;
/// Most recipients of one email channel.
pub const MAX_EMAIL_RECIPIENTS: usize = 20;
/// Longest email address, in bytes (RFC 5321 path limit).
pub const MAX_EMAIL_ADDRESS: usize = 254;
/// Longest SMTP host name, in bytes.
pub const MAX_SMTP_HOST: usize = 253;
/// Longest SMTP user name, in bytes.
pub const MAX_SMTP_USERNAME: usize = 256;
/// Longest SMTP password, in bytes.
pub const MAX_SMTP_PASSWORD: usize = 1024;
/// Longest subject prefix, in characters.
pub const MAX_SUBJECT_PREFIX: usize = 40;
/// Most users one policy emails by name.
pub const MAX_POLICY_EMAIL_USERS: usize = 100;
/// Most roles one policy emails.
pub const MAX_POLICY_EMAIL_ROLES: usize = 20;
/// Most addresses one policy delivery is sent to (people found through
/// roles included). The rest are recorded as skipped (`too_many`).
pub const MAX_RESOLVED_RECIPIENTS: usize = 200;
/// Shortest and longest reminder interval, in seconds (5 minutes, 7 days).
pub const REPEAT_RANGE_SECS: std::ops::RangeInclusive<u32> = 300..=604_800;
/// Most reminders one alarm sends to one channel of one policy.
pub const MAX_REMINDERS: i64 = 50;

/// How a channel speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelKind {
    /// `POST` of Forge's JSON payload, optionally signed (HMAC-SHA256).
    Webhook,
    /// `POST` of `{"text": …}`: Slack incoming webhooks and compatible
    /// receivers (Mattermost, Discord's `/slack` endpoint).
    Slack,
    /// A plain-text email over SMTP (D-113).
    Email,
}

impl ChannelKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            ChannelKind::Webhook => "webhook",
            ChannelKind::Slack => "slack",
            ChannelKind::Email => "email",
        }
    }
}

/// How an email channel protects its SMTP connection.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SmtpTls {
    /// Plain connection upgraded with `STARTTLS`, which is required: a
    /// server that does not offer it is not sent to. Port 587 by default.
    #[default]
    Starttls,
    /// TLS from the first byte ("implicit TLS", SMTPS). Port 465 by default.
    Tls,
    /// No encryption. Allowed only where the operator allows unencrypted
    /// targets, and never with a user name. Port 25 by default.
    None,
}

impl SmtpTls {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            SmtpTls::Starttls => "starttls",
            SmtpTls::Tls => "tls",
            SmtpTls::None => "none",
        }
    }

    /// The usual port of this mode.
    #[must_use]
    pub const fn default_port(self) -> u16 {
        match self {
            SmtpTls::Starttls => 587,
            SmtpTls::Tls => 465,
            SmtpTls::None => 25,
        }
    }
}

/// The settings of an email channel that people may see. The SMTP password
/// is sealed with the channel (in `sealed_url`) and never serialized here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmailSettings {
    pub smtp_host: String,
    pub smtp_port: u16,
    #[serde(default)]
    pub tls: SmtpTls,
    /// SMTP user name; `None`: the server is used without authentication.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    /// Whether a password is stored (it is never returned).
    #[serde(default)]
    pub password_set: bool,
    /// The sender (`ops@example.com` or `Forge <ops@example.com>`).
    pub from: String,
    /// 1 to [`MAX_EMAIL_RECIPIENTS`] recipients.
    pub to: Vec<String>,
    /// Put before every subject (`[prod]`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject_prefix: Option<String>,
}

/// A configured transport. The URL is sealed at rest and never serialized:
/// webhook URLs often carry a token. `target` is what people see. An email
/// channel seals its SMTP password (or nothing) in the same place and shows
/// its other settings in `email`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Channel {
    pub id: ChannelId,
    pub workspace_id: WorkspaceId,
    pub name: String,
    pub kind: ChannelKind,
    pub enabled: bool,
    /// The URL's scheme, host and port only (`https://hooks.example.com`).
    pub target: String,
    /// The whole URL, sealed (envelope encryption bound to the channel id).
    /// For an email channel: its SMTP password, sealed (empty when none).
    #[serde(skip)]
    pub sealed_url: String,
    /// The SMTP settings of an email channel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<EmailSettings>,
    /// Vault item whose password signs webhook bodies. Resolved when a
    /// message is sent; never copied into the channel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_item: Option<VaultItemId>,
    pub created_by: Principal,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

impl std::fmt::Debug for Channel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The sealed URL stays out of logs even in its sealed form.
        f.debug_struct("Channel")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("kind", &self.kind)
            .field("enabled", &self.enabled)
            .field("target", &self.target)
            .finish_non_exhaustive()
    }
}

/// What a policy reacts to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyEvent {
    /// The alarm opened (and reminders while it keeps firing).
    Fired,
    /// The alarm closed: it recovered, or tracking ended without recovery.
    Resolved,
}

/// Which alarms notify which channels.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationPolicy {
    pub id: NotificationPolicyId,
    pub workspace_id: WorkspaceId,
    pub name: String,
    pub enabled: bool,
    /// Alarms of this severity and worse.
    pub min_severity: Severity,
    /// Only alarms of these rules (empty: every rule).
    #[serde(default)]
    pub rules: Vec<AlertRuleId>,
    /// Only alarms on these resources (`None`: every resource).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<RuleScope>,
    pub events: Vec<PolicyEvent>,
    /// Remind this often while the alarm fires and nobody acknowledged it
    /// (`None`: no reminders).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat_interval_secs: Option<u32>,
    pub channels: Vec<ChannelId>,
    /// The email channel (SMTP server) that sends to `email_users` and
    /// `email_roles`. Set exactly when one of them is not empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email_channel: Option<ChannelId>,
    /// People emailed by name, when they may see the alarm (`alert.view` on
    /// its resource) and have an address.
    #[serde(default)]
    pub email_users: Vec<UserId>,
    /// Roles whose holders on the alarm's resource, or on any of its
    /// ancestors, are emailed: the people responsible for the resource.
    #[serde(default)]
    pub email_roles: Vec<Role>,
    pub created_by: Principal,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

impl NotificationPolicy {
    /// Whether the policy emails people (named users or role holders).
    #[must_use]
    pub fn emails_people(&self) -> bool {
        self.email_channel.is_some() && (!self.email_users.is_empty() || !self.email_roles.is_empty())
    }
}

/// The email recipients of a policy, as stored with it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyEmailRecipients {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<ChannelId>,
    #[serde(default)]
    pub users: Vec<UserId>,
    #[serde(default)]
    pub roles: Vec<Role>,
}

/// Why a person a policy names was not emailed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecipientSkip {
    /// The account has no email address.
    NoEmail,
    /// The person may not see the alarm (`alert.view` on its resource), or
    /// the account no longer exists.
    NoAccess,
    /// The account is disabled.
    Disabled,
    /// Over [`MAX_RESOLVED_RECIPIENTS`].
    TooMany,
}

impl RecipientSkip {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            RecipientSkip::NoEmail => "no_email",
            RecipientSkip::NoAccess => "no_access",
            RecipientSkip::Disabled => "disabled",
            RecipientSkip::TooMany => "too_many",
        }
    }
}

/// A person left out of a delivery. Never their address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SkippedRecipient {
    pub user_id: UserId,
    pub reason: RecipientSkip,
}

/// Whom a policy delivery to people goes to, fixed when it is planned:
/// retries and the message after a silence send to the same addresses.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryRecipients {
    /// How many addresses the message goes to.
    pub count: usize,
    /// The addresses. Only callers who manage notification channels see
    /// them; [`DeliveryRecipients::redact`] empties the list for others.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub to: Vec<String>,
    /// People left out, and why.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<SkippedRecipient>,
}

impl DeliveryRecipients {
    /// Drops the addresses; the count and the skipped ids stay.
    pub fn redact(&mut self) {
        self.to.clear();
    }
}

/// An alarm transition, as the outbox records it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransitionKind {
    Fired,
    Resolved,
    Ended,
}

impl TransitionKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            TransitionKind::Fired => "fired",
            TransitionKind::Resolved => "resolved",
            TransitionKind::Ended => "ended",
        }
    }
}

/// One row of the transition outbox, written with the alarm change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlarmTransition {
    pub alarm_id: AlarmId,
    pub workspace_id: WorkspaceId,
    pub kind: TransitionKind,
    pub at: Timestamp,
}

/// Why a message is sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryEvent {
    Fired,
    Resolved,
    /// Tracking ended without recovery evidence.
    Ended,
    /// Still firing, not acknowledged.
    Reminder,
    /// A test message someone asked for.
    Test,
}

impl DeliveryEvent {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            DeliveryEvent::Fired => "fired",
            DeliveryEvent::Resolved => "resolved",
            DeliveryEvent::Ended => "ended",
            DeliveryEvent::Reminder => "reminder",
            DeliveryEvent::Test => "test",
        }
    }

    /// Messages that say the alarm is firing.
    #[must_use]
    pub const fn is_firing(self) -> bool {
        matches!(self, DeliveryEvent::Fired | DeliveryEvent::Reminder)
    }
}

impl From<TransitionKind> for DeliveryEvent {
    fn from(k: TransitionKind) -> Self {
        match k {
            TransitionKind::Fired => DeliveryEvent::Fired,
            TransitionKind::Resolved => DeliveryEvent::Resolved,
            TransitionKind::Ended => DeliveryEvent::Ended,
        }
    }
}

/// Where a delivery stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryStatus {
    /// Waiting for its first or next attempt.
    Pending,
    /// An attempt is running (leased).
    Sending,
    /// The receiver accepted it (a 2xx answer).
    Sent,
    /// Refused for good, or out of attempts.
    Failed,
    /// Not sent because it no longer applies; `detail` says why.
    Cancelled,
    /// Held back by a silence (`silence_id`); `detail` gives its reason.
    Suppressed,
}

impl DeliveryStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            DeliveryStatus::Pending => "pending",
            DeliveryStatus::Sending => "sending",
            DeliveryStatus::Sent => "sent",
            DeliveryStatus::Failed => "failed",
            DeliveryStatus::Cancelled => "cancelled",
            DeliveryStatus::Suppressed => "suppressed",
        }
    }

    #[must_use]
    pub const fn is_final(self) -> bool {
        matches!(
            self,
            DeliveryStatus::Sent | DeliveryStatus::Failed | DeliveryStatus::Cancelled | DeliveryStatus::Suppressed
        )
    }
}

/// Why an attempt failed. A class, never the receiver's text: an answer
/// body or an error string could carry the URL's token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureClass {
    /// No answer in time.
    Timeout,
    /// The name did not resolve or the connection was refused or reset.
    Connect,
    /// The TLS handshake or certificate check failed.
    Tls,
    /// The target resolves to an address Forge does not call.
    BlockedTarget,
    /// The receiver answered with a status outside 2xx.
    HttpStatus,
    /// The signing secret is gone or cannot be read.
    Secret,
    /// The channel's stored configuration cannot be used.
    Config,
    /// The SMTP server answered with a reply code outside 2xx (the code is
    /// kept in the status field): 4xx is retried, 5xx is final.
    SmtpReply,
}

impl FailureClass {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            FailureClass::Timeout => "timeout",
            FailureClass::Connect => "connect",
            FailureClass::Tls => "tls",
            FailureClass::BlockedTarget => "blocked_target",
            FailureClass::HttpStatus => "http_status",
            FailureClass::Secret => "secret",
            FailureClass::Config => "config",
            FailureClass::SmtpReply => "smtp_reply",
        }
    }
}

/// One message intent: an event of one alarm, for one channel of one policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delivery {
    pub id: DeliveryId,
    pub workspace_id: WorkspaceId,
    /// `None` for a test message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alarm_id: Option<AlarmId>,
    pub event: DeliveryEvent,
    /// 0, or the number of the reminder.
    pub occurrence: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_id: Option<NotificationPolicyId>,
    pub channel_id: ChannelId,
    /// Unique: alarm, event, occurrence, policy and channel.
    #[serde(skip)]
    pub dedupe_key: String,
    pub status: DeliveryStatus,
    /// Attempts started so far. Named `attempt_count` on the wire: in the
    /// detail view `attempts` is the list of attempts.
    #[serde(rename = "attempt_count")]
    pub attempts: u32,
    pub next_attempt_at: Timestamp,
    #[serde(skip)]
    pub lease_until: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<FailureClass>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_http_status: Option<u16>,
    /// Why it was cancelled or suppressed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// The silence that held this delivery back (`suppressed`), or the one
    /// this message follows (`after_silence`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub silence_id: Option<SilenceId>,
    /// This message was sent because a silence ended: it stands for the
    /// messages the silence held back.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub after_silence: bool,
    /// When a suppressed delivery was settled after its silence ended.
    #[serde(skip)]
    pub released_at: Option<Timestamp>,
    pub created_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<Timestamp>,
    /// Names when the delivery was planned (channels and policies can be
    /// renamed or deleted).
    pub channel_name: String,
    #[serde(default)]
    pub policy_name: String,
    #[serde(default)]
    pub alarm_title: String,
    /// The alarm's resource, for filtering history by what a caller may see.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<EntityRef>,
    /// Set on a policy's delivery to people (its email users and roles):
    /// the addresses resolved when it was planned. `None`: the channel's own
    /// recipients.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipients: Option<DeliveryRecipients>,
}

/// The dedupe key of the message that follows a silence: one per alarm,
/// event, silence and channel.
#[must_use]
pub fn after_silence_key(alarm: AlarmId, event: DeliveryEvent, silence: SilenceId, channel: ChannelId) -> String {
    format!("{alarm}|{}|after-silence:{silence}|{channel}", event.as_str())
}

/// The dedupe key of an alarm's delivery.
#[must_use]
pub fn dedupe_key(
    alarm: AlarmId,
    event: DeliveryEvent,
    occurrence: i64,
    policy: NotificationPolicyId,
    channel: ChannelId,
) -> String {
    format!("{alarm}|{}|{occurrence}|{policy}|{channel}", event.as_str())
}

/// The dedupe key of a policy's delivery to people through `channel`. Not
/// the key of the same channel attached to the policy as a channel: the two
/// are separate messages to separate recipients.
#[must_use]
pub fn people_dedupe_key(
    alarm: AlarmId,
    event: DeliveryEvent,
    occurrence: i64,
    policy: NotificationPolicyId,
    channel: ChannelId,
) -> String {
    format!("{}|people", dedupe_key(alarm, event, occurrence, policy, channel))
}

/// One try at sending a delivery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryAttempt {
    pub delivery_id: DeliveryId,
    /// 1-based.
    pub number: u32,
    pub at: Timestamp,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<FailureClass>,
    pub duration_ms: i64,
}

/// What an attempt leaves the delivery as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttemptResult {
    Sent,
    /// Try again at this time.
    Retry(Timestamp),
    Failed,
}

/// Which deliveries to list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeliveryFilter {
    pub workspace: Option<WorkspaceId>,
    pub alarm: Option<AlarmId>,
    pub channel: Option<ChannelId>,
    pub status: Option<DeliveryStatus>,
    /// Only deliveries older than this one (`created_at`, `id`): the next page.
    pub before: Option<(Timestamp, DeliveryId)>,
}

/// Channels, policies and the delivery outbox.
#[async_trait]
pub trait NotificationRepository: Send + Sync + std::fmt::Debug {
    /// Creates or replaces a channel.
    async fn put_channel(&self, c: &Channel) -> DomainResult<()>;
    async fn get_channel(&self, id: ChannelId) -> DomainResult<Option<Channel>>;
    /// By name.
    async fn list_channels(&self, workspace: WorkspaceId) -> DomainResult<Vec<Channel>>;
    async fn delete_channel(&self, id: ChannelId) -> DomainResult<()>;

    /// Creates or replaces a policy.
    async fn put_policy(&self, p: &NotificationPolicy) -> DomainResult<()>;
    async fn get_policy(&self, id: NotificationPolicyId) -> DomainResult<Option<NotificationPolicy>>;
    /// All policies (`None`), or one workspace's, by name.
    async fn list_policies(&self, workspace: Option<WorkspaceId>) -> DomainResult<Vec<NotificationPolicy>>;
    async fn delete_policy(&self, id: NotificationPolicyId) -> DomainResult<()>;

    /// Transitions not planned yet, oldest first.
    async fn unplanned_transitions(&self, limit: u32) -> DomainResult<Vec<AlarmTransition>>;
    /// Adds the transition's deliveries and marks it planned, in one
    /// transaction. Deliveries whose dedupe key exists are skipped. Returns
    /// how many were added.
    async fn plan_transition(
        &self,
        transition: &AlarmTransition,
        deliveries: &[Delivery],
        at: Timestamp,
    ) -> DomainResult<usize>;
    /// Records that planning a transition failed. After `limit` failures
    /// the transition is marked planned with no deliveries and `reason`
    /// kept, so one transition that cannot be planned never holds up the
    /// ones behind it. `true` when it was given up.
    async fn fail_transition(
        &self,
        transition: &AlarmTransition,
        limit: u32,
        reason: &str,
        at: Timestamp,
    ) -> DomainResult<bool>;
    /// Adds deliveries outside a transition (reminders, tests); existing
    /// dedupe keys are skipped. Returns how many were added.
    async fn add_deliveries(&self, deliveries: &[Delivery]) -> DomainResult<usize>;

    /// Deliveries to work on at `now`: pending and due, or sending with an
    /// expired lease; at most `per_channel` of each channel, so one
    /// channel's backlog cannot fill the batch. Longest due first.
    async fn due_deliveries(&self, now: Timestamp, per_channel: u32, limit: u32) -> DomainResult<Vec<Delivery>>;
    /// Takes a due delivery for one attempt: `sending`, one more attempt,
    /// leased until `lease_until`. `false` when someone else took it.
    async fn claim_delivery(&self, id: DeliveryId, now: Timestamp, lease_until: Timestamp) -> DomainResult<bool>;
    /// Records an attempt and the delivery's new state, in one transaction.
    /// Only the attempt that holds the delivery may: it must still be
    /// `sending` and on attempt `attempt.number`. `false`, with nothing
    /// written, when the lease was lost to a later attempt.
    async fn finish_attempt(&self, attempt: &DeliveryAttempt, result: AttemptResult) -> DomainResult<bool>;
    /// Moves a pending delivery's next attempt (no attempt is counted).
    async fn postpone_delivery(&self, id: DeliveryId, until: Timestamp) -> DomainResult<()>;
    /// Cancels a delivery that is not final yet.
    async fn cancel_delivery(&self, id: DeliveryId, why: &str, at: Timestamp) -> DomainResult<()>;
    /// Records that a silence held back a delivery that is not final yet:
    /// `suppressed`, with the silence and the reason.
    async fn suppress_delivery(&self, id: DeliveryId, silence: SilenceId, why: &str, at: Timestamp)
    -> DomainResult<()>;
    /// Suppressed deliveries nobody settled yet whose silence has ended at
    /// `now` (or is gone), oldest first.
    async fn ended_suppressions(&self, now: Timestamp, limit: u32) -> DomainResult<Vec<Delivery>>;
    /// An alarm's suppressed deliveries nobody settled yet, oldest first.
    async fn unsettled_suppressions(&self, alarm: AlarmId) -> DomainResult<Vec<Delivery>>;
    /// Moves unsettled suppressed deliveries under another silence that
    /// still holds their alarm back.
    async fn hold_suppressed(&self, ids: &[DeliveryId], silence: SilenceId, why: &str) -> DomainResult<()>;
    /// Settles suppressed deliveries and adds the messages that follow the
    /// silence, in one transaction. A delivery already settled is left
    /// alone, and when none of `ids` was unsettled nothing is added. Returns
    /// how many deliveries were added.
    async fn release_suppressed(
        &self,
        ids: &[DeliveryId],
        follow_ups: &[Delivery],
        at: Timestamp,
    ) -> DomainResult<usize>;

    async fn get_delivery(&self, id: DeliveryId) -> DomainResult<Option<Delivery>>;
    /// Newest first.
    async fn list_deliveries(&self, filter: &DeliveryFilter, limit: u32) -> DomainResult<Vec<Delivery>>;
    /// Oldest first.
    async fn delivery_attempts(&self, id: DeliveryId) -> DomainResult<Vec<DeliveryAttempt>>;
    /// Deletes final deliveries created before `before`, their attempts, and
    /// planned transitions older than that. Returns the deliveries deleted.
    async fn prune_deliveries(&self, before: Timestamp) -> DomainResult<u64>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedupe_keys_separate_events_reminders_policies_and_channels() {
        let (a, p, c) = (AlarmId::new(), NotificationPolicyId::new(), ChannelId::new());
        let k = dedupe_key(a, DeliveryEvent::Fired, 0, p, c);
        assert_eq!(k, dedupe_key(a, DeliveryEvent::Fired, 0, p, c), "stable");
        assert_ne!(k, dedupe_key(a, DeliveryEvent::Resolved, 0, p, c));
        assert_ne!(k, dedupe_key(a, DeliveryEvent::Reminder, 1, p, c));
        assert_ne!(dedupe_key(a, DeliveryEvent::Reminder, 1, p, c), dedupe_key(a, DeliveryEvent::Reminder, 2, p, c));
        assert_ne!(k, dedupe_key(a, DeliveryEvent::Fired, 0, NotificationPolicyId::new(), c));
        assert_ne!(k, dedupe_key(a, DeliveryEvent::Fired, 0, p, ChannelId::new()));
        assert_ne!(k, dedupe_key(AlarmId::new(), DeliveryEvent::Fired, 0, p, c));
    }

    #[test]
    fn a_channel_never_serializes_or_prints_its_url() {
        let c = Channel {
            id: ChannelId::new(),
            workspace_id: WorkspaceId::new(),
            name: "ops".into(),
            kind: ChannelKind::Webhook,
            enabled: true,
            target: "https://hooks.example.com".into(),
            sealed_url: "SEALED-URL".into(),
            email: None,
            secret_item: None,
            created_by: Principal::User(crate::UserId::new()),
            created_at: Timestamp(1),
            updated_at: Timestamp(1),
        };
        let json = serde_json::to_string(&c).unwrap_or_default();
        assert!(!json.contains("SEALED-URL") && !json.contains("sealed_url"), "{json}");
        assert!(!format!("{c:?}").contains("SEALED-URL"));
    }
}
