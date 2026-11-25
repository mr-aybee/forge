//! Alert rules, alarms and the evaluator's state (#11, D-087).

use async_trait::async_trait;
use forge_core::alerts::{
    Alarm, AlarmEvent, AlarmFilter, AlarmState, AlertRepository, AlertRule, AlertStateRepository, RuleScope, Severity,
};
use forge_core::*;
use sqlx::any::AnyRow;

use crate::row::*;
use crate::{Store, db_err};

fn severity(s: &str) -> DomainResult<Severity> {
    serde_json::from_value(serde_json::Value::String(s.to_owned())).map_err(DomainError::internal)
}

fn alarm_state(s: &str) -> DomainResult<AlarmState> {
    serde_json::from_value(serde_json::Value::String(s.to_owned())).map_err(DomainError::internal)
}

const fn state_str(s: AlarmState) -> &'static str {
    match s {
        AlarmState::Firing => "firing",
        AlarmState::Resolved => "resolved",
        AlarmState::Ended => "ended",
    }
}

fn rule_row(r: &AnyRow) -> DomainResult<AlertRule> {
    Ok(AlertRule {
        id: parsed(r, "id")?,
        workspace_id: parsed(r, "workspace_id")?,
        name: text(r, "name")?,
        description: text(r, "description")?,
        enabled: flag(r, "enabled")?,
        severity: severity(&text(r, "severity")?)?,
        scope: serde_json::from_value::<RuleScope>(json(r, "scope")?).map_err(DomainError::internal)?,
        subject: text(r, "subject")?,
        spec: json(r, "spec")?,
        revision: int(r, "revision")?,
        created_by: parsed(r, "created_by")?,
        created_at: ts(r, "created_at")?,
        updated_at: ts(r, "updated_at")?,
    })
}

fn alarm_row(r: &AnyRow) -> DomainResult<Alarm> {
    Ok(Alarm {
        id: parsed(r, "id")?,
        workspace_id: parsed(r, "workspace_id")?,
        rule_id: parsed(r, "rule_id")?,
        rule_revision: int(r, "rule_revision")?,
        title: text(r, "title")?,
        severity: severity(&text(r, "severity")?)?,
        fingerprint: text(r, "fingerprint")?,
        resource: EntityRef { kind: parsed(r, "resource_kind")?, id: text(r, "resource_id")? },
        server: opt_parsed(r, "server_id")?,
        subject: text(r, "subject")?,
        state: alarm_state(&text(r, "state")?)?,
        fired_value: json(r, "fired_value")?,
        fired_at: ts(r, "fired_at")?,
        resolved_at: opt_ts(r, "resolved_at")?,
        end_reason: opt_text(r, "end_reason")?,
        acknowledged_by: opt_parsed(r, "acknowledged_by")?,
        acknowledged_at: opt_ts(r, "acknowledged_at")?,
        previous: opt_parsed(r, "previous_id")?,
    })
}

fn event_row(r: &AnyRow) -> DomainResult<AlarmEvent> {
    Ok(AlarmEvent {
        alarm_id: parsed(r, "alarm_id")?,
        at: ts(r, "at")?,
        kind: text(r, "kind")?,
        actor: opt_parsed(r, "actor")?,
        detail: json(r, "detail")?,
    })
}

/// Writes the alarm's transition to the notification outbox (D-110). It
/// runs in the transaction that changes the alarm, so a transition is never
/// lost between the change and its notifications, and never recorded for a
/// change that did not happen.
async fn insert_transition<'e, E>(ex: E, alarm: AlarmId, workspace: &str, kind: &str, at: Timestamp) -> DomainResult<()>
where
    E: sqlx::Executor<'e, Database = sqlx::Any>,
{
    sqlx::query(
        "INSERT INTO alarm_transitions (alarm_id, kind, workspace_id, at, planned_at) VALUES ($1, $2, $3, $4, NULL)",
    )
    .bind(alarm.to_string())
    .bind(kind.to_owned())
    .bind(workspace.to_owned())
    .bind(at.0)
    .execute(ex)
    .await
    .map_err(db_err)?;
    Ok(())
}

