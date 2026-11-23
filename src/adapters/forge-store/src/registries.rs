use async_trait::async_trait;
use forge_core::registries::{Registry, RegistryRepository};
use forge_core::*;
use sqlx::any::AnyRow;

use crate::row::*;
use crate::{Store, db_err};

fn registry_row(r: &AnyRow) -> DomainResult<Registry> {
    Ok(Registry {
        id: parsed(r, "id")?,
        workspace_id: parsed(r, "workspace_id")?,
        name: text(r, "name")?,
        kind: text(r, "kind")?.parse()?,
        host: text(r, "host")?,
        username: text(r, "username")?,
        secret_sealed: text(r, "secret_sealed")?,
        allow_push: flag(r, "allow_push")?,
        created_by: parsed(r, "created_by")?,
        created_at: ts(r, "created_at")?,
        updated_at: ts(r, "updated_at")?,
    })
}

#[async_trait]
impl RegistryRepository for Store {
    async fn list_registries(&self, ws: WorkspaceId) -> DomainResult<Vec<Registry>> {
        let rows = sqlx::query("SELECT * FROM registries WHERE workspace_id = $1 ORDER BY name")
            .bind(ws.to_string())
            .fetch_all(self.pool())
            .await
            .map_err(db_err)?;
        rows.iter().map(registry_row).collect()
    }

    async fn get_registry(&self, id: RegistryId) -> DomainResult<Option<Registry>> {
        let r = sqlx::query("SELECT * FROM registries WHERE id = $1")
            .bind(id.to_string())
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(registry_row).transpose()
    }

    async fn create_registry(&self, r: &Registry) -> DomainResult<()> {
        sqlx::query(
            "INSERT INTO registries (id, workspace_id, name, kind, host, username, secret_sealed, allow_push, created_by, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
        )
        .bind(r.id.to_string())
        .bind(r.workspace_id.to_string())
        .bind(&r.name)
        .bind(r.kind.as_str())
        .bind(&r.host)
        .bind(&r.username)
        .bind(&r.secret_sealed)
        .bind(i64::from(r.allow_push))
        .bind(r.created_by.to_string())
        .bind(r.created_at.0)
        .bind(r.updated_at.0)
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn update_registry(&self, r: &Registry) -> DomainResult<()> {
        let done = sqlx::query(
            "UPDATE registries SET name = $1, kind = $2, host = $3, username = $4, secret_sealed = $5, allow_push = $6,
                 updated_at = $7 WHERE id = $8",
        )
        .bind(&r.name)
        .bind(r.kind.as_str())
        .bind(&r.host)
        .bind(&r.username)
        .bind(&r.secret_sealed)
        .bind(i64::from(r.allow_push))
        .bind(r.updated_at.0)
        .bind(r.id.to_string())
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        // A row deleted meanwhile is not silently "updated".
        if done.rows_affected() == 1 { Ok(()) } else { Err(DomainError::NotFound("registry not found".into())) }
    }

    async fn delete_registry(&self, id: RegistryId) -> DomainResult<()> {
        sqlx::query("DELETE FROM registries WHERE id = $1")
            .bind(id.to_string())
            .execute(self.pool())
            .await
            .map_err(db_err)?;
        Ok(())
    }
}
