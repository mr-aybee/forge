//! `forge alerts channels | policies | deliveries`: alarm notifications
//! (#11, D-110).
//!
//! A channel is where a message goes, a policy says which alarms notify
//! which channels, a delivery is one message with its attempts. `update`
//! reads the stored object and changes only what the flags name (the server
//! replaces the whole object on `PUT`). A channel's URL usually carries a
//! token: it is read from a file or stdin, never from the command line, and
//! never printed. An email channel's SMTP password is handled the same way
//! (D-113).

use std::path::PathBuf;

use clap::{Args, Subcommand, ValueEnum};
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::Global;
use crate::admincmd::{enc, looks_like_id, node_id, pick};
use crate::api::Api;
use crate::appspec::{SecretSource, read_secrets};
use crate::cmds::confirm;
use crate::error::{CliError, Exit, Result};
use crate::out::{self, Col};
use crate::tunnelcmd::parse_lifetime;

const CHANNELS: &str = "/api/v1/notification-channels";
const POLICIES: &str = "/api/v1/notification-policies";
const DELIVERIES: &str = "/api/v1/notification-deliveries";
const VAULT_ITEMS: &str = "/api/v1/vault/items";
const USERS: &str = "/api/v1/users";

/// The reminder interval the server accepts, in seconds (5 minutes to 7 days).
const REPEAT_SECS: std::ops::RangeInclusive<u32> = 300..=604_800;

// ------------------------------------------------------------------ vocabulary

/// How a channel speaks.
#[derive(ValueEnum, Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// POST of Forge's JSON payload, optionally signed.
    Webhook,
    /// POST of `{"text": …}`: Slack incoming webhooks and compatible receivers.
    Slack,
    /// A plain-text email over SMTP.
    Email,
}

/// How an email channel protects its SMTP connection.
#[derive(ValueEnum, Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TlsMode {
    /// STARTTLS, required (port 587 by default).
    #[default]
    Starttls,
    /// Implicit TLS, SMTPS (port 465 by default).
    Tls,
    /// No encryption (port 25 by default); only where the server allows
    /// unencrypted targets, and never with --smtp-user.
    None,
}

/// How bad an alarm is.
#[derive(ValueEnum, Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Warning,
    Major,
    Critical,
}

/// What a policy reacts to.
#[derive(ValueEnum, Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Event {
    /// The alarm opened (and reminders while it keeps firing).
    Fired,
    /// The alarm closed: it recovered, or tracking ended without recovery.
    Resolved,
}

/// A role whose holders a policy emails (D-117).
#[derive(ValueEnum, Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[value(rename_all = "snake_case")]
pub enum EmailRole {
    PlatformAdmin,
    WorkspaceAdmin,
    Operator,
    Developer,
    Dba,
    Auditor,
    Customer,
}

/// `--scope`: the resources a policy is limited to.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeKind {
    /// Alarms on nodes (all of them, or the ones named with --scope-id).
    Servers,
    /// Alarms on services (all of them, or the ones named with --scope-id).
    Services,
    /// Alarms on every resource (removes a stored scope).
    All,
}

/// Where a delivery stands.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Pending,
    Sending,
    Sent,
    Failed,
    Cancelled,
    /// Held back by a silence.
    Suppressed,
}

impl Status {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Sending => "sending",
            Self::Sent => "sent",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Suppressed => "suppressed",
        }
    }
}

// ------------------------------------------------------------------ commands

/// The channel fields `create` and `update` take as flags.
#[derive(Args, Debug, Default, Clone)]
pub struct ChannelFlags {
    /// Name, 1 to 80 characters (required to create).
    #[arg(long)]
    pub name: Option<String>,
    /// What the receiver understands (required to create).
    #[arg(long, value_enum)]
    pub kind: Option<Kind>,
    /// The target URL, read from a file (`@FILE`) or from stdin (`@-`); a
    /// URL on the command line is refused. Required to create; without it
    /// an update keeps the stored URL.
    #[arg(long, value_name = "@FILE")]
    pub url: Option<String>,
    /// Vault item (id or name) whose password signs webhook bodies.
    #[arg(long, value_name = "ITEM", conflicts_with = "no_secret_item")]
    pub secret_item: Option<String>,
    /// Stop signing: remove the stored secret item.
    #[arg(long)]
    pub no_secret_item: bool,
    /// Turn the channel on (the default for a new one).
    #[arg(long, conflicts_with = "disabled")]
    pub enabled: bool,
    /// Turn the channel off: nothing is sent to it.
    #[arg(long)]
    pub disabled: bool,
    #[command(flatten)]
    pub email: EmailFlags,
}

/// The SMTP settings of an email channel (`--kind email`).
#[derive(Args, Debug, Default, Clone)]
pub struct EmailFlags {
    /// SMTP server host name or IP address (required for a new email channel).
    #[arg(long, value_name = "HOST")]
    pub smtp_host: Option<String>,
    /// SMTP port (default: 587 for starttls, 465 for tls, 25 for none).
    #[arg(long, value_name = "PORT")]
    pub smtp_port: Option<u16>,
    /// How the connection is protected (default for a new channel: starttls).
    #[arg(long, value_enum)]
    pub tls: Option<TlsMode>,
    /// SMTP user name; needs --smtp-password the first time.
    #[arg(long, value_name = "USER", conflicts_with = "no_smtp_user")]
    pub smtp_user: Option<String>,
    /// Send without authentication: removes the stored user name and password.
    #[arg(long, conflicts_with = "smtp_password")]
    pub no_smtp_user: bool,
    /// The SMTP password, read from a file (`@FILE`) or stdin (`@-`); a
    /// password on the command line is refused.
    #[arg(long, value_name = "@FILE")]
    pub smtp_password: Option<String>,
    /// Sender address: `forge@example.com` or `Forge <forge@example.com>`.
    #[arg(long, value_name = "ADDRESS")]
    pub from: Option<String>,
    /// A recipient; repeat for up to 20. Given at all, the flags replace the
    /// whole list.
    #[arg(long = "to", value_name = "ADDRESS")]
    pub to: Vec<String>,
    /// Put before every subject, up to 40 characters; `""` removes it.
    #[arg(long, value_name = "TEXT")]
    pub subject_prefix: Option<String>,
}

impl EmailFlags {
    fn any(&self) -> bool {
        self.smtp_host.is_some()
            || self.smtp_port.is_some()
            || self.tls.is_some()
            || self.smtp_user.is_some()
            || self.no_smtp_user
            || self.smtp_password.is_some()
            || self.from.is_some()
            || !self.to.is_empty()
            || self.subject_prefix.is_some()
    }
}

impl ChannelFlags {
    fn any(&self) -> bool {
        self.name.is_some()
            || self.kind.is_some()
            || self.url.is_some()
            || self.secret_item.is_some()
            || self.no_secret_item
            || self.enabled
            || self.disabled
            || self.email.any()
    }
}

#[derive(Subcommand, Debug)]
pub enum ChannelsCmd {
    /// Notification channels, by name. The URL is never shown, only its host.
    List {
        /// Only this workspace's channels (id).
        #[arg(long)]
        workspace: Option<String>,
        #[command(flatten)]
        paging: out::Paging,
    },
    /// One channel.
    #[command(alias = "get")]
    Show {
        /// The channel (id or name).
        channel: String,
    },
    /// Create a channel. The URL comes from a file or stdin.
    Create {
        #[command(flatten)]
        flags: ChannelFlags,
    },
    /// Change a channel: only what the flags name; the rest stays.
    Update {
        /// The channel (id or name).
        channel: String,
        #[command(flatten)]
        flags: ChannelFlags,
        /// Do not ask first (required with --non-interactive).
        #[arg(long)]
        yes: bool,
    },
    /// Delete a channel that no policy uses. Its delivery history stays.
    Delete {
        /// The channel (id or name).
        channel: String,
        /// Do not ask first (required with --non-interactive).
        #[arg(long)]
        yes: bool,
    },
    /// Send a test message now and show what happened (exit 10 unless sent).
    Test {
        /// The channel (id or name).
        channel: String,
    },
}

