use async_trait::async_trait;
use forge_core::model::ApiKey;
use forge_core::ports::{ApiKeyRepository, SettingsRepository};
use forge_core::*;
use sqlx::any::AnyRow;

use crate::row::*;
use crate::{Store, db_err};

#[async_trait]
impl SettingsRepository for Store {
    async fn all_settings(&self) -> DomainResult<Vec<(String, String)>> {
        let rows =
            sqlx::query("SELECT key, value FROM settings ORDER BY key").fetch_all(self.pool()).await.map_err(db_err)?;
        rows.iter().map(|r| Ok((text(r, "key")?, text(r, "value")?))).collect()
    }

    async fn put_setting(&self, key: &str, value: &str, now: Timestamp) -> DomainResult<()> {
        sqlx::query(
            "INSERT INTO settings (key, value, updated_at) VALUES ($1, $2, $3)
             ON CONFLICT (key) DO UPDATE SET value = $2, updated_at = $3",
        )
        .bind(key)
        .bind(value)
        .bind(now.0)
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn delete_setting(&self, key: &str) -> DomainResult<()> {
        sqlx::query("DELETE FROM settings WHERE key = $1").bind(key).execute(self.pool()).await.map_err(db_err)?;
        Ok(())
    }
}

fn key_row(r: &AnyRow) -> DomainResult<ApiKey> {
    Ok(ApiKey {
        id: text(r, "id")?,
        user_id: parsed(r, "user_id")?,
        name: text(r, "name")?,
        prefix: text(r, "prefix")?,
        secret_hash: text(r, "secret_hash")?,
        expires_at: opt_ts(r, "expires_at")?,
        last_used_at: opt_ts(r, "last_used_at")?,
        created_at: ts(r, "created_at")?,
        elevated: flag(r, "elevated")?,
        restrictions: restrictions(r)?,
    })
}

/// A stored ceiling. Re-checked on the way out like profile actions: a
/// hand-edited row that names an unknown action or scope fails, so the key
/// is refused rather than widened.
fn restrictions(r: &AnyRow) -> DomainResult<Option<Vec<forge_core::permissions::TokenGrant>>> {
    #[derive(serde::Deserialize)]
    struct Stored {
        scope: String,
        actions: Vec<String>,
    }
    let Some(raw) = opt_text(r, "restrictions")? else { return Ok(None) };
    let stored: Vec<Stored> = serde_json::from_str(&raw).map_err(DomainError::internal)?;
    let mut out = Vec::with_capacity(stored.len());

    for s in stored {
        out.push(forge_core::permissions::TokenGrant {
            scope: forge_core::permissions::parse_scope(&s.scope)?,
            actions: forge_core::permissions::load_actions(&s.actions)?,
        });
    }

    Ok(Some(out))
}

fn restrictions_json(k: &ApiKey) -> Option<String> {
    k.restrictions.as_ref().map(|gs| {
        let v: Vec<serde_json::Value> = gs
            .iter()
            .map(|g| {
                serde_json::json!({
                    "scope": g.scope.to_string(),
                    "actions": g.actions.iter().map(|a| a.as_str()).collect::<Vec<_>>(),
                })
            })
            .collect();
        serde_json::Value::Array(v).to_string()
    })
}

#[async_trait]
impl ApiKeyRepository for Store {
    async fn create_api_key(&self, k: &ApiKey) -> DomainResult<()> {
        sqlx::query(
            "INSERT INTO api_keys (id, user_id, name, prefix, secret_hash, expires_at, last_used_at, created_at, elevated,
                                   restrictions)
             VALUES ($1, $2, $3, $4, $5, $6, NULL, $7, $8, $9)",
        )
        .bind(&k.id)
        .bind(k.user_id.to_string())
        .bind(&k.name)
        .bind(&k.prefix)
        .bind(&k.secret_hash)
        .bind(k.expires_at.map(|t| t.0))
        .bind(k.created_at.0)
        .bind(i64::from(k.elevated))
        .bind(restrictions_json(k))
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn api_key_by_hash(&self, hash: &str) -> DomainResult<Option<ApiKey>> {
        let r = sqlx::query("SELECT * FROM api_keys WHERE secret_hash = $1")
            .bind(hash)
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(key_row).transpose()
    }

    async fn list_api_keys(&self, user: UserId) -> DomainResult<Vec<ApiKey>> {
        let rows = sqlx::query("SELECT * FROM api_keys WHERE user_id = $1 ORDER BY created_at DESC")
            .bind(user.to_string())
            .fetch_all(self.pool())
            .await
            .map_err(db_err)?;
        rows.iter().map(key_row).collect()
    }

    async fn delete_api_key(&self, user: UserId, id: &str) -> DomainResult<bool> {
        let r = sqlx::query("DELETE FROM api_keys WHERE id = $1 AND user_id = $2")
            .bind(id)
            .bind(user.to_string())
            .execute(self.pool())
            .await
            .map_err(db_err)?;
        Ok(r.rows_affected() > 0)
    }

    async fn delete_user_api_keys(&self, user: UserId) -> DomainResult<u64> {
        let r = sqlx::query("DELETE FROM api_keys WHERE user_id = $1")
            .bind(user.to_string())
            .execute(self.pool())
            .await
            .map_err(db_err)?;
        Ok(r.rows_affected())
    }

    async fn touch_api_key(&self, id: &str, now: Timestamp) -> DomainResult<()> {
        sqlx::query("UPDATE api_keys SET last_used_at = $1 WHERE id = $2")
            .bind(now.0)
            .bind(id)
            .execute(self.pool())
            .await
            .map_err(db_err)?;
        Ok(())
    }
}
