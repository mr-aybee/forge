//! Notification channels, policies, the alarm transition outbox and
//! deliveries (#11, D-110).

use async_trait::async_trait;
use forge_core::notifications::{
    AlarmTransition, AttemptResult, Channel, Delivery, DeliveryAttempt, DeliveryFilter, DeliveryRecipients,
    DeliveryStatus, NotificationPolicy, NotificationRepository, PolicyEmailRecipients,
};
use forge_core::*;
use serde::de::DeserializeOwned;
use sqlx::any::AnyRow;

use crate::row::*;
use crate::{Store, db_err};

/// A column holding a serde enum's name.
fn named<T: DeserializeOwned>(r: &AnyRow, col: &str) -> DomainResult<T> {
    serde_json::from_value(serde_json::Value::String(text(r, col)?))
        .map_err(|e| DomainError::Internal(format!("column {col}: {e}")))
}

fn opt_named<T: DeserializeOwned>(r: &AnyRow, col: &str) -> DomainResult<Option<T>> {
    opt_text(r, col)?
        .map(|s| {
            serde_json::from_value(serde_json::Value::String(s))
                .map_err(|e| DomainError::Internal(format!("column {col}: {e}")))
        })
        .transpose()
}

/// A column holding JSON of a known type.
fn typed<T: DeserializeOwned>(r: &AnyRow, col: &str) -> DomainResult<T> {
    serde_json::from_value(json(r, col)?).map_err(|e| DomainError::Internal(format!("column {col}: {e}")))
}

fn to_json<T: serde::Serialize>(v: &T) -> DomainResult<String> {
    serde_json::to_string(v).map_err(DomainError::internal)
}

fn channel_row(r: &AnyRow) -> DomainResult<Channel> {
    Ok(Channel {
        id: parsed(r, "id")?,
        workspace_id: parsed(r, "workspace_id")?,
        name: text(r, "name")?,
        kind: named(r, "kind")?,
        enabled: flag(r, "enabled")?,
        target: text(r, "target")?,
        sealed_url: text(r, "sealed_url")?,
        email: opt_text(r, "email")?
            .map(|j| serde_json::from_str(&j).map_err(|e| DomainError::Internal(format!("column email: {e}"))))
            .transpose()?,
        secret_item: opt_parsed(r, "secret_item")?,
        created_by: parsed(r, "created_by")?,
        created_at: ts(r, "created_at")?,
        updated_at: ts(r, "updated_at")?,
    })
}

/// A nullable column holding JSON of a known type.
fn opt_typed<T: DeserializeOwned>(r: &AnyRow, col: &str) -> DomainResult<Option<T>> {
    opt_text(r, col)?
        .map(|j| serde_json::from_str(&j).map_err(|e| DomainError::Internal(format!("column {col}: {e}"))))
        .transpose()
}

fn policy_row(r: &AnyRow) -> DomainResult<NotificationPolicy> {
    // NULL: the policy emails nobody by user or role.
    let email: PolicyEmailRecipients = opt_typed(r, "email_recipients")?.unwrap_or_default();
    Ok(NotificationPolicy {
        id: parsed(r, "id")?,
        workspace_id: parsed(r, "workspace_id")?,
        name: text(r, "name")?,
        enabled: flag(r, "enabled")?,
        min_severity: named(r, "min_severity")?,
        rules: typed(r, "rules")?,
        scope: typed(r, "scope")?,
        events: typed(r, "events")?,
        repeat_interval_secs: opt_int(r, "repeat_interval_secs")?.map(|v| u32::try_from(v).unwrap_or(u32::MAX)),
        channels: typed(r, "channels")?,
        email_channel: email.channel,
        email_users: email.users,
        email_roles: email.roles,
        created_by: parsed(r, "created_by")?,
        created_at: ts(r, "created_at")?,
        updated_at: ts(r, "updated_at")?,
    })
}

