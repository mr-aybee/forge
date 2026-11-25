//! Alarm notifications (#11, D-110): channels, policies and durable delivery.
//!
//! Three steps, each safe to repeat:
//!
//! 1. **Outbox.** The store writes an alarm transition in the transaction
//!    that opens or closes the alarm.
//! 2. **Plan** ([`Notifier::plan`]). Each unplanned transition becomes one
//!    delivery per matching policy and channel, added together with the
//!    "planned" mark. A unique key per alarm, event, policy and channel makes
//!    a repeat add nothing.
//! 3. **Dispatch** ([`Notifier::dispatch`]). Due deliveries are leased, sent
//!    and recorded with every attempt. Failures retry with backoff up to a
//!    limit, then the delivery is `failed`.
//!
//! **Silences** (D-112) are checked in one place, just before a message
//! would be sent: a delivery whose alarm an active silence covers is recorded
//! as `suppressed` with that silence. When the silence has ended, planning
//! settles what it held back: one message per channel that brings the
//! receiver up to date (still firing, or recovered where the receiver had
//! been told it fired), or nothing when nothing is left to say.
//!
//! Nothing here writes to an alarm or a rule: a receiver that is down can
//! delay or lose a message, never change what the evaluator recorded.
//! Delivery is at least once: when a receiver accepts a message and the
//! answer is lost, the retry sends it again with the same delivery id.

pub mod email;
pub mod payload;
pub mod people;
pub mod target;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use forge_core::alerts::{Alarm, AlarmFilter, AlarmState, AlertRepository, RuleScope, Severity};
use forge_core::notifications::{
    AttemptResult, Channel, ChannelKind, Delivery, DeliveryAttempt, DeliveryEvent, DeliveryFilter, DeliveryStatus,
    FailureClass, MAX_CHANNELS, MAX_NAME, MAX_POLICIES, MAX_POLICY_CHANNELS, MAX_POLICY_EMAIL_ROLES,
    MAX_POLICY_EMAIL_USERS, MAX_POLICY_RULES, MAX_REMINDERS, NotificationPolicy, NotificationRepository, PolicyEvent,
    REPEAT_RANGE_SECS, TransitionKind, after_silence_key, dedupe_key, people_dedupe_key,
};
use forge_core::notifications::{
    EmailSettings, MAX_EMAIL_ADDRESS, MAX_EMAIL_RECIPIENTS, MAX_SMTP_PASSWORD, MAX_SMTP_USERNAME, MAX_SUBJECT_PREFIX,
    SmtpTls,
};
use forge_core::silences::Silence;
use forge_core::{
    AlarmId, AlertRuleId, Backoff, ChannelId, Clock, DeliveryId, DomainError, DomainResult, EntityRef,
    NotificationPolicyId, Principal, Role, Timestamp, UserId, VaultItemId, WorkspaceId,
};
use futures::StreamExt;

use self::people::People;
use self::target::{TargetRules, parse_smtp_host, parse_target};
use crate::in_scope;
use crate::silence::Silences;

/// One HTTP request to a receiver.
#[derive(Clone, PartialEq, Eq)]
pub struct Outbound {
    pub url: url::Url,
    pub headers: Vec<(&'static str, String)>,
    pub body: Vec<u8>,
}

impl std::fmt::Debug for Outbound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The URL's path can carry a token and a header carries the
        // signature: neither is printed.
        f.debug_struct("Outbound")
            .field("host", &self.url.host_str())
            .field("bytes", &self.body.len())
            .finish_non_exhaustive()
    }
}

/// What a send attempt came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendOutcome {
    /// The receiver answered with this status.
    Status(u16),
    /// The SMTP server's last reply: 2xx accepted the message; 4xx is a
    /// transient refusal, 5xx a permanent one.
    Smtp(u16),
    /// No usable answer.
    Failed(FailureClass),
}

/// Sends requests. The implementation enforces the target rules on the
/// addresses it connects to, follows no redirect, verifies TLS, and bounds
/// time and the answer it reads.
#[async_trait]
pub trait Transport: Send + Sync + std::fmt::Debug {
    /// Sends one request.
    async fn send(&self, request: &Outbound) -> SendOutcome;
}

/// One email to an SMTP server (D-113).
#[derive(Clone, PartialEq, Eq)]
pub struct OutboundMail {
    /// Checked against the target rules; the transport checks every address
    /// it resolves to again before it connects.
    pub host: String,
    pub port: u16,
    pub tls: SmtpTls,
    pub username: Option<String>,
    pub password: Option<String>,
    pub from: String,
    pub to: Vec<String>,
    /// Blind copies: a policy's people (D-117) do not see each other's
    /// addresses.
    pub bcc: Vec<String>,
    pub subject: String,
    /// Plain text.
    pub body: String,
    /// The same message as HTML (D-119); sent with `body` as
    /// `multipart/alternative`.
    pub html: Option<String>,
    /// The same on every retry; the transport makes it the `Message-ID`, so
    /// a mailbox can drop a repeat.
    pub delivery_id: DeliveryId,
    /// When the message is written (its `Date`), from the clock.
    pub at: Timestamp,
}

impl std::fmt::Debug for OutboundMail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never the password; recipients are not secret but stay out of logs.
        f.debug_struct("OutboundMail")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("tls", &self.tls)
            .field("recipients", &(self.to.len() + self.bcc.len()))
            .finish_non_exhaustive()
    }
}

/// Sends email. The implementation enforces the target rules on every
/// address it connects to, requires the TLS mode it is given, verifies
/// certificates and bounds the time of a send.
#[async_trait]
pub trait MailTransport: Send + Sync + std::fmt::Debug {
    /// Checks one address (`ops@example.com` or `Ops <ops@example.com>`)
    /// and returns it as it is sent. `Invalid` when it is not one.
    fn mailbox(&self, raw: &str) -> DomainResult<String>;
    /// Sends one mail.
    async fn send(&self, mail: &OutboundMail) -> SendOutcome;
}

/// Sealing of channel URLs and access to signing secrets (the vault).
#[async_trait]
pub trait ChannelSecrets: Send + Sync + std::fmt::Debug {
    /// Seals a URL so it can only be opened for this channel.
    fn seal_url(&self, channel: ChannelId, url: &str) -> DomainResult<String>;
    /// Opens a URL sealed for this channel.
    fn open_url(&self, channel: ChannelId, sealed: &str) -> DomainResult<String>;
    /// The signing secret behind a vault item of this workspace.
    /// `NotFound` when the item is gone or belongs to another workspace.
    async fn signing_secret(&self, workspace: WorkspaceId, item: VaultItemId) -> DomainResult<Vec<u8>>;
}

/// Display names of resources, for messages people read.
#[async_trait]
pub trait ResourceNames: Send + Sync + std::fmt::Debug {
    /// The display name of `resource`, if it has one.
    async fn name(&self, resource: &EntityRef) -> Option<String>;
}

/// Limits and settings of delivery.
#[derive(Debug, Clone)]
pub struct NotifyConfig {
    pub rules: TargetRules,
    /// Attempts per delivery before it is `failed`.
    pub max_attempts: u32,
    pub retry: Backoff,
    /// How long one attempt may hold a delivery before another worker may
    /// take it. Longer than the transport's total timeout.
    pub lease: Duration,
    /// The console's public address, for the link in a message (the
    /// runtime setting, else the configured URL; shared, read per message).
    pub public_url: forge_core::public_address::PublicAddress,
    /// Deliveries worked on per dispatch pass, and how many at once.
    pub batch: u32,
    pub concurrency: usize,
    /// Final deliveries are deleted after this long.
    pub retention: Duration,
    /// Due deliveries of one channel taken into a pass. They are sent one
    /// after the other, so a slow receiver holds one place, not the batch.
    pub per_channel: u32,
    /// Timeouts in a row after which a channel is left alone for
    /// `channel_pause`.
    pub pause_after_timeouts: u32,
    pub channel_pause: Duration,
    /// Shortest time between two test messages to one channel.
    pub test_cooldown: Duration,
}

impl Default for NotifyConfig {
    fn default() -> Self {
        Self {
            rules: TargetRules::default(),
            max_attempts: 6,
            retry: Backoff::new(Duration::from_secs(30), Duration::from_secs(15 * 60)),
            lease: Duration::from_secs(60),
            public_url: forge_core::public_address::PublicAddress::default(),
            batch: 50,
            concurrency: 4,
            retention: Duration::from_secs(30 * 24 * 3600),
            per_channel: 2,
            pause_after_timeouts: 2,
            channel_pause: Duration::from_secs(60),
            test_cooldown: Duration::from_secs(10),
        }
    }
}