/// The policy fields `create` and `update` take as flags.
#[derive(Args, Debug, Default, Clone)]
pub struct PolicyFlags {
    /// Name, 1 to 80 characters (required to create).
    #[arg(long)]
    pub name: Option<String>,
    /// Notify for alarms of this severity and worse (required to create).
    #[arg(long, value_enum)]
    pub min_severity: Option<Severity>,
    /// An alarm event to notify for; repeat for both (at least one to
    /// create). Given at all, the flags replace the whole list.
    #[arg(long = "event", value_enum, value_name = "EVENT")]
    pub events: Vec<Event>,
    /// A channel to notify (id or name); repeat for up to 10 (at least one
    /// to create). Given at all, the flags replace the whole list.
    #[arg(long = "channel", value_name = "CHANNEL")]
    pub channels: Vec<String>,
    /// Only alarms of this rule (id); repeat for more. Given at all, the
    /// flags replace the whole list. Default: every rule.
    #[arg(long = "rule", value_name = "RULE", conflicts_with = "all_rules")]
    pub rules: Vec<String>,
    /// Alarms of every rule (removes the stored rule list).
    #[arg(long)]
    pub all_rules: bool,
    /// Only alarms on nodes or on services; `all` removes the limit.
    #[arg(long, value_enum)]
    pub scope: Option<ScopeKind>,
    /// A node (id, host name or name) or service (id) of --scope;
    /// repeat for more. Without any, the scope covers all of that kind.
    #[arg(long = "scope-id", value_name = "ID", requires = "scope")]
    pub scope_ids: Vec<String>,
    /// Remind this often while the alarm fires and nobody acknowledged it:
    /// a duration from 5m to 7d (`30m`, `4h`, `1d`), seconds, or `never`.
    #[arg(long, value_name = "DURATION")]
    pub repeat: Option<String>,
    /// Email this person (id, username or email) when they may see the
    /// alarm; repeat for up to 100. Given at all, the flags replace the
    /// whole list. Needs --email-channel (or one stored).
    #[arg(long = "email-user", value_name = "USER", conflicts_with = "no_email_recipients")]
    pub email_users: Vec<String>,
    /// Email everyone holding this role on the alarm's resource or an
    /// ancestor of it; repeat for up to 20. Given at all, the flags replace
    /// the whole list.
    #[arg(long = "email-role", value_enum, value_name = "ROLE", conflicts_with = "no_email_recipients")]
    pub email_roles: Vec<EmailRole>,
    /// The email channel (id or name) that sends to --email-user and
    /// --email-role.
    #[arg(long, value_name = "CHANNEL", conflicts_with = "no_email_recipients")]
    pub email_channel: Option<String>,
    /// Stop emailing people by user and role (removes the stored users,
    /// roles and email channel).
    #[arg(long)]
    pub no_email_recipients: bool,
    /// Turn the policy on (the default for a new one).
    #[arg(long, conflicts_with = "disabled")]
    pub enabled: bool,
    /// Turn the policy off: it notifies nobody.
    #[arg(long)]
    pub disabled: bool,
}

impl PolicyFlags {
    /// Whether the flags name people to email.
    fn people(&self) -> bool {
        !self.email_users.is_empty() || !self.email_roles.is_empty()
    }

    fn any(&self) -> bool {
        self.name.is_some()
            || self.min_severity.is_some()
            || !self.events.is_empty()
            || !self.channels.is_empty()
            || !self.rules.is_empty()
            || self.all_rules
            || self.scope.is_some()
            || self.repeat.is_some()
            || self.people()
            || self.email_channel.is_some()
            || self.no_email_recipients
            || self.enabled
            || self.disabled
    }
}

#[derive(Subcommand, Debug)]
pub enum PoliciesCmd {
    /// Notification policies, by name.
    List {
        /// Only this workspace's policies (id).
        #[arg(long)]
        workspace: Option<String>,
        #[command(flatten)]
        paging: out::Paging,
    },
    /// One policy.
    #[command(alias = "get")]
    Show {
        /// The policy (id or name).
        policy: String,
    },
    /// Create a policy from flags.
    Create {
        #[command(flatten)]
        flags: PolicyFlags,
    },
    /// Change a policy: only what the flags name; the rest stays.
    Update {
        /// The policy (id or name).
        policy: String,
        #[command(flatten)]
        flags: PolicyFlags,
        /// Do not ask first (required with --non-interactive).
        #[arg(long)]
        yes: bool,
    },
    /// Delete a policy. Deliveries already planned are still sent.
    Delete {
        /// The policy (id or name).
        policy: String,
        /// Do not ask first (required with --non-interactive).
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum DeliveriesCmd {
    /// Notification deliveries, newest first.
    List {
        /// Only this alarm's deliveries (alarm id).
        #[arg(long)]
        alarm: Option<String>,
        /// Only deliveries to this channel (id or name).
        #[arg(long)]
        channel: Option<String>,
        /// Only deliveries in this status.
        #[arg(long, value_enum)]
        status: Option<Status>,
        /// Only this workspace's deliveries (id).
        #[arg(long)]
        workspace: Option<String>,
        #[command(flatten)]
        paging: out::Paging,
    },
    /// One delivery with its attempts.
    #[command(alias = "get")]
    Show {
        /// The delivery (id).
        id: String,
    },
}

// ------------------------------------------------------------------ bodies

const NO_CHANNEL_CHANGE: &str = "pass what to change: --name, --kind, --url, --secret-item, --no-secret-item, --enabled, --disabled, or an email flag (--smtp-host, --smtp-port, --tls, --smtp-user, --no-smtp-user, --smtp-password, --from, --to, --subject-prefix)";
const NO_POLICY_CHANGE: &str = "pass what to change: --name, --min-severity, --event, --channel, --rule, --all-rules, --scope, --repeat, --email-user, --email-role, --email-channel, --no-email-recipients, --enabled or --disabled";
const NEW_CHANNEL_NEEDS: &str =
    "a new channel needs --name and --kind, then --url (webhook, slack) or --smtp-host, --from and --to (email)";

const fn yes() -> bool {
    true
}

/// The body of `POST|PUT /notification-channels`. Read back from a stored
/// channel, `url` is absent: the server never returns it.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct ChannelBody {
    pub name: String,
    pub kind: Kind,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_item: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<EmailBody>,
}

// Written by hand: the derived form would print the URL.
impl std::fmt::Debug for ChannelBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChannelBody")
            .field("name", &self.name)
            .field("kind", &self.kind)
            .field("enabled", &self.enabled)
            .field("url", &self.url.as_ref().map(|_| "<hidden>"))
            .field("secret_item", &self.secret_item)
            .field("email", &self.email)
            .finish()
    }
}

/// The `email` object of a channel body. Read back from a stored channel,
/// `password` is absent: the server never returns it (and the server keeps
/// the stored one while the user name stays).
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct EmailBody {
    pub smtp_host: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub smtp_port: Option<u16>,
    #[serde(default)]
    pub tls: TlsMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    pub from: String,
    pub to: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject_prefix: Option<String>,
}

// Written by hand: the derived form would print the password.
impl std::fmt::Debug for EmailBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmailBody")
            .field("smtp_host", &self.smtp_host)
            .field("smtp_port", &self.smtp_port)
            .field("tls", &self.tls)
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "<hidden>"))
            .field("from", &self.from)
            .field("to", &self.to)
            .field("subject_prefix", &self.subject_prefix)
            .finish()
    }
}

const NEW_EMAIL_NEEDS: &str = "an email channel needs --smtp-host, --from and at least one --to";

/// The email settings to send: `stored` changed by the flags, or new ones.
/// `password` is what `--smtp-password` read.
fn email_body(stored: Option<EmailBody>, f: &EmailFlags, password: Option<String>) -> Result<EmailBody> {
    let mut e = match stored {
        Some(e) => e,
        None => match (&f.smtp_host, &f.from) {
            (Some(host), Some(from)) if !f.to.is_empty() => EmailBody {
                smtp_host: host.clone(),
                smtp_port: None,
                tls: TlsMode::default(),
                username: None,
                password: None,
                from: from.clone(),
                to: Vec::new(),
                subject_prefix: None,
            },
            _ => return Err(CliError::usage(NEW_EMAIL_NEEDS)),
        },
    };

    if let Some(h) = &f.smtp_host {
        e.smtp_host.clone_from(h);
    }

    if let Some(t) = f.tls {
        // A new mode without a new port takes the mode's usual port.
        if f.smtp_port.is_none() && t != e.tls {
            e.smtp_port = None;
        }

        e.tls = t;
    }

    if let Some(p) = f.smtp_port {
        e.smtp_port = Some(p);
    }

    if f.no_smtp_user {
        e.username = None;
    } else if let Some(u) = &f.smtp_user {
        e.username = Some(u.clone());
    }

    e.password = password;

    if e.password.is_some() && e.username.is_none() {
        return Err(CliError::usage("--smtp-password needs --smtp-user"));
    }

    if e.tls == TlsMode::None && e.username.is_some() {
        return Err(CliError::usage(
            "a password is never sent without TLS: use --tls starttls or --tls tls, or pass --no-smtp-user",
        ));
    }

    if let Some(from) = &f.from {
        e.from.clone_from(from);
    }

    if !f.to.is_empty() {
        e.to = dedup(&f.to);
    }

    if let Some(p) = &f.subject_prefix {
        e.subject_prefix = Some(p.trim().to_owned()).filter(|p| !p.is_empty());
    }

    Ok(e)
}

/// `--enabled` / `--disabled` as a change (`None`: neither was given).
const fn switch(enabled: bool, disabled: bool) -> Option<bool> {
    if disabled {
        Some(false)
    } else if enabled {
        Some(true)
    } else {
        None
    }
}

/// What `--url`, `--secret-item` and `--smtp-password` resolved to.
#[derive(Default)]
pub struct Resolved {
    pub url: Option<String>,
    pub secret_item: Option<String>,
    pub smtp_password: Option<String>,
}

