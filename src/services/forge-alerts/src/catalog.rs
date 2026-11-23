//! What alert rules can watch, and ready-made rules (D-118).
//!
//! [`subjects`] lists every subject Forge produces: its value kind, states
//! or unit, the scopes it applies to and where it comes from.
//! [`check_subject`] validates a rule's subject against that list, so a typo
//! is refused when the rule is saved instead of never firing.
//! [`templates`] is the one definition of the rule templates the API, the
//! CLI and the console offer.

use std::collections::{BTreeMap, BTreeSet};

use forge_core::alerts::{RuleScope, ScopeKind, Severity};
use forge_core::observation::{KNOWN_SUBJECT_PREFIXES, ObservationFilter, ObservationQuery};
use forge_core::ports::{CertificateRepository, InstallationRepository, ResourceResolver};
use forge_core::{DomainError, DomainResult, EntityKind, EntityRef, WorkspaceId};
use forge_rules::{Cmp, Condition, Mode, Recovery, RuleSpec};
use serde::{Deserialize, Serialize};

/// The node agent's connection status (control plane).
pub const NODE_AGENT_STATUS: &str = "node.agent.status";
/// Days until a node's agent certificate expires (control plane).
pub const NODE_CERT_EXPIRES: &str = "node.cert.expires_in_days";
/// A service instance's state as its node reports it.
pub const SERVICE_INSTANCE_STATE: &str = "app.instance.state";
/// A service instance's deployment status.
pub const APP_DEPLOY_STATUS: &str = "app.deploy.status";
/// A package installation's service state.
pub const PACKAGE_STATE: &str = "package.state";
/// A package installation's lifecycle status.
pub const PACKAGE_STATUS: &str = "package.status";
/// Days until an uploaded ingress certificate expires.
pub const TLS_CERT_EXPIRES: &str = "tls.cert.expires_in_days";

/// Subjects the control plane computes itself on every evaluation pass.
pub const CONTROL_PLANE_SUBJECTS: &[&str] = &[
    NODE_AGENT_STATUS,
    NODE_CERT_EXPIRES,
    SERVICE_INSTANCE_STATE,
    APP_DEPLOY_STATUS,
    PACKAGE_STATE,
    PACKAGE_STATUS,
    TLS_CERT_EXPIRES,
];

/// Health purposes a probe reports (`health.<purpose>.<check>`).
pub const HEALTH_PURPOSES: &[&str] = &["readiness", "liveness", "startup"];

/// Most distinct `health.*` subjects [`seen_health_subjects`] returns.
pub const MAX_SEEN_SUBJECTS: usize = 200;
/// Observation rows [`seen_health_subjects`] reads.
const SEEN_SCAN_ROWS: u32 = 5_000;

/// Whether a subject's values are numbers or states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueKind {
    Number,
    State,
}

/// Where a subject's evidence comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubjectSource {
    /// The node's heartbeat telemetry, recorded as observations.
    Agent,
    /// Health probes, recorded as observations.
    Probe,
    /// The control plane's own records, read on every pass.
    ControlPlane,
}

/// One subject a rule can watch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubjectInfo {
    /// The subject, or a pattern such as `health.readiness.{check}`.
    pub subject: String,
    /// `true` when `subject` contains the `{check}` placeholder.
    #[serde(default)]
    pub pattern: bool,
    pub kind: ValueKind,
    /// Allowed states (state subjects).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub states: Vec<String>,
    /// `percent`, `ratio`, `seconds` or `days` (number subjects).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    pub scopes: Vec<ScopeKind>,
    pub source: SubjectSource,
    pub description: String,
}

struct Def {
    subject: &'static str,
    kind: ValueKind,
    states: &'static [&'static str],
    unit: Option<&'static str>,
    scope: ScopeKind,
    source: SubjectSource,
    description: &'static str,
}

const HEALTH_STATES: &[&str] = &["pending", "passing", "failing", "disabled"];
const STARTUP_STATES: &[&str] = &["waiting", "succeeded", "exhausted"];