/// A channel as a person writes it.
#[derive(Clone, PartialEq, Eq)]
pub struct ChannelInput {
    pub name: String,
    pub kind: ChannelKind,
    pub enabled: bool,
    /// Required for a new webhook or Slack channel; `None` keeps the stored
    /// URL on an update.
    pub url: Option<String>,
    pub secret_item: Option<VaultItemId>,
    /// The SMTP settings; required for, and only for, an email channel.
    pub email: Option<EmailInput>,
}

/// An email channel's settings as a person writes them.
#[derive(Clone, PartialEq, Eq)]
pub struct EmailInput {
    pub smtp_host: String,
    /// Default: the usual port of `tls`.
    pub smtp_port: Option<u16>,
    pub tls: SmtpTls,
    /// `None`: no authentication (and no password is kept).
    pub username: Option<String>,
    /// A new password. `None` keeps the stored one while the user name stays.
    pub password: Option<String>,
    pub from: String,
    pub to: Vec<String>,
    pub subject_prefix: Option<String>,
}

impl std::fmt::Debug for EmailInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmailInput")
            .field("smtp_host", &self.smtp_host)
            .field("smtp_port", &self.smtp_port)
            .field("tls", &self.tls)
            .field("password", &self.password.as_ref().map(|_| "<hidden>"))
            .finish_non_exhaustive()
    }
}

/// What an email channel stores: the settings people see, and the sealed
/// password (or sealed nothing).
struct CheckedEmail {
    settings: EmailSettings,
    target: String,
    sealed: String,
}

impl std::fmt::Debug for ChannelInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChannelInput").field("name", &self.name).field("kind", &self.kind).finish_non_exhaustive()
    }
}

/// A policy as a person writes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyInput {
    pub name: String,
    pub enabled: bool,
    pub min_severity: Severity,
    pub rules: Vec<AlertRuleId>,
    pub scope: Option<RuleScope>,
    pub events: Vec<PolicyEvent>,
    pub repeat_interval_secs: Option<u32>,
    pub channels: Vec<ChannelId>,
    /// The email channel that sends to `email_users` and `email_roles`;
    /// required when either is not empty.
    pub email_channel: Option<ChannelId>,
    pub email_users: Vec<UserId>,
    pub email_roles: Vec<Role>,
}

/// What a planning pass did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PlanReport {
    pub transitions: usize,
    pub deliveries: usize,
    pub reminders: usize,
    /// Messages added because a silence ended.
    pub after_silence: usize,
    /// Transitions that could not be planned this pass.
    pub failed: usize,
    /// Of those, the ones given up for good.
    pub abandoned: usize,
}

/// Planning failures after which a transition is given up: marked planned
/// with no deliveries and the reason kept. Planning runs every few seconds,
/// so a store that is briefly unreadable does not reach it.
pub const PLAN_FAILURE_LIMIT: u32 = 5;

/// Shortest signing secret, in bytes. A shorter one can be guessed from one
/// captured request.
pub const MIN_SIGNING_SECRET: usize = 16;

/// The detail of a delivery to people that resolved to nobody.
pub const NO_RECIPIENTS: &str = "no recipients";

/// Why a test message was not sent.
#[derive(Debug, thiserror::Error)]
pub enum TestError {
    /// A test went to this channel a moment ago; wait this many seconds.
    #[error("a test message was sent to this channel a moment ago; try again in {0} s")]
    Cooldown(u64),
    #[error(transparent)]
    Domain(#[from] DomainError),
}

/// Timeouts in a row per channel, and until when a channel is left alone.
/// Kept in memory on purpose: it only spares a hung receiver's time, and a
/// restart that forgets it costs one more timeout.
#[derive(Debug, Clone, Default)]
pub struct ChannelPauses(Arc<std::sync::Mutex<HashMap<ChannelId, (u32, Timestamp)>>>);

impl ChannelPauses {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<ChannelId, (u32, Timestamp)>> {
        // The map holds counters only; a panic elsewhere leaves it usable.
        self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn paused(&self, channel: ChannelId, now: Timestamp) -> bool {
        self.lock().get(&channel).is_some_and(|(_, until)| now < *until)
    }

    /// Counts a timeout; pauses the channel when `limit` came in a row.
    fn timed_out(&self, channel: ChannelId, limit: u32, until: Timestamp) {
        let mut map = self.lock();
        let entry = map.entry(channel).or_insert((0, Timestamp(0)));
        entry.0 += 1;

        if entry.0 >= limit.max(1) {
            *entry = (0, until);
        }
    }

    /// The receiver answered (whatever it said): it is not hung.
    fn answered(&self, channel: ChannelId) {
        self.lock().remove(&channel);
    }
}

/// What a dispatch pass did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DispatchReport {
    pub sent: usize,
    pub retried: usize,
    pub failed: usize,
    pub cancelled: usize,
    pub postponed: usize,
    /// Held back by a silence.
    pub suppressed: usize,
}

impl DispatchReport {
    fn count(&mut self, o: Outcome) {
        match o {
            Outcome::Sent => self.sent += 1,
            Outcome::Retried => self.retried += 1,
            Outcome::Failed => self.failed += 1,
            Outcome::Cancelled => self.cancelled += 1,
            Outcome::Postponed => self.postponed += 1,
            Outcome::Suppressed => self.suppressed += 1,
            Outcome::Skipped => {}
        }
    }
}

/// What became of one delivery in a pass, and the failure class if an
/// attempt was made and failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Done {
    outcome: Outcome,
    error: Option<FailureClass>,
    /// An attempt reached the receiver's answer or its absence.
    attempted: bool,
}

impl Done {
    const fn idle(outcome: Outcome) -> Self {
        Self { outcome, error: None, attempted: false }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Sent,
    Retried,
    Failed,
    Cancelled,
    Postponed,
    Suppressed,
    /// Another worker has it.
    Skipped,
}

/// Whether a delivery may go out now.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Gate {
    Send,
    Cancel(&'static str),
    /// Wait for the firing message of the same alarm and channel.
    Wait,
    /// An active silence covers the alarm.
    Suppress(Box<Silence>),
}

/// Whom a delivery speaks to: a channel's own recipients, or one policy's
/// people through an email channel (D-117). Recoveries and the messages
/// after a silence are decided per lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Lane {
    channel: ChannelId,
    people_of: Option<NotificationPolicyId>,
}

impl Lane {
    fn of(d: &Delivery) -> Self {
        Self { channel: d.channel_id, people_of: d.recipients.as_ref().and(d.policy_id) }
    }
}

/// Why a delivery is suppressed, for the delivery history. The silence's
/// end is not repeated here: it can move, and `silence_id` leads to it.
fn silenced_detail(s: &Silence) -> String {
    format!("silenced: {}", s.reason.chars().take(200).collect::<String>())
}

/// What one attempt sends.
#[derive(Debug)]
enum Message {
    Http(Outbound),
    Mail(OutboundMail),
}

/// Why a request could not be built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Refusal {
    class: FailureClass,
    retry: bool,
}

impl Refusal {
    const fn fatal(class: FailureClass) -> Self {
        Self { class, retry: false }
    }
}

/// Whether a policy wants an alarm.
#[must_use]
pub fn policy_matches(p: &NotificationPolicy, alarm: &Alarm) -> bool {
    p.enabled
        && p.workspace_id == alarm.workspace_id
        && alarm.severity >= p.min_severity
        && (p.rules.is_empty() || p.rules.contains(&alarm.rule_id))
        && p.scope.as_ref().is_none_or(|s| in_scope(s, &alarm.resource))
}

/// Statuses worth another attempt: the receiver was busy or briefly broken.
/// Every other status outside 2xx says the request itself is refused, and
/// sending it again would not change the answer.
#[must_use]
pub const fn retryable_status(status: u16) -> bool {
    matches!(status, 408 | 425 | 429) || (status >= 500 && status <= 599)
}

/// SMTP replies worth another attempt: 4xx says "not now" (greylisting, a
/// full mailbox, a busy server); 5xx says no.
#[must_use]
pub const fn retryable_smtp(code: u16) -> bool {
    code >= 400 && code <= 499
}

const fn retryable_failure(class: FailureClass) -> bool {
    // A blocked target, an unusable configuration or a missing secret needs
    // a person, not another try.
    matches!(class, FailureClass::Timeout | FailureClass::Connect | FailureClass::Tls)
}

/// The wait before the next attempt, after `attempts` attempts: between half
/// of the backoff ceiling and all of it. The spread comes from the
/// delivery's id, so deliveries that failed together do not retry together
/// and a test can predict the time.
#[must_use]
pub fn retry_delay(retry: Backoff, attempts: u32, id: DeliveryId) -> Duration {
    let bytes = id.0.as_bytes();
    let spread = f64::from(u16::from_be_bytes([bytes[14], bytes[15]])) / 65_536.0;
    let ceiling = retry.ceiling(attempts.saturating_sub(1));

    ceiling / 2 + (ceiling / 2).mul_f64(spread)
}