/// The channel to send. `stored` is the channel being updated (`None`: a
/// new one).
pub fn channel_body(stored: Option<ChannelBody>, f: &ChannelFlags, r: Resolved) -> Result<ChannelBody> {
    let Resolved { url, secret_item, smtp_password } = r;

    let mut body = match stored {
        Some(b) => {
            if !f.any() {
                return Err(CliError::usage(NO_CHANNEL_CHANGE));
            }

            b
        }
        None => match (&f.name, f.kind, &url) {
            (Some(name), Some(Kind::Email), None) => ChannelBody {
                name: name.clone(),
                kind: Kind::Email,
                enabled: true,
                url: None,
                secret_item: None,
                email: None,
            },
            (Some(name), Some(kind), Some(_)) if kind != Kind::Email => {
                ChannelBody { name: name.clone(), kind, enabled: true, url: None, secret_item: None, email: None }
            }
            _ => return Err(CliError::usage(NEW_CHANNEL_NEEDS)),
        },
    };

    if let Some(n) = &f.name {
        body.name.clone_from(n);
    }

    if let Some(k) = f.kind {
        body.kind = k;
    }

    if let Some(on) = switch(f.enabled, f.disabled) {
        body.enabled = on;
    }

    // Absent: the server keeps the stored URL.
    body.url = url;

    if f.no_secret_item {
        body.secret_item = None;
    } else if secret_item.is_some() {
        body.secret_item = secret_item;
    }

    if body.kind == Kind::Slack && body.secret_item.is_some() {
        return Err(CliError::usage(if f.secret_item.is_some() {
            "a slack channel is not signed (its URL is the secret): leave out --secret-item"
        } else {
            "a slack channel is not signed (its URL is the secret): pass --no-secret-item"
        }));
    }

    if body.kind == Kind::Email {
        if body.url.is_some() {
            return Err(CliError::usage("an email channel has no URL: leave out --url"));
        }

        if body.secret_item.is_some() {
            return Err(CliError::usage(if f.secret_item.is_some() {
                "an email channel is not signed: leave out --secret-item"
            } else {
                "an email channel is not signed: pass --no-secret-item"
            }));
        }

        body.email = Some(email_body(body.email.take(), &f.email, smtp_password)?);
    } else {
        if f.email.any() {
            return Err(CliError::usage("the SMTP flags (--smtp-host, --from, --to, …) are for --kind email only"));
        }

        if body.email.take().is_some() && body.url.is_none() {
            return Err(CliError::usage("an email channel becomes a webhook or slack channel only with a new --url"));
        }
    }

    Ok(body)
}

/// The resources a policy is limited to, as the API spells them.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Scope {
    Servers {
        #[serde(default)]
        ids: Vec<String>,
    },
    Services {
        #[serde(default)]
        ids: Vec<String>,
    },
}

/// The body of `POST|PUT /notification-policies`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct PolicyBody {
    pub name: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    pub min_severity: Severity,
    #[serde(default)]
    pub rules: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<Scope>,
    pub events: Vec<Event>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat_interval_secs: Option<u32>,
    #[serde(default)]
    pub channels: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email_channel: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub email_users: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub email_roles: Vec<EmailRole>,
}

/// The ids `--email-channel` and `--email-user` resolved to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct People {
    pub channel: Option<String>,
    pub users: Vec<String>,
}

/// `--repeat`: seconds, a duration (`30m`, `4h`, `1d`, `1h30m`) or `never`
/// (`None`). The server accepts 300 to 604800 seconds.
pub fn parse_repeat(s: &str) -> Result<Option<u32>> {
    let t = s.trim();
    let bad = || {
        CliError::usage(format!(
            "--repeat {t}: use a duration from 5m to 7d (such as 30m, 4h or 1d), seconds (300 to 604800), or `never`"
        ))
    };

    let secs = if !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()) {
        t.parse::<u64>().map_err(|_| bad())?
    } else {
        match parse_lifetime(t).map_err(|_| bad())? {
            None => return Ok(None),
            Some(minutes) => minutes.checked_mul(60).ok_or_else(bad)?,
        }
    };

    u32::try_from(secs).ok().filter(|s| REPEAT_SECS.contains(s)).map(Some).ok_or_else(bad)
}

fn dedup<T: PartialEq + Clone>(items: &[T]) -> Vec<T> {
    let mut out: Vec<T> = Vec::with_capacity(items.len());

    for i in items {
        if !out.contains(i) {
            out.push(i.clone());
        }
    }

    out
}

/// Applies `--email-user`, `--email-role`, `--email-channel` and
/// `--no-email-recipients` (resolved to ids in `people`) to a policy body.
pub fn apply_people(body: &mut PolicyBody, f: &PolicyFlags, people: &People) -> Result<()> {
    if f.no_email_recipients {
        body.email_channel = None;
        body.email_users.clear();
        body.email_roles.clear();
    } else {
        if !f.email_users.is_empty() {
            body.email_users = dedup(&people.users);
        }

        if !f.email_roles.is_empty() {
            body.email_roles = dedup(&f.email_roles);
        }

        if let Some(c) = &people.channel {
            body.email_channel = Some(c.clone());
        }
    }

    let has_people = !body.email_users.is_empty() || !body.email_roles.is_empty();
    if has_people && body.email_channel.is_none() {
        return Err(CliError::usage(
            "--email-user and --email-role need --email-channel: the email channel (SMTP server) that sends",
        ));
    }

    if !has_people {
        body.email_channel = None;
    }

    if body.channels.is_empty() && !has_people {
        return Err(CliError::usage("a policy needs at least one --channel, --email-user or --email-role"));
    }

    Ok(())
}

/// The policy to send. `stored` is the policy being updated (`None`: a new
/// one); `channels` and `scope_ids` are the ids `--channel` and `--scope-id`
/// resolved to.
pub fn policy_body(
    stored: Option<PolicyBody>,
    f: &PolicyFlags,
    channels: &[String],
    scope_ids: &[String],
) -> Result<PolicyBody> {
    let repeat = f.repeat.as_deref().map(parse_repeat).transpose()?;

    let mut body = match stored {
        Some(b) => {
            if !f.any() {
                return Err(CliError::usage(NO_POLICY_CHANGE));
            }

            b
        }
        None => match (&f.name, f.min_severity) {
            (Some(name), Some(min_severity)) if !f.events.is_empty() && (!channels.is_empty() || f.people()) => {
                PolicyBody {
                    name: name.clone(),
                    enabled: true,
                    min_severity,
                    rules: Vec::new(),
                    scope: None,
                    events: Vec::new(),
                    repeat_interval_secs: None,
                    channels: Vec::new(),
                    email_channel: None,
                    email_users: Vec::new(),
                    email_roles: Vec::new(),
                }
            }
            _ => {
                return Err(CliError::usage(
                    "a new policy needs --name, --min-severity, at least one --event and at least one --channel (or --email-user or --email-role)",
                ));
            }
        },
    };

    if let Some(n) = &f.name {
        body.name.clone_from(n);
    }

    if let Some(s) = f.min_severity {
        body.min_severity = s;
    }

    if let Some(on) = switch(f.enabled, f.disabled) {
        body.enabled = on;
    }

    if !f.events.is_empty() {
        body.events = dedup(&f.events);
    }

    if !channels.is_empty() {
        body.channels = dedup(channels);
    }

    if f.all_rules {
        body.rules.clear();
    } else if !f.rules.is_empty() {
        body.rules = dedup(&f.rules);
    }

    match f.scope {
        None => {}
        Some(ScopeKind::Servers) => body.scope = Some(Scope::Servers { ids: dedup(scope_ids) }),
        Some(ScopeKind::Services) => body.scope = Some(Scope::Services { ids: dedup(scope_ids) }),
        Some(ScopeKind::All) if scope_ids.is_empty() => body.scope = None,
        Some(ScopeKind::All) => {
            return Err(CliError::usage("--scope all takes no --scope-id: it covers every resource"));
        }
    }

    if let Some(r) = repeat {
        body.repeat_interval_secs = r;
    }

    if body.repeat_interval_secs.is_some() && !body.events.contains(&Event::Fired) {
        return Err(CliError::usage(
            "reminders (--repeat) need --event fired: pass it, or turn reminders off with --repeat never",
        ));
    }

    Ok(body)
}

/// A stored channel or policy as the body an update starts from.
fn stored<T: serde::de::DeserializeOwned>(what: &str, v: &Value) -> Result<T> {
    T::deserialize(v).map_err(|e| {
        CliError::new(
            Exit::General,
            format!("the stored {what} has a value this CLI does not know ({e}); update the CLI"),
        )
        .with_code("unknown_value")
    })
}

// ------------------------------------------------------------------ the URL

/// A secret flag: `--url` (a channel URL) or `--smtp-password`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretFlag {
    Url,
    SmtpPassword,
}

impl SecretFlag {
    const fn flag(self) -> &'static str {
        match self {
            Self::Url => "--url",
            Self::SmtpPassword => "--smtp-password",
        }
    }

    const fn what(self) -> &'static str {
        match self {
            Self::Url => "a channel URL carries a token and",
            Self::SmtpPassword => "an SMTP password",
        }
    }
}

/// Where `--url` or `--smtp-password` reads its value from. A literal value
/// is refused without repeating it: it would stay in the shell history and
/// the process list.
pub fn secret_source(which: SecretFlag, spec: &str) -> Result<SecretSource> {
    match spec.strip_prefix('@') {
        Some("-") => Ok(SecretSource::Stdin),
        Some(path) if !path.is_empty() => Ok(SecretSource::File(PathBuf::from(path))),
        _ => {
            let (flag, what) = (which.flag(), which.what());
            Err(CliError::usage(format!(
                "{flag}: {what} is never taken from the command line; pass `{flag} @<file>`, or pipe it on stdin and pass `{flag} @-`"
            ))
            .with_code("secret_on_command_line"))
        }
    }
}

