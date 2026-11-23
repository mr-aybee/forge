//! Alerts (#11, D-087): rules over observations and the alarms they open.
//!
//! A rule watches one observation subject on the resources in its scope.
//! Its condition is a `forge_rules::RuleSpec`, kept here as JSON so the
//! domain stays free of the evaluation crate. Each series (rule + resource +
//! node + subject) has one fingerprint; at most one alarm per fingerprint is
//! active at a time, and the store enforces it. A recurrence after recovery
//! is a new alarm.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{AlarmId, AlertRuleId, DomainResult, EntityRef, Principal, ServerId, Timestamp, WorkspaceId};

/// Longest rule name, in characters.
pub const MAX_RULE_NAME: usize = 120;
/// Longest note on an alarm, in characters.
pub const MAX_NOTE: usize = 2000;

/// How bad an alarm is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Warning,
    Major,
    Critical,
}

impl Severity {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Warning => "warning",
            Severity::Major => "major",
            Severity::Critical => "critical",
        }
    }
}

/// Which resources a rule watches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RuleScope {
    /// Every node of the workspace, or these nodes.
    Servers {
        #[serde(default)]
        ids: Vec<ServerId>,
    },
    /// Every service of the workspace, or these services.
    Services {
        #[serde(default)]
        ids: Vec<String>,
    },
    /// Every package installation of the workspace (databases included),
    /// or these installations (D-118).
    Installations {
        #[serde(default)]
        ids: Vec<String>,
    },
    /// Every uploaded ingress certificate of the workspace, or these
    /// certificates (D-118).
    Certificates {
        #[serde(default)]
        ids: Vec<String>,
    },
}

/// The kind of a [`RuleScope`], without its ids: what a subject or a
/// template applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeKind {
    Servers,
    Services,
    Installations,
    Certificates,
}

impl ScopeKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            ScopeKind::Servers => "servers",
            ScopeKind::Services => "services",
            ScopeKind::Installations => "installations",
            ScopeKind::Certificates => "certificates",
        }
    }

    /// The resource kind a series in this scope is about.
    #[must_use]
    pub const fn entity(self) -> crate::EntityKind {
        match self {
            ScopeKind::Servers => crate::EntityKind::Server,
            ScopeKind::Services => crate::EntityKind::Service,
            ScopeKind::Installations => crate::EntityKind::Installation,
            ScopeKind::Certificates => crate::EntityKind::Certificate,
        }
    }
}

impl RuleScope {
    #[must_use]
    pub const fn kind(&self) -> ScopeKind {
        match self {
            RuleScope::Servers { .. } => ScopeKind::Servers,
            RuleScope::Services { .. } => ScopeKind::Services,
            RuleScope::Installations { .. } => ScopeKind::Installations,
            RuleScope::Certificates { .. } => ScopeKind::Certificates,
        }
    }

    /// How many ids the scope names (`0`: every resource of its kind).
    #[must_use]
    pub fn len(&self) -> usize {
        match self {
            RuleScope::Servers { ids } => ids.len(),
            RuleScope::Services { ids } | RuleScope::Installations { ids } | RuleScope::Certificates { ids } => {
                ids.len()
            }
        }
    }

    /// Whether the scope covers every resource of its kind.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// An alert rule. Edits create a new revision; alarms keep the revision
/// that opened them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AlertRule {
    pub id: AlertRuleId,
    pub workspace_id: WorkspaceId,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub enabled: bool,
    pub severity: Severity,
    pub scope: RuleScope,
    /// Observation subject, e.g. `node.cpu.percent` or
    /// `health.readiness.http`.
    pub subject: String,
    /// `forge_rules::RuleSpec` as JSON (validated by the service).
    pub spec: serde_json::Value,
    pub revision: i64,
    pub created_by: Principal,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

/// Where an alarm stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlarmState {
    Firing,
    /// Recovery held long enough.
    Resolved,
    /// Tracking stopped without recovery evidence: the rule or the resource
    /// was deleted, or the rule was disabled or changed incompatibly.
    Ended,
}

/// One problem episode.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Alarm {
    pub id: AlarmId,
    pub workspace_id: WorkspaceId,
    pub rule_id: AlertRuleId,
    pub rule_revision: i64,
    /// Rule name when it fired (rules can be renamed or deleted).
    pub title: String,
    pub severity: Severity,
    /// Identifies the series: rule, resource, node and subject.
    pub fingerprint: String,
    pub resource: EntityRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<ServerId>,
    pub subject: String,
    pub state: AlarmState,
    /// The evidence that fired it (a `forge_rules::Value`).
    #[serde(default)]
    pub fired_value: serde_json::Value,
    pub fired_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<Timestamp>,
    /// Why it ended when `state` is `ended`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acknowledged_by: Option<Principal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acknowledged_at: Option<Timestamp>,
    /// The previous episode of the same series, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous: Option<AlarmId>,
}

/// One line of an alarm's timeline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AlarmEvent {
    pub alarm_id: AlarmId,
    pub at: Timestamp,
    /// `fired`, `resolved`, `ended`, `acknowledged`, `unacknowledged`, `note`.
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<Principal>,
    #[serde(default)]
    pub detail: serde_json::Value,
}