fn delivery_row(r: &AnyRow) -> DomainResult<Delivery> {
    let resource = match (opt_text(r, "resource_kind")?, opt_text(r, "resource_id")?) {
        (Some(kind), Some(id)) => Some(EntityRef {
            kind: kind.parse().map_err(|e| DomainError::Internal(format!("column resource_kind: {e}")))?,
            id,
        }),
        _ => None,
    };

    Ok(Delivery {
        id: parsed(r, "id")?,
        workspace_id: parsed(r, "workspace_id")?,
        alarm_id: opt_parsed(r, "alarm_id")?,
        event: named(r, "event")?,
        occurrence: int(r, "occurrence")?,
        policy_id: opt_parsed(r, "policy_id")?,
        channel_id: parsed(r, "channel_id")?,
        dedupe_key: text(r, "dedupe_key")?,
        status: named(r, "status")?,
        attempts: u32_col(r, "attempts")?,
        next_attempt_at: ts(r, "next_attempt_at")?,
        lease_until: opt_ts(r, "lease_until")?,
        last_error: opt_named(r, "last_error")?,
        last_http_status: opt_int(r, "last_http_status")?.and_then(|v| u16::try_from(v).ok()),
        detail: opt_text(r, "detail")?,
        silence_id: opt_parsed(r, "silence_id")?,
        after_silence: flag(r, "after_silence")?,
        released_at: opt_ts(r, "released_at")?,
        created_at: ts(r, "created_at")?,
        finished_at: opt_ts(r, "finished_at")?,
        channel_name: text(r, "channel_name")?,
        policy_name: text(r, "policy_name")?,
        alarm_title: text(r, "alarm_title")?,
        resource,
        recipients: opt_typed::<DeliveryRecipients>(r, "recipients")?,
    })
}

fn attempt_row(r: &AnyRow) -> DomainResult<DeliveryAttempt> {
    Ok(DeliveryAttempt {
        delivery_id: parsed(r, "delivery_id")?,
        number: u32_col(r, "number")?,
        at: ts(r, "at")?,
        ok: flag(r, "ok")?,
        http_status: opt_int(r, "http_status")?.and_then(|v| u16::try_from(v).ok()),
        error: opt_named(r, "error")?,
        duration_ms: int(r, "duration_ms")?,
    })
}

/// Inserts a delivery unless its dedupe key exists. `true` when added. A
/// delivery is added pending, or `cancelled` with its `detail` (a delivery
/// to people that resolved to nobody).
async fn insert_delivery<'e, E>(ex: E, d: &Delivery) -> DomainResult<bool>
where
    E: sqlx::Executor<'e, Database = sqlx::Any>,
{
    let cancelled = d.status == DeliveryStatus::Cancelled;
    let status = if cancelled { DeliveryStatus::Cancelled } else { DeliveryStatus::Pending };
    let done = sqlx::query(
        "INSERT INTO notification_deliveries (id, workspace_id, alarm_id, event, occurrence, policy_id, channel_id,
         dedupe_key, status, attempts, next_attempt_at, lease_until, last_error, last_http_status, detail, created_at,
         finished_at, channel_name, policy_name, alarm_title, resource_kind, resource_id, silence_id, after_silence,
         recipients)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 0, $10, NULL, NULL, NULL, $19, $11, $20, $12, $13, $14, $15, $16,
         $17, $18, $21)
         ON CONFLICT (dedupe_key) DO NOTHING",
    )
    .bind(d.id.to_string())
    .bind(d.workspace_id.to_string())
    .bind(d.alarm_id.map(|a| a.to_string()))
    .bind(d.event.as_str())
    .bind(d.occurrence)
    .bind(d.policy_id.map(|p| p.to_string()))
    .bind(d.channel_id.to_string())
    .bind(d.dedupe_key.clone())
    .bind(status.as_str())
    .bind(d.next_attempt_at.0)
    .bind(d.created_at.0)
    .bind(d.channel_name.clone())
    .bind(d.policy_name.clone())
    .bind(d.alarm_title.clone())
    .bind(d.resource.as_ref().map(|r| r.kind.as_str().to_owned()))
    .bind(d.resource.as_ref().map(|r| r.id.clone()))
    .bind(d.silence_id.map(|s| s.to_string()))
    .bind(b(d.after_silence))
    .bind(d.detail.clone().filter(|_| cancelled))
    .bind(cancelled.then_some(d.finished_at.unwrap_or(d.created_at).0))
    .bind(d.recipients.as_ref().map(to_json).transpose()?)
    .execute(ex)
    .await
    .map_err(db_err)?;

    Ok(done.rows_affected() > 0)
}