fn clean_name(name: &str) -> DomainResult<String> {
    let n = name.trim();
    if n.is_empty() || n.chars().count() > MAX_NAME || n.chars().any(char::is_control) {
        return Err(DomainError::Invalid(format!("name: 1 to {MAX_NAME} characters, without control characters")));
    }

    Ok(n.to_owned())
}

fn ms(d: Duration) -> i64 {
    i64::try_from(d.as_millis()).unwrap_or(i64::MAX)
}

/// Channels, policies and delivery.
#[derive(Debug, Clone)]
pub struct Notifier {
    pub store: Arc<dyn NotificationRepository>,
    pub alerts: Arc<dyn AlertRepository>,
    pub transport: Arc<dyn Transport>,
    /// Sends email channels' messages (D-113).
    pub mail: Arc<dyn MailTransport>,
    pub secrets: Arc<dyn ChannelSecrets>,
    pub names: Arc<dyn ResourceNames>,
    pub clock: Arc<dyn Clock>,
    pub config: NotifyConfig,
    pub pauses: ChannelPauses,
    pub silences: Silences,
    /// Users, roles and access, for the people a policy emails (D-117).
    pub people: People,
}

impl Notifier {
    // ---- channels --------------------------------------------------------

    /// Lists the channels of a workspace.
    pub async fn channels(&self, workspace: WorkspaceId) -> DomainResult<Vec<Channel>> {
        self.store.list_channels(workspace).await
    }

    /// One channel by id.
    pub async fn channel(&self, id: ChannelId) -> DomainResult<Channel> {
        self.store.get_channel(id).await?.ok_or_else(|| DomainError::NotFound("notification channel".into()))
    }

    fn check_channel(input: &ChannelInput) -> DomainResult<String> {
        if input.kind == ChannelKind::Slack && input.secret_item.is_some() {
            return Err(DomainError::Invalid(
                "secret_item: only webhook channels sign their messages; a Slack-compatible URL is its own secret"
                    .into(),
            ));
        }

        match (input.kind, &input.email) {
            (ChannelKind::Email, None) => {
                return Err(DomainError::Invalid("email: required for an email channel".into()));
            }
            (ChannelKind::Email, Some(_)) if input.url.is_some() => {
                return Err(DomainError::Invalid("url: an email channel has no URL".into()));
            }
            (ChannelKind::Email, Some(_)) if input.secret_item.is_some() => {
                return Err(DomainError::Invalid("secret_item: email messages are not signed".into()));
            }
            (ChannelKind::Webhook | ChannelKind::Slack, Some(_)) => {
                return Err(DomainError::Invalid("email: only for an email channel".into()));
            }
            _ => {}
        }

        clean_name(&input.name)
    }

    /// Checks an email channel's settings and seals its password. `old` is
    /// the stored channel on an update: its password is kept when no new
    /// one is given and the user name stays set.
    fn check_email(&self, id: ChannelId, input: &EmailInput, old: Option<&Channel>) -> DomainResult<CheckedEmail> {
        let bad = |m: String| DomainError::Invalid(m);
        let port = input.smtp_port.unwrap_or_else(|| input.tls.default_port());
        if port == 0 {
            return Err(bad("smtp_port: 1 to 65535".into()));
        }

        let host =
            parse_smtp_host(&input.smtp_host, port, self.config.rules).map_err(|e| bad(format!("smtp_host: {e}")))?;
        let username = input.username.as_deref().map(str::trim).filter(|u| !u.is_empty());
        if let Some(u) = username
            && (u.len() > MAX_SMTP_USERNAME || u.chars().any(char::is_control))
        {
            return Err(bad(format!("username: at most {MAX_SMTP_USERNAME} bytes, without control characters")));
        }

        if input.tls == SmtpTls::None {
            if !self.config.rules.allow_http {
                return Err(bad("tls: `none` sends mail unencrypted; use `starttls` or `tls` (an operator can allow unencrypted targets with [alerts] webhook_allow_http)".into()));
            }

            if username.is_some() {
                return Err(bad("username: a password is never sent without TLS; use `starttls` or `tls`".into()));
            }
        }

        if let Some(p) = &input.password {
            if username.is_none() {
                return Err(bad("password: needs a username".into()));
            }

            if p.is_empty() || p.len() > MAX_SMTP_PASSWORD || p.contains(['\r', '\n', '\0']) {
                return Err(bad(format!("password: 1 to {MAX_SMTP_PASSWORD} bytes, one line")));
            }
        }

        let from = self.address("from", &input.from)?;

        if input.to.is_empty() || input.to.len() > MAX_EMAIL_RECIPIENTS {
            return Err(bad(format!("to: 1 to {MAX_EMAIL_RECIPIENTS} recipients")));
        }

        let mut to: Vec<String> = Vec::with_capacity(input.to.len());

        for raw in &input.to {
            let a = self.address("to", raw)?;
            if !to.contains(&a) {
                to.push(a);
            }
        }

        let subject_prefix = input.subject_prefix.as_deref().map(str::trim).filter(|p| !p.is_empty());
        if let Some(p) = subject_prefix
            && (p.chars().count() > MAX_SUBJECT_PREFIX || p.chars().any(char::is_control))
        {
            return Err(bad(format!("subject_prefix: at most {MAX_SUBJECT_PREFIX} characters, one line")));
        }

        let sealed = match (&input.password, username) {
            (_, None) => self.secrets.seal_url(id, "")?,
            (Some(p), Some(_)) => self.secrets.seal_url(id, p)?,
            (None, Some(u)) => match old {
                // Only an email channel's sealed value is a password, and it
                // is kept only for the same server, TLS mode and user: a
                // changed host must not receive the stored password.
                Some(c)
                    if c.kind == ChannelKind::Email
                        && c.email.as_ref().is_some_and(|e| {
                            e.password_set
                                && e.smtp_host == host
                                && e.smtp_port == port
                                && e.tls == input.tls
                                && e.username.as_deref() == Some(u)
                        }) =>
                {
                    c.sealed_url.clone()
                }
                _ => {
                    return Err(bad(
                        "password: required with a new username, or when the SMTP host, port or TLS mode changes"
                            .into(),
                    ));
                }
            },
        };

        let scheme = if input.tls == SmtpTls::Tls { "smtps" } else { "smtp" };
        let shown = if host.contains(':') { format!("[{host}]") } else { host.clone() };

        Ok(CheckedEmail {
            target: format!("{scheme}://{shown}:{port}"),
            settings: EmailSettings {
                smtp_host: host,
                smtp_port: port,
                tls: input.tls,
                username: username.map(str::to_owned),
                password_set: username.is_some(),
                from,
                to,
                subject_prefix: subject_prefix.map(str::to_owned),
            },
            sealed,
        })
    }

    fn address(&self, field: &str, raw: &str) -> DomainResult<String> {
        let raw = raw.trim();
        if raw.is_empty() || raw.len() > MAX_EMAIL_ADDRESS {
            return Err(DomainError::Invalid(format!("{field}: an address of 1 to {MAX_EMAIL_ADDRESS} bytes")));
        }

        self.mail.mailbox(raw).map_err(|_| DomainError::Invalid(format!("{field}: `{raw}` is not an email address")))
    }

    /// Refuses a signing secret that is missing or too short to sign with.
    async fn check_secret(&self, workspace: WorkspaceId, item: Option<VaultItemId>) -> DomainResult<()> {
        let Some(item) = item else { return Ok(()) };
        let secret = self.secrets.signing_secret(workspace, item).await.map_err(|e| match e {
            DomainError::NotFound(_) => DomainError::Invalid("secret_item: no such vault item".into()),
            other => other,
        })?;

        if secret.len() < MIN_SIGNING_SECRET {
            return Err(DomainError::Invalid(format!(
                "secret_item: the vault item's password is shorter than {MIN_SIGNING_SECRET} bytes; use a longer, random one"
            )));
        }

        Ok(())
    }

