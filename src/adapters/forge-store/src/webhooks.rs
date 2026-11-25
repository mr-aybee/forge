use async_trait::async_trait;
use forge_core::webhooks::{DELIVERIES_KEPT, Delivery, Webhook, WebhookRepository};
use forge_core::*;
use sqlx::any::AnyRow;

use crate::row::*;
use crate::{Store, db_err};

fn webhook_row(r: &AnyRow) -> DomainResult<Webhook> {
    Ok(Webhook {
        id: parsed(r, "id")?,
        workspace_id: parsed(r, "workspace_id")?,
        name: text(r, "name")?,
        target: serde_json::from_str(&text(r, "target")?).map_err(DomainError::internal)?,
        token_hash: text(r, "token_hash")?,
        token_hint: text(r, "token_hint")?,
        signature: text(r, "signature")?.parse()?,
        secret_sealed: opt_text(r, "secret_sealed")?,
        tag_pattern: opt_text(r, "tag_pattern")?,
        enabled: flag(r, "enabled")?,
        created_by: parsed(r, "created_by")?,
        created_at: ts(r, "created_at")?,
        updated_at: ts(r, "updated_at")?,
        last_delivery_at: opt_ts(r, "last_delivery_at")?,
    })
}

fn delivery_row(r: &AnyRow) -> DomainResult<Delivery> {
    Ok(Delivery {
        id: parsed(r, "id")?,
        webhook_id: parsed(r, "webhook_id")?,
        at: ts(r, "at")?,
        outcome: text(r, "outcome")?.parse()?,
        reason: text(r, "reason")?,
        job_id: opt_parsed(r, "job_id")?,
        tag: opt_text(r, "tag")?,
        source_ip: text(r, "source_ip")?,
    })
}

#[async_trait]
impl WebhookRepository for Store {
    async fn list_webhooks(&self, ws: WorkspaceId) -> DomainResult<Vec<Webhook>> {
        let rows = sqlx::query("SELECT * FROM webhooks WHERE workspace_id = $1 ORDER BY name")
            .bind(ws.to_string())
            .fetch_all(self.pool())
            .await
            .map_err(db_err)?;
        rows.iter().map(webhook_row).collect()
    }

    async fn get_webhook(&self, id: WebhookId) -> DomainResult<Option<Webhook>> {
        let r = sqlx::query("SELECT * FROM webhooks WHERE id = $1")
            .bind(id.to_string())
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(webhook_row).transpose()
    }

    async fn webhook_by_token_hash(&self, hash: &str) -> DomainResult<Option<Webhook>> {
        let r = sqlx::query("SELECT * FROM webhooks WHERE token_hash = $1")
            .bind(hash)
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(webhook_row).transpose()
    }

    async fn create_webhook(&self, w: &Webhook) -> DomainResult<()> {
        sqlx::query(
            "INSERT INTO webhooks (id, workspace_id, name, target, token_hash, token_hint, signature, secret_sealed,
                 tag_pattern, enabled, created_by, created_at, updated_at, last_delivery_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)",
        )
        .bind(w.id.to_string())
        .bind(w.workspace_id.to_string())
        .bind(&w.name)
        .bind(serde_json::to_string(&w.target).map_err(DomainError::internal)?)
        .bind(&w.token_hash)
        .bind(&w.token_hint)
        .bind(w.signature.as_str())
        .bind(w.secret_sealed.clone())
        .bind(w.tag_pattern.clone())
        .bind(i64::from(w.enabled))
        .bind(w.created_by.to_string())
        .bind(w.created_at.0)
        .bind(w.updated_at.0)
        .bind(w.last_delivery_at.map(|t| t.0))
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn update_webhook(&self, w: &Webhook) -> DomainResult<()> {
        let done = sqlx::query(
            "UPDATE webhooks SET name = $1, token_hash = $2, token_hint = $3, signature = $4, secret_sealed = $5,
                 tag_pattern = $6, enabled = $7, updated_at = $8, last_delivery_at = $9
             WHERE id = $10",
        )
        .bind(&w.name)
        .bind(&w.token_hash)
        .bind(&w.token_hint)
        .bind(w.signature.as_str())
        .bind(w.secret_sealed.clone())
        .bind(w.tag_pattern.clone())
        .bind(i64::from(w.enabled))
        .bind(w.updated_at.0)
        .bind(w.last_delivery_at.map(|t| t.0))
        .bind(w.id.to_string())
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        if done.rows_affected() == 1 { Ok(()) } else { Err(DomainError::NotFound("webhook not found".into())) }
    }

