//! `forge alerts silences …`: silences and maintenance windows (#11, D-112).
//!
//! A silence holds back the notifications of the alarms it matches for a
//! bounded time; one that starts later is a maintenance window. What it
//! matches is fixed at creation: afterwards it can only be extended or
//! ended early.

use clap::{Args, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Global;
use crate::admincmd::{enc, looks_like_id, node_id};
use crate::api::Api;
use crate::cmds::confirm;
use crate::error::{CliError, Exit, Result};
use crate::notifycmd::Severity;
use crate::out::{self, Col};
use crate::tunnelcmd::parse_lifetime;

const SILENCES: &str = "/api/v1/silences";

/// The most silences the server returns for one request.
const SERVER_MAX: usize = 500;

/// Where a silence stands, by the server's clock.
#[derive(ValueEnum, Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// It starts later (a maintenance window that has not begun).
    Scheduled,
    /// It holds notifications back now.
    Active,
    /// Its end has passed, or it was ended early.
    Expired,
}

impl State {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Scheduled => "scheduled",
            Self::Active => "active",
            Self::Expired => "expired",
        }
    }
}

// ------------------------------------------------------------------ commands

/// What a new silence covers: exactly one kind.
#[derive(Args, Debug, Default, Clone)]
#[group(id = "scope", required = true, multiple = false)]
pub struct ScopeFlags {
    /// Every alarm of the workspace.
    #[arg(long)]
    pub workspace_wide: bool,
    /// Alarms on this cluster's nodes and the services on them (cluster id).
    #[arg(long, value_name = "CLUSTER")]
    pub cluster: Option<String>,
    /// Alarms on this environment's services and nodes (environment id).
    #[arg(long, value_name = "ENVIRONMENT")]
    pub environment: Option<String>,
    /// Alarms on this node and the services on it (id, host name or
    /// name); repeat for up to 100.
    #[arg(long = "node", value_name = "NODE")]
    pub nodes: Vec<String>,
    /// Alarms on this service (id); repeat for up to 100.
    #[arg(long = "service", value_name = "SERVICE")]
    pub apps: Vec<String>,
}

/// When a new silence ends: exactly one of the two.
#[derive(Args, Debug, Default, Clone)]
#[group(id = "end", required = true, multiple = false)]
pub struct EndFlags {
    /// Length from the start: 1m to 30d (`30m`, `2h`, `1d`, `1h30m`).
    #[arg(long, value_name = "DURATION")]
    pub duration: Option<String>,
    /// End time: RFC 3339 (`2026-11-20T06:00:00Z`) or epoch milliseconds.
    #[arg(long, value_name = "TIME")]
    pub ends_at: Option<String>,
}

#[derive(Args, Debug, Clone)]
pub struct CreateFlags {
    #[command(flatten)]
    pub scope: ScopeFlags,
    /// Only alarms of this rule (rule id); repeat for up to 100. Default: every rule.
    #[arg(long = "rule", value_name = "RULE")]
    pub rules: Vec<String>,
    /// Only alarms of this severity and worse. Default: no lower bound.
    #[arg(long, value_enum, value_name = "SEVERITY")]
    pub min_severity: Option<Severity>,
    /// Only alarms of this severity and milder; a worse alarm still
    /// notifies. Default: no upper bound.
    #[arg(long, value_enum, value_name = "SEVERITY")]
    pub max_severity: Option<Severity>,
    /// Start: RFC 3339 (`2026-11-20T02:00:00Z`) or epoch milliseconds, up to
    /// 365 days ahead. Default: now. A later start makes a maintenance window.
    #[arg(long, value_name = "TIME")]
    pub starts_at: Option<String>,
    #[command(flatten)]
    pub end: EndFlags,
    /// Why alarms are quiet, 1 to 500 characters (required).
    #[arg(long)]
    pub reason: String,
    /// The workspace (id). Default: this deployment's workspace.
    #[arg(long)]
    pub workspace: Option<String>,
}

