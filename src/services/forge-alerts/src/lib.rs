//! Alert evaluation (#11, D-087).
//!
//! The [`Evaluator`] runs one pass at a time (the server calls it every few
//! seconds): for each enabled rule it reads the current evidence of the
//! rule's subject from a [`SeriesSource`], keeps only series of the rule's
//! workspace and scope (resolved through `ResourceResolver`, per D-085: the
//! observation data has no workspace of its own), feeds each series'
//! `forge_rules::Tracker`, and turns the tracker's events into durable alarm
//! episodes. Trackers are saved after every pass, so a restart keeps the
//! timers.
//!
//! The evaluator only opens and closes alarms. It never sends anything and
//! never starts a remediation: notifications read alarm transitions later,
//! so a failed message can never undo or repeat what happened here.

pub mod catalog;
pub mod notify;
pub mod silence;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use forge_core::alerts::{
    Alarm, AlarmEvent, AlarmState, AlertRepository, AlertRule, AlertStateRepository, RuleScope, fingerprint,
};
use forge_core::ports::ResourceResolver;
use forge_core::{
    AlarmId, AlertRuleId, DomainError, DomainResult, EntityKind, EntityRef, ServerId, Timestamp, WorkspaceId,
};
use forge_rules::{Event, RuleSpec, Sample, Tracker};
use serde_json::json;

/// The current evidence of one series.
#[derive(Debug, Clone, PartialEq)]
pub struct SeriesSample {
    pub resource: EntityRef,
    /// The node a service instance lives on.
    pub server: Option<ServerId>,
    pub sample: Sample,
}

/// Where the evidence comes from (observations, adapted by the server).
#[async_trait]
pub trait SeriesSource: Send + Sync + std::fmt::Debug {
    /// The latest evidence of every series with this subject.
    async fn latest(&self, subject: &str) -> DomainResult<Vec<SeriesSample>>;
}

/// What one pass did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PassReport {
    pub series: usize,
    pub fired: usize,
    pub resolved: usize,
    pub ended: usize,
    /// Rules skipped because their spec does not parse or validate.
    pub invalid_rules: usize,
}

/// Parses and checks a rule's spec.
pub fn parse_spec(spec: &serde_json::Value) -> Result<RuleSpec, String> {
    let s: RuleSpec = serde_json::from_value(spec.clone()).map_err(|e| format!("invalid rule condition: {e}"))?;
    s.validate()?;

    Ok(s)
}

/// Whether a resource is in a rule's scope.
#[must_use]
pub fn in_scope(scope: &RuleScope, resource: &EntityRef) -> bool {
    match scope {
        RuleScope::Servers { ids } => {
            resource.kind == EntityKind::Server && (ids.is_empty() || ids.iter().any(|i| i.to_string() == resource.id))
        }
        RuleScope::Services { ids } => {
            resource.kind.as_str() == "service" && (ids.is_empty() || ids.contains(&resource.id))
        }
        RuleScope::Installations { ids } => {
            resource.kind == EntityKind::Installation && (ids.is_empty() || ids.contains(&resource.id))
        }
        RuleScope::Certificates { ids } => {
            resource.kind == EntityKind::Certificate && (ids.is_empty() || ids.contains(&resource.id))
        }
    }
}

/// The rule id at the start of a fingerprint.
fn rule_of(fp: &str) -> Option<AlertRuleId> {
    fp.split('|').next()?.parse().ok()
}

/// The resource in a fingerprint (`rule|kind:id|node|subject`).
fn resource_of(fp: &str) -> Option<EntityRef> {
    let (kind, id) = fp.split('|').nth(1)?.split_once(':')?;
    Some(EntityRef { kind: kind.parse().ok()?, id: id.to_owned() })
}

#[derive(Debug, Clone)]
pub struct Evaluator {
    pub alerts: Arc<dyn AlertRepository>,
    pub state: Arc<dyn AlertStateRepository>,
    pub source: Arc<dyn SeriesSource>,
    pub resolver: Arc<dyn ResourceResolver>,
}