    async fn delete_webhook(&self, id: WebhookId) -> DomainResult<()> {
        // Deliveries go with the hook, in one transaction.
        let mut tx = self.pool().begin().await.map_err(db_err)?;

        for sql in [
            "DELETE FROM webhook_deliveries WHERE webhook_id = $1",
            "DELETE FROM webhook_replay_markers WHERE webhook_id = $1",
            "DELETE FROM webhooks WHERE id = $1",
        ] {
            sqlx::query(sql).bind(id.to_string()).execute(&mut *tx).await.map_err(db_err)?;
        }

        tx.commit().await.map_err(db_err)
    }

    async fn add_delivery(&self, d: &Delivery) -> DomainResult<()> {
        // Insert and trim in one transaction, so the history stays bounded.
        let mut tx = self.pool().begin().await.map_err(db_err)?;
        sqlx::query(
            "INSERT INTO webhook_deliveries (id, webhook_id, at, outcome, reason, job_id, tag, source_ip)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(d.id.to_string())
        .bind(d.webhook_id.to_string())
        .bind(d.at.0)
        .bind(d.outcome.as_str())
        .bind(&d.reason)
        .bind(d.job_id.map(|j| j.to_string()))
        .bind(d.tag.clone())
        .bind(&d.source_ip)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        sqlx::query(
            "DELETE FROM webhook_deliveries WHERE webhook_id = $1 AND id NOT IN
                 (SELECT id FROM webhook_deliveries WHERE webhook_id = $1 ORDER BY at DESC, id DESC LIMIT $2)",
        )
        .bind(d.webhook_id.to_string())
        .bind(i64::from(DELIVERIES_KEPT))
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        sqlx::query("UPDATE webhooks SET last_delivery_at = $1 WHERE id = $2")
            .bind(d.at.0)
            .bind(d.webhook_id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;

        tx.commit().await.map_err(db_err)
    }

    async fn list_deliveries(&self, id: WebhookId, limit: u32) -> DomainResult<Vec<Delivery>> {
        let rows =
            sqlx::query("SELECT * FROM webhook_deliveries WHERE webhook_id = $1 ORDER BY at DESC, id DESC LIMIT $2")
                .bind(id.to_string())
                .bind(i64::from(limit.min(DELIVERIES_KEPT)))
                .fetch_all(self.pool())
                .await
                .map_err(db_err)?;
        rows.iter().map(delivery_row).collect()
    }

    async fn claim_replay_marker(
        &self,
        id: WebhookId,
        marker: &str,
        at: Timestamp,
        forget_before: Timestamp,
    ) -> DomainResult<bool> {
        // Prune and claim in one transaction; the primary key makes the claim
        // atomic across concurrent deliveries and servers.
        let mut tx = self.pool().begin().await.map_err(db_err)?;
        sqlx::query("DELETE FROM webhook_replay_markers WHERE at < $1")
            .bind(forget_before.0)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        let done = sqlx::query(
            "INSERT INTO webhook_replay_markers (webhook_id, marker, at) VALUES ($1, $2, $3)
             ON CONFLICT (webhook_id, marker) DO NOTHING",
        )
        .bind(id.to_string())
        .bind(marker)
        .bind(at.0)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        tx.commit().await.map_err(db_err)?;

        Ok(done.rows_affected() == 1)
    }
}
