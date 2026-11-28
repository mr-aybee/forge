use async_trait::async_trait;
use forge_core::model::VaultItem;
use forge_core::ports::VaultRepository;
use forge_core::*;
use sqlx::any::AnyRow;

use crate::row::*;
use crate::{Store, db_err};

fn item_row(r: &AnyRow) -> DomainResult<VaultItem> {
    Ok(VaultItem {
        id: parsed(r, "id")?,
        workspace_id: parsed(r, "workspace_id")?,
        name: text(r, "name")?,
        username: text(r, "username")?,
        url: text(r, "url")?,
        notes: text(r, "notes")?,
        tags: serde_json::from_value(json(r, "tags")?).map_err(DomainError::internal)?,
        links: serde_json::from_value(json(r, "links")?).map_err(DomainError::internal)?,
        sealed: text(r, "sealed")?,
        created_by: text(r, "created_by")?.parse().map_err(DomainError::Internal)?,
        created_at: ts(r, "created_at")?,
        updated_at: ts(r, "updated_at")?,
        rotated_at: ts(r, "rotated_at")?,
    })
}

#[async_trait]
impl VaultRepository for Store {
    async fn put_vault_item(&self, i: &VaultItem) -> DomainResult<()> {
        sqlx::query(
            "INSERT INTO vault_items (id, workspace_id, name, username, url, notes, tags, links, sealed, created_by, created_at, updated_at, rotated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
             ON CONFLICT (id) DO UPDATE SET name = $3, username = $4, url = $5, notes = $6, tags = $7, links = $8,
               sealed = $9, updated_at = $12, rotated_at = $13",
        )
        .bind(i.id.to_string())
        .bind(i.workspace_id.to_string())
        .bind(&i.name)
        .bind(&i.username)
        .bind(&i.url)
        .bind(&i.notes)
        .bind(serde_json::to_string(&i.tags).map_err(DomainError::internal)?)
        .bind(serde_json::to_string(&i.links).map_err(DomainError::internal)?)
        .bind(&i.sealed)
        .bind(i.created_by.to_string())
        .bind(i.created_at.0)
        .bind(i.updated_at.0)
        .bind(i.rotated_at.0)
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn get_vault_item(&self, id: VaultItemId) -> DomainResult<Option<VaultItem>> {
        let r = sqlx::query("SELECT * FROM vault_items WHERE id = $1")
            .bind(id.to_string())
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(item_row).transpose()
    }

    async fn list_vault_items(&self, workspace: WorkspaceId) -> DomainResult<Vec<VaultItem>> {
        let rows = sqlx::query("SELECT * FROM vault_items WHERE workspace_id = $1 ORDER BY name")
            .bind(workspace.to_string())
            .fetch_all(self.pool())
            .await
            .map_err(db_err)?;
        rows.iter().map(item_row).collect()
    }

    async fn delete_vault_item(&self, id: VaultItemId) -> DomainResult<()> {
        sqlx::query("DELETE FROM vault_items WHERE id = $1")
            .bind(id.to_string())
            .execute(self.pool())
            .await
            .map_err(db_err)?;
        Ok(())
    }
}