impl Evaluator {
    /// One evaluation pass at `now`.
    pub async fn pass(&self, now: Timestamp) -> DomainResult<PassReport> {
        let mut report = PassReport::default();
        let rules: Vec<(AlertRule, RuleSpec)> = self
            .alerts
            .list_rules(None)
            .await?
            .into_iter()
            .filter_map(|r| match parse_spec(&r.spec) {
                Ok(s) => Some((r, s)),
                Err(e) => {
                    report.invalid_rules += 1;
                    tracing::warn!(rule = %r.id, error = %e, "alert rule skipped");
                    None
                }
            })
            .collect();
        let mut trackers: HashMap<String, (i64, Tracker)> = HashMap::new();

        for (fp, rev, v) in self.state.trackers().await? {
            if let Ok(t) = serde_json::from_value::<Tracker>(v) {
                trackers.insert(fp, (rev, t));
            }
        }

        // Evidence per subject, with each resource's workspace.
        let subjects: HashSet<&str> =
            rules.iter().filter(|(r, _)| r.enabled).map(|(r, _)| r.subject.as_str()).collect();
        let mut series: HashMap<&str, Vec<(SeriesSample, WorkspaceId)>> = HashMap::new();

        for subject in subjects {
            let samples = self.source.latest(subject).await?;
            let resources: Vec<EntityRef> = samples.iter().map(|s| s.resource.clone()).collect();
            let resolved = self.resolver.resolve_many(&resources).await;
            let mut kept = Vec::new();

            for (s, r) in samples.into_iter().zip(resolved) {
                // Deleted or unresolvable resources are dropped (D-085).
                let Ok(r) = r else { continue };
                let ws = r.ancestors.iter().find(|a| a.kind == EntityKind::Workspace).and_then(|a| a.id.parse().ok());
                if let Some(ws) = ws {
                    kept.push((s, ws));
                }
            }

            series.insert(subject, kept);
        }

        let mut seen: HashSet<String> = HashSet::new();
        let mut save: Vec<(String, AlertRuleId, i64, serde_json::Value)> = Vec::new();

        for (rule, spec) in rules.iter().filter(|(r, _)| r.enabled) {
            for (s, ws) in series.get(rule.subject.as_str()).into_iter().flatten() {
                if *ws != rule.workspace_id || !in_scope(&rule.scope, &s.resource) {
                    continue;
                }

                report.series += 1;
                let fp = fingerprint(rule.id, &s.resource, s.server, &rule.subject);
                seen.insert(fp.clone());

                let mut tracker = match trackers.remove(&fp) {
                    Some((rev, t)) if rev == rule.revision => t,
                    Some(_) => {
                        // The rule changed: never mix the old condition's
                        // history with the new one.
                        report.ended += self.end_active(&fp, now, "the rule changed").await?;
                        Tracker::new()
                    }
                    None => Tracker::new(),
                };

                let events = match tracker.observe(spec, s.sample.clone(), now.0) {
                    Ok(ev) => ev,
                    // Not newer than the last sample: nothing new, only time passed.
                    Err(_) => tracker.tick(spec, now.0),
                };

                self.apply(rule, s, &fp, &events, now, &mut report).await?;
                save.push((fp, rule.id, rule.revision, serde_json::to_value(&tracker).map_err(DomainError::internal)?));
            }
        }

        // Series without evidence this pass: time still passes for them.
        let by_id: HashMap<AlertRuleId, &(AlertRule, RuleSpec)> = rules.iter().map(|r| (r.0.id, r)).collect();
        let mut forget = Vec::new();

        for (fp, (rev, mut tracker)) in trackers {
            if seen.contains(&fp) {
                continue;
            }

            let rule = rule_of(&fp).and_then(|id| by_id.get(&id));
            let reason = match rule {
                None => Some("the rule was deleted"),
                Some((r, _)) if !r.enabled => Some("the rule was disabled"),
                Some((r, _)) if r.revision != rev => Some("the rule changed"),
                Some(_) => match resource_of(&fp) {
                    Some(res) if matches!(self.resolver.resolve(&res).await, Err(DomainError::NotFound(_))) => {
                        Some("the resource was deleted")
                    }
                    _ => None,
                },
            };

            if let Some(why) = reason {
                report.ended += self.end_active(&fp, now, why).await?;
                forget.push(fp);

                continue;
            }

            if let Some((r, spec)) = rule {
                let events = tracker.tick(spec, now.0);

                if let Some(res) = resource_of(&fp) {
                    let server = fp.split('|').nth(2).and_then(|s| s.parse().ok());
                    let fake = SeriesSample {
                        resource: res,
                        server,
                        sample: Sample { at: now.0, valid_until: now.0, value: None },
                    };
                    self.apply(r, &fake, &fp, &events, now, &mut report).await?;
                }

                save.push((fp, r.id, r.revision, serde_json::to_value(&tracker).map_err(DomainError::internal)?));
            }
        }

        if !forget.is_empty() {
            self.state.delete_trackers(&forget).await?;
        }

        if !save.is_empty() {
            self.state.save_trackers(&save, now).await?;
        }

        Ok(report)
    }