#[async_trait]
impl NotificationRepository for Store {
    async fn put_channel(&self, c: &Channel) -> DomainResult<()> {
        sqlx::query(
            "INSERT INTO notification_channels (id, workspace_id, name, kind, enabled, target, sealed_url, secret_item,
             created_by, created_at, updated_at, email) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
             ON CONFLICT (id) DO UPDATE SET name = $3, kind = $4, enabled = $5, target = $6, sealed_url = $7,
             secret_item = $8, updated_at = $11, email = $12",
        )
        .bind(c.id.to_string())
        .bind(c.workspace_id.to_string())
        .bind(c.name.clone())
        .bind(c.kind.as_str())
        .bind(b(c.enabled))
        .bind(c.target.clone())
        .bind(c.sealed_url.clone())
        .bind(c.secret_item.map(|i| i.to_string()))
        .bind(c.created_by.to_string())
        .bind(c.created_at.0)
        .bind(c.updated_at.0)
        .bind(c.email.as_ref().map(to_json).transpose()?)
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn get_channel(&self, id: ChannelId) -> DomainResult<Option<Channel>> {
        let r = sqlx::query("SELECT * FROM notification_channels WHERE id = $1")
            .bind(id.to_string())
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(channel_row).transpose()
    }

    async fn list_channels(&self, workspace: WorkspaceId) -> DomainResult<Vec<Channel>> {
        let rows = sqlx::query("SELECT * FROM notification_channels WHERE workspace_id = $1 ORDER BY name, id")
            .bind(workspace.to_string())
            .fetch_all(self.pool())
            .await
            .map_err(db_err)?;
        rows.iter().map(channel_row).collect()
    }

    async fn delete_channel(&self, id: ChannelId) -> DomainResult<()> {
        let done = sqlx::query("DELETE FROM notification_channels WHERE id = $1")
            .bind(id.to_string())
            .execute(self.pool())
            .await
            .map_err(db_err)?;

        if done.rows_affected() == 0 {
            return Err(DomainError::NotFound(format!("notification channel {id}")));
        }

        Ok(())
    }

    async fn put_policy(&self, p: &NotificationPolicy) -> DomainResult<()> {
        sqlx::query(
            "INSERT INTO notification_policies (id, workspace_id, name, enabled, min_severity, rules, scope, events,
             repeat_interval_secs, channels, created_by, created_at, updated_at, email_recipients)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
             ON CONFLICT (id) DO UPDATE SET name = $3, enabled = $4, min_severity = $5, rules = $6, scope = $7,
             events = $8, repeat_interval_secs = $9, channels = $10, updated_at = $13, email_recipients = $14",
        )
        .bind(p.id.to_string())
        .bind(p.workspace_id.to_string())
        .bind(p.name.clone())
        .bind(b(p.enabled))
        .bind(p.min_severity.as_str())
        .bind(to_json(&p.rules)?)
        .bind(to_json(&p.scope)?)
        .bind(to_json(&p.events)?)
        .bind(p.repeat_interval_secs.map(i64::from))
        .bind(to_json(&p.channels)?)
        .bind(p.created_by.to_string())
        .bind(p.created_at.0)
        .bind(p.updated_at.0)
        .bind(
            p.emails_people()
                .then(|| PolicyEmailRecipients {
                    channel: p.email_channel,
                    users: p.email_users.clone(),
                    roles: p.email_roles.clone(),
                })
                .as_ref()
                .map(to_json)
                .transpose()?,
        )
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn get_policy(&self, id: NotificationPolicyId) -> DomainResult<Option<NotificationPolicy>> {
        let r = sqlx::query("SELECT * FROM notification_policies WHERE id = $1")
            .bind(id.to_string())
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(policy_row).transpose()
    }

