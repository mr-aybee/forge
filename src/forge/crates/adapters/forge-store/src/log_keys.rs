use async_trait::async_trait;
use forge_core::logs::LogKey;
use forge_core::ports::LogKeyRepository;
use forge_core::*;
use sqlx::any::AnyRow;

use crate::row::*;
use crate::{Store, db_err};

const COLUMNS: &str = "id, service_id, name, prefix, created_by, created_at, last_used_at";

fn key_row(r: &AnyRow) -> DomainResult<LogKey> {
    Ok(LogKey {
        id: parsed(r, "id")?,
        service_id: parsed(r, "service_id")?,
        name: text(r, "name")?,
        prefix: text(r, "prefix")?,
        created_at: ts(r, "created_at")?,
        created_by: opt_parsed(r, "created_by")?,
        last_used_at: opt_ts(r, "last_used_at")?,
    })
}

#[async_trait]
impl LogKeyRepository for Store {
    async fn create_log_key(&self, k: &LogKey, token_hash: &str) -> DomainResult<()> {
        sqlx::query(
            "INSERT INTO log_ingest_keys (id, service_id, name, prefix, token_hash, created_by, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(k.id.to_string())
        .bind(k.service_id.to_string())
        .bind(&k.name)
        .bind(&k.prefix)
        .bind(token_hash)
        .bind(k.created_by.map(|u| u.to_string()))
        .bind(k.created_at.0)
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn list_log_keys(&self, service: ServiceId) -> DomainResult<Vec<LogKey>> {
        let rows = sqlx::query(&format!(
            "SELECT {COLUMNS} FROM log_ingest_keys WHERE service_id = $1 ORDER BY created_at DESC LIMIT 200"
        ))
        .bind(service.to_string())
        .fetch_all(self.pool())
        .await
        .map_err(db_err)?;
        rows.iter().map(key_row).collect()
    }

    async fn get_log_key(&self, id: LogKeyId) -> DomainResult<Option<LogKey>> {
        let r = sqlx::query(&format!("SELECT {COLUMNS} FROM log_ingest_keys WHERE id = $1"))
            .bind(id.to_string())
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(key_row).transpose()
    }

    async fn find_log_key(&self, token_hash: &str) -> DomainResult<Option<LogKey>> {
        let r = sqlx::query(&format!("SELECT {COLUMNS} FROM log_ingest_keys WHERE token_hash = $1"))
            .bind(token_hash)
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(key_row).transpose()
    }

    async fn delete_log_key(&self, id: LogKeyId) -> DomainResult<()> {
        sqlx::query("DELETE FROM log_ingest_keys WHERE id = $1")
            .bind(id.to_string())
            .execute(self.pool())
            .await
            .map_err(db_err)?;
        Ok(())
    }

    async fn touch_log_key(&self, id: LogKeyId, at: Timestamp) -> DomainResult<()> {
        sqlx::query("UPDATE log_ingest_keys SET last_used_at = $2 WHERE id = $1")
            .bind(id.to_string())
            .bind(at.0)
            .execute(self.pool())
            .await
            .map_err(db_err)?;
        Ok(())
    }

    async fn list_log_key_hashes(&self, limit: u32) -> DomainResult<Vec<forge_core::analytics::KeyService>> {
        let rows = sqlx::query("SELECT token_hash, service_id FROM log_ingest_keys ORDER BY id LIMIT $1")
            .bind(i64::from(limit))
            .fetch_all(self.pool())
            .await
            .map_err(db_err)?;
        rows.iter()
            .map(|r| {
                Ok(forge_core::analytics::KeyService {
                    token_hash: text(r, "token_hash")?,
                    service_id: parsed(r, "service_id")?,
                })
            })
            .collect()
    }
}
