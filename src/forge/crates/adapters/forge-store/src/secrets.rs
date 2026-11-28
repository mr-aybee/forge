use async_trait::async_trait;
use forge_core::model::SecretRecord;
use forge_core::ports::SecretRepository;
use forge_core::*;
use sqlx::any::AnyRow;

use crate::row::*;
use crate::{Store, db_err};

fn secret_row(r: &AnyRow) -> DomainResult<SecretRecord> {
    Ok(SecretRecord {
        id: parsed(r, "id")?,
        workspace_id: parsed(r, "workspace_id")?,
        kind: text(r, "kind")?,
        label: text(r, "label")?,
        sealed: text(r, "sealed")?,
        expires_at: opt_ts(r, "expires_at")?,
        created_by: text(r, "created_by")?.parse().map_err(DomainError::Internal)?,
        created_at: ts(r, "created_at")?,
    })
}

#[async_trait]
impl SecretRepository for Store {
    async fn put_secret(&self, s: &SecretRecord) -> DomainResult<()> {
        sqlx::query(
            "INSERT INTO secrets (id, workspace_id, kind, label, sealed, expires_at, created_by, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
             ON CONFLICT (id) DO UPDATE SET sealed = $5, expires_at = $6",
        )
        .bind(s.id.to_string())
        .bind(s.workspace_id.to_string())
        .bind(&s.kind)
        .bind(&s.label)
        .bind(&s.sealed)
        .bind(s.expires_at.map(|t| t.0))
        .bind(s.created_by.to_string())
        .bind(s.created_at.0)
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn get_secret(&self, id: SecretId) -> DomainResult<Option<SecretRecord>> {
        let r = sqlx::query("SELECT * FROM secrets WHERE id = $1")
            .bind(id.to_string())
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(secret_row).transpose()
    }

    async fn list_secrets(&self, workspace: WorkspaceId) -> DomainResult<Vec<SecretRecord>> {
        let rows = sqlx::query("SELECT * FROM secrets WHERE workspace_id = $1 ORDER BY created_at DESC")
            .bind(workspace.to_string())
            .fetch_all(self.pool())
            .await
            .map_err(db_err)?;
        rows.iter().map(secret_row).collect()
    }

    async fn delete_secret(&self, id: SecretId) -> DomainResult<()> {
        sqlx::query("DELETE FROM secrets WHERE id = $1")
            .bind(id.to_string())
            .execute(self.pool())
            .await
            .map_err(db_err)?;
        Ok(())
    }

    async fn purge_expired(&self, now: Timestamp) -> DomainResult<u64> {
        let r = sqlx::query("DELETE FROM secrets WHERE expires_at IS NOT NULL AND expires_at <= $1")
            .bind(now.0)
            .execute(self.pool())
            .await
            .map_err(db_err)?;
        Ok(r.rows_affected())
    }
}