async fn insert_event<'e, E>(ex: E, e: &AlarmEvent) -> DomainResult<()>
where
    E: sqlx::Executor<'e, Database = sqlx::Any>,
{
    sqlx::query("INSERT INTO alarm_events (alarm_id, at, kind, actor, detail) VALUES ($1, $2, $3, $4, $5)")
        .bind(e.alarm_id.to_string())
        .bind(e.at.0)
        .bind(e.kind.clone())
        .bind(e.actor.as_ref().map(ToString::to_string))
        .bind(e.detail.to_string())
        .execute(ex)
        .await
        .map_err(db_err)?;
    Ok(())
}

#[async_trait]
impl AlertRepository for Store {
    async fn create_rule(&self, r: &AlertRule) -> DomainResult<()> {
        sqlx::query(
            "INSERT INTO alert_rules (id, workspace_id, name, description, enabled, severity, scope, subject, spec,
             revision, created_by, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
        )
        .bind(r.id.to_string())
        .bind(r.workspace_id.to_string())
        .bind(r.name.clone())
        .bind(r.description.clone())
        .bind(i64::from(r.enabled))
        .bind(r.severity.as_str())
        .bind(serde_json::to_string(&r.scope).map_err(DomainError::internal)?)
        .bind(r.subject.clone())
        .bind(r.spec.to_string())
        .bind(r.revision)
        .bind(r.created_by.to_string())
        .bind(r.created_at.0)
        .bind(r.updated_at.0)
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn update_rule(&self, r: &AlertRule) -> DomainResult<()> {
        let done = sqlx::query(
            "UPDATE alert_rules SET name = $2, description = $3, enabled = $4, severity = $5, scope = $6, subject = $7,
             spec = $8, revision = $9, updated_at = $10 WHERE id = $1",
        )
        .bind(r.id.to_string())
        .bind(r.name.clone())
        .bind(r.description.clone())
        .bind(i64::from(r.enabled))
        .bind(r.severity.as_str())
        .bind(serde_json::to_string(&r.scope).map_err(DomainError::internal)?)
        .bind(r.subject.clone())
        .bind(r.spec.to_string())
        .bind(r.revision)
        .bind(r.updated_at.0)
        .execute(self.pool())
        .await
        .map_err(db_err)?;

        if done.rows_affected() == 0 {
            return Err(DomainError::NotFound(format!("alert rule {}", r.id)));
        }

        Ok(())
    }

    async fn delete_rule(&self, id: AlertRuleId) -> DomainResult<()> {
        let done = sqlx::query("DELETE FROM alert_rules WHERE id = $1")
            .bind(id.to_string())
            .execute(self.pool())
            .await
            .map_err(db_err)?;

        if done.rows_affected() == 0 {
            return Err(DomainError::NotFound(format!("alert rule {id}")));
        }

        Ok(())
    }

    async fn get_rule(&self, id: AlertRuleId) -> DomainResult<AlertRule> {
        let r = sqlx::query("SELECT * FROM alert_rules WHERE id = $1")
            .bind(id.to_string())
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(rule_row).transpose()?.ok_or_else(|| DomainError::NotFound(format!("alert rule {id}")))
    }

    async fn list_rules(&self, workspace: Option<WorkspaceId>) -> DomainResult<Vec<AlertRule>> {
        let rows = match workspace {
            Some(w) => {
                sqlx::query("SELECT * FROM alert_rules WHERE workspace_id = $1 ORDER BY name")
                    .bind(w.to_string())
                    .fetch_all(self.pool())
                    .await
            }
            None => sqlx::query("SELECT * FROM alert_rules ORDER BY name").fetch_all(self.pool()).await,
        }
        .map_err(db_err)?;

        rows.iter().map(rule_row).collect()
    }

    async fn open_alarm(&self, a: &Alarm, event: &AlarmEvent) -> DomainResult<()> {
        let mut tx = self.pool().begin().await.map_err(db_err)?;
        sqlx::query(
            "INSERT INTO alarms (id, workspace_id, rule_id, rule_revision, title, severity, fingerprint, resource_kind,
             resource_id, server_id, subject, state, fired_value, fired_at, resolved_at, end_reason, acknowledged_by,
             acknowledged_at, previous_id)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, NULL, NULL, NULL, NULL, $15)",
        )
        .bind(a.id.to_string())
        .bind(a.workspace_id.to_string())
        .bind(a.rule_id.to_string())
        .bind(a.rule_revision)
        .bind(a.title.clone())
        .bind(a.severity.as_str())
        .bind(a.fingerprint.clone())
        .bind(a.resource.kind.as_str())
        .bind(a.resource.id.clone())
        .bind(a.server.map(|s| s.to_string()))
        .bind(a.subject.clone())
        .bind(state_str(AlarmState::Firing))
        .bind(a.fired_value.to_string())
        .bind(a.fired_at.0)
        .bind(a.previous.map(|p| p.to_string()))
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        insert_event(&mut *tx, event).await?;
        insert_transition(&mut *tx, a.id, &a.workspace_id.to_string(), "fired", a.fired_at).await?;
        tx.commit().await.map_err(db_err)?;