    /// Creates a notification channel.
    pub async fn create_channel(
        &self,
        workspace: WorkspaceId,
        input: ChannelInput,
        by: Principal,
    ) -> DomainResult<Channel> {
        let name = Self::check_channel(&input)?;
        let id = ChannelId::new();

        let (target, sealed_url, email) = match &input.email {
            Some(e) => {
                let c = self.check_email(id, e, None)?;
                (c.target, c.sealed, Some(c.settings))
            }
            None => {
                let url = input.url.as_deref().ok_or_else(|| DomainError::Invalid("url: required".into()))?;
                let t = parse_target(url, self.config.rules).map_err(|e| DomainError::Invalid(format!("url: {e}")))?;
                let sealed = self.secrets.seal_url(id, t.url.as_str())?;
                (t.display, sealed, None)
            }
        };

        self.check_secret(workspace, input.secret_item).await?;

        if self.store.list_channels(workspace).await?.len() >= MAX_CHANNELS {
            return Err(DomainError::Conflict(format!("a workspace has at most {MAX_CHANNELS} notification channels")));
        }

        let now = self.clock.now();
        let channel = Channel {
            id,
            workspace_id: workspace,
            name,
            kind: input.kind,
            enabled: input.enabled,
            target,
            sealed_url,
            email,
            secret_item: input.secret_item,
            created_by: by,
            created_at: now,
            updated_at: now,
        };
        self.store.put_channel(&channel).await?;

        Ok(channel)
    }

    /// Replaces a channel's settings.
    pub async fn update_channel(&self, id: ChannelId, input: ChannelInput) -> DomainResult<Channel> {
        let old = self.channel(id).await?;
        let name = Self::check_channel(&input)?;
        self.check_secret(old.workspace_id, input.secret_item).await?;

        let (target, sealed_url, email) = match (&input.email, input.url.as_deref()) {
            (Some(e), _) => {
                let c = self.check_email(id, e, Some(&old))?;
                (c.target, c.sealed, Some(c.settings))
            }
            (None, Some(url)) => {
                let t = parse_target(url, self.config.rules).map_err(|e| DomainError::Invalid(format!("url: {e}")))?;
                let sealed = self.secrets.seal_url(id, t.url.as_str())?;
                (t.display, sealed, None)
            }
            // An email channel's sealed value is a password, not a URL.
            (None, None) if old.kind == ChannelKind::Email => {
                return Err(DomainError::Invalid("url: required to turn an email channel into a URL channel".into()));
            }
            (None, None) => (old.target.clone(), old.sealed_url.clone(), None),
        };

        let channel = Channel {
            name,
            kind: input.kind,
            enabled: input.enabled,
            target,
            sealed_url,
            email,
            secret_item: input.secret_item,
            updated_at: self.clock.now(),
            ..old
        };
        self.store.put_channel(&channel).await?;

        Ok(channel)
    }

    /// Deletes a channel no policy uses. Its deliveries stay as history.
    pub async fn delete_channel(&self, id: ChannelId) -> DomainResult<Channel> {
        let channel = self.channel(id).await?;
        let users: Vec<String> = self
            .store
            .list_policies(Some(channel.workspace_id))
            .await?
            .into_iter()
            .filter(|p| p.channels.contains(&id) || p.email_channel == Some(id))
            .map(|p| p.name)
            .collect();

        if !users.is_empty() {
            return Err(DomainError::Conflict(format!(
                "the channel is used by {}: remove it from the policy first",
                users.join(", ")
            )));
        }

        self.store.delete_channel(id).await?;

        Ok(channel)
    }

    /// Sends one test message now and returns what happened. One attempt, no
    /// retry: the person is waiting for the answer. Refused while the last
    /// test to this channel is younger than the cooldown, so the endpoint
    /// cannot be used to hammer a receiver.
    pub async fn test_channel(&self, id: ChannelId) -> Result<(Delivery, Vec<DeliveryAttempt>), TestError> {
        let channel = self.channel(id).await?;
        if !channel.enabled {
            return Err(DomainError::Conflict("the channel is turned off".into()).into());
        }

        let now = self.clock.now();
        let recent = DeliveryFilter { channel: Some(id), ..DeliveryFilter::default() };
        let cooldown = ms(self.config.test_cooldown);
        let last_test = self
            .store
            .list_deliveries(&recent, 50)
            .await?
            .into_iter()
            .filter(|d| d.event == DeliveryEvent::Test)
            .map(|d| d.created_at.0)
            .max();

        if let Some(at) = last_test.filter(|at| now.0 - at < cooldown) {
            let wait = (cooldown - (now.0 - at) + 999) / 1000;
            return Err(TestError::Cooldown(u64::try_from(wait).unwrap_or(1)));
        }

        let did = DeliveryId::new();
        let d = Delivery {
            id: did,
            workspace_id: channel.workspace_id,
            alarm_id: None,
            event: DeliveryEvent::Test,
            occurrence: 0,
            policy_id: None,
            channel_id: channel.id,
            dedupe_key: format!("test|{did}"),
            status: DeliveryStatus::Pending,
            attempts: 0,
            // Not due for the background loop: this call sends it, so the
            // answer below is the attempt's result and not "sending".
            next_attempt_at: Timestamp(now.0 + ms(self.config.lease)),
            lease_until: None,
            last_error: None,
            last_http_status: None,
            detail: None,
            silence_id: None,
            after_silence: false,
            released_at: None,
            created_at: now,
            finished_at: None,
            channel_name: channel.name.clone(),
            policy_name: String::new(),
            alarm_title: String::new(),
            resource: None,
            recipients: None,
        };
        self.store.add_deliveries(std::slice::from_ref(&d)).await?;
        self.dispatch_one(d).await?;

        Ok(self.delivery(did).await?)
    }

    // ---- policies --------------------------------------------------------

    /// Lists the notification policies of a workspace.
    pub async fn policies(&self, workspace: WorkspaceId) -> DomainResult<Vec<NotificationPolicy>> {
        self.store.list_policies(Some(workspace)).await
    }

    /// One notification policy by id.
    pub async fn policy(&self, id: NotificationPolicyId) -> DomainResult<NotificationPolicy> {
        self.store.get_policy(id).await?.ok_or_else(|| DomainError::NotFound("notification policy".into()))
    }

    /// Checks a policy's fields against the workspace it belongs to.
    async fn check_policy(&self, workspace: WorkspaceId, input: PolicyInput) -> DomainResult<PolicyInput> {
        let name = clean_name(&input.name)?;
        let mut events = Vec::new();

        for e in &input.events {
            if !events.contains(e) {
                events.push(*e);
            }
        }

        if events.is_empty() {
            return Err(DomainError::Invalid("events: at least one of fired, resolved".into()));
        }

        if let Some(secs) = input.repeat_interval_secs {
            if !REPEAT_RANGE_SECS.contains(&secs) {
                return Err(DomainError::Invalid(format!(
                    "repeat_interval_secs: {} to {} seconds",
                    REPEAT_RANGE_SECS.start(),
                    REPEAT_RANGE_SECS.end()
                )));
            }

            if !events.contains(&PolicyEvent::Fired) {
                return Err(DomainError::Invalid("repeat_interval_secs: reminders need the fired event".into()));
            }
        }

        let mut channels = Vec::new();

        for c in &input.channels {
            if !channels.contains(c) {
                channels.push(*c);
            }
        }

        let mut email_users = input.email_users.clone();
        email_users.sort();
        email_users.dedup();
        let mut email_roles: Vec<Role> = Vec::new();

        for r in &input.email_roles {
            if !email_roles.contains(r) {
                email_roles.push(*r);
            }
        }

        if email_users.len() > MAX_POLICY_EMAIL_USERS {
            return Err(DomainError::Invalid(format!("email_users: at most {MAX_POLICY_EMAIL_USERS}")));
        }

        if email_roles.len() > MAX_POLICY_EMAIL_ROLES {
            return Err(DomainError::Invalid(format!("email_roles: at most {MAX_POLICY_EMAIL_ROLES}")));
        }

        let people = !email_users.is_empty() || !email_roles.is_empty();
        let email_channel = match (input.email_channel, people) {
            (None, true) => {
                return Err(DomainError::Invalid(
                    "email_channel: required to email users or roles; name the email channel that sends".into(),
                ));
            }
            // Without people the channel has nothing to send: it is dropped.
            (_, false) => None,
            (Some(c), true) => Some(c),
        };

        if channels.len() > MAX_POLICY_CHANNELS || (channels.is_empty() && !people) {
            return Err(DomainError::Invalid(format!(
                "channels: 1 to {MAX_POLICY_CHANNELS} (or none when the policy emails users or roles)"
            )));
        }

        let known: HashMap<ChannelId, ChannelKind> =
            self.store.list_channels(workspace).await?.into_iter().map(|c| (c.id, c.kind)).collect();

        if let Some(missing) = channels.iter().find(|c| !known.contains_key(c)) {
            return Err(DomainError::Invalid(format!("channels: no channel {missing}")));
        }

        if let Some(c) = email_channel {
            match known.get(&c) {
                Some(ChannelKind::Email) => {}
                Some(_) => return Err(DomainError::Invalid(format!("email_channel: {c} is not an email channel"))),
                None => return Err(DomainError::Invalid(format!("email_channel: no channel {c}"))),
            }
        }

        if !email_users.is_empty() {
            self.people.check_members(workspace, &email_users).await?;
        }

        let mut rules = input.rules.clone();
        rules.sort();
        rules.dedup();

        if rules.len() > MAX_POLICY_RULES {
            return Err(DomainError::Invalid(format!("rules: at most {MAX_POLICY_RULES}")));
        }

        if !rules.is_empty() {
            let known: HashSet<AlertRuleId> =
                self.alerts.list_rules(Some(workspace)).await?.into_iter().map(|r| r.id).collect();

            if let Some(missing) = rules.iter().find(|r| !known.contains(r)) {
                return Err(DomainError::Invalid(format!("rules: no alert rule {missing}")));
            }
        }

        let scope_size = input.scope.as_ref().map_or(0, RuleScope::len);
        if scope_size > 500 {
            return Err(DomainError::Invalid("scope: at most 500 ids".into()));
        }

        Ok(PolicyInput { name, events, channels, rules, email_channel, email_users, email_roles, ..input })
    }