const fn num(subject: &'static str, unit: &'static str, scope: ScopeKind, description: &'static str) -> Def {
    Def {
        subject,
        kind: ValueKind::Number,
        states: &[],
        unit: Some(unit),
        scope,
        source: SubjectSource::Agent,
        description,
    }
}

const fn state(
    subject: &'static str,
    states: &'static [&'static str],
    scope: ScopeKind,
    description: &'static str,
) -> Def {
    Def { subject, kind: ValueKind::State, states, unit: None, scope, source: SubjectSource::ControlPlane, description }
}

const DEFS: &[Def] = &[
    state(
        NODE_AGENT_STATUS,
        &["online", "offline", "pending"],
        ScopeKind::Servers,
        "The node agent's connection: online, offline, or pending (enrolled, never connected).",
    ),
    num("node.cpu.percent", "percent", ScopeKind::Servers, "CPU busy, % of all cores."),
    num("node.mem.used_percent", "percent", ScopeKind::Servers, "Memory in use, %."),
    num("node.disk.used_percent", "percent", ScopeKind::Servers, "Root filesystem (`/` or `C:\\`) in use, %."),
    num(
        "node.disk.max_used_percent",
        "percent",
        ScopeKind::Servers,
        "The fullest local volume in use, % (removable, tmpfs, overlay and other virtual filesystems excluded). The observation detail names the mount.",
    ),
    num("node.swap.used_percent", "percent", ScopeKind::Servers, "Swap in use, %. 0 on a node without swap."),
    num("node.load.per_cpu", "ratio", ScopeKind::Servers, "One-minute load average divided by the CPU count (Linux)."),
    num(
        "node.uptime.seconds",
        "seconds",
        ScopeKind::Servers,
        "Seconds since the node booted. A drop means it rebooted.",
    ),
    Def {
        source: SubjectSource::ControlPlane,
        ..num(
            NODE_CERT_EXPIRES,
            "days",
            ScopeKind::Servers,
            "Days until the node's agent certificate expires (it renews itself before that).",
        )
    },
    state(
        SERVICE_INSTANCE_STATE,
        &["active", "inactive", "failed"],
        ScopeKind::Services,
        "A service instance's state as its node last reported it, per node.",
    ),
    state(
        APP_DEPLOY_STATUS,
        &["pending", "deploying", "deployed", "failed"],
        ScopeKind::Services,
        "A service instance's deployment status, per node.",
    ),
    state(
        PACKAGE_STATE,
        &["running", "stopped", "not-installed"],
        ScopeKind::Installations,
        "A package installation's service state as its recipe last reported it (databases are packages).",
    ),
    state(
        PACKAGE_STATUS,
        &[
            "pending",
            "installing",
            "installed",
            "updating",
            "starting",
            "stopping",
            "restarting",
            "removing",
            "checking",
            "failed",
        ],
        ScopeKind::Installations,
        "A package installation's lifecycle status.",
    ),
    Def {
        source: SubjectSource::ControlPlane,
        ..num(
            TLS_CERT_EXPIRES,
            "days",
            ScopeKind::Certificates,
            "Days until an uploaded ingress TLS certificate expires (negative once expired).",
        )
    },
];

fn info(d: &Def) -> SubjectInfo {
    SubjectInfo {
        subject: d.subject.to_owned(),
        pattern: false,
        kind: d.kind,
        states: d.states.iter().map(|s| (*s).to_owned()).collect(),
        unit: d.unit.map(str::to_owned),
        scopes: vec![d.scope],
        source: d.source,
        description: d.description.to_owned(),
    }
}

fn health_info(purpose: &str, check: &str) -> SubjectInfo {
    let startup = purpose == "startup";
    SubjectInfo {
        subject: format!("health.{purpose}.{check}"),
        pattern: check == "{check}",
        kind: ValueKind::State,
        states: (if startup { STARTUP_STATES } else { HEALTH_STATES }).iter().map(|s| (*s).to_owned()).collect(),
        unit: None,
        scopes: vec![ScopeKind::Services],
        source: SubjectSource::Probe,
        description: if startup {
            "A startup check: waiting, succeeded, or exhausted (its failure budget ran out).".into()
        } else {
            format!("A {purpose} check of a service instance, per node.")
        },
    }
}