        Ok(())
    }

    async fn close_alarm(
        &self,
        id: AlarmId,
        state: AlarmState,
        at: Timestamp,
        reason: Option<&str>,
        event: &AlarmEvent,
    ) -> DomainResult<bool> {
        if state == AlarmState::Firing {
            return Err(DomainError::Invalid("an alarm closes as resolved or ended".into()));
        }

        let mut tx = self.pool().begin().await.map_err(db_err)?;
        let done = sqlx::query(
            "UPDATE alarms SET state = $2, resolved_at = $3, end_reason = $4 WHERE id = $1 AND state = 'firing'",
        )
        .bind(id.to_string())
        .bind(state_str(state))
        .bind(at.0)
        .bind(reason.map(str::to_owned))
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;

        if done.rows_affected() == 0 {
            return Ok(false);
        }

        insert_event(&mut *tx, event).await?;
        let row = sqlx::query("SELECT workspace_id FROM alarms WHERE id = $1")
            .bind(id.to_string())
            .fetch_one(&mut *tx)
            .await
            .map_err(db_err)?;
        insert_transition(&mut *tx, id, &text(&row, "workspace_id")?, state_str(state), at).await?;
        tx.commit().await.map_err(db_err)?;

        Ok(true)
    }

    async fn acknowledge(
        &self,
        id: AlarmId,
        by: Option<&Principal>,
        at: Timestamp,
        event: &AlarmEvent,
    ) -> DomainResult<()> {
        let mut tx = self.pool().begin().await.map_err(db_err)?;
        let done = sqlx::query("UPDATE alarms SET acknowledged_by = $2, acknowledged_at = $3 WHERE id = $1")
            .bind(id.to_string())
            .bind(by.map(ToString::to_string))
            .bind(by.map(|_| at.0))
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;

        if done.rows_affected() == 0 {
            return Err(DomainError::NotFound(format!("alarm {id}")));
        }

        insert_event(&mut *tx, event).await?;
        tx.commit().await.map_err(db_err)?;

        Ok(())
    }

    async fn add_alarm_event(&self, event: &AlarmEvent) -> DomainResult<()> {
        insert_event(self.pool(), event).await
    }

    async fn get_alarm(&self, id: AlarmId) -> DomainResult<Alarm> {
        let r = sqlx::query("SELECT * FROM alarms WHERE id = $1")
            .bind(id.to_string())
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(alarm_row).transpose()?.ok_or_else(|| DomainError::NotFound(format!("alarm {id}")))
    }

    async fn list_alarms(&self, f: &AlarmFilter, limit: u32) -> DomainResult<Vec<Alarm>> {
        // Filters are few and bounded; build the WHERE clause with numbered
        // binds only (no values in the SQL text).
        let mut sql = String::from("SELECT * FROM alarms WHERE 1 = 1");
        enum B {
            S(String),
            I(i64),
        }
        let mut binds: Vec<B> = Vec::new();

        if f.active {
            sql.push_str(" AND state = 'firing'");
        }

        if let Some(r) = f.rule {
            binds.push(B::S(r.to_string()));
            sql.push_str(&format!(" AND rule_id = ${}", binds.len()));
        }

        if let Some(res) = &f.resource {
            binds.push(B::S(res.kind.as_str().to_owned()));
            sql.push_str(&format!(" AND resource_kind = ${}", binds.len()));
            binds.push(B::S(res.id.clone()));
            sql.push_str(&format!(" AND resource_id = ${}", binds.len()));
        }

        if let Some(ws) = &f.workspaces {
            if ws.is_empty() {
                return Ok(Vec::new());
            }

            let mut marks = Vec::new();

            for w in ws.iter().take(500) {
                binds.push(B::S(w.to_string()));
                marks.push(format!("${}", binds.len()));
            }

            sql.push_str(&format!(" AND workspace_id IN ({})", marks.join(", ")));
        }

        if let Some((at, id)) = f.before {
            binds.push(B::I(at.0));
            let t = binds.len();
            binds.push(B::S(id.to_string()));
            sql.push_str(&format!(" AND (fired_at < ${t} OR (fired_at = ${t} AND id < ${}))", binds.len()));
        }

        sql.push_str(&format!(" ORDER BY fired_at DESC, id DESC LIMIT {}", limit.clamp(1, 1000)));
        let mut q = sqlx::query(&sql);

        for b in binds {
            q = match b {
                B::S(s) => q.bind(s),
                B::I(i) => q.bind(i),
            };
        }

        let rows = q.fetch_all(self.pool()).await.map_err(db_err)?;

        rows.iter().map(alarm_row).collect()
    }

    async fn active_alarm(&self, fingerprint: &str) -> DomainResult<Option<Alarm>> {
        let r = sqlx::query("SELECT * FROM alarms WHERE fingerprint = $1 AND state = 'firing'")
            .bind(fingerprint.to_owned())
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(alarm_row).transpose()
    }

    async fn last_alarm(&self, fingerprint: &str) -> DomainResult<Option<Alarm>> {
        let r = sqlx::query("SELECT * FROM alarms WHERE fingerprint = $1 ORDER BY fired_at DESC LIMIT 1")
            .bind(fingerprint.to_owned())
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(alarm_row).transpose()
    }

    async fn alarm_events(&self, id: AlarmId) -> DomainResult<Vec<AlarmEvent>> {
        let rows = sqlx::query("SELECT * FROM alarm_events WHERE alarm_id = $1 ORDER BY at")
            .bind(id.to_string())
            .fetch_all(self.pool())
            .await
            .map_err(db_err)?;
        rows.iter().map(event_row).collect()
    }
}