    /// The users a policy emails by name who have no email address: they
    /// are skipped until they set one.
    pub async fn policy_warnings(&self, p: &NotificationPolicy) -> DomainResult<Vec<UserId>> {
        Ok(self.policies_warnings(std::slice::from_ref(p)).await?.into_iter().next().unwrap_or_default())
    }

    /// [`Notifier::policy_warnings`] of many policies, in one read.
    pub async fn policies_warnings(&self, ps: &[NotificationPolicy]) -> DomainResult<Vec<Vec<UserId>>> {
        let mut all: Vec<UserId> = ps.iter().flat_map(|p| p.email_users.iter().copied()).collect();
        all.sort();
        all.dedup();
        let missing: HashSet<UserId> = self.people.without_email(&all).await?.into_iter().collect();

        Ok(ps.iter().map(|p| p.email_users.iter().copied().filter(|u| missing.contains(u)).collect()).collect())
    }

    /// Creates a notification policy in a workspace.
    pub async fn create_policy(
        &self,
        workspace: WorkspaceId,
        input: PolicyInput,
        by: Principal,
    ) -> DomainResult<NotificationPolicy> {
        let input = self.check_policy(workspace, input).await?;

        if self.store.list_policies(Some(workspace)).await?.len() >= MAX_POLICIES {
            return Err(DomainError::Conflict(format!("a workspace has at most {MAX_POLICIES} notification policies")));
        }

        let now = self.clock.now();
        let policy = NotificationPolicy {
            id: NotificationPolicyId::new(),
            workspace_id: workspace,
            name: input.name,
            enabled: input.enabled,
            min_severity: input.min_severity,
            rules: input.rules,
            scope: input.scope,
            events: input.events,
            repeat_interval_secs: input.repeat_interval_secs,
            channels: input.channels,
            email_channel: input.email_channel,
            email_users: input.email_users,
            email_roles: input.email_roles,
            created_by: by,
            created_at: now,
            updated_at: now,
        };
        self.store.put_policy(&policy).await?;

        Ok(policy)
    }

    /// Replaces a notification policy's settings.
    pub async fn update_policy(
        &self,
        id: NotificationPolicyId,
        input: PolicyInput,
    ) -> DomainResult<NotificationPolicy> {
        let old = self.policy(id).await?;
        let input = self.check_policy(old.workspace_id, input).await?;
        let policy = NotificationPolicy {
            name: input.name,
            enabled: input.enabled,
            min_severity: input.min_severity,
            rules: input.rules,
            scope: input.scope,
            events: input.events,
            repeat_interval_secs: input.repeat_interval_secs,
            channels: input.channels,
            email_channel: input.email_channel,
            email_users: input.email_users,
            email_roles: input.email_roles,
            updated_at: self.clock.now(),
            ..old
        };
        self.store.put_policy(&policy).await?;

        Ok(policy)
    }

    /// Deletes a notification policy and returns it.
    pub async fn delete_policy(&self, id: NotificationPolicyId) -> DomainResult<NotificationPolicy> {
        let policy = self.policy(id).await?;
        self.store.delete_policy(id).await?;

        Ok(policy)
    }

    // ---- history ---------------------------------------------------------

    /// Lists deliveries matching `filter`, at most `limit`.
    pub async fn deliveries(&self, filter: &DeliveryFilter, limit: u32) -> DomainResult<Vec<Delivery>> {
        self.store.list_deliveries(filter, limit).await
    }

    /// One delivery with its attempts.
    pub async fn delivery(&self, id: DeliveryId) -> DomainResult<(Delivery, Vec<DeliveryAttempt>)> {
        let d = self.store.get_delivery(id).await?.ok_or_else(|| DomainError::NotFound("delivery".into()))?;
        let attempts = self.store.delivery_attempts(id).await?;

        Ok((d, attempts))
    }

    // ---- planning --------------------------------------------------------

    fn new_delivery(
        alarm: &Alarm,
        event: DeliveryEvent,
        occurrence: i64,
        policy: &NotificationPolicy,
        channel: &Channel,
        now: Timestamp,
    ) -> Delivery {
        Delivery {
            id: DeliveryId::new(),
            workspace_id: alarm.workspace_id,
            alarm_id: Some(alarm.id),
            event,
            occurrence,
            policy_id: Some(policy.id),
            channel_id: channel.id,
            dedupe_key: dedupe_key(alarm.id, event, occurrence, policy.id, channel.id),
            status: DeliveryStatus::Pending,
            attempts: 0,
            next_attempt_at: now,
            lease_until: None,
            last_error: None,
            last_http_status: None,
            detail: None,
            silence_id: None,
            after_silence: false,
            released_at: None,
            created_at: now,
            finished_at: None,
            channel_name: channel.name.clone(),
            policy_name: policy.name.clone(),
            alarm_title: alarm.title.clone(),
            resource: Some(alarm.resource.clone()),
            recipients: None,
        }
    }

    /// The deliveries to people of the policies that email users or roles:
    /// one per policy, through its email channel, to the people resolved
    /// now. The addresses are kept with the delivery, so retries and the
    /// message after a silence go to the same people. A policy whose people
    /// all resolve to nobody records a `cancelled` delivery that says so.
    async fn people_deliveries(
        &self,
        alarm: &Alarm,
        event: DeliveryEvent,
        occurrence: i64,
        policies: &[&NotificationPolicy],
        channels: &HashMap<ChannelId, Channel>,
        now: Timestamp,
    ) -> DomainResult<Vec<Delivery>> {
        let mut out = Vec::new();

        for p in policies.iter().filter(|p| p.emails_people()) {
            let Some(c) =
                p.email_channel.and_then(|id| channels.get(&id)).filter(|c| c.enabled && c.kind == ChannelKind::Email)
            else {
                continue;
            };

            let key = people_dedupe_key(alarm.id, event, occurrence, p.id, c.id);
            if event == DeliveryEvent::Reminder && self.planned(alarm.id, c.id, &key).await? {
                // Planning runs every few seconds: resolving people again
                // for a reminder that exists would only cost reads.
                continue;
            }

            let recipients = self.people.resolve(p, alarm, |raw| self.mail.mailbox(raw).ok()).await?;
            let mut d = Self::new_delivery(alarm, event, occurrence, p, c, now);
            d.dedupe_key = key;

            if recipients.to.is_empty() {
                d.status = DeliveryStatus::Cancelled;
                d.detail = Some(NO_RECIPIENTS.to_owned());
                d.finished_at = Some(now);
            }

            d.recipients = Some(recipients);
            out.push(d);
        }

        Ok(out)
    }

    /// Whether a delivery with this dedupe key exists for the alarm and
    /// channel.
    async fn planned(&self, alarm: AlarmId, channel: ChannelId, key: &str) -> DomainResult<bool> {
        let filter = DeliveryFilter { alarm: Some(alarm), channel: Some(channel), ..DeliveryFilter::default() };
        Ok(self.store.list_deliveries(&filter, 500).await?.iter().any(|d| d.dedupe_key == key))
    }

    /// The deliveries of one alarm event: one per enabled channel of every
    /// matching policy. A channel reached through two policies gets one
    /// message (the first policy by name).
    fn fan_out(
        alarm: &Alarm,
        event: DeliveryEvent,
        occurrence: i64,
        policies: &[&NotificationPolicy],
        channels: &HashMap<ChannelId, Channel>,
        now: Timestamp,
    ) -> Vec<Delivery> {
        let mut seen: HashSet<ChannelId> = HashSet::new();
        let mut out = Vec::new();

        for p in policies {
            for c in p.channels.iter().filter_map(|id| channels.get(id)).filter(|c| c.enabled) {
                if seen.insert(c.id) {
                    out.push(Self::new_delivery(alarm, event, occurrence, p, c, now));
                }
            }
        }

        out
    }

