//! Silences and maintenance windows (#11, D-112).
//!
//! A **silence** holds back the notifications of the alarms it matches for a
//! bounded time. It never touches evaluation: alarms still open, fire and
//! close, and their history is complete. A **maintenance window** is a
//! silence whose start lies in the future; there is one object and one
//! mechanism.
//!
//! A silence is never edited apart from its end: it can be extended and it
//! can be ended early. What it matches (scope, rules, severities) is fixed,
//! so the audit record of its creation stays true for its whole life.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::alerts::Severity;
use crate::{
    AlertRuleId, ClusterId, DomainResult, EntityKind, EntityRef, EnvironmentId, Principal, ServerId, SilenceId,
    Timestamp, WorkspaceId,
};

/// Longest silence, start to end, in milliseconds (30 days).
pub const MAX_SILENCE_MS: i64 = 30 * 24 * 3600 * 1000;
/// Shortest silence, in milliseconds (one minute).
pub const MIN_SILENCE_MS: i64 = 60 * 1000;
/// How far ahead a silence may start, in milliseconds (365 days).
pub const MAX_SILENCE_AHEAD_MS: i64 = 365 * 24 * 3600 * 1000;
/// Longest reason, in characters.
pub const MAX_SILENCE_REASON: usize = 500;
/// Most rules one silence names.
pub const MAX_SILENCE_RULES: usize = 100;
/// Most nodes or services one silence names.
pub const MAX_SILENCE_SCOPE_IDS: usize = 100;
/// Most silences of one workspace that have not ended yet.
pub const MAX_OPEN_SILENCES: u64 = 200;
/// Most silences one person has open in a workspace (also for workspace
/// managers, who are exempt from [`MAX_OPEN_SILENCES`]).
pub const MAX_OPEN_SILENCES_PER_PERSON: u64 = 50;

/// Which alarms a silence covers, by where their resource is.
///
/// The shapes of `servers` and `services` are those of a rule's or a
/// policy's scope (`RuleScope`); a silence must name at least one id, and
/// has three wider scopes of its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SilenceScope {
    /// Every alarm of the workspace.
    Workspace,
    /// Alarms on the cluster's nodes and on service instances that run
    /// on them.
    Cluster { id: ClusterId },
    /// Alarms on the environment's (environment's) services and nodes.
    Environment { id: EnvironmentId },
    /// Alarms on these nodes and on service instances that run on them.
    Servers { ids: Vec<ServerId> },
    /// Alarms on these services.
    Services { ids: Vec<String> },
}

impl SilenceScope {
    /// The entities the scope names: what a caller must hold `alert.manage`
    /// on to create, extend or end the silence.
    #[must_use]
    pub fn refs(&self, workspace: WorkspaceId) -> Vec<EntityRef> {
        match self {
            SilenceScope::Workspace => vec![EntityRef::new(EntityKind::Workspace, workspace)],
            SilenceScope::Cluster { id } => vec![EntityRef::new(EntityKind::Cluster, id)],
            SilenceScope::Environment { id } => vec![EntityRef::new(EntityKind::Environment, id)],
            SilenceScope::Servers { ids } => ids.iter().map(|i| EntityRef::new(EntityKind::Server, i)).collect(),
            SilenceScope::Services { ids } => ids.iter().map(|i| EntityRef::new(EntityKind::Service, i)).collect(),
        }
    }
}

/// Where a silence stands at a point in time. Derived, never stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SilenceState {
    /// It starts later (a maintenance window that has not begun).
    Scheduled,
    /// It holds notifications back now.
    Active,
    /// Its end has passed, or it was ended early.
    Expired,
}

impl SilenceState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            SilenceState::Scheduled => "scheduled",
            SilenceState::Active => "active",
            SilenceState::Expired => "expired",
        }
    }
}

/// A time-bounded suppression of notifications.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Silence {
    pub id: SilenceId,
    pub workspace_id: WorkspaceId,
    pub scope: SilenceScope,
    /// Only alarms of these rules (empty: every rule).
    #[serde(default)]
    pub rules: Vec<AlertRuleId>,
    /// Only alarms of this severity and worse (`None`: no lower bound).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_severity: Option<Severity>,
    /// Only alarms of this severity and milder (`None`: no upper bound). A
    /// worse alarm is not silenced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_severity: Option<Severity>,
    pub starts_at: Timestamp,
    /// Exclusive: at this instant the silence no longer holds.
    pub ends_at: Timestamp,
    pub reason: String,
    pub created_by: Principal,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    /// Who ended it early.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_by: Option<Principal>,
}

