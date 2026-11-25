//! Container settings of object stores and Forge's records of RustFS access
//! keys (D-170). Both go with their store (`delete_object_store`).

use async_trait::async_trait;
use forge_core::storage_iam::{KeyGrant, StoreKey, StoreKeyRepository};
use forge_core::storage_settings::{RustfsSettings, StoreSettingsRepository};
use forge_core::*;
use sqlx::any::AnyRow;

use crate::row::*;
use crate::{Store, db_err};

fn key_row(r: &AnyRow) -> DomainResult<StoreKey> {
    let grants: Option<Vec<KeyGrant>> =
        opt_text(r, "grants")?.map(|g| serde_json::from_str(&g)).transpose().map_err(DomainError::internal)?;
    Ok(StoreKey {
        id: parsed(r, "id")?,
        store_id: parsed(r, "store_id")?,
        access_key: text(r, "access_key")?,
        name: text(r, "name")?,
        grants,
        vault_item_id: opt_parsed(r, "vault_item_id")?,
        created_by: text(r, "created_by")?,
        created_at: ts(r, "created_at")?,
        updated_at: ts(r, "updated_at")?,
        synced_at: opt_ts(r, "synced_at")?,
    })
}

#[async_trait]
impl StoreSettingsRepository for Store {
    async fn get_store_settings(&self, store: ObjectStoreId) -> DomainResult<Option<RustfsSettings>> {
        let r = sqlx::query("SELECT settings FROM object_store_settings WHERE store_id = $1")
            .bind(store.to_string())
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.map(|r| text(&r, "settings").and_then(|s| serde_json::from_str(&s).map_err(DomainError::internal)))
            .transpose()
    }

    async fn put_store_settings(&self, store: ObjectStoreId, s: &RustfsSettings, at: Timestamp) -> DomainResult<()> {
        sqlx::query(
            "INSERT INTO object_store_settings (store_id, settings, updated_at) VALUES ($1, $2, $3)
             ON CONFLICT (store_id) DO UPDATE SET settings = $2, updated_at = $3",
        )
        .bind(store.to_string())
        .bind(serde_json::to_string(s).map_err(DomainError::internal)?)
        .bind(at.0)
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }
}

#[async_trait]
impl StoreKeyRepository for Store {
    async fn put_store_key(&self, k: &StoreKey) -> DomainResult<()> {
        let grants = k.grants.as_ref().map(serde_json::to_string).transpose().map_err(DomainError::internal)?;
        sqlx::query(
            "INSERT INTO object_store_keys (id, store_id, access_key, name, grants, vault_item_id, created_by,
                                            created_at, updated_at, synced_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
             ON CONFLICT (id) DO UPDATE SET access_key = $3, name = $4, grants = $5, vault_item_id = $6,
                                            updated_at = $9, synced_at = $10",
        )
        .bind(k.id.to_string())
        .bind(k.store_id.to_string())
        .bind(&k.access_key)
        .bind(&k.name)
        .bind(grants)
        .bind(k.vault_item_id.map(|v| v.to_string()))
        .bind(&k.created_by)
        .bind(k.created_at.0)
        .bind(k.updated_at.0)
        .bind(k.synced_at.map(|t| t.0))
        .execute(self.pool())
        .await
        .map_err(db_err)?;

        Ok(())
    }

    async fn list_store_keys(&self, store: ObjectStoreId) -> DomainResult<Vec<StoreKey>> {
        let rows = sqlx::query("SELECT * FROM object_store_keys WHERE store_id = $1 ORDER BY name, access_key")
            .bind(store.to_string())
            .fetch_all(self.pool())
            .await
            .map_err(db_err)?;
        rows.iter().map(key_row).collect()
    }

    async fn delete_store_key(&self, id: StoreKeyId) -> DomainResult<()> {
        sqlx::query("DELETE FROM object_store_keys WHERE id = $1")
            .bind(id.to_string())
            .execute(self.pool())
            .await
            .map_err(db_err)?;
        Ok(())
    }
}