/// Every known subject, with the three `health.<purpose>.{check}` patterns.
#[must_use]
pub fn subjects() -> Vec<SubjectInfo> {
    let mut out: Vec<SubjectInfo> = DEFS.iter().map(info).collect();
    out.extend(HEALTH_PURPOSES.iter().map(|p| health_info(p, "{check}")));
    out
}

/// The catalogue entry for a concrete subject (`health.*` included).
#[must_use]
pub fn describe(subject: &str) -> Option<SubjectInfo> {
    if let Some(d) = DEFS.iter().find(|d| d.subject == subject) {
        return Some(info(d));
    }
    let mut parts = subject.splitn(3, '.');
    match (parts.next(), parts.next(), parts.next()) {
        (Some("health"), Some(p), Some(check)) if HEALTH_PURPOSES.contains(&p) && valid_segment(check) => {
            Some(health_info(p, check))
        }
        _ => None,
    }
}

fn valid_segment(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

/// Levenshtein distance, for "did you mean".
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cur = row[j + 1];
            row[j + 1] = (prev + usize::from(ca != *cb)).min(row[j] + 1).min(cur + 1);
            prev = cur;
        }
    }
    row[b.len()]
}

/// Checks that a rule's subject exists and fits its scope.
///
/// Known subjects and `health.<purpose>.<check>` pass when the scope
/// matches. An unknown `node.*` subject is accepted for a node scope, so a
/// newer agent's measurement can be watched before the server knows it,
/// unless it is a near miss of a known subject (a typo). Every other unknown
/// subject is refused.
pub fn check_subject(subject: &str, scope: ScopeKind) -> DomainResult<()> {
    let fits = |i: &SubjectInfo| {
        if i.scopes.contains(&scope) {
            Ok(())
        } else {
            let want: Vec<&str> = i.scopes.iter().map(|s| s.as_str()).collect();
            Err(DomainError::Invalid(format!(
                "subject `{subject}` applies to a {} scope, not {}",
                want.join(" or "),
                scope.as_str()
            )))
        }
    };
    if let Some(i) = describe(subject) {
        return fits(&i);
    }
    let near =
        DEFS.iter().map(|d| d.subject).min_by_key(|k| distance(k, subject)).filter(|k| distance(k, subject) <= 3);
    if subject.starts_with("health.") {
        return Err(DomainError::Invalid(format!(
            "unknown subject `{subject}`: health subjects are health.<readiness|liveness|startup>.<check>"
        )));
    }
    if let Some(k) = near {
        return Err(DomainError::Invalid(format!("unknown subject `{subject}`; did you mean `{k}`?")));
    }
    if subject.starts_with("node.") && forge_core::observation::Subject::new(subject).is_ok() {
        // A newer agent's node measurement.
        return if scope == ScopeKind::Servers {
            Ok(())
        } else {
            Err(DomainError::Invalid(format!("subject `{subject}` applies to a servers scope, not {}", scope.as_str())))
        };
    }
    Err(DomainError::Invalid(format!(
        "unknown subject `{subject}`; known families are {} (GET /api/v1/alert-subjects lists them)",
        KNOWN_SUBJECT_PREFIXES.join(", ")
    )))
}