    async fn list_policies(&self, workspace: Option<WorkspaceId>) -> DomainResult<Vec<NotificationPolicy>> {
        let rows = match workspace {
            Some(w) => {
                sqlx::query("SELECT * FROM notification_policies WHERE workspace_id = $1 ORDER BY name, id")
                    .bind(w.to_string())
                    .fetch_all(self.pool())
                    .await
            }
            None => sqlx::query("SELECT * FROM notification_policies ORDER BY name, id").fetch_all(self.pool()).await,
        }
        .map_err(db_err)?;

        rows.iter().map(policy_row).collect()
    }

    async fn delete_policy(&self, id: NotificationPolicyId) -> DomainResult<()> {
        let done = sqlx::query("DELETE FROM notification_policies WHERE id = $1")
            .bind(id.to_string())
            .execute(self.pool())
            .await
            .map_err(db_err)?;

        if done.rows_affected() == 0 {
            return Err(DomainError::NotFound(format!("notification policy {id}")));
        }

        Ok(())
    }

    async fn unplanned_transitions(&self, limit: u32) -> DomainResult<Vec<AlarmTransition>> {
        let rows = sqlx::query(&format!(
            "SELECT alarm_id, kind, workspace_id, at FROM alarm_transitions WHERE planned_at IS NULL
             ORDER BY at, alarm_id LIMIT {}",
            limit.clamp(1, 1000)
        ))
        .fetch_all(self.pool())
        .await
        .map_err(db_err)?;
        rows.iter()
            .map(|r| {
                Ok(AlarmTransition {
                    alarm_id: parsed(r, "alarm_id")?,
                    workspace_id: parsed(r, "workspace_id")?,
                    kind: named(r, "kind")?,
                    at: ts(r, "at")?,
                })
            })
            .collect()
    }

    async fn plan_transition(
        &self,
        t: &AlarmTransition,
        deliveries: &[Delivery],
        at: Timestamp,
    ) -> DomainResult<usize> {
        // Atomic: the deliveries exist exactly when the transition is marked
        // planned. A second call finds it planned and adds nothing.
        let mut tx = self.pool().begin().await.map_err(db_err)?;
        let marked = sqlx::query(
            "UPDATE alarm_transitions SET planned_at = $3 WHERE alarm_id = $1 AND kind = $2 AND planned_at IS NULL",
        )
        .bind(t.alarm_id.to_string())
        .bind(t.kind.as_str())
        .bind(at.0)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;

        if marked.rows_affected() == 0 {
            return Ok(0);
        }

        let mut added = 0;

        for d in deliveries {
            added += usize::from(insert_delivery(&mut *tx, d).await?);
        }

        tx.commit().await.map_err(db_err)?;

        Ok(added)
    }

    async fn fail_transition(
        &self,
        t: &AlarmTransition,
        limit: u32,
        reason: &str,
        at: Timestamp,
    ) -> DomainResult<bool> {
        // Atomic: the count and the decision to give up are one step.
        let mut tx = self.pool().begin().await.map_err(db_err)?;
        sqlx::query(
            "UPDATE alarm_transitions SET plan_failures = plan_failures + 1, plan_error = $3
             WHERE alarm_id = $1 AND kind = $2 AND planned_at IS NULL",
        )
        .bind(t.alarm_id.to_string())
        .bind(t.kind.as_str())
        .bind(reason.to_owned())
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        let given_up = sqlx::query(
            "UPDATE alarm_transitions SET planned_at = $3
             WHERE alarm_id = $1 AND kind = $2 AND planned_at IS NULL AND plan_failures >= $4",
        )
        .bind(t.alarm_id.to_string())
        .bind(t.kind.as_str())
        .bind(at.0)
        .bind(i64::from(limit))
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        tx.commit().await.map_err(db_err)?;

        Ok(given_up.rows_affected() > 0)
    }