/// How far to move a silence's end: exactly one of the two.
#[derive(Args, Debug, Default, Clone)]
#[group(id = "extend", required = true, multiple = false)]
pub struct ExtendFlags {
    /// Time to add to the current end (`30m`, `2h`, `1d`).
    #[arg(long, value_name = "DURATION")]
    pub by: Option<String>,
    /// The new end, later than the current one: RFC 3339 or epoch milliseconds.
    #[arg(long, value_name = "TIME")]
    pub until: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum SilencesCmd {
    /// Silences and maintenance windows, latest start first.
    List {
        /// Only silences in this state.
        #[arg(long, value_enum)]
        state: Option<State>,
        /// Only this workspace's silences (id).
        #[arg(long)]
        workspace: Option<String>,
        #[command(flatten)]
        paging: out::Paging,
    },
    /// One silence.
    #[command(alias = "get")]
    Show {
        /// The silence (id).
        id: String,
    },
    /// Create a silence, or a maintenance window with a later --starts-at.
    Create {
        #[command(flatten)]
        flags: CreateFlags,
    },
    /// Move a silence's end later.
    Extend {
        /// The silence (id).
        id: String,
        #[command(flatten)]
        flags: ExtendFlags,
    },
    /// End a silence now; alarms it covered that still fire notify again.
    End {
        /// The silence (id).
        id: String,
        /// Do not ask first (required with --non-interactive).
        #[arg(long)]
        yes: bool,
    },
}

// ------------------------------------------------------------------ bodies

/// Which alarms a silence covers, as the API spells it.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Scope {
    Workspace,
    Cluster { id: String },
    Environment { id: String },
    Servers { ids: Vec<String> },
    Services { ids: Vec<String> },
}

impl Scope {
    fn text(&self) -> String {
        match self {
            Self::Workspace => "workspace".to_owned(),
            Self::Cluster { id } => format!("cluster {id}"),
            Self::Environment { id } => format!("environment {id}"),
            Self::Servers { ids } => format!("nodes {}", ids.join(", ")),
            Self::Services { ids } => format!("apps {}", ids.join(", ")),
        }
    }
}

/// The body of `POST /silences`.
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct SilenceBody {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    pub scope: Scope,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_severity: Option<Severity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_severity: Option<Severity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub starts_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ends_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_secs: Option<u64>,
    pub reason: String,
}

/// The body of `POST /silences/{id}/extend`.
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct ExtendBody {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ends_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extend_secs: Option<u64>,
}

/// A silence as the server returns it (the fields the table needs).
#[derive(Deserialize, Debug, Clone)]
pub struct SilenceView {
    pub id: String,
    pub state: State,
    pub scope: Scope,
    #[serde(default)]
    pub rules: Vec<String>,
    #[serde(default)]
    pub min_severity: Option<Severity>,
    #[serde(default)]
    pub max_severity: Option<Severity>,
    pub starts_at: i64,
    pub ends_at: i64,
    pub reason: String,
}

/// One table row.
#[derive(Serialize, Debug)]
struct SilenceRow {
    id: String,
    state: &'static str,
    scope: String,
    rules: String,
    severity: String,
    starts_at: i64,
    ends_at: i64,
    reason: String,
}

const SILENCE_COLS: &[Col] = &[
    ("ID", "/id"),
    ("STATE", "/state"),
    ("SCOPE", "/scope"),
    ("RULES", "/rules"),
    ("SEVERITY", "/severity"),
    ("STARTS", "/starts_at"),
    ("ENDS", "/ends_at"),
    ("REASON", "/reason"),
];

fn sev(s: Severity) -> &'static str {
    match s {
        Severity::Info => "info",
        Severity::Warning => "warning",
        Severity::Major => "major",
        Severity::Critical => "critical",
    }
}