impl Silence {
    /// The state at `now`. The end wins: a silence ended before it started
    /// is expired.
    #[must_use]
    pub const fn state(&self, now: Timestamp) -> SilenceState {
        if now.0 >= self.ends_at.0 {
            SilenceState::Expired
        } else if now.0 < self.starts_at.0 {
            SilenceState::Scheduled
        } else {
            SilenceState::Active
        }
    }
}

/// Which silences to list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SilenceFilter {
    pub workspace: Option<WorkspaceId>,
    /// Only silences in this state at the given time.
    pub state: Option<(SilenceState, Timestamp)>,
    /// Only silences that start before this one (`starts_at`, `id`): the
    /// next page.
    pub before: Option<(Timestamp, SilenceId)>,
}

/// A change to when a silence holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SilenceWindow {
    pub starts_at: Timestamp,
    pub ends_at: Timestamp,
    /// Set when the change ends the silence early.
    pub ended_by: Option<Principal>,
    pub updated_at: Timestamp,
}

/// Stored silences.
#[async_trait]
pub trait SilenceRepository: Send + Sync + std::fmt::Debug {
    async fn create_silence(&self, s: &Silence) -> DomainResult<()>;
    async fn get_silence(&self, id: SilenceId) -> DomainResult<Option<Silence>>;
    /// Latest start first.
    async fn list_silences(&self, filter: &SilenceFilter, limit: u32) -> DomainResult<Vec<Silence>>;
    /// A workspace's silences that hold at `now`. At most
    /// [`MAX_OPEN_SILENCES`], which the service enforces on creation.
    async fn active_silences(&self, workspace: WorkspaceId, now: Timestamp) -> DomainResult<Vec<Silence>>;
    /// How many of a workspace's silences have not ended at `now`; with
    /// `created_by`, only those that person created.
    async fn count_open_silences(
        &self,
        workspace: WorkspaceId,
        now: Timestamp,
        created_by: Option<&Principal>,
    ) -> DomainResult<u64>;
    /// Moves a silence's start and end, only while it has not ended at
    /// `now`. `false`, with nothing written, when it had ended (or does not
    /// exist): an expired silence never changes.
    async fn set_silence_window(&self, id: SilenceId, window: &SilenceWindow, now: Timestamp) -> DomainResult<bool>;
    /// Deletes silences that ended before `before`. Returns how many.
    async fn prune_silences(&self, before: Timestamp) -> DomainResult<u64>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::UserId;

    fn silence(starts: i64, ends: i64) -> Silence {
        Silence {
            id: SilenceId::new(),
            workspace_id: WorkspaceId::new(),
            scope: SilenceScope::Workspace,
            rules: vec![],
            min_severity: None,
            max_severity: None,
            starts_at: Timestamp(starts),
            ends_at: Timestamp(ends),
            reason: "kernel upgrade".into(),
            created_by: Principal::User(UserId::new()),
            created_at: Timestamp(0),
            updated_at: Timestamp(0),
            ended_by: None,
        }
    }

    #[test]
    fn the_state_follows_the_clock_and_the_end_is_exclusive() {
        let s = silence(100, 200);
        assert_eq!(s.state(Timestamp(99)), SilenceState::Scheduled);
        assert_eq!(s.state(Timestamp(100)), SilenceState::Active);
        assert_eq!(s.state(Timestamp(199)), SilenceState::Active);
        assert_eq!(s.state(Timestamp(200)), SilenceState::Expired);
        // Ended before it started: expired, never scheduled.
        assert_eq!(silence(100, 50).state(Timestamp(60)), SilenceState::Expired);
    }

    #[test]
    fn a_scope_names_the_entities_to_authorize_on() {
        let ws = WorkspaceId::new();
        assert_eq!(SilenceScope::Workspace.refs(ws), vec![EntityRef::new(EntityKind::Workspace, ws)]);
        let (a, b) = (ServerId::new(), ServerId::new());
        let refs = SilenceScope::Servers { ids: vec![a, b] }.refs(ws);
        assert_eq!(refs, vec![EntityRef::new(EntityKind::Server, a), EntityRef::new(EntityKind::Server, b)]);
        let ns = EnvironmentId::new();
        assert_eq!(SilenceScope::Environment { id: ns }.refs(ws), vec![EntityRef::new(EntityKind::Environment, ns)]);
    }

    #[test]
    fn scopes_serialize_like_rule_scopes() {
        let id = ServerId::new();
        let json = serde_json::to_value(SilenceScope::Servers { ids: vec![id] }).unwrap_or_default();
        assert_eq!(json, serde_json::json!({"kind": "servers", "ids": [id]}));
        let json = serde_json::to_value(SilenceScope::Workspace).unwrap_or_default();
        assert_eq!(json, serde_json::json!({"kind": "workspace"}));
    }
}
