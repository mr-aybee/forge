//! Silences and maintenance windows (#11, D-112).
//!
//! A silence holds back the notifications of the alarms it matches while it
//! is active. It is consulted in exactly one place, just before a message
//! would be sent (`notify::Notifier`), and shown on alarms. The evaluator
//! never reads it: alarms open and close as if no silence existed.
//!
//! A silence is matched against the alarm, not against an alarm id, so a
//! recurrence inside the window (a new alarm of the same series) is covered
//! like the first episode.

use std::collections::HashMap;
use std::sync::Arc;

use forge_core::alerts::{Alarm, AlarmState, AlertRepository, Severity};
use forge_core::ports::ResourceResolver;
use forge_core::silences::{
    MAX_OPEN_SILENCES, MAX_OPEN_SILENCES_PER_PERSON, MAX_SILENCE_AHEAD_MS, MAX_SILENCE_MS, MAX_SILENCE_REASON,
    MAX_SILENCE_RULES, MAX_SILENCE_SCOPE_IDS, MIN_SILENCE_MS, Silence, SilenceFilter, SilenceRepository, SilenceScope,
    SilenceState, SilenceWindow,
};
use forge_core::{
    AlertRuleId, Clock, DomainError, DomainResult, EntityKind, EntityRef, Principal, SilenceId, Timestamp, WorkspaceId,
};
use serde::Serialize;

/// A start this far in the past is taken as "now"; an older one is refused
/// (a clock that is slightly ahead on the caller's side is not an error).
const START_SLACK_MS: i64 = 5 * 60 * 1000;

/// Where an alarm's resource sits: what cluster and environment scopes match
/// against.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AlarmPlace {
    /// The resource's ancestors (environment, project, cluster, workspace).
    pub ancestors: Vec<EntityRef>,
}

impl AlarmPlace {
    fn within(&self, kind: EntityKind, id: &str) -> bool {
        self.ancestors.iter().any(|a| a.kind == kind && a.id == id)
    }
}

/// Whether a scope needs the alarm's place to be matched.
const fn needs_place(scope: &SilenceScope) -> bool {
    matches!(scope, SilenceScope::Cluster { .. } | SilenceScope::Environment { .. })
}

fn scope_matches(scope: &SilenceScope, alarm: &Alarm, place: &AlarmPlace) -> bool {
    match scope {
        SilenceScope::Workspace => true,
        // A scope covers only what sits under what it names, the same tree
        // its authorization is checked on. A service instance on a
        // silenced node is not covered: the service may belong to an
        // environment the person who silenced the node holds no role on.
        SilenceScope::Cluster { id } => place.within(EntityKind::Cluster, &id.to_string()),
        SilenceScope::Environment { id } => place.within(EntityKind::Environment, &id.to_string()),
        SilenceScope::Servers { ids } => match alarm.resource.kind {
            EntityKind::Server => ids.iter().any(|i| i.to_string() == alarm.resource.id),
            // A package installation sits under its node (D-118).
            EntityKind::Installation => alarm.server.is_some_and(|s| ids.contains(&s)),
            _ => false,
        },
        SilenceScope::Services { ids } => {
            alarm.resource.kind == EntityKind::Service && ids.contains(&alarm.resource.id)
        }
    }
}

/// Whether a silence covers an alarm, whatever the time. A severity outside
/// the silence's bounds is not covered: an alarm worse than `max_severity`
/// still notifies.
#[must_use]
pub fn silence_matches(s: &Silence, alarm: &Alarm, place: &AlarmPlace) -> bool {
    s.workspace_id == alarm.workspace_id
        && s.min_severity.is_none_or(|m| alarm.severity >= m)
        && s.max_severity.is_none_or(|m| alarm.severity <= m)
        && (s.rules.is_empty() || s.rules.contains(&alarm.rule_id))
        && scope_matches(&s.scope, alarm, place)
}

/// The silence that holds an alarm back, as shown on the alarm.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SilenceMark {
    pub silence_id: SilenceId,
    /// When that silence ends (epoch ms).
    pub until: Timestamp,
    pub reason: String,
}