fn severity_text(min: Option<Severity>, max: Option<Severity>) -> String {
    match (min, max) {
        (None, None) => "any".to_owned(),
        (Some(a), None) => format!("{} and worse", sev(a)),
        (None, Some(b)) => format!("{} and milder", sev(b)),
        (Some(a), Some(b)) if a == b => sev(a).to_owned(),
        (Some(a), Some(b)) => format!("{} to {}", sev(a), sev(b)),
    }
}

impl From<SilenceView> for SilenceRow {
    fn from(v: SilenceView) -> Self {
        Self {
            rules: if v.rules.is_empty() { "all".to_owned() } else { v.rules.join(", ") },
            severity: severity_text(v.min_severity, v.max_severity),
            scope: v.scope.text(),
            state: v.state.as_str(),
            id: v.id,
            starts_at: v.starts_at,
            ends_at: v.ends_at,
            reason: v.reason,
        }
    }
}

fn row_of(v: &Value) -> Result<SilenceRow> {
    SilenceView::deserialize(v).map(SilenceRow::from).map_err(|e| {
        CliError::new(Exit::General, format!("a silence has a value this CLI does not know ({e}); update the CLI"))
            .with_code("unknown_value")
    })
}

// ------------------------------------------------------------------ parsing

/// A duration flag (`30m`, `2h`, `1d`, `1h30m`) in seconds.
pub fn parse_duration(flag: &str, s: &str) -> Result<u64> {
    let bad = || CliError::usage(format!("--{flag} {s}: use a duration like 30m, 2h, 1d or 1h30m"));
    match parse_lifetime(s).map_err(|_| bad())? {
        Some(minutes) => minutes.checked_mul(60).ok_or_else(bad),
        None => Err(bad()),
    }
}

/// Two ASCII digits or more as a number.
fn num(s: &str) -> Option<i64> {
    (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())).then(|| s.parse().ok()).flatten()
}

/// RFC 3339 (`2026-11-20T02:00:00Z`, `2026-11-20T04:00+02:00`, seconds and
/// fractions optional) as epoch milliseconds.
fn rfc3339(s: &str) -> Option<i64> {
    let s = s.trim();
    let (date, rest) = s.split_at_checked(10)?;
    let rest = rest.strip_prefix(['T', 't', ' '])?;
    let mut d = date.split('-');
    let (y, mo, da) = (num(d.next()?)?, num(d.next()?)?, num(d.next()?)?);
    // The offset: `Z`, or `+HH:MM` / `-HH:MM` at the end.
    let (clock, offset_secs) = if let Some(c) = rest.strip_suffix(['Z', 'z']) {
        (c, 0)
    } else {
        let at = rest.rfind(['+', '-'])?;
        let (c, off) = rest.split_at(at);
        let sign = if off.starts_with('-') { -1 } else { 1 };
        let (h, m) = off.get(1..)?.split_once(':')?;
        (c, sign * (num(h)? * 3600 + num(m)? * 60))
    };
    let (hms, frac) = clock.split_once('.').unwrap_or((clock, ""));
    let mut t = hms.split(':');
    let (h, mi) = (num(t.next()?)?, num(t.next()?)?);
    let sec = t.next().map_or(Some(0), num)?;
    if t.next().is_some() || !frac.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let ms_part: i64 = format!("{frac:0<3}").get(..3).and_then(num).unwrap_or(0);
    let month = time::Month::try_from(u8::try_from(mo).ok()?).ok()?;
    let date = time::Date::from_calendar_date(i32::try_from(y).ok()?, month, u8::try_from(da).ok()?).ok()?;
    let clock = time::Time::from_hms(u8::try_from(h).ok()?, u8::try_from(mi).ok()?, u8::try_from(sec).ok()?).ok()?;
    let offset = time::UtcOffset::from_whole_seconds(i32::try_from(offset_secs).ok()?).ok()?;
    let at = time::PrimitiveDateTime::new(date, clock).assume_offset(offset);
    at.unix_timestamp().checked_mul(1000)?.checked_add(ms_part)
}