/// Where `--url` reads the URL from.
#[cfg(test)]
pub fn url_source(spec: &str) -> Result<SecretSource> {
    secret_source(SecretFlag::Url, spec)
}

/// The value `--url` or `--smtp-password` names. A URL loses surrounding
/// white space; a password keeps it (only the line break is dropped).
fn read_secret(which: SecretFlag, spec: &str, stdin: impl FnOnce() -> Result<String>) -> Result<String> {
    let source = secret_source(which, spec)?;
    read_secrets(&[(which.flag().trim_start_matches('-').to_owned(), source)], stdin)?
        .pop()
        .map(|(_, v)| if which == SecretFlag::Url { v.trim().to_owned() } else { v })
        .filter(|v| !v.is_empty())
        .ok_or_else(|| CliError::usage(format!("the value of {} is empty", which.flag())))
}

fn secret_from_flag(which: SecretFlag, spec: Option<&str>) -> Result<Option<String>> {
    use std::io::IsTerminal as _;
    spec.map(|s| {
        read_secret(which, s, || {
            if std::io::stdin().is_terminal() {
                return Err(CliError::usage(format!(
                    "pipe the value of {0} on stdin (it is never taken from a flag), or pass `{0} @<file>`",
                    which.flag()
                )));
            }

            Ok(std::io::read_to_string(std::io::stdin())?)
        })
    })
    .transpose()
}

/// Reads `--url` and `--smtp-password` (at most one of them from stdin) and
/// resolves `--secret-item`.
async fn resolve_flags(api: &mut Api, flags: &ChannelFlags) -> Result<Resolved> {
    if flags.url.as_deref() == Some("@-") && flags.email.smtp_password.as_deref() == Some("@-") {
        return Err(CliError::usage("only one of --url and --smtp-password can read stdin (`@-`)"));
    }

    Ok(Resolved {
        url: secret_from_flag(SecretFlag::Url, flags.url.as_deref())?,
        smtp_password: secret_from_flag(SecretFlag::SmtpPassword, flags.email.smtp_password.as_deref())?,
        secret_item: secret_item_id(api, flags.secret_item.as_deref()).await?,
    })
}

// ------------------------------------------------------------------ views

const CHANNEL_COLS: &[Col] = &[
    ("ID", "/id"),
    ("NAME", "/name"),
    ("KIND", "/kind"),
    ("ENABLED", "/enabled"),
    ("TARGET", "/target"),
    ("SECRET ITEM", "/secret_item"),
];

const POLICY_COLS: &[Col] = &[
    ("ID", "/id"),
    ("NAME", "/name"),
    ("ENABLED", "/enabled"),
    ("MIN SEVERITY", "/min_severity"),
    ("EVENTS", "/events"),
    ("CHANNELS", "/channel_names"),
    ("REPEAT (S)", "/repeat_interval_secs"),
];

const DELIVERY_COLS: &[Col] = &[
    ("ID", "/id"),
    ("STATUS", "/status"),
    ("EVENT", "/event"),
    ("CHANNEL", "/channel_name"),
    ("ALARM", "/alarm_title"),
    ("ATTEMPTS", "/attempt_count"),
    ("HTTP", "/last_http_status"),
    ("ERROR", "/last_error"),
    ("CREATED", "/created_at"),
];

const ATTEMPT_COLS: &[Col] = &[
    ("NUMBER", "/number"),
    ("AT", "/at"),
    ("OK", "/ok"),
    ("HTTP", "/http_status"),
    ("ERROR", "/error"),
    ("DURATION (MS)", "/duration_ms"),
];

/// A policy list row: the policy plus its channels' names as one cell (the
/// id of a channel that is not in `channels`).
fn policy_row(policy: &Value, channels: &Value) -> Value {
    let names: Vec<&str> = policy["channels"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|id| {
            channels
                .as_array()
                .into_iter()
                .flatten()
                .find(|c| c["id"] == id)
                .and_then(|c| c["name"].as_str())
                .unwrap_or(id)
        })
        .collect();
    let mut row = policy.clone();
    row["channel_names"] = json!(names.join(", "));

    row
}

/// A delivery with its attempts, for people: the delivery's fields, then
/// one table row per attempt.
fn delivery_text(view: &Value) -> String {
    let mut delivery = view.clone();
    let attempts = delivery.as_object_mut().and_then(|d| d.remove("attempts")).unwrap_or(Value::Null);
    let mut text = out::render_human(&delivery, None);

    if attempts.as_array().is_some_and(|a| !a.is_empty()) {
        text.push_str("attempts:\n");

        for line in out::render_human(&attempts, Some(ATTEMPT_COLS)).lines() {
            text.push_str("  ");
            text.push_str(line);
            text.push('\n');
        }
    }

    text
}

fn show_delivery(g: &Global, view: &Value) {
    if g.output.is_machine() {
        out::result(g.output, view, None);
    } else {
        out::emit(&delivery_text(view));
    }
}

/// Whether a test delivery reached the receiver; otherwise the error the
/// command ends with (exit 10), naming the status and the failure class.
pub fn test_outcome(channel: &str, view: &Value) -> Result<()> {
    let status = view["status"].as_str().unwrap_or("unknown");
    if status == "sent" {
        return Ok(());
    }

    let mut why = format!("status {status}");

    if let Some(code) = view["last_http_status"].as_u64() {
        let proto = if view["last_error"] == "smtp_reply" { "SMTP" } else { "HTTP" };
        why.push_str(&format!(", {proto} {code}"));
    }

    if let Some(class) = view["last_error"].as_str() {
        why.push_str(&format!(", error {class}"));
    }

    Err(CliError::new(
        Exit::JobFailed,
        format!(
            "the test message to channel {channel} was not delivered ({why}); see `forge alerts deliveries show {}`",
            view["id"].as_str().unwrap_or("<id>")
        ),
    )
    .with_code("delivery_failed"))
}

// ------------------------------------------------------------------ lookups

fn with_workspace(base: &str, workspace: Option<&str>) -> String {
    match workspace {
        Some(w) => format!("{base}?workspace={}", enc(w)),
        None => base.to_owned(),
    }
}

/// One channel or policy by id, or by its name among those of the workspace.
async fn resolve(api: &mut Api, base: &str, what: &str, needle: &str) -> Result<Value> {
    if looks_like_id(needle) {
        return api.get(&format!("{base}/{}", enc(needle))).await;
    }

    pick(&api.get(base).await?, &["name"], what, needle)
}

fn id_of(v: &Value) -> &str {
    v["id"].as_str().unwrap_or_default()
}

fn name_of<'a>(v: &'a Value, typed: &'a str) -> &'a str {
    v["name"].as_str().unwrap_or(typed)
}

/// The ids of channels given by id or name.
async fn channel_ids(api: &mut Api, channels: &[String]) -> Result<Vec<String>> {
    if channels.iter().all(|c| looks_like_id(c)) {
        return Ok(channels.to_vec());
    }

    let all = api.get(CHANNELS).await?;

    channels.iter().map(|c| pick(&all, &["name"], "notification channel", c).map(|c| id_of(&c).to_owned())).collect()
}

/// The id of the vault item `--secret-item` names (id or name).
async fn secret_item_id(api: &mut Api, item: Option<&str>) -> Result<Option<String>> {
    match item {
        None => Ok(None),
        Some(i) if looks_like_id(i) => Ok(Some(i.to_owned())),
        Some(i) => {
            let all = api.get(VAULT_ITEMS).await?;
            Ok(Some(id_of(&pick(&all, &["name"], "vault item", i)?).to_owned()))
        }
    }
}

/// The ids `--email-channel` and `--email-user` name: the channel by id or
/// name, users by id, username or email.
async fn people_ids(api: &mut Api, f: &PolicyFlags) -> Result<People> {
    let channel = match &f.email_channel {
        Some(c) => channel_ids(api, std::slice::from_ref(c)).await?.pop(),
        None => None,
    };

    let users = if f.email_users.iter().all(|u| looks_like_id(u)) {
        f.email_users.clone()
    } else {
        let all = api.get(USERS).await?;
        f.email_users
            .iter()
            .map(|u| {
                if looks_like_id(u) {
                    Ok(u.clone())
                } else {
                    pick(&all, &["username", "email"], "user", u).map(|v| id_of(&v).to_owned())
                }
            })
            .collect::<Result<Vec<_>>>()?
    };

    Ok(People { channel, users })
}

/// The ids `--scope-id` names: nodes by id, host name or name; services by id.
async fn scope_ids(api: &mut Api, f: &PolicyFlags) -> Result<Vec<String>> {
    if f.scope != Some(ScopeKind::Servers) {
        return Ok(f.scope_ids.clone());
    }

    let mut ids = Vec::with_capacity(f.scope_ids.len());

    for s in &f.scope_ids {
        ids.push(if looks_like_id(s) { s.clone() } else { node_id(api, s).await? });
    }

    Ok(ids)
}

// ------------------------------------------------------------------ run