/// Which alarms to list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AlarmFilter {
    /// Only these workspaces (`None`: all).
    pub workspaces: Option<Vec<WorkspaceId>>,
    /// Only firing alarms.
    pub active: bool,
    pub rule: Option<AlertRuleId>,
    pub resource: Option<EntityRef>,
    /// Only alarms older than this one (`fired_at`, `id`): the next page.
    pub before: Option<(Timestamp, AlarmId)>,
}

/// Rules and alarms.
#[async_trait]
pub trait AlertRepository: Send + Sync + std::fmt::Debug {
    async fn create_rule(&self, rule: &AlertRule) -> DomainResult<()>;
    /// Replaces a rule (the caller bumps `revision`).
    async fn update_rule(&self, rule: &AlertRule) -> DomainResult<()>;
    async fn delete_rule(&self, id: AlertRuleId) -> DomainResult<()>;
    async fn get_rule(&self, id: AlertRuleId) -> DomainResult<AlertRule>;
    /// All rules (`None`), or one workspace's.
    async fn list_rules(&self, workspace: Option<WorkspaceId>) -> DomainResult<Vec<AlertRule>>;

    /// Opens an alarm. A conflict when the fingerprint already has a firing
    /// alarm (exactly one active episode per series).
    async fn open_alarm(&self, alarm: &Alarm, event: &AlarmEvent) -> DomainResult<()>;
    /// Moves a firing alarm to `resolved` or `ended` and records the event.
    /// `false` when it was no longer firing.
    async fn close_alarm(
        &self,
        id: AlarmId,
        state: AlarmState,
        at: Timestamp,
        reason: Option<&str>,
        event: &AlarmEvent,
    ) -> DomainResult<bool>;
    /// Sets or clears the acknowledgement and records the event.
    async fn acknowledge(
        &self,
        id: AlarmId,
        by: Option<&Principal>,
        at: Timestamp,
        event: &AlarmEvent,
    ) -> DomainResult<()>;
    async fn add_alarm_event(&self, event: &AlarmEvent) -> DomainResult<()>;
    async fn get_alarm(&self, id: AlarmId) -> DomainResult<Alarm>;
    /// Newest first.
    async fn list_alarms(&self, filter: &AlarmFilter, limit: u32) -> DomainResult<Vec<Alarm>>;
    /// The firing alarm of a series, if any.
    async fn active_alarm(&self, fingerprint: &str) -> DomainResult<Option<Alarm>>;
    /// The last alarm of a series (for linking a recurrence).
    async fn last_alarm(&self, fingerprint: &str) -> DomainResult<Option<Alarm>>;
    async fn alarm_events(&self, id: AlarmId) -> DomainResult<Vec<AlarmEvent>>;
}

/// The evaluator's own durable state: one tracker per series and the
/// observation cursor.
#[async_trait]
pub trait AlertStateRepository: Send + Sync + std::fmt::Debug {
    /// Every stored tracker: fingerprint, rule revision, tracker JSON.
    async fn trackers(&self) -> DomainResult<Vec<(String, i64, serde_json::Value)>>;
    /// Saves trackers in one transaction.
    async fn save_trackers(
        &self,
        rows: &[(String, AlertRuleId, i64, serde_json::Value)],
        at: Timestamp,
    ) -> DomainResult<()>;
    /// Forgets trackers (deleted rules, resources gone).
    async fn delete_trackers(&self, fingerprints: &[String]) -> DomainResult<()>;
    async fn cursor(&self, name: &str) -> DomainResult<i64>;
    async fn set_cursor(&self, name: &str, seq: i64) -> DomainResult<()>;
}

/// The fingerprint of a series: rule, resource, node and subject.
#[must_use]
pub fn fingerprint(rule: AlertRuleId, resource: &EntityRef, server: Option<ServerId>, subject: &str) -> String {
    let node = server.map(|s| s.to_string()).unwrap_or_default();
    format!("{rule}|{}:{}|{node}|{subject}", resource.kind.as_str(), resource.id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EntityKind;

    #[test]
    fn fingerprints_separate_series() {
        let r = AlertRuleId::new();
        let s = ServerId::new();
        let res = EntityRef::new(EntityKind::Server, s);
        let a = fingerprint(r, &res, None, "node.cpu.percent");
        assert_ne!(a, fingerprint(r, &res, Some(s), "node.cpu.percent"));
        assert_ne!(a, fingerprint(r, &res, None, "node.mem.percent"));
        assert_ne!(a, fingerprint(AlertRuleId::new(), &res, None, "node.cpu.percent"));
        assert_eq!(a, fingerprint(r, &res, None, "node.cpu.percent"), "stable");
    }

    #[test]
    fn severities_order() {
        assert!(Severity::Critical > Severity::Major && Severity::Warning > Severity::Info);
        assert_eq!(serde_json::to_string(&Severity::Major).unwrap_or_default(), "\"major\"");
    }
}