/// A time flag: RFC 3339 or epoch milliseconds.
pub fn parse_time(flag: &str, s: &str) -> Result<i64> {
    num(s.trim()).or_else(|| rfc3339(s)).ok_or_else(|| {
        CliError::usage(format!("--{flag} {s}: use RFC 3339 (such as 2026-11-20T02:00:00Z) or epoch milliseconds"))
    })
}

/// The body of a new silence. `nodes` are the ids `--node` resolved to.
pub fn silence_body(f: &CreateFlags, nodes: Vec<String>) -> Result<SilenceBody> {
    let s = &f.scope;
    let scope = if s.workspace_wide {
        Scope::Workspace
    } else if let Some(id) = &s.cluster {
        Scope::Cluster { id: id.clone() }
    } else if let Some(id) = &s.environment {
        Scope::Environment { id: id.clone() }
    } else if !s.nodes.is_empty() {
        Scope::Servers { ids: dedup(nodes) }
    } else if !s.apps.is_empty() {
        Scope::Services { ids: dedup(s.apps.clone()) }
    } else {
        return Err(CliError::usage("pass one scope: --workspace-wide, --cluster, --environment, --node or --service"));
    };
    if let (Some(a), Some(b)) = (f.min_severity, f.max_severity)
        && rank(a) > rank(b)
    {
        return Err(CliError::usage("--min-severity is worse than --max-severity: no alarm would match"));
    }
    let reason = f.reason.trim();
    if reason.is_empty() {
        return Err(CliError::usage("--reason is empty: say why alarms are quiet"));
    }
    Ok(SilenceBody {
        workspace_id: f.workspace.clone(),
        scope,
        rules: dedup(f.rules.clone()),
        min_severity: f.min_severity,
        max_severity: f.max_severity,
        starts_at: f.starts_at.as_deref().map(|t| parse_time("starts-at", t)).transpose()?,
        ends_at: f.end.ends_at.as_deref().map(|t| parse_time("ends-at", t)).transpose()?,
        duration_secs: f.end.duration.as_deref().map(|d| parse_duration("duration", d)).transpose()?,
        reason: reason.to_owned(),
    })
}

/// The body of an extension.
pub fn extend_body(f: &ExtendFlags) -> Result<ExtendBody> {
    Ok(ExtendBody {
        ends_at: f.until.as_deref().map(|t| parse_time("until", t)).transpose()?,
        extend_secs: f.by.as_deref().map(|d| parse_duration("by", d)).transpose()?,
    })
}

const fn rank(s: Severity) -> u8 {
    match s {
        Severity::Info => 0,
        Severity::Warning => 1,
        Severity::Major => 2,
        Severity::Critical => 3,
    }
}

fn dedup(items: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(items.len());
    for i in items {
        if !out.contains(&i) {
            out.push(i);
        }
    }
    out
}

// ------------------------------------------------------------------ run

fn show_one(g: &Global, v: &Value) -> Result<()> {
    if g.output.is_machine() {
        out::result(g.output, v, None);
    } else {
        // Fail early on a shape this CLI does not know, then show every field.
        row_of(v)?;
        out::result(g.output, v, None);
    }
    Ok(())
}