pub async fn channels(g: &Global, server: &str, api: &mut Api, cmd: ChannelsCmd) -> Result<()> {
    match cmd {
        ChannelsCmd::List { workspace, paging } => {
            let v = api.get(&with_workspace(CHANNELS, workspace.as_deref())).await?;
            out::list_value(g.output, v, "", Some(CHANNEL_COLS), paging);
            Ok(())
        }
        ChannelsCmd::Show { channel } => {
            let c = resolve(api, CHANNELS, "notification channel", &channel).await?;
            out::result(g.output, &c, None);
            Ok(())
        }
        ChannelsCmd::Create { flags } => {
            // Checked before stdin or the server is touched.
            let complete = match flags.kind {
                Some(Kind::Email) => {
                    flags.email.smtp_host.is_some() && flags.email.from.is_some() && !flags.email.to.is_empty()
                }
                Some(_) => flags.url.is_some(),
                None => false,
            };

            if flags.name.is_none() || !complete {
                return Err(CliError::usage(NEW_CHANNEL_NEEDS));
            }

            let resolved = resolve_flags(api, &flags).await?;
            let body = channel_body(None, &flags, resolved)?;
            let v = api.post(CHANNELS, &serde_json::to_value(&body)?).await?;
            out::result(g.output, &v, None);
            Ok(())
        }
        ChannelsCmd::Update { channel, flags, yes } => {
            if !flags.any() {
                return Err(CliError::usage(NO_CHANNEL_CHANGE));
            }

            let cur = resolve(api, CHANNELS, "notification channel", &channel).await?;
            let resolved = resolve_flags(api, &flags).await?;
            let body = channel_body(Some(stored("channel", &cur)?), &flags, resolved)?;
            confirm(g, yes, &format!("Update notification channel {} on {server}?", name_of(&cur, &channel)))?;
            let path = format!("{CHANNELS}/{}", enc(id_of(&cur)));
            let v = api.call(Method::PUT, &path, Some(&serde_json::to_value(&body)?)).await?;
            out::result(g.output, &v, None);
            Ok(())
        }
        ChannelsCmd::Delete { channel, yes } => {
            let cur = resolve(api, CHANNELS, "notification channel", &channel).await?;
            let (id, name) = (id_of(&cur), name_of(&cur, &channel));
            confirm(g, yes, &format!("Delete notification channel {name} on {server}? Its delivery history stays."))?;
            api.call(Method::DELETE, &format!("{CHANNELS}/{}", enc(id)), None).await?;
            out::result(g.output, &json!({ "deleted": id, "name": name }), None);
            Ok(())
        }
        ChannelsCmd::Test { channel } => {
            let cur = resolve(api, CHANNELS, "notification channel", &channel).await?;
            let view = api.post(&format!("{CHANNELS}/{}/test", enc(id_of(&cur))), &json!({})).await?;
            let outcome = test_outcome(name_of(&cur, &channel), &view);

            // One document on stdout in machine formats: the delivery, or the error.
            if outcome.is_ok() || !g.output.is_machine() {
                show_delivery(g, &view);
            }

            outcome
        }
    }
}

pub async fn policies(g: &Global, server: &str, api: &mut Api, cmd: PoliciesCmd) -> Result<()> {
    match cmd {
        PoliciesCmd::List { workspace, paging } => {
            let v = api.get(&with_workspace(POLICIES, workspace.as_deref())).await?;
            let all = api.get(&with_workspace(CHANNELS, workspace.as_deref())).await?;
            let rows: Vec<Value> = v.as_array().into_iter().flatten().map(|p| policy_row(p, &all)).collect();
            out::list(g.output, rows, Some(POLICY_COLS), paging.into(), out::Total::Rows);
            Ok(())
        }
        PoliciesCmd::Show { policy } => {
            let p = resolve(api, POLICIES, "notification policy", &policy).await?;
            out::result(g.output, &p, None);
            Ok(())
        }
        PoliciesCmd::Create { flags } => {
            // Checked before the server is asked for anything.
            let mut check = policy_body(None, &flags, &flags.channels, &flags.scope_ids)?;
            let typed = People { channel: flags.email_channel.clone(), users: flags.email_users.clone() };
            apply_people(&mut check, &flags, &typed)?;
            let channels = channel_ids(api, &flags.channels).await?;
            let ids = scope_ids(api, &flags).await?;
            let people = people_ids(api, &flags).await?;
            let mut body = policy_body(None, &flags, &channels, &ids)?;
            apply_people(&mut body, &flags, &people)?;
            let v = api.post(POLICIES, &serde_json::to_value(&body)?).await?;
            out::result(g.output, &v, None);
            Ok(())
        }
        PoliciesCmd::Update { policy, flags, yes } => {
            if !flags.any() {
                return Err(CliError::usage(NO_POLICY_CHANGE));
            }

            let cur = resolve(api, POLICIES, "notification policy", &policy).await?;
            let channels = channel_ids(api, &flags.channels).await?;
            let ids = scope_ids(api, &flags).await?;
            let people = people_ids(api, &flags).await?;
            let mut body = policy_body(Some(stored("policy", &cur)?), &flags, &channels, &ids)?;
            apply_people(&mut body, &flags, &people)?;
            confirm(g, yes, &format!("Update notification policy {} on {server}?", name_of(&cur, &policy)))?;
            let path = format!("{POLICIES}/{}", enc(id_of(&cur)));
            let v = api.call(Method::PUT, &path, Some(&serde_json::to_value(&body)?)).await?;
            out::result(g.output, &v, None);
            Ok(())
        }
        PoliciesCmd::Delete { policy, yes } => {
            let cur = resolve(api, POLICIES, "notification policy", &policy).await?;
            let (id, name) = (id_of(&cur), name_of(&cur, &policy));
            confirm(
                g,
                yes,
                &format!("Delete notification policy {name} on {server}? Deliveries already planned are still sent."),
            )?;
            api.call(Method::DELETE, &format!("{POLICIES}/{}", enc(id)), None).await?;
            out::result(g.output, &json!({ "deleted": id, "name": name }), None);
            Ok(())
        }
    }
}