impl From<&Silence> for SilenceMark {
    fn from(s: &Silence) -> Self {
        Self { silence_id: s.id, until: s.ends_at, reason: s.reason.clone() }
    }
}

/// A silence as a person writes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SilenceInput {
    pub scope: SilenceScope,
    pub rules: Vec<AlertRuleId>,
    pub min_severity: Option<Severity>,
    pub max_severity: Option<Severity>,
    /// `None`: now. A later time makes a maintenance window.
    pub starts_at: Option<Timestamp>,
    pub ends_at: Timestamp,
    pub reason: String,
    /// The creator holds `alert.manage` on the whole workspace: the
    /// workspace-wide cap on open silences does not apply to them, so people
    /// with narrow roles cannot use it up during an incident.
    pub manages_workspace: bool,
}

fn clean_reason(reason: &str) -> DomainResult<String> {
    let r = reason.trim();
    if r.is_empty() {
        return Err(DomainError::Invalid("reason: required, so others can tell why alarms are quiet".into()));
    }

    if r.chars().count() > MAX_SILENCE_REASON || r.chars().any(|c| c.is_control() && c != '\n') {
        return Err(DomainError::Invalid(format!(
            "reason: at most {MAX_SILENCE_REASON} characters, without control characters"
        )));
    }

    Ok(r.to_owned())
}

/// Checks the length of a window that starts at `starts` and ends at `ends`.
fn check_window(starts: Timestamp, ends: Timestamp, now: Timestamp) -> DomainResult<()> {
    if ends.0 <= now.0 {
        return Err(DomainError::Invalid("ends_at: the end is not in the future".into()));
    }

    let length = ends.0 - starts.0;
    if length < MIN_SILENCE_MS {
        return Err(DomainError::Invalid("ends_at: a silence lasts at least one minute".into()));
    }

    if length > MAX_SILENCE_MS {
        return Err(DomainError::Invalid(format!(
            "ends_at: a silence lasts at most {} days from its start; create another one for longer",
            MAX_SILENCE_MS / (24 * 3600 * 1000)
        )));
    }

    Ok(())
}

/// Silences: creating, extending, ending, and matching alarms.
#[derive(Debug, Clone)]
pub struct Silences {
    pub store: Arc<dyn SilenceRepository>,
    pub alerts: Arc<dyn AlertRepository>,
    pub resolver: Arc<dyn ResourceResolver>,
    pub clock: Arc<dyn Clock>,
}

impl Silences {
    pub async fn get(&self, id: SilenceId) -> DomainResult<Silence> {
        self.store.get_silence(id).await?.ok_or_else(|| DomainError::NotFound("silence".into()))
    }

    /// A workspace's silences, latest start first, optionally only those in
    /// one state now.
    pub async fn list(
        &self,
        workspace: WorkspaceId,
        state: Option<SilenceState>,
        before: Option<(Timestamp, SilenceId)>,
        limit: u32,
    ) -> DomainResult<Vec<Silence>> {
        let now = self.clock.now();
        let filter = SilenceFilter { workspace: Some(workspace), state: state.map(|s| (s, now)), before };

        self.store.list_silences(&filter, limit).await
    }

    /// Checks that every entity a scope names exists in the workspace. An
    /// entity of another workspace reads like one that does not exist.
    async fn check_scope(&self, workspace: WorkspaceId, scope: &SilenceScope) -> DomainResult<()> {
        let count = match scope {
            SilenceScope::Servers { ids } => ids.len(),
            SilenceScope::Services { ids } => ids.len(),
            _ => 1,
        };

        if count == 0 {
            return Err(DomainError::Invalid(
                "scope: name at least one id, or use the workspace scope to cover everything".into(),
            ));
        }

        if count > MAX_SILENCE_SCOPE_IDS {
            return Err(DomainError::Invalid(format!("scope: at most {MAX_SILENCE_SCOPE_IDS} ids")));
        }

        let refs = scope.refs(workspace);
        let home = EntityRef::new(EntityKind::Workspace, workspace);

        for (r, resolved) in refs.iter().zip(self.resolver.resolve_many(&refs).await) {
            let inside = match resolved {
                Ok(res) => res.entity == home || res.ancestors.contains(&home),
                Err(DomainError::NotFound(_) | DomainError::Invalid(_)) => false,
                Err(e) => return Err(e),
            };

            if !inside {
                return Err(DomainError::Invalid(format!("scope: no {} {}", r.kind.as_str(), r.id)));
            }
        }

        Ok(())
    }