    async fn add_deliveries(&self, deliveries: &[Delivery]) -> DomainResult<usize> {
        let mut tx = self.pool().begin().await.map_err(db_err)?;
        let mut added = 0;

        for d in deliveries {
            added += usize::from(insert_delivery(&mut *tx, d).await?);
        }

        tx.commit().await.map_err(db_err)?;

        Ok(added)
    }

    async fn due_deliveries(&self, now: Timestamp, per_channel: u32, limit: u32) -> DomainResult<Vec<Delivery>> {
        // Ranked per channel, so a channel with a long backlog takes at most
        // `per_channel` places and the others are still seen.
        let rows = sqlx::query(&format!(
            "SELECT * FROM (
                 SELECT d.*, ROW_NUMBER() OVER (
                     PARTITION BY d.channel_id ORDER BY d.next_attempt_at, d.created_at, d.id
                 ) AS lane_position
                 FROM notification_deliveries d
                 WHERE (d.status = 'pending' AND d.next_attempt_at <= $1)
                    OR (d.status = 'sending' AND d.lease_until <= $1)
             ) due
             WHERE lane_position <= $2
             ORDER BY next_attempt_at, created_at, id LIMIT {}",
            limit.clamp(1, 1000)
        ))
        .bind(now.0)
        .bind(i64::from(per_channel.max(1)))
        .fetch_all(self.pool())
        .await
        .map_err(db_err)?;
        rows.iter().map(delivery_row).collect()
    }

    async fn claim_delivery(&self, id: DeliveryId, now: Timestamp, lease_until: Timestamp) -> DomainResult<bool> {
        let done = sqlx::query(
            "UPDATE notification_deliveries SET status = 'sending', attempts = attempts + 1, lease_until = $3
             WHERE id = $1
               AND ((status = 'pending' AND next_attempt_at <= $2) OR (status = 'sending' AND lease_until <= $2))",
        )
        .bind(id.to_string())
        .bind(now.0)
        .bind(lease_until.0)
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(done.rows_affected() > 0)
    }

    async fn finish_attempt(&self, a: &DeliveryAttempt, result: AttemptResult) -> DomainResult<bool> {
        let (status, next, finished) = match result {
            AttemptResult::Sent => (DeliveryStatus::Sent, None, Some(a.at.0)),
            AttemptResult::Retry(t) => (DeliveryStatus::Pending, Some(t.0), None),
            AttemptResult::Failed => (DeliveryStatus::Failed, None, Some(a.at.0)),
        };

        // Atomic: the attempt row and the delivery's state change together.
        // Fenced on the attempt number: an attempt that reports after its
        // lease ran out and another attempt started changes nothing.
        let mut tx = self.pool().begin().await.map_err(db_err)?;
        let held = sqlx::query(
            "UPDATE notification_deliveries SET status = $2, next_attempt_at = COALESCE($3, next_attempt_at),
             finished_at = $4, last_error = $5, last_http_status = $6, lease_until = NULL
             WHERE id = $1 AND status = 'sending' AND attempts = $7",
        )
        .bind(a.delivery_id.to_string())
        .bind(status.as_str())
        .bind(next)
        .bind(finished)
        .bind(a.error.map(|e| e.as_str().to_owned()))
        .bind(a.http_status.map(i64::from))
        .bind(i64::from(a.number))
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;

        if held.rows_affected() == 0 {
            return Ok(false);
        }

        sqlx::query(
            "INSERT INTO notification_attempts (delivery_id, number, at, ok, http_status, error, duration_ms)
             VALUES ($1, $2, $3, $4, $5, $6, $7) ON CONFLICT (delivery_id, number) DO NOTHING",
        )
        .bind(a.delivery_id.to_string())
        .bind(i64::from(a.number))
        .bind(a.at.0)
        .bind(b(a.ok))
        .bind(a.http_status.map(i64::from))
        .bind(a.error.map(|e| e.as_str().to_owned()))
        .bind(a.duration_ms)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        tx.commit().await.map_err(db_err)?;

        Ok(true)
    }

    async fn postpone_delivery(&self, id: DeliveryId, until: Timestamp) -> DomainResult<()> {
        sqlx::query("UPDATE notification_deliveries SET next_attempt_at = $2 WHERE id = $1 AND status = 'pending'")
            .bind(id.to_string())
            .bind(until.0)
            .execute(self.pool())
            .await
            .map_err(db_err)?;
        Ok(())
    }

    async fn cancel_delivery(&self, id: DeliveryId, why: &str, at: Timestamp) -> DomainResult<()> {
        sqlx::query(
            "UPDATE notification_deliveries SET status = 'cancelled', detail = $2, finished_at = $3, lease_until = NULL
             WHERE id = $1 AND status IN ('pending', 'sending')",
        )
        .bind(id.to_string())
        .bind(why.to_owned())
        .bind(at.0)
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn suppress_delivery(
        &self,
        id: DeliveryId,
        silence: SilenceId,
        why: &str,
        at: Timestamp,
    ) -> DomainResult<()> {
        sqlx::query(
            "UPDATE notification_deliveries SET status = 'suppressed', silence_id = $2, detail = $3, finished_at = $4,
             lease_until = NULL, released_at = NULL
             WHERE id = $1 AND status IN ('pending', 'sending')",
        )
        .bind(id.to_string())
        .bind(silence.to_string())
        .bind(why.to_owned())
        .bind(at.0)
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn ended_suppressions(&self, now: Timestamp, limit: u32) -> DomainResult<Vec<Delivery>> {
        let rows = sqlx::query(&format!(
            "SELECT d.* FROM notification_deliveries d
             WHERE d.status = 'suppressed' AND d.released_at IS NULL
               AND NOT EXISTS (SELECT 1 FROM silences s WHERE s.id = d.silence_id AND s.ends_at > $1)
             ORDER BY d.created_at, d.id LIMIT {}",
            limit.clamp(1, 1000)
        ))
        .bind(now.0)
        .fetch_all(self.pool())
        .await
        .map_err(db_err)?;
        rows.iter().map(delivery_row).collect()
    }

    async fn unsettled_suppressions(&self, alarm: AlarmId) -> DomainResult<Vec<Delivery>> {
        // Bounded by what one alarm can plan: its transitions and reminders
        // per policy and channel.
        let rows = sqlx::query(
            "SELECT * FROM notification_deliveries
             WHERE alarm_id = $1 AND status = 'suppressed' AND released_at IS NULL
             ORDER BY created_at, id LIMIT 1000",
        )
        .bind(alarm.to_string())
        .fetch_all(self.pool())
        .await
        .map_err(db_err)?;
        rows.iter().map(delivery_row).collect()
    }

    async fn hold_suppressed(&self, ids: &[DeliveryId], silence: SilenceId, why: &str) -> DomainResult<()> {
        // Atomic: the deliveries of one alarm move together.
        let mut tx = self.pool().begin().await.map_err(db_err)?;

        for id in ids {
            sqlx::query(
                "UPDATE notification_deliveries SET silence_id = $2, detail = $3
                 WHERE id = $1 AND status = 'suppressed' AND released_at IS NULL",
            )
            .bind(id.to_string())
            .bind(silence.to_string())
            .bind(why.to_owned())
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        }

        tx.commit().await.map_err(db_err)?;

        Ok(())
    }

    async fn release_suppressed(
        &self,
        ids: &[DeliveryId],
        follow_ups: &[Delivery],
        at: Timestamp,
    ) -> DomainResult<usize> {
        // Atomic: the follow-up messages exist exactly when the suppressed
        // deliveries are settled. A second call settles nothing and
        // therefore adds nothing.
        let mut tx = self.pool().begin().await.map_err(db_err)?;
        let mut settled = 0;

        for id in ids {
            let done = sqlx::query(
                "UPDATE notification_deliveries SET released_at = $2
                 WHERE id = $1 AND status = 'suppressed' AND released_at IS NULL",
            )
            .bind(id.to_string())
            .bind(at.0)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
            settled += done.rows_affected();
        }

        if settled == 0 {
            return Ok(0);
        }

        let mut added = 0;

        for d in follow_ups {
            added += usize::from(insert_delivery(&mut *tx, d).await?);
        }

        tx.commit().await.map_err(db_err)?;

        Ok(added)
    }

    async fn get_delivery(&self, id: DeliveryId) -> DomainResult<Option<Delivery>> {
        let r = sqlx::query("SELECT * FROM notification_deliveries WHERE id = $1")
            .bind(id.to_string())
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(delivery_row).transpose()
    }

    async fn list_deliveries(&self, f: &DeliveryFilter, limit: u32) -> DomainResult<Vec<Delivery>> {
        // Numbered binds only; no values in the SQL text.
        let mut sql = String::from("SELECT * FROM notification_deliveries WHERE 1 = 1");
        enum B {
            S(String),
            I(i64),
        }
        let mut binds: Vec<B> = Vec::new();
        let mut text_filter = |sql: &mut String, col: &str, v: Option<String>| {
            if let Some(v) = v {
                binds.push(B::S(v));
                sql.push_str(&format!(" AND {col} = ${}", binds.len()));
            }
        };
        text_filter(&mut sql, "workspace_id", f.workspace.map(|w| w.to_string()));
        text_filter(&mut sql, "alarm_id", f.alarm.map(|a| a.to_string()));
        text_filter(&mut sql, "channel_id", f.channel.map(|c| c.to_string()));
        text_filter(&mut sql, "status", f.status.map(|s| s.as_str().to_owned()));

        if let Some((at, id)) = f.before {
            binds.push(B::I(at.0));
            let t = binds.len();
            binds.push(B::S(id.to_string()));
            sql.push_str(&format!(" AND (created_at < ${t} OR (created_at = ${t} AND id < ${}))", binds.len()));
        }

        sql.push_str(&format!(" ORDER BY created_at DESC, id DESC LIMIT {}", limit.clamp(1, 1000)));
        let mut q = sqlx::query(&sql);

        for bind in binds {
            q = match bind {
                B::S(s) => q.bind(s),
                B::I(i) => q.bind(i),
            };
        }

        let rows = q.fetch_all(self.pool()).await.map_err(db_err)?;

        rows.iter().map(delivery_row).collect()
    }

    async fn delivery_attempts(&self, id: DeliveryId) -> DomainResult<Vec<DeliveryAttempt>> {
        let rows = sqlx::query("SELECT * FROM notification_attempts WHERE delivery_id = $1 ORDER BY number")
            .bind(id.to_string())
            .fetch_all(self.pool())
            .await
            .map_err(db_err)?;
        rows.iter().map(attempt_row).collect()
    }

    async fn prune_deliveries(&self, before: Timestamp) -> DomainResult<u64> {
        // A suppressed delivery is kept until it was settled after its
        // silence: the message that follows the silence is planned from it.
        const OLD: &str = "created_at < $1 AND (status IN ('sent', 'failed', 'cancelled')
             OR (status = 'suppressed' AND released_at IS NOT NULL))";

        // Atomic: attempts never outlive their delivery.
        let mut tx = self.pool().begin().await.map_err(db_err)?;
        sqlx::query(&format!(
            "DELETE FROM notification_attempts WHERE delivery_id IN (SELECT id FROM notification_deliveries WHERE {OLD})"
        ))
        .bind(before.0)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        let done = sqlx::query(&format!("DELETE FROM notification_deliveries WHERE {OLD}"))
            .bind(before.0)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        sqlx::query("DELETE FROM alarm_transitions WHERE planned_at IS NOT NULL AND at < $1")
            .bind(before.0)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        tx.commit().await.map_err(db_err)?;

        Ok(done.rows_affected())
    }
}