    /// Turns unplanned alarm transitions into deliveries, settles what ended
    /// silences held back, then adds due reminders. Safe to run again at any
    /// point. Each step runs even when an earlier one could not read the
    /// store, and a transition that fails is skipped, not waited for.
    pub async fn plan(&self) -> DomainResult<PlanReport> {
        let now = self.clock.now();
        let mut report = PlanReport::default();
        let transitions = self.plan_transitions(now, &mut report).await;
        let after_silence = self.plan_after_silences(now).await;
        let reminders = self.plan_reminders(now).await;
        transitions?;
        report.after_silence = after_silence?;
        report.reminders = reminders?;

        Ok(report)
    }

    /// Settles the suppressed deliveries whose silence has ended.
    ///
    /// Per alarm: while another active silence still covers it, its
    /// suppressed deliveries move under that silence. Otherwise each channel
    /// gets at most one message that brings it up to date, marked
    /// `after_silence`:
    ///
    /// * the alarm still fires: `fired` when the firing message was held
    ///   back, `reminder` when only reminders were;
    /// * the alarm closed and its recovery was held back: the recovery,
    ///   where the channel had been told the alarm fired;
    /// * otherwise nothing (an alarm that fired and closed inside the
    ///   silence sends no message at all).
    async fn plan_after_silences(&self, now: Timestamp) -> DomainResult<usize> {
        let ended = self.store.ended_suppressions(now, self.config.batch.max(1) * 4).await?;
        let mut alarms: Vec<AlarmId> = Vec::new();
        let mut orphans: Vec<DeliveryId> = Vec::new();

        for d in &ended {
            match d.alarm_id {
                Some(a) if !alarms.contains(&a) => alarms.push(a),
                Some(_) => {}
                None => orphans.push(d.id),
            }
        }

        if !orphans.is_empty() {
            self.store.release_suppressed(&orphans, &[], now).await?;
        }

        let mut added = 0;

        for alarm_id in alarms {
            // One alarm that cannot be settled must not hold up the others;
            // it is found again on the next pass.
            match self.settle_alarm(alarm_id, now).await {
                Ok(n) => added += n,
                Err(e) => tracing::warn!(alarm = %alarm_id, error = %e, "suppressed deliveries not settled this pass"),
            }
        }

        Ok(added)
    }

    /// Settles one alarm's suppressed deliveries. Returns the messages added.
    async fn settle_alarm(&self, alarm_id: AlarmId, now: Timestamp) -> DomainResult<usize> {
        // The whole group, not only the part a batch happened to hold: the
        // decision below is made once per alarm and channel.
        let held = self.store.unsettled_suppressions(alarm_id).await?;
        let ids: Vec<DeliveryId> = held.iter().map(|d| d.id).collect();

        let alarm = match self.alerts.get_alarm(alarm_id).await {
            Ok(a) => a,
            // The alarm is gone: nothing to say about it.
            Err(DomainError::NotFound(_)) => return self.store.release_suppressed(&ids, &[], now).await,
            Err(e) => return Err(e),
        };

        if let Some(s) = self.silences.covering_one(&alarm, now).await? {
            let moved: Vec<DeliveryId> = held.iter().filter(|d| d.silence_id != Some(s.id)).map(|d| d.id).collect();
            self.store.hold_suppressed(&moved, s.id, &silenced_detail(&s)).await?;

            return Ok(0);
        }

        let mut by_lane: Vec<(Lane, Vec<&Delivery>)> = Vec::new();

        for d in &held {
            match by_lane.iter_mut().find(|(l, _)| *l == Lane::of(d)) {
                Some((_, group)) => group.push(d),
                None => by_lane.push((Lane::of(d), vec![d])),
            }
        }

        let mut follow_ups = Vec::new();

        for (lane, group) in by_lane {
            // Oldest first: the newest of a kind is the last one.
            let newest = |event: fn(DeliveryEvent) -> bool| group.iter().rev().find(|d| event(d.event)).copied();

            let stands_for = if alarm.state == AlarmState::Firing {
                newest(|e| e == DeliveryEvent::Fired).or_else(|| newest(|e| e == DeliveryEvent::Reminder))
            } else {
                match newest(|e| !e.is_firing()) {
                    Some(d) if !matches!(self.recovery_gate(alarm.id, lane).await?, Gate::Cancel(_)) => Some(d),
                    _ => None,
                }
            };

            if let Some(d) = stands_for {
                follow_ups.push(Self::after_silence(d, now));
            }
        }

        self.store.release_suppressed(&ids, &follow_ups, now).await
    }

    /// The message that stands for a suppressed delivery once its silence
    /// has ended.
    fn after_silence(held: &Delivery, now: Timestamp) -> Delivery {
        let id = DeliveryId::new();

        let dedupe_key = match (held.alarm_id, held.silence_id, Lane::of(held).people_of) {
            (Some(alarm), Some(silence), None) => after_silence_key(alarm, held.event, silence, held.channel_id),
            // A policy's people keep their own message, with the addresses
            // the held delivery was planned for.
            (Some(alarm), Some(silence), Some(policy)) => {
                format!("{}|people:{policy}", after_silence_key(alarm, held.event, silence, held.channel_id))
            }
            // Not reachable for a suppressed alarm delivery; still unique.
            _ => format!("after-silence|{id}"),
        };

        Delivery {
            id,
            dedupe_key,
            status: DeliveryStatus::Pending,
            attempts: 0,
            next_attempt_at: now,
            lease_until: None,
            last_error: None,
            last_http_status: None,
            detail: None,
            after_silence: true,
            released_at: None,
            created_at: now,
            finished_at: None,
            ..held.clone()
        }
    }

    /// The deliveries of one transition, added with its "planned" mark.
    async fn plan_one(
        &self,
        t: &forge_core::notifications::AlarmTransition,
        policies: &[NotificationPolicy],
        channels: &HashMap<ChannelId, Channel>,
        now: Timestamp,
    ) -> DomainResult<usize> {
        let deliveries = match self.alerts.get_alarm(t.alarm_id).await {
            Ok(alarm) => {
                let wanted = match t.kind {
                    TransitionKind::Fired => PolicyEvent::Fired,
                    TransitionKind::Resolved | TransitionKind::Ended => PolicyEvent::Resolved,
                };

                let matching: Vec<&NotificationPolicy> =
                    policies.iter().filter(|p| p.events.contains(&wanted) && policy_matches(p, &alarm)).collect();
                let mut out = Self::fan_out(&alarm, t.kind.into(), 0, &matching, channels, now);
                out.extend(self.people_deliveries(&alarm, t.kind.into(), 0, &matching, channels, now).await?);
                out
            }
            // The alarm is gone: nothing to say about it.
            Err(DomainError::NotFound(_)) => Vec::new(),
            Err(e) => return Err(e),
        };

        self.store.plan_transition(t, &deliveries, now).await
    }