pub async fn deliveries(g: &Global, api: &mut Api, cmd: DeliveriesCmd) -> Result<()> {
    match cmd {
        DeliveriesCmd::List { alarm, channel, status, workspace, paging } => {
            let page: out::Page = paging.into();

            // The server caps a request at 500 deliveries.
            let fetch = page.fetch().unwrap_or(500).min(500);
            let mut q = format!("{DELIVERIES}?limit={fetch}");

            if let Some(w) = workspace {
                q.push_str(&format!("&workspace={}", enc(&w)));
            }

            if let Some(a) = alarm {
                q.push_str(&format!("&alarm={}", enc(&a)));
            }

            if let Some(c) = channel {
                let ids = channel_ids(api, std::slice::from_ref(&c)).await?;
                q.push_str(&format!("&channel={}", enc(ids.first().map_or(c.as_str(), String::as_str))));
            }

            if let Some(s) = status {
                q.push_str(&format!("&status={}", s.as_str()));
            }

            let rows = crate::cmds::rows_of(api.get(&q).await?);
            let total = if rows.len() >= fetch { out::Total::AtLeast } else { out::Total::Rows };
            out::list(g.output, rows, Some(DELIVERY_COLS), page, total);
            Ok(())
        }
        DeliveriesCmd::Show { id } => {
            let view = api.get(&format!("{DELIVERIES}/{}", enc(&id))).await?;
            show_delivery(g, &view);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;
    use crate::alertscmd::AlertsCmd;
    use crate::{Cli, Cmd};

    const URL: &str = "https://hooks.example.com/services/T000/B000/token";

    fn parse(args: &[&str]) -> AlertsCmd {
        let mut all = vec!["forge", "alerts"];
        all.extend_from_slice(args);

        match Cli::try_parse_from(all).map(|c| c.cmd) {
            Ok(Cmd::Alerts(c)) => c,
            other => panic!("{other:?}"),
        }
    }

    fn refused(args: &[&str]) -> bool {
        let mut all = vec!["forge", "alerts"];
        all.extend_from_slice(args);

        Cli::try_parse_from(all).is_err()
    }

    fn channel_flags(args: &[&str]) -> ChannelFlags {
        let mut all = vec!["channels", "create"];
        all.extend_from_slice(args);

        match parse(&all) {
            AlertsCmd::Channels(ChannelsCmd::Create { flags }) => flags,
            other => panic!("{other:?}"),
        }
    }

    fn policy_flags(args: &[&str]) -> PolicyFlags {
        let mut all = vec!["policies", "create"];
        all.extend_from_slice(args);

        match parse(&all) {
            AlertsCmd::Policies(PoliciesCmd::Create { flags }) => flags,
            other => panic!("{other:?}"),
        }
    }

    fn r(url: Option<&str>, item: Option<&str>) -> Resolved {
        Resolved { url: url.map(str::to_owned), secret_item: item.map(str::to_owned), smtp_password: None }
    }

    fn ids(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    fn stored_policy() -> PolicyBody {
        PolicyBody {
            name: "Critical to ops".into(),
            enabled: true,
            min_severity: Severity::Critical,
            rules: ids(&["r1"]),
            scope: Some(Scope::Servers { ids: ids(&["s1"]) }),
            events: vec![Event::Fired, Event::Resolved],
            repeat_interval_secs: Some(3600),
            channels: ids(&["c1", "c2"]),
            email_channel: None,
            email_users: vec![],
            email_roles: vec![],
        }
    }

    #[test]
    fn policy_people_flags_parse_and_apply() {
        let f = policy_flags(&[
            "--name",
            "People",
            "--min-severity",
            "major",
            "--event",
            "fired",
            "--email-user",
            "ana",
            "--email-user",
            "ana",
            "--email-user",
            "bo@example.com",
            "--email-role",
            "operator",
            "--email-role",
            "workspace_admin",
            "--email-channel",
            "Ops mail",
        ]);
        assert_eq!(f.email_users, ["ana", "ana", "bo@example.com"]);
        assert_eq!(f.email_roles, [EmailRole::Operator, EmailRole::WorkspaceAdmin]);
        assert_eq!(f.email_channel.as_deref(), Some("Ops mail"));

        // No --channel is needed when people are emailed.
        let mut body = policy_body(None, &f, &[], &[]).unwrap_or_else(|e| panic!("{e}"));
        let people = People { channel: Some("c9".into()), users: ids(&["u1", "u1", "u2"]) };
        apply_people(&mut body, &f, &people).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            (body.email_channel.as_deref(), body.email_users.clone(), body.email_roles.clone()),
            (Some("c9"), ids(&["u1", "u2"]), vec![EmailRole::Operator, EmailRole::WorkspaceAdmin])
        );
        let json = serde_json::to_value(&body).unwrap_or_default();
        assert_eq!(json["email_roles"], json!(["operator", "workspace_admin"]));
        assert_eq!(json["channels"], json!([]));

        // People without an email channel are refused.
        let mut no_channel = body.clone();
        no_channel.email_channel = None;
        let only_users = policy_flags(&["--email-user", "u1"]);
        assert!(
            apply_people(&mut no_channel, &only_users, &People { channel: None, users: ids(&["u1"]) })
                .is_err_and(|e| e.message.contains("--email-channel"))
        );

        // --no-email-recipients clears them; the stored channels keep the policy whole.
        let mut stored = PolicyBody { email_channel: Some("c9".into()), email_users: ids(&["u1"]), ..stored_policy() };
        let all = vec!["policies", "update", "p1", "--no-email-recipients"];
        let AlertsCmd::Policies(PoliciesCmd::Update { flags, .. }) = parse(&all) else { panic!("update") };
        assert!(flags.any());
        apply_people(&mut stored, &flags, &People::default()).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(stored, stored_policy());
        assert!(refused(&["policies", "update", "p1", "--no-email-recipients", "--email-role", "operator"]));
        assert!(refused(&["policies", "create", "--email-role", "boss"]));
    }

    #[test]
    fn channel_commands_parse_their_flags() {
        let f = channel_flags(&["--name", "Ops webhook", "--kind", "webhook", "--url", "@url.txt", "--disabled"]);
        assert_eq!(
            (f.name.as_deref(), f.kind, f.url.as_deref()),
            (Some("Ops webhook"), Some(Kind::Webhook), Some("@url.txt"))
        );
        assert!(f.disabled && !f.enabled && f.secret_item.is_none());

        match parse(&["channels", "update", "ops", "--secret-item", "hmac-key", "--enabled", "--yes"]) {
            AlertsCmd::Channels(ChannelsCmd::Update { channel, flags, yes }) => {
                assert_eq!(
                    (channel.as_str(), flags.secret_item.as_deref(), flags.enabled, yes),
                    ("ops", Some("hmac-key"), true, true)
                );
            }
            other => panic!("{other:?}"),
        }

        assert!(matches!(parse(&["channels", "get", "ops"]), AlertsCmd::Channels(ChannelsCmd::Show { .. })));
        assert!(matches!(parse(&["channels", "test", "ops"]), AlertsCmd::Channels(ChannelsCmd::Test { .. })));
        assert!(matches!(
            parse(&["channels", "delete", "ops", "--yes"]),
            AlertsCmd::Channels(ChannelsCmd::Delete { yes: true, .. })
        ));
        assert!(refused(&["channels", "create", "--kind", "sms"]));
        assert!(refused(&["channels", "update", "ops", "--enabled", "--disabled"]));
        assert!(refused(&["channels", "update", "ops", "--secret-item", "x", "--no-secret-item"]));
    }

    #[test]
    fn a_url_is_read_from_a_file_or_stdin_and_never_from_the_command_line() {
        assert_eq!(url_source("@-").ok(), Some(SecretSource::Stdin));
        assert_eq!(url_source("@hook.txt").ok(), Some(SecretSource::File(PathBuf::from("hook.txt"))));

        for literal in [URL, "@", ""] {
            let e = url_source(literal).err().map(|e| (e.exit, e.code, e.message));
            let Some((exit, code, message)) = e else { panic!("{literal} accepted") };
            assert_eq!((exit, code.as_str()), (Exit::Usage, "secret_on_command_line"));
            assert!(!message.contains("hooks.example.com"), "the refusal does not repeat the URL");
        }

        // Stdin: the trailing newline of `echo` goes.
        assert_eq!(read_secret(SecretFlag::Url, "@-", || Ok(format!("{URL}\r\n"))).ok().as_deref(), Some(URL));
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let file = dir.path().join("hook.txt");
        std::fs::write(&file, format!("  {URL}\n")).unwrap_or_else(|e| panic!("{e}"));
        let spec = format!("@{}", file.display());
        assert_eq!(
            read_secret(SecretFlag::Url, &spec, || panic!("stdin is not read for a file")).ok().as_deref(),
            Some(URL)
        );
        assert!(read_secret(SecretFlag::Url, "@-", || Ok("\n".into())).is_err_and(|e| e.exit == Exit::Usage));
        assert!(read_secret(SecretFlag::Url, "@-", || Ok(format!("{URL}\nsecond line\n"))).is_err());
        assert!(
            read_secret(SecretFlag::Url, "@no-such-file.txt", || Ok(String::new()))
                .is_err_and(|e| e.exit == Exit::Usage)
        );
    }

    #[test]
    fn a_new_channel_needs_name_kind_and_url() {
        let f = channel_flags(&["--name", "Ops webhook", "--kind", "webhook", "--url", "@u"]);
        let body = channel_body(None, &f, r(Some(URL), Some("item-1"))).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            serde_json::to_value(&body).ok(),
            Some(
                json!({ "name": "Ops webhook", "kind": "webhook", "enabled": true, "url": URL, "secret_item": "item-1" })
            )
        );
        assert!(!format!("{body:?}").contains("hooks.example.com"), "Debug hides the URL");
        let off = channel_flags(&["--name", "Chat", "--kind", "slack", "--url", "@u", "--disabled"]);
        let body = channel_body(None, &off, r(Some(URL), None)).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            serde_json::to_value(&body).ok(),
            Some(json!({ "name": "Chat", "kind": "slack", "enabled": false, "url": URL }))
        );

        // Missing pieces, and a signed slack channel.
        assert!(channel_body(None, &f, r(None, None)).is_err_and(|e| e.exit == Exit::Usage));
        let no_kind = channel_flags(&["--name", "x", "--url", "@u"]);
        assert!(channel_body(None, &no_kind, r(Some(URL), None)).is_err());
        let signed = channel_flags(&["--name", "Chat", "--kind", "slack", "--url", "@u", "--secret-item", "k"]);
        assert!(
            channel_body(None, &signed, r(Some(URL), Some("k")))
                .is_err_and(|e| e.message.contains("leave out --secret-item"))
        );
    }

    #[test]
    fn a_channel_update_changes_only_what_was_given() {
        let cur: ChannelBody = stored(
            "channel",
            &json!({
                "id": "c1", "workspace_id": "w1", "name": "Ops webhook", "kind": "webhook", "enabled": true,
                "target": "https://hooks.example.com", "secret_item": "item-1",
                "created_by": { "type": "user", "id": "u1" }, "created_at": 1, "updated_at": 2,
            }),
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let update = |args: &[&str], url: Option<&str>, secret: Option<&str>| {
            let mut all = vec!["channels", "update", "ops"];
            all.extend_from_slice(args);
            let AlertsCmd::Channels(ChannelsCmd::Update { flags, .. }) = parse(&all) else { panic!("update") };
            channel_body(Some(cur.clone()), &flags, r(url, secret))
        };

        // Off: name, kind and secret stay; no URL is sent, so the stored one stays.
        let body = update(&["--disabled"], None, None).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            serde_json::to_value(&body).ok(),
            Some(json!({ "name": "Ops webhook", "kind": "webhook", "enabled": false, "secret_item": "item-1" }))
        );
        let body = update(&["--name", "Ops", "--url", "@u"], Some(URL), None).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!((body.name.as_str(), body.url.as_deref(), body.enabled), ("Ops", Some(URL), true));
        let body = update(&["--no-secret-item"], None, None).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(body.secret_item, None);
        let body = update(&["--secret-item", "other"], None, Some("item-2")).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(body.secret_item.as_deref(), Some("item-2"));

        // A signed channel cannot become a slack one without dropping the secret.
        assert!(update(&["--kind", "slack"], None, None).is_err_and(|e| e.message.contains("--no-secret-item")));
        assert!(update(&["--kind", "slack", "--no-secret-item"], None, None).is_ok());
        assert!(update(&[], None, None).is_err_and(|e| e.exit == Exit::Usage));

        // A kind this build does not know is an error, not a guess.
        let unknown = stored::<ChannelBody>("channel", &json!({ "name": "Pager", "kind": "sms" }));
        assert!(unknown.is_err_and(|e| e.code == "unknown_value"));
    }

    #[test]
    fn an_email_channel_is_built_from_the_smtp_flags() {
        let f = channel_flags(&[
            "--name",
            "Ops mail",
            "--kind",
            "email",
            "--smtp-host",
            "smtp.example.com",
            "--smtp-user",
            "forge",
            "--smtp-password",
            "@pw.txt",
            "--from",
            "Forge <forge@example.com>",
            "--to",
            "ops@example.com",
            "--to",
            "oncall@example.com",
            "--to",
            "ops@example.com",
            "--subject-prefix",
            "[prod]",
        ]);
        let with_pw = Resolved { smtp_password: Some("Sm7p pw ".into()), ..Resolved::default() };
        let body = channel_body(None, &f, with_pw).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            serde_json::to_value(&body).ok(),
            Some(json!({
                "name": "Ops mail", "kind": "email", "enabled": true,
                "email": {
                    "smtp_host": "smtp.example.com", "tls": "starttls", "username": "forge", "password": "Sm7p pw ",
                    "from": "Forge <forge@example.com>", "to": ["ops@example.com", "oncall@example.com"],
                    "subject_prefix": "[prod]",
                },
            }))
        );
        assert!(!format!("{body:?}").contains("Sm7p"), "Debug hides the password");

        // Missing SMTP pieces, a URL, a signature, SMTP flags on a webhook.
        let partial = channel_flags(&["--name", "m", "--kind", "email", "--smtp-host", "smtp.example.com"]);
        assert!(channel_body(None, &partial, r(None, None)).is_err_and(|e| e.message.contains("--from")));
        assert!(channel_body(None, &f, r(Some(URL), None)).is_err());
        let hook = channel_flags(&["--name", "h", "--kind", "webhook", "--url", "@u", "--to", "ops@example.com"]);
        assert!(channel_body(None, &hook, r(Some(URL), None)).is_err_and(|e| e.message.contains("--kind email")));

        // A user name never goes without TLS; a password needs a user name.
        let plain = channel_flags(&[
            "--name",
            "m",
            "--kind",
            "email",
            "--smtp-host",
            "h",
            "--from",
            "a@b.c",
            "--to",
            "d@e.f",
            "--tls",
            "none",
            "--smtp-user",
            "u",
        ]);
        assert!(channel_body(None, &plain, r(None, None)).is_err_and(|e| e.message.contains("without TLS")));
        let orphan = channel_flags(&[
            "--name",
            "m",
            "--kind",
            "email",
            "--smtp-host",
            "h",
            "--from",
            "a@b.c",
            "--to",
            "d@e.f",
            "--smtp-password",
            "@p",
        ]);
        let pw = Resolved { smtp_password: Some("x".into()), ..Resolved::default() };
        assert!(channel_body(None, &orphan, pw).is_err_and(|e| e.message.contains("--smtp-user")));

        // A password on the command line is refused like a URL.
        let e = secret_source(SecretFlag::SmtpPassword, "hunter2").err().map(|e| (e.code, e.message));
        let Some((code, message)) = e else { panic!("accepted") };
        assert_eq!(code, "secret_on_command_line");
        assert!(message.contains("--smtp-password") && !message.contains("hunter2"), "{message}");

        // A password keeps its spaces; only the line break goes.
        assert_eq!(read_secret(SecretFlag::SmtpPassword, "@-", || Ok(" p w \n".into())).ok().as_deref(), Some(" p w "));
    }

    #[test]
    fn an_email_channel_update_changes_only_what_was_given() {
        let cur: ChannelBody = stored(
            "channel",
            &json!({
                "id": "c1", "name": "Ops mail", "kind": "email", "enabled": true, "target": "smtp://smtp.example.com:587",
                "email": {
                    "smtp_host": "smtp.example.com", "smtp_port": 587, "tls": "starttls", "username": "forge",
                    "password_set": true, "from": "forge@example.com", "to": ["ops@example.com"],
                },
            }),
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let update = |args: &[&str], res: Resolved| {
            let mut all = vec!["channels", "update", "ops"];
            all.extend_from_slice(args);
            let AlertsCmd::Channels(ChannelsCmd::Update { flags, .. }) = parse(&all) else { panic!("update") };
            channel_body(Some(cur.clone()), &flags, res)
        };
        let body = update(&["--to", "a@example.com", "--to", "b@example.com"], Resolved::default())
            .unwrap_or_else(|e| panic!("{e}"));
        let e = body.email.unwrap_or_else(|| panic!("email"));
        assert_eq!((e.to.len(), e.username.as_deref(), e.password.as_deref()), (2, Some("forge"), None));
        assert_eq!(e.smtp_port, Some(587));

        // A new TLS mode takes its usual port unless one is given.
        let e = update(&["--tls", "tls"], Resolved::default()).unwrap_or_else(|e| panic!("{e}")).email;
        assert_eq!(e.map(|e| (e.tls, e.smtp_port)), Some((TlsMode::Tls, None)));
        let e = update(&["--no-smtp-user"], Resolved::default()).unwrap_or_else(|e| panic!("{e}")).email;
        assert_eq!(e.and_then(|e| e.username), None);
        let e = update(&["--subject-prefix", ""], Resolved::default()).unwrap_or_else(|e| panic!("{e}")).email;
        assert_eq!(e.and_then(|e| e.subject_prefix), None);

        // Away from email only with a URL.
        assert!(update(&["--kind", "webhook"], Resolved::default()).is_err());
        let to_hook =
            update(&["--kind", "webhook", "--url", "@u"], r(Some(URL), None)).unwrap_or_else(|e| panic!("{e}"));
        assert!(to_hook.email.is_none() && to_hook.url.is_some());
        assert!(refused(&["channels", "update", "ops", "--no-smtp-user", "--smtp-password", "@p"]));
    }

    #[test]
    fn repeat_takes_durations_seconds_and_never() {
        assert_eq!(parse_repeat("never").ok(), Some(None));
        assert_eq!(parse_repeat("300").ok(), Some(Some(300)));
        assert_eq!(parse_repeat("5m").ok(), Some(Some(300)));
        assert_eq!(parse_repeat("4h").ok(), Some(Some(14_400)));
        assert_eq!(parse_repeat("1h30m").ok(), Some(Some(5400)));
        assert_eq!(parse_repeat("7d").ok(), Some(Some(604_800)));
        assert_eq!(parse_repeat("1w").ok(), Some(Some(604_800)));

        for bad in ["", "299", "4m", "8d", "604801", "soon", "1.5h", "-300", "99999999999999999999"] {
            assert!(parse_repeat(bad).is_err_and(|e| e.exit == Exit::Usage && e.message.contains("--repeat")), "{bad}");
        }
    }

    #[test]
    fn policy_commands_parse_their_flags() {
        let f = policy_flags(&[
            "--name",
            "Critical to ops",
            "--min-severity",
            "critical",
            "--event",
            "fired",
            "--event",
            "resolved",
            "--channel",
            "ops",
            "--channel",
            "chat",
            "--rule",
            "r1",
            "--scope",
            "servers",
            "--scope-id",
            "web-01",
            "--repeat",
            "1h",
            "--disabled",
        ]);
        assert_eq!((f.name.as_deref(), f.min_severity), (Some("Critical to ops"), Some(Severity::Critical)));
        assert_eq!(f.events, [Event::Fired, Event::Resolved]);
        assert_eq!((f.channels.len(), f.rules.len(), f.scope, f.scope_ids.len()), (2, 1, Some(ScopeKind::Servers), 1));
        assert_eq!((f.repeat.as_deref(), f.disabled), (Some("1h"), true));
        assert!(matches!(
            parse(&["policies", "update", "p1", "--all-rules", "--scope", "all", "--yes"]),
            AlertsCmd::Policies(PoliciesCmd::Update { yes: true, .. })
        ));
        assert!(matches!(parse(&["policies", "get", "p1"]), AlertsCmd::Policies(PoliciesCmd::Show { .. })));
        assert!(refused(&["policies", "create", "--min-severity", "fatal"]));
        assert!(refused(&["policies", "create", "--event", "ended"]));
        assert!(refused(&["policies", "create", "--scope-id", "web-01"]), "--scope-id needs --scope");
        assert!(refused(&["policies", "update", "p1", "--rule", "r1", "--all-rules"]));
        assert!(refused(&["policies", "update", "p1", "--enabled", "--disabled"]));
    }

    #[test]
    fn a_new_policy_is_built_from_the_flags() {
        let f = policy_flags(&[
            "--name",
            "Critical to ops",
            "--min-severity",
            "critical",
            "--event",
            "fired",
            "--event",
            "fired",
            "--channel",
            "ops",
            "--scope",
            "servers",
            "--scope-id",
            "web-01",
            "--repeat",
            "30m",
        ]);
        let body = policy_body(None, &f, &ids(&["c1", "c1"]), &ids(&["s1"])).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            serde_json::to_value(&body).ok(),
            Some(json!({
                "name": "Critical to ops", "enabled": true, "min_severity": "critical", "rules": [],
                "scope": { "kind": "servers", "ids": ["s1"] }, "events": ["fired"],
                "repeat_interval_secs": 1800, "channels": ["c1"],
            }))
        );

        // The smallest policy: no scope and no reminder fields at all.
        let f = policy_flags(&["--name", "All", "--min-severity", "info", "--event", "resolved", "--channel", "ops"]);
        let body = policy_body(None, &f, &ids(&["c1"]), &[]).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            serde_json::to_value(&body).ok(),
            Some(json!({
                "name": "All", "enabled": true, "min_severity": "info", "rules": [],
                "events": ["resolved"], "channels": ["c1"],
            }))
        );

        // What a new policy needs, and what cannot be combined.
        for missing in [
            &["--min-severity", "info", "--event", "fired", "--channel", "c"][..],
            &["--name", "x", "--event", "fired", "--channel", "c"],
            &["--name", "x", "--min-severity", "info", "--channel", "c"],
            &["--name", "x", "--min-severity", "info", "--event", "fired"],
        ] {
            let f = policy_flags(missing);
            assert!(policy_body(None, &f, &f.channels, &[]).is_err_and(|e| e.message.contains("a new policy needs")));
        }

        let f = policy_flags(&[
            "--name",
            "x",
            "--min-severity",
            "info",
            "--event",
            "resolved",
            "--channel",
            "c",
            "--repeat",
            "1h",
        ]);
        assert!(policy_body(None, &f, &ids(&["c1"]), &[]).is_err_and(|e| e.message.contains("--event fired")));
        let f = policy_flags(&[
            "--name",
            "x",
            "--min-severity",
            "info",
            "--event",
            "fired",
            "--channel",
            "c",
            "--repeat",
            "1m",
        ]);
        assert!(policy_body(None, &f, &ids(&["c1"]), &[]).is_err_and(|e| e.message.contains("--repeat")));
        let f = policy_flags(&[
            "--name",
            "x",
            "--min-severity",
            "info",
            "--event",
            "fired",
            "--channel",
            "c",
            "--scope",
            "all",
            "--scope-id",
            "s",
        ]);
        assert!(policy_body(None, &f, &ids(&["c1"]), &ids(&["s"])).is_err_and(|e| e.message.contains("--scope all")));
    }

    #[test]
    fn a_policy_update_changes_only_what_was_given() {
        let update = |args: &[&str], channels: &[&str], scope: &[&str]| {
            let mut all = vec!["policies", "update", "p1"];
            all.extend_from_slice(args);
            let AlertsCmd::Policies(PoliciesCmd::Update { flags, .. }) = parse(&all) else { panic!("update") };
            policy_body(Some(stored_policy()), &flags, &ids(channels), &ids(scope))
        };
        let ok = |args: &[&str], channels: &[&str], scope: &[&str]| {
            update(args, channels, scope).unwrap_or_else(|e| panic!("{e}"))
        };
        let b = ok(&["--disabled"], &[], &[]);
        assert_eq!(b, PolicyBody { enabled: false, ..stored_policy() }, "everything else stays");
        let b = ok(&["--min-severity", "warning", "--name", "Warnings"], &[], &[]);
        assert_eq!((b.min_severity, b.name.as_str(), b.channels.len()), (Severity::Warning, "Warnings", 2));

        // Lists are replaced as a whole.
        let b = ok(&["--channel", "chat"], &["c9"], &[]);
        assert_eq!((b.channels, b.rules, b.events.len()), (ids(&["c9"]), ids(&["r1"]), 2));
        let b = ok(&["--rule", "r7", "--rule", "r8"], &[], &[]);
        assert_eq!(b.rules, ids(&["r7", "r8"]));
        assert!(ok(&["--all-rules"], &[], &[]).rules.is_empty());

        // Scope: another kind, all of a kind, or none.
        assert_eq!(
            ok(&["--scope", "services", "--scope-id", "a1"], &[], &["a1"]).scope,
            Some(Scope::Services { ids: ids(&["a1"]) })
        );
        assert_eq!(ok(&["--scope", "servers"], &[], &[]).scope, Some(Scope::Servers { ids: Vec::new() }));
        assert_eq!(ok(&["--scope", "all"], &[], &[]).scope, None);

        // Reminders: changed, turned off, and tied to `fired`.
        assert_eq!(ok(&["--repeat", "600"], &[], &[]).repeat_interval_secs, Some(600));
        assert_eq!(ok(&["--repeat", "never"], &[], &[]).repeat_interval_secs, None);
        assert!(update(&["--event", "resolved"], &[], &[]).is_err_and(|e| e.message.contains("--repeat never")));
        let b = ok(&["--event", "resolved", "--repeat", "never"], &[], &[]);
        assert_eq!((b.events, b.repeat_interval_secs), (vec![Event::Resolved], None));
        assert!(update(&[], &[], &[]).is_err_and(|e| e.exit == Exit::Usage));
    }

    #[test]
    fn a_stored_policy_reads_back_as_a_body() {
        let p = json!({
            "id": "p1", "workspace_id": "w1", "name": "Critical to ops", "enabled": true,
            "min_severity": "critical", "rules": ["r1"], "scope": { "kind": "servers", "ids": ["s1"] },
            "events": ["fired", "resolved"], "repeat_interval_secs": 3600, "channels": ["c1", "c2"],
            "created_by": { "type": "user", "id": "u1" }, "created_at": 1, "updated_at": 2,
        });
        assert_eq!(stored::<PolicyBody>("policy", &p).ok(), Some(stored_policy()));

        // `scope` and `repeat_interval_secs` are left out when unset.
        let bare =
            json!({ "name": "All", "enabled": false, "min_severity": "info", "events": ["fired"], "channels": ["c1"] });
        let b = stored::<PolicyBody>("policy", &bare).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!((b.scope, b.repeat_interval_secs, b.rules.len(), b.enabled), (None, None, 0, false));
    }

    #[test]
    fn delivery_commands_parse_their_flags() {
        match parse(&["deliveries", "list", "--alarm", "a1", "--channel", "ops", "--status", "failed", "--limit", "20"])
        {
            AlertsCmd::Deliveries(DeliveriesCmd::List { alarm, channel, status, workspace, paging }) => {
                assert_eq!(
                    (alarm.as_deref(), channel.as_deref(), status),
                    (Some("a1"), Some("ops"), Some(Status::Failed))
                );
                assert_eq!((workspace, paging.p.limit), (None, 20));
            }
            other => panic!("{other:?}"),
        }

        assert!(matches!(parse(&["deliveries", "show", "d1"]), AlertsCmd::Deliveries(DeliveriesCmd::Show { .. })));
        assert!(refused(&["deliveries", "list", "--status", "lost"]));

        for s in [Status::Pending, Status::Sending, Status::Sent, Status::Failed, Status::Cancelled, Status::Suppressed]
        {
            assert_eq!(s.to_possible_value().map(|v| v.get_name().to_owned()).as_deref(), Some(s.as_str()));
        }

        assert_eq!(with_workspace(CHANNELS, Some("w 1")), "/api/v1/notification-channels?workspace=w+1");
    }

    #[test]
    fn a_test_that_was_not_sent_fails_the_command() {
        let sent = json!({ "id": "d1", "status": "sent", "last_http_status": 204 });
        assert!(test_outcome("ops", &sent).is_ok());
        let failed = json!({ "id": "d2", "status": "failed", "last_http_status": 500, "last_error": "http_status" });
        let e = test_outcome("ops", &failed).err().map(|e| (e.exit, e.code, e.message));
        let Some((exit, code, message)) = e else { panic!("a failed test passed") };
        assert_eq!((exit, code.as_str()), (Exit::JobFailed, "delivery_failed"));
        assert!(message.contains("status failed, HTTP 500, error http_status"), "{message}");
        assert!(message.contains("deliveries show d2"), "{message}");
        let blocked = json!({ "id": "d3", "status": "failed", "last_error": "blocked_target" });
        assert!(
            test_outcome("ops", &blocked).is_err_and(|e| e.message.contains("status failed, error blocked_target"))
        );
    }

    #[test]
    fn views_name_channels_and_list_attempts() {
        let channels = json!([{ "id": "c1", "name": "Ops webhook" }]);
        let row = policy_row(&json!({ "id": "p1", "channels": ["c1", "gone"] }), &channels);
        assert_eq!(row["channel_names"], "Ops webhook, gone");
        let view = json!({
            "id": "d1", "status": "failed", "channel_name": "Ops webhook",
            "attempts": [
                { "delivery_id": "d1", "number": 1, "at": 0, "ok": false, "http_status": 503, "error": "http_status", "duration_ms": 12 },
                { "delivery_id": "d1", "number": 2, "at": 0, "ok": false, "error": "timeout", "duration_ms": 15000 },
            ],
        });
        let text = delivery_text(&view);
        assert!(text.contains("channel_name  Ops webhook"), "{text}");
        assert!(text.contains("attempts:\n  NUMBER  AT  OK  HTTP  ERROR        DURATION (MS)\n"), "{text}");
        assert!(text.contains("  1       0   no  503   http_status  12\n"), "{text}");
        assert!(text.contains("  2       0   no  -     timeout      15000\n"), "{text}");

        // No attempts yet: no empty table.
        assert!(!delivery_text(&json!({ "id": "d2", "status": "pending", "attempts": [] })).contains("attempts"));
    }
}