#[async_trait]
impl AlertStateRepository for Store {
    async fn trackers(&self) -> DomainResult<Vec<(String, i64, serde_json::Value)>> {
        let rows = sqlx::query("SELECT fingerprint, rule_revision, state FROM alert_trackers")
            .fetch_all(self.pool())
            .await
            .map_err(db_err)?;
        rows.iter().map(|r| Ok((text(r, "fingerprint")?, int(r, "rule_revision")?, json(r, "state")?))).collect()
    }

    async fn save_trackers(
        &self,
        rows: &[(String, AlertRuleId, i64, serde_json::Value)],
        at: Timestamp,
    ) -> DomainResult<()> {
        let mut tx = self.pool().begin().await.map_err(db_err)?;

        for (fp, rule, rev, state) in rows {
            sqlx::query(
                "INSERT INTO alert_trackers (fingerprint, rule_id, rule_revision, state, updated_at) VALUES ($1, $2, $3, $4, $5)
                 ON CONFLICT (fingerprint) DO UPDATE SET rule_id = $2, rule_revision = $3, state = $4, updated_at = $5",
            )
            .bind(fp.clone())
            .bind(rule.to_string())
            .bind(*rev)
            .bind(state.to_string())
            .bind(at.0)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        }

        tx.commit().await.map_err(db_err)?;

        Ok(())
    }

    async fn delete_trackers(&self, fingerprints: &[String]) -> DomainResult<()> {
        for fp in fingerprints {
            sqlx::query("DELETE FROM alert_trackers WHERE fingerprint = $1")
                .bind(fp.clone())
                .execute(self.pool())
                .await
                .map_err(db_err)?;
        }

        Ok(())
    }

    async fn cursor(&self, name: &str) -> DomainResult<i64> {
        let r = sqlx::query("SELECT seq FROM alert_cursors WHERE name = $1")
            .bind(name.to_owned())
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map_or(Ok(0), |r| int(r, "seq"))
    }

    async fn set_cursor(&self, name: &str, seq: i64) -> DomainResult<()> {
        sqlx::query("INSERT INTO alert_cursors (name, seq) VALUES ($1, $2) ON CONFLICT (name) DO UPDATE SET seq = $2")
            .bind(name.to_owned())
            .bind(seq)
            .execute(self.pool())
            .await
            .map_err(db_err)?;
        Ok(())
    }
}