/// The distinct `health.*` subjects observed on the workspace's
/// services, sorted, at most [`MAX_SEEN_SUBJECTS`]. Reads a bounded
/// number of observation rows.
pub async fn seen_health_subjects(
    observations: &dyn ObservationQuery,
    resolver: &dyn ResourceResolver,
    workspace: WorkspaceId,
) -> DomainResult<Vec<String>> {
    let filter = ObservationFilter { subject_prefix: Some("health.".into()), ..ObservationFilter::default() };
    let rows = observations.latest(&filter, SEEN_SCAN_ROWS).await?;
    // Subjects per service id.
    let mut by_app: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for r in rows {
        if r.key.resource.kind == EntityKind::Service {
            by_app.entry(r.key.resource.id).or_default().insert(r.key.subject.as_str().to_owned());
        }
    }
    let apps: Vec<EntityRef> = by_app.keys().map(|id| EntityRef::new(EntityKind::Service, id)).collect();
    let resolved = resolver.resolve_many(&apps).await;
    let ws = EntityRef::new(EntityKind::Workspace, workspace);
    let mut subjects: BTreeSet<String> = BTreeSet::new();
    for ((_, subs), r) in by_app.into_iter().zip(resolved) {
        // Deleted or unresolvable services are skipped.
        if r.as_ref().is_ok_and(|r| r.ancestors.contains(&ws)) {
            subjects.extend(subs);
        }
    }
    Ok(subjects.into_iter().take(MAX_SEEN_SUBJECTS).collect())
}

/// The installations or certificates a scope names must exist in the
/// workspace. Other scope kinds pass (the API checks nodes itself).
pub async fn check_scope_ids(
    installations: &dyn InstallationRepository,
    certificates: &dyn CertificateRepository,
    workspace: WorkspaceId,
    scope: &RuleScope,
) -> DomainResult<()> {
    let (what, ids, known): (&str, &Vec<String>, BTreeSet<String>) = match scope {
        RuleScope::Installations { ids } if !ids.is_empty() => {
            let known = installations
                .list_installations(None)
                .await?
                .into_iter()
                .filter(|i| i.workspace_id == workspace)
                .map(|i| i.id.to_string())
                .collect();
            ("installation", ids, known)
        }
        RuleScope::Certificates { ids } if !ids.is_empty() => {
            let known =
                certificates.list_certificates(Some(workspace)).await?.into_iter().map(|c| c.id.to_string()).collect();
            ("certificate", ids, known)
        }
        _ => return Ok(()),
    };
    match ids.iter().find(|i| !known.contains(*i)) {
        Some(bad) => Err(DomainError::Invalid(format!("no {what} {bad} in this workspace"))),
        None => Ok(()),
    }
}
/// Where a template is offered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Place {
    Node,
    Service,
    Database,
    Certificate,
    Any,
}

impl Place {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Place::Node => "node",
            Place::Service => "service",
            Place::Database => "database",
            Place::Certificate => "certificate",
            Place::Any => "any",
        }
    }
}

/// The placeholder a template subject may contain.
pub const CHECK_PLACEHOLDER: &str = "{check}";

/// A ready-made rule.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AlertTemplate {
    pub id: String,
    pub place: Place,
    pub name: String,
    pub description: String,
    pub severity: Severity,
    /// May contain `{check}`, filled with a health check name.
    pub subject: String,
    /// The scope kind a rule made from it uses.
    pub scope: ScopeKind,
    pub spec: RuleSpec,
}

impl AlertTemplate {
    /// Whether the subject needs a check name.
    #[must_use]
    pub fn needs_check(&self) -> bool {
        self.subject.contains(CHECK_PLACEHOLDER)
    }

    /// The subject with `{check}` filled in.
    pub fn subject_for(&self, check: Option<&str>) -> DomainResult<String> {
        match (self.needs_check(), check) {
            (false, _) => Ok(self.subject.clone()),
            (true, Some(c)) if valid_segment(c) => Ok(self.subject.replace(CHECK_PLACEHOLDER, c)),
            (true, Some(c)) => {
                Err(DomainError::Invalid(format!("invalid check name `{c}`: use lowercase letters, digits, - and _")))
            }
            (true, None) => Err(DomainError::Invalid(format!("template `{}` needs a health check name", self.id))),
        }
    }
}

const MIN: i64 = 60_000;

fn spec(trigger: Condition, mode: Mode, recovery: Recovery) -> RuleSpec {
    RuleSpec { trigger, mode, recovery }
}

fn is(states: &[&str]) -> Condition {
    Condition::StateIn { states: states.iter().map(|s| (*s).to_owned()).collect() }
}

const fn th(cmp: Cmp, value: f64) -> Condition {
    Condition::Threshold { cmp, value }
}

