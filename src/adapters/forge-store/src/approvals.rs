//! Approvals of changes AI connectors asked for (migration 0014).

use async_trait::async_trait;
use forge_core::model::Approval;
use forge_core::ports::ApprovalRepository;
use forge_core::*;
use sqlx::any::AnyRow;

use crate::row::*;
use crate::{Store, db_err};

fn approval_row(r: &AnyRow) -> DomainResult<Approval> {
    let json = |col: &str| -> DomainResult<Option<serde_json::Value>> {
        opt_text(r, col)?.map(|t| serde_json::from_str(&t).map_err(DomainError::internal)).transpose()
    };
    Ok(Approval {
        id: text(r, "id")?,
        kind: text(r, "kind")?,
        user_id: parsed(r, "user_id")?,
        connection_id: opt_text(r, "connection_id")?,
        requester: text(r, "requester")?,
        action: text(r, "action")?,
        request: json("request")?.unwrap_or(serde_json::Value::Null),
        summary: text(r, "summary")?,
        mfa_required: flag(r, "mfa_required")?,
        status: text(r, "status")?,
        created_at: ts(r, "created_at")?,
        expires_at: ts(r, "expires_at")?,
        decided_by: opt_text(r, "decided_by")?.and_then(|s| s.parse().ok()),
        decided_at: opt_ts(r, "decided_at")?,
        result: json("result")?,
        error: opt_text(r, "error")?,
    })
}

#[async_trait]
impl ApprovalRepository for Store {
    async fn create_approval(&self, a: &Approval) -> DomainResult<()> {
        sqlx::query(
            "INSERT INTO approvals (id, kind, user_id, connection_id, requester, action, request, summary, mfa_required,
                                    status, created_at, expires_at, decided_by, decided_at, result, error)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)",
        )
        .bind(&a.id)
        .bind(&a.kind)
        .bind(a.user_id.to_string())
        .bind(a.connection_id.clone())
        .bind(&a.requester)
        .bind(&a.action)
        .bind(a.request.to_string())
        .bind(&a.summary)
        .bind(b(a.mfa_required))
        .bind(&a.status)
        .bind(a.created_at.0)
        .bind(a.expires_at.0)
        .bind(a.decided_by.map(|u| u.to_string()))
        .bind(a.decided_at.map(|t| t.0))
        .bind(a.result.as_ref().map(ToString::to_string))
        .bind(a.error.clone())
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn get_approval(&self, id: &str) -> DomainResult<Option<Approval>> {
        let r = sqlx::query("SELECT * FROM approvals WHERE id = $1")
            .bind(id)
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(approval_row).transpose()
    }

    async fn list_approvals(&self, user: Option<UserId>, limit: u32) -> DomainResult<Vec<Approval>> {
        let rows = match user {
            Some(u) => {
                sqlx::query("SELECT * FROM approvals WHERE user_id = $1 ORDER BY created_at DESC LIMIT $2")
                    .bind(u.to_string())
                    .bind(i64::from(limit))
                    .fetch_all(self.pool())
                    .await
            }
            None => {
                sqlx::query("SELECT * FROM approvals ORDER BY created_at DESC LIMIT $1")
                    .bind(i64::from(limit))
                    .fetch_all(self.pool())
                    .await
            }
        }
        .map_err(db_err)?;

        rows.iter().map(approval_row).collect()
    }

    async fn count_pending(&self, connection_id: &str) -> DomainResult<u64> {
        let r = sqlx::query("SELECT COUNT(*) AS n FROM approvals WHERE connection_id = $1 AND status = 'pending'")
            .bind(connection_id)
            .fetch_one(self.pool())
            .await
            .map_err(db_err)?;
        Ok(u64::try_from(int(&r, "n")?).unwrap_or(0))
    }

    async fn decide_approval(&self, id: &str, status: &str, by: UserId, at: Timestamp) -> DomainResult<bool> {
        let res = sqlx::query(
            "UPDATE approvals SET status = $1, decided_by = $2, decided_at = $3
             WHERE id = $4 AND status = 'pending' AND expires_at > $3",
        )
        .bind(status)
        .bind(by.to_string())
        .bind(at.0)
        .bind(id)
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(res.rows_affected() == 1)
    }

    async fn finish_approval(
        &self,
        id: &str,
        status: &str,
        result: Option<&serde_json::Value>,
        error: Option<&str>,
    ) -> DomainResult<()> {
        sqlx::query("UPDATE approvals SET status = $1, result = $2, error = $3 WHERE id = $4")
            .bind(status)
            .bind(result.map(ToString::to_string))
            .bind(error)
            .bind(id)
            .execute(self.pool())
            .await
            .map_err(db_err)?;
        Ok(())
    }

    async fn sweep_approvals(&self, now: Timestamp, drop_before: Timestamp) -> DomainResult<u64> {
        let expired =
            sqlx::query("UPDATE approvals SET status = 'expired' WHERE status = 'pending' AND expires_at <= $1")
                .bind(now.0)
                .execute(self.pool())
                .await
                .map_err(db_err)?;
        let dropped =
            sqlx::query("DELETE FROM approvals WHERE status <> 'pending' AND status <> 'approved' AND created_at < $1")
                .bind(drop_before.0)
                .execute(self.pool())
                .await
                .map_err(db_err)?;

        Ok(expired.rows_affected() + dropped.rows_affected())
    }

    async fn cancel_pending_approvals(
        &self,
        connection_id: &str,
        by: UserId,
        at: Timestamp,
    ) -> DomainResult<Vec<String>> {
        let rows = sqlx::query("SELECT id FROM approvals WHERE connection_id = $1 AND status = 'pending'")
            .bind(connection_id)
            .fetch_all(self.pool())
            .await
            .map_err(db_err)?;
        let mut cancelled = Vec::new();

        for r in &rows {
            let id = text(r, "id")?;

            // Same guard as deciding: one that was approved meanwhile is left alone.
            let res = sqlx::query(
                "UPDATE approvals SET status = 'cancelled', decided_by = $1, decided_at = $2
                 WHERE id = $3 AND status = 'pending'",
            )
            .bind(by.to_string())
            .bind(at.0)
            .bind(&id)
            .execute(self.pool())
            .await
            .map_err(db_err)?;

            if res.rows_affected() == 1 {
                cancelled.push(id);
            }
        }

        Ok(cancelled)
    }
}