    async fn plan_transitions(&self, now: Timestamp, report: &mut PlanReport) -> DomainResult<()> {
        let transitions = self.store.unplanned_transitions(self.config.batch.max(1) * 4).await?;
        let mut policies: HashMap<WorkspaceId, Vec<NotificationPolicy>> = HashMap::new();
        let mut channels: HashMap<WorkspaceId, HashMap<ChannelId, Channel>> = HashMap::new();
        let workspaces: HashSet<WorkspaceId> = transitions.iter().map(|t| t.workspace_id).collect();

        for ws in workspaces {
            policies.insert(ws, self.store.list_policies(Some(ws)).await?);
            let list = self.store.list_channels(ws).await?;
            channels.insert(ws, list.into_iter().map(|c| (c.id, c)).collect());
        }

        let none = HashMap::new();

        for t in &transitions {
            let known_policies = policies.get(&t.workspace_id).map_or(&[][..], Vec::as_slice);
            let known_channels = channels.get(&t.workspace_id).unwrap_or(&none);
            match self.plan_one(t, known_policies, known_channels, now).await {
                Ok(added) => {
                    report.deliveries += added;
                    report.transitions += 1;
                }
                // One transition that cannot be planned must not hold up the
                // others: count the failure, go on, and give it up for good
                // after `PLAN_FAILURE_LIMIT` passes.
                Err(e) => {
                    report.failed += 1;
                    let reason: String = e.to_string().chars().take(200).collect();
                    match self.store.fail_transition(t, PLAN_FAILURE_LIMIT, &reason, now).await {
                        Ok(true) => {
                            report.abandoned += 1;
                            tracing::error!(
                                alarm = %t.alarm_id,
                                kind = t.kind.as_str(),
                                error = %e,
                                "alarm transition given up: no notification is sent for it"
                            );
                        }
                        Ok(false) => {
                            tracing::warn!(alarm = %t.alarm_id, kind = t.kind.as_str(), error = %e, "alarm transition not planned this pass");
                        }
                        Err(e2) => {
                            tracing::warn!(alarm = %t.alarm_id, error = %e2, "could not record a planning failure");
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Adds the reminder that is due for every firing, unacknowledged alarm
    /// a policy with a repeat interval matches. The reminder's number is the
    /// count of whole intervals since the alarm fired, so a pass that runs
    /// late adds one reminder, not one per interval missed.
    async fn plan_reminders(&self, now: Timestamp) -> DomainResult<usize> {
        let repeating: Vec<NotificationPolicy> = self
            .store
            .list_policies(None)
            .await?
            .into_iter()
            .filter(|p| p.enabled && p.repeat_interval_secs.is_some() && p.events.contains(&PolicyEvent::Fired))
            .collect();
        let mut by_workspace: HashMap<WorkspaceId, Vec<NotificationPolicy>> = HashMap::new();

        for p in repeating {
            by_workspace.entry(p.workspace_id).or_default().push(p);
        }

        let mut added = 0;

        for (ws, policies) in by_workspace {
            let channels: HashMap<ChannelId, Channel> =
                self.store.list_channels(ws).await?.into_iter().map(|c| (c.id, c)).collect();
            let filter = AlarmFilter { workspaces: Some(vec![ws]), active: true, ..AlarmFilter::default() };
            let firing = self.alerts.list_alarms(&filter, 1000).await?;
            let mut due = Vec::new();

            for alarm in firing.iter().filter(|a| a.acknowledged_at.is_none()) {
                // Policies by reminder number, so each number fans out once.
                let mut by_number: HashMap<i64, Vec<&NotificationPolicy>> = HashMap::new();

                for p in policies.iter().filter(|p| policy_matches(p, alarm)) {
                    let every = i64::from(p.repeat_interval_secs.unwrap_or(u32::MAX)) * 1000;
                    let n = (now.0 - alarm.fired_at.0) / every;
                    if (1..=MAX_REMINDERS).contains(&n) {
                        by_number.entry(n).or_default().push(p);
                    }
                }

                for (n, matching) in by_number {
                    due.extend(Self::fan_out(alarm, DeliveryEvent::Reminder, n, &matching, &channels, now));

                    // One alarm whose people cannot be resolved must not
                    // hold up the other reminders; it is tried next pass.
                    match self.people_deliveries(alarm, DeliveryEvent::Reminder, n, &matching, &channels, now).await {
                        Ok(people) => due.extend(people),
                        Err(e) => {
                            tracing::warn!(alarm = %alarm.id, error = %e, "reminder to people not planned this pass")
                        }
                    }
                }
            }

            if !due.is_empty() {
                added += self.store.add_deliveries(&due).await?;
            }
        }

        Ok(added)
    }

    // ---- dispatch --------------------------------------------------------

    /// Works on the deliveries that are due. One delivery's failure never
    /// stops the others.
    ///
    /// Channels are independent lanes: the deliveries of one channel go out
    /// one after the other, lanes run side by side, and a lane stops for
    /// this pass at its first timeout. A channel that timed out several
    /// times in a row is left alone for a while. A receiver that hangs
    /// therefore costs one request per pass, never the whole batch.
    pub async fn dispatch(&self) -> DomainResult<DispatchReport> {
        let now = self.clock.now();
        let due = self.store.due_deliveries(now, self.config.per_channel, self.config.batch).await?;
        let mut lanes: Vec<(ChannelId, Vec<Delivery>)> = Vec::new();

        for d in due {
            if self.pauses.paused(d.channel_id, now) {
                continue;
            }

            match lanes.iter_mut().find(|(c, _)| *c == d.channel_id) {
                Some((_, lane)) => lane.push(d),
                None => lanes.push((d.channel_id, vec![d])),
            }
        }

        let outcomes: Vec<Vec<Outcome>> = futures::stream::iter(lanes)
            .map(|(channel, lane)| self.dispatch_lane(channel, lane))
            .buffer_unordered(self.config.concurrency.max(1))
            .collect()
            .await;
        let mut report = DispatchReport::default();

        for o in outcomes.into_iter().flatten() {
            report.count(o);
        }

        Ok(report)
    }

    /// One channel's due deliveries, in order, until the first timeout.
    async fn dispatch_lane(&self, channel: ChannelId, lane: Vec<Delivery>) -> Vec<Outcome> {
        let mut outcomes = Vec::new();

        for d in lane {
            let id = d.id;

            let done = match self.dispatch_one(d).await {
                Ok(done) => done,
                Err(e) => {
                    tracing::warn!(delivery = %id, error = %e, "notification delivery skipped this pass");
                    Done::idle(Outcome::Skipped)
                }
            };

            outcomes.push(done.outcome);

            if !done.attempted {
                continue;
            }

            if done.error == Some(FailureClass::Timeout) {
                let until = Timestamp(self.clock.now().0 + ms(self.config.channel_pause));
                self.pauses.timed_out(channel, self.config.pause_after_timeouts, until);

                break;
            }

            self.pauses.answered(channel);
        }

        outcomes
    }

    /// Deletes final deliveries older than the retention, then the silences
    /// that ended before it and that no delivery names any more.
    pub async fn prune(&self) -> DomainResult<u64> {
        let before = Timestamp(self.clock.now().0 - ms(self.config.retention));
        let deliveries = self.store.prune_deliveries(before).await?;
        self.silences.prune(before).await?;

        Ok(deliveries)
    }

    /// Whether a delivery still applies to its alarm, and whether a silence
    /// holds it back. This is the one place a silence takes effect, and it
    /// runs just before the send: a silence that started after a delivery
    /// was planned (or while it waited for a retry) still covers it.
    async fn gate(&self, d: &Delivery, alarm: Option<&Alarm>) -> DomainResult<Gate> {
        let Some(alarm) = alarm else {
            // A test is sent by the request that asked for it. One found
            // here later was interrupted; nobody is waiting for it. A test
            // names no alarm, so no silence covers it.
            let stale = d.event == DeliveryEvent::Test && self.clock.now() >= d.next_attempt_at;
            return Ok(if stale { Gate::Cancel("the test was interrupted") } else { Gate::Send });
        };

        let applies = if d.event.is_firing() {
            if alarm.state != AlarmState::Firing {
                return Ok(Gate::Cancel("the alarm is no longer firing"));
            }

            // Acknowledging and silencing are independent: either one stops
            // a reminder, neither implies the other.
            if d.event == DeliveryEvent::Reminder && alarm.acknowledged_at.is_some() {
                return Ok(Gate::Cancel("the alarm was acknowledged"));
            }

            Gate::Send
        } else {
            match self.recovery_gate(alarm.id, Lane::of(d)).await? {
                cancel @ Gate::Cancel(_) => return Ok(cancel),
                other => other,
            }
        };

        match self.silences.covering_one(alarm, self.clock.now()).await? {
            Some(s) => Ok(Gate::Suppress(Box::new(s))),
            None => Ok(applies),
        }
    }

    /// Whether a recovery (resolved or ended) goes to a lane: only where
    /// the firing message arrived. A channel (or a policy's people) that
    /// never heard of the alarm gets no "recovered".
    async fn recovery_gate(&self, alarm: AlarmId, lane: Lane) -> DomainResult<Gate> {
        let filter = DeliveryFilter { alarm: Some(alarm), channel: Some(lane.channel), ..DeliveryFilter::default() };
        let firing: Vec<Delivery> = self
            .store
            .list_deliveries(&filter, 200)
            .await?
            .into_iter()
            .filter(|s| s.event.is_firing() && Lane::of(s) == lane)
            .collect();

        if firing.is_empty() || firing.iter().any(|s| s.status == DeliveryStatus::Sent) {
            return Ok(Gate::Send);
        }

        if firing.iter().any(|s| !s.status.is_final()) {
            return Ok(Gate::Wait);
        }

        if firing.iter().any(|s| s.status == DeliveryStatus::Suppressed) {
            return Ok(Gate::Cancel("the firing notification was silenced, so there is nothing to recover from"));
        }

        Ok(Gate::Cancel("the firing notification was never delivered"))
    }

    /// The message for a delivery, or the reason it cannot be built.
    async fn request(&self, d: &Delivery, channel: &Channel, alarm: Option<&Alarm>) -> Result<Message, Refusal> {
        let name = match alarm {
            Some(a) => self.names.name(&a.resource).await,
            None => None,
        };

        let base = self.config.public_url.get();
        let doc =
            payload::build(payload::Context { delivery: d, alarm, resource_name: name.as_deref(), public_url: &base });

        match channel.kind {
            // Turned into another kind after it was planned: people are
            // reached by email only.
            ChannelKind::Webhook | ChannelKind::Slack if d.recipients.is_some() => {
                Err(Refusal::fatal(FailureClass::Config))
            }
            ChannelKind::Email => self.mail_request(d, channel, &doc).map(Message::Mail),
            ChannelKind::Webhook | ChannelKind::Slack => self.http_request(d, channel, &doc).await.map(Message::Http),
        }
    }

    /// The email of an email channel's delivery.
    fn mail_request(&self, d: &Delivery, channel: &Channel, doc: &payload::Payload) -> Result<OutboundMail, Refusal> {
        let e = channel.email.as_ref().ok_or(Refusal::fatal(FailureClass::Config))?;

        // The rules may be stricter now than when the channel was saved.
        let host = parse_smtp_host(&e.smtp_host, e.smtp_port, self.config.rules)
            .map_err(|_| Refusal::fatal(FailureClass::BlockedTarget))?;

        if e.tls == SmtpTls::None && (!self.config.rules.allow_http || e.username.is_some()) {
            return Err(Refusal::fatal(FailureClass::BlockedTarget));
        }

        let password = if e.password_set {
            let p = self
                .secrets
                .open_url(channel.id, &channel.sealed_url)
                .map_err(|_| Refusal::fatal(FailureClass::Secret))?;
            Some(p).filter(|p| !p.is_empty())
        } else {
            None
        };

        if e.username.is_some() && password.is_none() {
            return Err(Refusal::fatal(FailureClass::Secret));
        }

        // A policy's people get blind copies at the addresses planned with
        // the delivery; the channel's own recipients are not added.
        let (to, bcc) = match &d.recipients {
            Some(r) if r.to.is_empty() => return Err(Refusal::fatal(FailureClass::Config)),
            Some(r) => (Vec::new(), r.to.clone()),
            None => (e.to.clone(), Vec::new()),
        };

        Ok(OutboundMail {
            host,
            port: e.smtp_port,
            tls: e.tls,
            username: e.username.clone(),
            password,
            from: e.from.clone(),
            to,
            bcc,
            subject: email::subject(doc, e.subject_prefix.as_deref()),
            body: email::body(doc),
            html: Some(email::html(doc, email::Why { policy: &d.policy_name, channel: &d.channel_name })),
            delivery_id: d.id,
            at: self.clock.now(),
        })
    }

    /// The HTTP request of a webhook or Slack-compatible delivery.
    async fn http_request(&self, d: &Delivery, channel: &Channel, doc: &payload::Payload) -> Result<Outbound, Refusal> {
        let url =
            self.secrets.open_url(channel.id, &channel.sealed_url).map_err(|_| Refusal::fatal(FailureClass::Config))?;

        // The rules may be stricter now than when the channel was saved.
        let target = parse_target(&url, self.config.rules).map_err(|_| Refusal::fatal(FailureClass::BlockedTarget))?;

        let body = match channel.kind {
            ChannelKind::Slack => serde_json::to_vec(&payload::SlackBody { text: payload::text(doc) }),
            ChannelKind::Webhook | ChannelKind::Email => serde_json::to_vec(doc),
        }
        .map_err(|_| Refusal::fatal(FailureClass::Config))?;

        let mut headers = vec![
            ("content-type", "application/json".to_owned()),
            (payload::HEADER_EVENT, d.event.as_str().to_owned()),
            (payload::HEADER_DELIVERY, d.id.to_string()),
        ];

        if let Some(item) = channel.secret_item {
            let secret = self.secrets.signing_secret(channel.workspace_id, item).await.map_err(|e| Refusal {
                class: FailureClass::Secret,
                // A missing item needs a person; a vault that could not be
                // read right now is worth another try.
                retry: !matches!(e, DomainError::NotFound(_)),
            })?;

            // The item's password may have been changed since the channel
            // was saved: never sign with a guessable key, never send unsigned.
            if secret.len() < MIN_SIGNING_SECRET {
                return Err(Refusal::fatal(FailureClass::Secret));
            }

            let at = self.clock.now().0 / 1000;
            headers.push((payload::HEADER_TIMESTAMP, at.to_string()));
            headers.push((payload::HEADER_SIGNATURE, payload::sign(&secret, at, &body)));
        }

        Ok(Outbound { url: target.url, headers, body })
    }

    async fn dispatch_one(&self, d: Delivery) -> DomainResult<Done> {
        let now = self.clock.now();

        let channel = match self.store.get_channel(d.channel_id).await? {
            Some(c) if c.enabled => c,
            other => {
                let why = if other.is_some() { "the channel is turned off" } else { "the channel was deleted" };
                self.store.cancel_delivery(d.id, why, now).await?;

                return Ok(Done::idle(Outcome::Cancelled));
            }
        };

        let alarm = match d.alarm_id {
            Some(id) => match self.alerts.get_alarm(id).await {
                Ok(a) => Some(a),
                Err(DomainError::NotFound(_)) => {
                    self.store.cancel_delivery(d.id, "the alarm no longer exists", now).await?;
                    return Ok(Done::idle(Outcome::Cancelled));
                }
                Err(e) => return Err(e),
            },
            None => None,
        };

        match self.gate(&d, alarm.as_ref()).await? {
            Gate::Send => {}
            Gate::Cancel(why) => {
                self.store.cancel_delivery(d.id, why, now).await?;
                return Ok(Done::idle(Outcome::Cancelled));
            }
            Gate::Wait => {
                self.store.postpone_delivery(d.id, Timestamp(now.0 + 15_000)).await?;
                return Ok(Done::idle(Outcome::Postponed));
            }
            Gate::Suppress(s) => {
                self.store.suppress_delivery(d.id, s.id, &silenced_detail(&s), now).await?;
                return Ok(Done::idle(Outcome::Suppressed));
            }
        }

        // A test is claimed by its own request ahead of its due time.
        let claim_at = if d.event == DeliveryEvent::Test { Timestamp(now.0.max(d.next_attempt_at.0)) } else { now };
        if !self.store.claim_delivery(d.id, claim_at, Timestamp(now.0 + ms(self.config.lease))).await? {
            return Ok(Done::idle(Outcome::Skipped));
        }

        let number = d.attempts + 1;
        let limit = if d.event == DeliveryEvent::Test { 1 } else { self.config.max_attempts.max(1) };
        let mut attempted = false;

        let (ok, http_status, error, retry) = if number > limit {
            // Every allowed attempt was started and the last one never
            // reported back (its lease ran out): no further send.
            (false, None, Some(FailureClass::Timeout), false)
        } else {
            match self.request(&d, &channel, alarm.as_ref()).await {
                Err(refusal) => (false, None, Some(refusal.class), refusal.retry),
                Ok(message) => {
                    attempted = true;

                    let outcome = match &message {
                        Message::Http(request) => self.transport.send(request).await,
                        Message::Mail(mail) => self.mail.send(mail).await,
                    };

                    match outcome {
                        SendOutcome::Status(s) | SendOutcome::Smtp(s) if (200..300).contains(&s) => {
                            (true, Some(s), None, false)
                        }
                        SendOutcome::Status(s) => (false, Some(s), Some(FailureClass::HttpStatus), retryable_status(s)),
                        SendOutcome::Smtp(c) => (false, Some(c), Some(FailureClass::SmtpReply), retryable_smtp(c)),
                        SendOutcome::Failed(class) => (false, None, Some(class), retryable_failure(class)),
                    }
                }
            }
        };

        let done = self.clock.now();
        let attempt = DeliveryAttempt {
            delivery_id: d.id,
            number,
            at: done,
            ok,
            http_status,
            error,
            duration_ms: (done.0 - now.0).max(0),
        };

        let (result, outcome) = if ok {
            (AttemptResult::Sent, Outcome::Sent)
        } else if retry && number < limit {
            let wait = retry_delay(self.config.retry, number, d.id);
            (AttemptResult::Retry(Timestamp(done.0 + ms(wait))), Outcome::Retried)
        } else {
            (AttemptResult::Failed, Outcome::Failed)
        };

        if !self.store.finish_attempt(&attempt, result).await? {
            // The lease ran out and a later attempt holds the delivery: this
            // one's result is not recorded and changes nothing.
            tracing::warn!(delivery = %d.id, attempt = number, "notification attempt finished after its lease was lost");
            return Ok(Done { outcome: Outcome::Skipped, error, attempted });
        }

        Ok(Done { outcome, error, attempted })
    }
}

#[cfg(test)]
mod tests;