    async fn check_rules(&self, workspace: WorkspaceId, rules: &[AlertRuleId]) -> DomainResult<Vec<AlertRuleId>> {
        let mut rules = rules.to_vec();
        rules.sort();
        rules.dedup();

        if rules.len() > MAX_SILENCE_RULES {
            return Err(DomainError::Invalid(format!("rules: at most {MAX_SILENCE_RULES}")));
        }

        if !rules.is_empty() {
            let known: std::collections::HashSet<AlertRuleId> =
                self.alerts.list_rules(Some(workspace)).await?.into_iter().map(|r| r.id).collect();

            if let Some(missing) = rules.iter().find(|r| !known.contains(r)) {
                return Err(DomainError::Invalid(format!("rules: no alert rule {missing}")));
            }
        }

        Ok(rules)
    }

    /// Creates a silence. The caller has authorized the scope.
    pub async fn create(&self, workspace: WorkspaceId, input: SilenceInput, by: Principal) -> DomainResult<Silence> {
        let now = self.clock.now();
        let reason = clean_reason(&input.reason)?;

        if let (Some(min), Some(max)) = (input.min_severity, input.max_severity)
            && min > max
        {
            return Err(DomainError::Invalid("max_severity: milder than min_severity, so nothing would match".into()));
        }

        let starts_at = match input.starts_at {
            None => now,
            Some(t) if t.0 < now.0 - START_SLACK_MS => {
                return Err(DomainError::Invalid("starts_at: the start is in the past".into()));
            }
            Some(t) if t.0 > now.0 + MAX_SILENCE_AHEAD_MS => {
                return Err(DomainError::Invalid(format!(
                    "starts_at: at most {} days ahead",
                    MAX_SILENCE_AHEAD_MS / (24 * 3600 * 1000)
                )));
            }
            Some(t) => Timestamp(t.0.max(now.0)),
        };

        check_window(starts_at, input.ends_at, now)?;
        self.check_scope(workspace, &input.scope).await?;
        let rules = self.check_rules(workspace, &input.rules).await?;

        // Check then insert, not atomic: two parallel requests can pass the
        // cap by one each. The caps bound growth; they are not a quota.
        if self.store.count_open_silences(workspace, now, Some(&by)).await? >= MAX_OPEN_SILENCES_PER_PERSON {
            return Err(DomainError::Conflict(format!(
                "one person has at most {MAX_OPEN_SILENCES_PER_PERSON} silences that have not ended; end one first"
            )));
        }

        if !input.manages_workspace && self.store.count_open_silences(workspace, now, None).await? >= MAX_OPEN_SILENCES
        {
            return Err(DomainError::Conflict(format!(
                "a workspace has at most {MAX_OPEN_SILENCES} silences that have not ended; end one first"
            )));
        }

        let silence = Silence {
            id: SilenceId::new(),
            workspace_id: workspace,
            scope: input.scope,
            rules,
            min_severity: input.min_severity,
            max_severity: input.max_severity,
            starts_at,
            ends_at: input.ends_at,
            reason,
            created_by: by,
            created_at: now,
            updated_at: now,
            ended_by: None,
        };
        self.store.create_silence(&silence).await?;

        Ok(silence)
    }

    /// Moves the end of a silence that has not ended to a later time.
    pub async fn extend(&self, id: SilenceId, ends_at: Timestamp) -> DomainResult<Silence> {
        let old = self.get(id).await?;
        let now = self.clock.now();
        if old.state(now) == SilenceState::Expired {
            return Err(DomainError::Conflict("the silence has ended; create a new one".into()));
        }

        if ends_at.0 <= old.ends_at.0 {
            return Err(DomainError::Invalid(
                "ends_at: not later than the current end; to stop a silence, end it".into(),
            ));
        }

        check_window(old.starts_at, ends_at, now)?;
        let window = SilenceWindow { starts_at: old.starts_at, ends_at, ended_by: None, updated_at: now };
        if !self.store.set_silence_window(id, &window, now).await? {
            return Err(DomainError::Conflict("the silence has ended; create a new one".into()));
        }

        self.get(id).await
    }