fn sustained(min: i64) -> Mode {
    Mode::Sustained { for_ms: min * MIN }
}

fn plain() -> Recovery {
    Recovery::default()
}

fn recover(c: Condition, hold_min: i64) -> Recovery {
    Recovery { condition: Some(c), hold_ms: hold_min * MIN }
}

fn build() -> Vec<AlertTemplate> {
    use Place::{Certificate, Database, Node, Service};
    use ScopeKind::{Certificates, Installations, Servers, Services};
    use Severity::{Critical, Info, Major, Warning};
    let t = |id: &str, place, name: &str, description: &str, severity, subject: &str, scope, spec| AlertTemplate {
        id: id.to_owned(),
        place,
        name: name.to_owned(),
        description: description.to_owned(),
        severity,
        subject: subject.to_owned(),
        scope,
        spec,
    };
    let disk_filling = spec(th(Cmp::Gt, 85.0), sustained(5), plain());
    let mem_pressure = spec(th(Cmp::Gt, 90.0), sustained(10), recover(th(Cmp::Lt, 85.0), 0));
    vec![
        // Nodes.
        t(
            "node-offline",
            Node,
            "Node offline",
            "The agent has not reported for 2 minutes.",
            Critical,
            NODE_AGENT_STATUS,
            Servers,
            spec(is(&["offline"]), sustained(2), plain()),
        ),
        t(
            "node-never-connected",
            Node,
            "Node never connected",
            "The node was enrolled but its agent has not connected for 30 minutes.",
            Warning,
            NODE_AGENT_STATUS,
            Servers,
            spec(is(&["pending"]), sustained(30), plain()),
        ),
        t(
            "node-cpu-high",
            Node,
            "High CPU",
            "CPU averages above 90 % over 10 minutes; clears below 80 % for 5 minutes.",
            Warning,
            "node.cpu.percent",
            Servers,
            spec(
                th(Cmp::Gt, 90.0),
                Mode::WindowAverage { window_ms: 10 * MIN, min_coverage_pct: 80 },
                recover(th(Cmp::Lt, 80.0), 5),
            ),
        ),
        t(
            "node-cpu-spikes",
            Node,
            "CPU spikes",
            "CPU reaches 95 % at least 5 separate times in 30 minutes.",
            Warning,
            "node.cpu.percent",
            Servers,
            spec(th(Cmp::Ge, 95.0), Mode::Repeated { count: 5, window_ms: 30 * MIN, min_spacing_ms: MIN }, plain()),
        ),
        t(
            "node-memory-pressure",
            Node,
            "Memory pressure",
            "Memory use above 90 % for 10 minutes; clears below 85 %.",
            Major,
            "node.mem.used_percent",
            Servers,
            mem_pressure.clone(),
        ),
        t(
            "node-disk-filling",
            Node,
            "Disk filling up",
            "The fullest volume is above 85 % for 5 minutes.",
            Warning,
            "node.disk.max_used_percent",
            Servers,
            disk_filling.clone(),
        ),
        t(
            "node-disk-almost-full",
            Node,
            "Disk almost full",
            "The fullest volume is above 95 % for 2 minutes.",
            Critical,
            "node.disk.max_used_percent",
            Servers,
            spec(th(Cmp::Gt, 95.0), sustained(2), plain()),
        ),
        t(
            "node-swap-in-use",
            Node,
            "Swap in use",
            "More than half of the swap is in use for 10 minutes.",
            Warning,
            "node.swap.used_percent",
            Servers,
            spec(th(Cmp::Gt, 50.0), sustained(10), plain()),
        ),
        t(
            "node-load-high",
            Node,
            "Load high",
            "The one-minute load is above 2 per CPU for 10 minutes.",
            Warning,
            "node.load.per_cpu",
            Servers,
            spec(th(Cmp::Gt, 2.0), sustained(10), plain()),
        ),
        t(
            "node-rebooted",
            Node,
            "Node rebooted",
            "The node booted less than 10 minutes ago; clears by itself after that.",
            Info,
            "node.uptime.seconds",
            Servers,
            spec(th(Cmp::Lt, 600.0), Mode::Immediate, plain()),
        ),
        // Services (services).
        t(
            "service-liveness-failing",
            Service,
            "Liveness failing",
            "A liveness check fails for 1 minute.",
            Critical,
            "health.liveness.{check}",
            Services,
            spec(is(&["failing"]), sustained(1), plain()),
        ),
        t(
            "service-readiness-failing",
            Service,
            "Readiness failing",
            "A readiness check fails for 1 minute.",
            Major,
            "health.readiness.{check}",
            Services,
            spec(is(&["failing"]), sustained(1), plain()),
        ),
        t(
            "service-startup-exhausted",
            Service,
            "Startup failed",
            "A startup check used up its failure budget.",
            Critical,
            "health.startup.{check}",
            Services,
            spec(is(&["exhausted"]), Mode::Immediate, plain()),
        ),
        t(
            "service-health-flapping",
            Service,
            "Health flapping",
            "A readiness check starts failing 3 times in 15 minutes.",
            Warning,
            "health.readiness.{check}",
            Services,
            spec(is(&["failing"]), Mode::Repeated { count: 3, window_ms: 15 * MIN, min_spacing_ms: 0 }, plain()),
        ),
        t(
            "service-instance-failed",
            Service,
            "Instance failed",
            "An instance reports `failed` for 1 minute.",
            Critical,
            SERVICE_INSTANCE_STATE,
            Services,
            spec(is(&["failed"]), sustained(1), plain()),
        ),
        t(
            "service-deploy-failed",
            Service,
            "Deployment failed",
            "A deployment to an instance failed.",
            Major,
            APP_DEPLOY_STATUS,
            Services,
            spec(is(&["failed"]), Mode::Immediate, plain()),
        ),
        // Databases (package installations).
        t(
            "database-stopped",
            Database,
            "Database stopped",
            "The database service reports `stopped` for 2 minutes.",
            Critical,
            PACKAGE_STATE,
            Installations,
            spec(is(&["stopped"]), sustained(2), plain()),
        ),
        t(
            "database-operation-failed",
            Database,
            "Install or upgrade failed",
            "An install, upgrade or other package job failed.",
            Major,
            PACKAGE_STATUS,
            Installations,
            spec(is(&["failed"]), Mode::Immediate, plain()),
        ),
        t(
            "database-host-disk-filling",
            Database,
            "Database host disk filling up",
            "The database node's fullest volume is above 85 % for 5 minutes.",
            Warning,
            "node.disk.max_used_percent",
            Servers,
            disk_filling,
        ),
        t(
            "database-host-memory-pressure",
            Database,
            "Database host memory pressure",
            "The database node's memory use is above 90 % for 10 minutes; clears below 85 %.",
            Major,
            "node.mem.used_percent",
            Servers,
            mem_pressure,
        ),
        // Certificates.
        t(
            "certificate-expires-14d",
            Certificate,
            "Certificate expires within 14 days",
            "An ingress certificate expires in less than 14 days.",
            Warning,
            TLS_CERT_EXPIRES,
            Certificates,
            spec(th(Cmp::Lt, 14.0), Mode::Immediate, plain()),
        ),
        t(
            "certificate-expires-3d",
            Certificate,
            "Certificate expires within 3 days",
            "An ingress certificate expires in less than 3 days.",
            Critical,
            TLS_CERT_EXPIRES,
            Certificates,
            spec(th(Cmp::Lt, 3.0), Mode::Immediate, plain()),
        ),
    ]
}

/// Every template, optionally only one place's. `Place::Any` lists all.
#[must_use]
pub fn templates(place: Option<Place>) -> Vec<AlertTemplate> {
    build().into_iter().filter(|t| place.is_none_or(|p| p == Place::Any || p == t.place)).collect()
}

/// One template by id.
pub fn template(id: &str) -> DomainResult<AlertTemplate> {
    build().into_iter().find(|t| t.id == id).ok_or_else(|| DomainError::NotFound(format!("alert rule template `{id}`")))
}

#[cfg(test)]
mod tests;