pub async fn run(g: &Global, server: &str, api: &mut Api, cmd: SilencesCmd) -> Result<()> {
    match cmd {
        SilencesCmd::List { state, workspace, paging } => {
            let page: out::Page = paging.into();
            let fetch = page.fetch().unwrap_or(SERVER_MAX).min(SERVER_MAX);
            let mut q = format!("{SILENCES}?limit={fetch}");
            if let Some(s) = state {
                q.push_str(&format!("&state={}", s.as_str()));
            }
            if let Some(w) = workspace {
                q.push_str(&format!("&workspace={}", enc(&w)));
            }
            let raw = crate::cmds::rows_of(api.get(&q).await?);
            let total = if raw.len() >= fetch { out::Total::AtLeast } else { out::Total::Rows };
            let rows = if g.output.is_machine() {
                raw
            } else {
                raw.iter().map(|v| row_of(v).and_then(|r| Ok(serde_json::to_value(r)?))).collect::<Result<_>>()?
            };
            out::list(g.output, rows, Some(SILENCE_COLS), page, total);
            Ok(())
        }
        SilencesCmd::Show { id } => {
            let v = api.get(&format!("{SILENCES}/{}", enc(&id))).await?;
            show_one(g, &v)
        }
        SilencesCmd::Create { flags } => {
            // Checked before the server is asked for anything.
            silence_body(&flags, flags.scope.nodes.clone())?;
            let mut nodes = Vec::with_capacity(flags.scope.nodes.len());
            for n in &flags.scope.nodes {
                nodes.push(if looks_like_id(n) { n.clone() } else { node_id(api, n).await? });
            }
            let body = silence_body(&flags, nodes)?;
            let v = api.post(SILENCES, &serde_json::to_value(&body)?).await?;
            show_one(g, &v)
        }
        SilencesCmd::Extend { id, flags } => {
            let body = extend_body(&flags)?;
            let v = api.post(&format!("{SILENCES}/{}/extend", enc(&id)), &serde_json::to_value(&body)?).await?;
            show_one(g, &v)
        }
        SilencesCmd::End { id, yes } => {
            confirm(
                g,
                yes,
                &format!("End silence {id} on {server} now? Alarms it covers that still fire will notify."),
            )?;
            let v = api.post(&format!("{SILENCES}/{}/end", enc(&id)), &Value::Object(serde_json::Map::new())).await?;
            show_one(g, &v)
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;
    use crate::alertscmd::AlertsCmd;
    use crate::{Cli, Cmd};

    fn parse(args: &[&str]) -> std::result::Result<SilencesCmd, clap::Error> {
        let mut all = vec!["forge", "alerts", "silences"];
        all.extend_from_slice(args);
        Cli::try_parse_from(all).map(|c| match c.cmd {
            Cmd::Alerts(AlertsCmd::Silences(s)) => s,
            other => panic!("{other:?}"),
        })
    }

    fn create(args: &[&str]) -> CreateFlags {
        let mut all = vec!["create"];
        all.extend_from_slice(args);
        match parse(&all) {
            Ok(SilencesCmd::Create { flags }) => flags,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn times_parse_as_rfc3339_or_epoch_ms() {
        assert_eq!(parse_time("t", "1763600000000").ok(), Some(1_763_600_000_000));
        assert_eq!(parse_time("t", "2026-11-20T02:00:00Z").ok(), Some(1_795_140_000_000));
        assert_eq!(parse_time("t", "2026-11-20T04:00+02:00").ok(), Some(1_795_140_000_000));
        assert_eq!(parse_time("t", "2026-11-20T02:00:00.250Z").ok(), Some(1_795_140_000_250));
        for bad in ["tomorrow", "2026-13-01T00:00Z", "2026-11-20T02:00", "2026-11-20", ""] {
            assert!(parse_time("t", bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn durations_are_seconds() {
        assert_eq!(parse_duration("by", "2h").ok(), Some(7200));
        assert_eq!(parse_duration("by", "1h30m").ok(), Some(5400));
        assert!(parse_duration("by", "never").is_err());
        assert!(parse_duration("by", "5x").is_err());
    }

    #[test]
    fn create_needs_one_scope_one_end_and_a_reason() {
        assert!(parse(&["create", "--duration", "1h", "--reason", "x"]).is_err());
        assert!(parse(&["create", "--workspace-wide", "--reason", "x"]).is_err());
        assert!(parse(&["create", "--workspace-wide", "--duration", "1h"]).is_err());
        assert!(parse(&["create", "--workspace-wide", "--node", "a", "--duration", "1h", "--reason", "x"]).is_err());
        assert!(parse(&["create", "--service", "a", "--duration", "1h", "--ends-at", "1", "--reason", "x"]).is_err());
    }

    #[test]
    fn a_body_is_built_from_the_flags() {
        let f = create(&[
            "--node",
            "n1",
            "--node",
            "n2",
            "--node",
            "n1",
            "--rule",
            "r1",
            "--min-severity",
            "major",
            "--starts-at",
            "2026-11-20T02:00:00Z",
            "--duration",
            "2h",
            "--reason",
            " Kernel patching ",
        ]);
        let b = silence_body(&f, vec!["id1".into(), "id2".into(), "id1".into()]).unwrap_or_else(|e| panic!("{e}"));
        let v = serde_json::to_value(&b).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            v,
            serde_json::json!({
                "scope": {"kind": "servers", "ids": ["id1", "id2"]},
                "rules": ["r1"],
                "min_severity": "major",
                "starts_at": 1_795_140_000_000_i64,
                "duration_secs": 7200,
                "reason": "Kernel patching",
            })
        );
        let w = silence_body(&create(&["--workspace-wide", "--ends-at", "5", "--reason", "x"]), vec![])
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            serde_json::to_value(&w).unwrap_or_else(|e| panic!("{e}"))["scope"],
            serde_json::json!({"kind": "workspace"})
        );
        assert_eq!(w.ends_at, Some(5));
        let bad = create(&[
            "--cluster",
            "c",
            "--min-severity",
            "critical",
            "--max-severity",
            "info",
            "--duration",
            "1h",
            "--reason",
            "x",
        ]);
        assert!(silence_body(&bad, vec![]).is_err());
        assert!(silence_body(&create(&["--environment", "n", "--duration", "1h", "--reason", " "]), vec![]).is_err());
    }

    #[test]
    fn extend_takes_by_or_until() {
        let f = |args: &[&str]| {
            let mut all = vec!["extend", "s1"];
            all.extend_from_slice(args);
            parse(&all)
        };
        assert!(f(&[]).is_err());
        assert!(f(&["--by", "1h", "--until", "5"]).is_err());
        let Ok(SilencesCmd::Extend { flags, .. }) = f(&["--by", "30m"]) else { panic!() };
        assert_eq!(
            serde_json::to_value(extend_body(&flags).unwrap_or_else(|e| panic!("{e}")))
                .unwrap_or_else(|e| panic!("{e}")),
            serde_json::json!({"extend_secs": 1800})
        );
        let Ok(SilencesCmd::Extend { flags, .. }) = f(&["--until", "2026-11-20T02:00:00Z"]) else { panic!() };
        assert_eq!(extend_body(&flags).unwrap_or_else(|e| panic!("{e}")).ends_at, Some(1_795_140_000_000));
    }

    #[test]
    fn list_and_end_parse() {
        assert!(matches!(
            parse(&["list", "--state", "active", "--limit", "10"]),
            Ok(SilencesCmd::List { state: Some(State::Active), .. })
        ));
        assert!(parse(&["list", "--state", "paused"]).is_err());
        assert!(matches!(parse(&["end", "s1", "--yes"]), Ok(SilencesCmd::End { yes: true, .. })));
    }

    #[test]
    fn rows_describe_scope_and_severity() {
        let v = serde_json::json!({
            "id": "s1", "workspace_id": "w", "state": "scheduled",
            "scope": {"kind": "services", "ids": ["a1", "a2"]},
            "rules": [], "max_severity": "warning",
            "starts_at": 1, "ends_at": 2, "reason": "r",
            "created_by": {"type": "user", "id": "u"}, "created_at": 1, "updated_at": 1,
        });
        let r = row_of(&v).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!((r.state, r.scope.as_str(), r.rules.as_str()), ("scheduled", "apps a1, a2", "all"));
        assert_eq!(r.severity, "warning and milder");
        assert_eq!(severity_text(Some(Severity::Major), Some(Severity::Critical)), "major to critical");
    }
}