    /// Turns tracker events into alarm changes.
    async fn apply(
        &self,
        rule: &AlertRule,
        s: &SeriesSample,
        fp: &str,
        events: &[Event],
        now: Timestamp,
        report: &mut PassReport,
    ) -> DomainResult<()> {
        for e in events {
            match e {
                Event::Fired { value, .. } => {
                    if self.alerts.active_alarm(fp).await?.is_some() {
                        continue;
                    }

                    let alarm = Alarm {
                        id: AlarmId::new(),
                        workspace_id: rule.workspace_id,
                        rule_id: rule.id,
                        rule_revision: rule.revision,
                        title: rule.name.clone(),
                        severity: rule.severity,
                        fingerprint: fp.to_owned(),
                        resource: s.resource.clone(),
                        server: s.server,
                        subject: rule.subject.clone(),
                        state: AlarmState::Firing,
                        fired_value: serde_json::to_value(value).unwrap_or_default(),
                        fired_at: now,
                        resolved_at: None,
                        end_reason: None,
                        acknowledged_by: None,
                        acknowledged_at: None,
                        previous: self.alerts.last_alarm(fp).await?.map(|a| a.id),
                    };
                    let ev = AlarmEvent {
                        alarm_id: alarm.id,
                        at: now,
                        kind: "fired".into(),
                        actor: None,
                        detail: json!({ "value": value, "revision": rule.revision }),
                    };

                    match self.alerts.open_alarm(&alarm, &ev).await {
                        Ok(()) => report.fired += 1,
                        // Another pass opened it first: one active episode.
                        Err(DomainError::Conflict(_)) => {}
                        Err(e) => return Err(e),
                    }
                }
                Event::Resolved { value, .. } => {
                    if let Some(a) = self.alerts.active_alarm(fp).await? {
                        let ev = AlarmEvent {
                            alarm_id: a.id,
                            at: now,
                            kind: "resolved".into(),
                            actor: None,
                            detail: json!({ "value": value }),
                        };

                        if self.alerts.close_alarm(a.id, AlarmState::Resolved, now, None, &ev).await? {
                            report.resolved += 1;
                        }
                    }
                }
                Event::Pending { .. } | Event::PendingCancelled { .. } => {}
            }
        }

        Ok(())
    }

    /// Ends the series' active alarm without recovery evidence. Returns 1
    /// when one was ended.
    async fn end_active(&self, fp: &str, now: Timestamp, why: &str) -> DomainResult<usize> {
        let Some(a) = self.alerts.active_alarm(fp).await? else { return Ok(0) };
        let ev =
            AlarmEvent { alarm_id: a.id, at: now, kind: "ended".into(), actor: None, detail: json!({ "reason": why }) };

        Ok(usize::from(self.alerts.close_alarm(a.id, AlarmState::Ended, now, Some(why), &ev).await?))
    }
}

#[cfg(test)]
mod tests;