    /// Ends a silence now. A scheduled one never takes effect. The row
    /// stays: deliveries it held back name it.
    pub async fn end(&self, id: SilenceId, by: Principal) -> DomainResult<Silence> {
        let old = self.get(id).await?;
        let now = self.clock.now();
        let window = SilenceWindow {
            starts_at: Timestamp(old.starts_at.0.min(now.0)),
            ends_at: now,
            ended_by: Some(by),
            updated_at: now,
        };

        if !self.store.set_silence_window(id, &window, now).await? {
            return Err(DomainError::Conflict("the silence has already ended".into()));
        }

        self.get(id).await
    }

    /// Where each alarm's resource sits. A resource that no longer exists
    /// has no place: cluster and environment silences do not cover its alarm,
    /// so a message about it is sent rather than lost.
    async fn places(&self, alarms: &[&Alarm]) -> DomainResult<Vec<AlarmPlace>> {
        let resources: Vec<EntityRef> = alarms.iter().map(|a| a.resource.clone()).collect();
        let mut places = Vec::with_capacity(alarms.len());

        for resolved in self.resolver.resolve_many(&resources).await {
            places.push(match resolved {
                Ok(r) => AlarmPlace { ancestors: r.ancestors },
                Err(DomainError::NotFound(_) | DomainError::Invalid(_)) => AlarmPlace::default(),
                Err(e) => return Err(e),
            });
        }

        Ok(places)
    }

    /// For each alarm, the active silence that covers it now: the one that
    /// ends last when several do.
    pub async fn covering(&self, alarms: &[&Alarm], now: Timestamp) -> DomainResult<Vec<Option<Silence>>> {
        let mut active: HashMap<WorkspaceId, Vec<Silence>> = HashMap::new();

        for a in alarms {
            if let std::collections::hash_map::Entry::Vacant(v) = active.entry(a.workspace_id) {
                v.insert(self.store.active_silences(a.workspace_id, now).await?);
            }
        }

        if active.values().all(Vec::is_empty) {
            return Ok(vec![None; alarms.len()]);
        }

        // Ancestry is read only when a silence asks where a resource sits.
        let places = if active.values().flatten().any(|s| needs_place(&s.scope)) {
            self.places(alarms).await?
        } else {
            vec![AlarmPlace::default(); alarms.len()]
        };

        Ok(alarms
            .iter()
            .zip(&places)
            .map(|(alarm, place)| {
                active
                    .get(&alarm.workspace_id)
                    .into_iter()
                    .flatten()
                    .filter(|s| silence_matches(s, alarm, place))
                    .max_by_key(|s| (s.ends_at, s.id))
                    .cloned()
            })
            .collect())
    }

    /// The active silence that covers one alarm now.
    pub async fn covering_one(&self, alarm: &Alarm, now: Timestamp) -> DomainResult<Option<Silence>> {
        Ok(self.covering(&[alarm], now).await?.pop().flatten())
    }

    /// What to show on each alarm: the silence that holds a firing alarm's
    /// notifications back now. A closed alarm shows none.
    pub async fn marks(&self, alarms: &[Alarm]) -> DomainResult<Vec<Option<SilenceMark>>> {
        let firing: Vec<&Alarm> = alarms.iter().filter(|a| a.state == AlarmState::Firing).collect();
        let mut covering = self.covering(&firing, self.clock.now()).await?.into_iter();

        Ok(alarms
            .iter()
            .map(|a| {
                if a.state == AlarmState::Firing {
                    covering.next().flatten().as_ref().map(SilenceMark::from)
                } else {
                    None
                }
            })
            .collect())
    }

    /// Deletes silences that ended before `before` and that no delivery
    /// names any more.
    pub async fn prune(&self, before: Timestamp) -> DomainResult<u64> {
        self.store.prune_silences(before).await
    }
}

#[cfg(test)]
mod tests;
