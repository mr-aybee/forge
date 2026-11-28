use async_trait::async_trait;
use forge_core::model::Site;
use forge_core::ports::SiteRepository;
use forge_core::*;
use sqlx::any::AnyRow;

use crate::row::*;
use crate::{Store, db_err};

fn site_row(r: &AnyRow) -> DomainResult<Site> {
    Ok(Site {
        id: text(r, "id")?,
        workspace_id: parsed(r, "workspace_id")?,
        name: text(r, "name")?,
        config: json(r, "config")?,
        edge_status: json(r, "edge_status")?,
        created_at: ts(r, "created_at")?,
        updated_at: ts(r, "updated_at")?,
    })
}

impl Store {
    /// The sites of one workspace, by name.
    pub async fn list_workspace_sites(&self, ws: WorkspaceId) -> DomainResult<Vec<Site>> {
        let rows = sqlx::query("SELECT * FROM sites WHERE workspace_id = $1 ORDER BY name")
            .bind(ws.to_string())
            .fetch_all(self.pool())
            .await
            .map_err(db_err)?;
        rows.iter().map(site_row).collect()
    }
}

#[async_trait]
impl SiteRepository for Store {
    async fn put_site(&self, s: &Site) -> DomainResult<()> {
        sqlx::query(
            "INSERT INTO sites (id, workspace_id, name, config, edge_status, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             ON CONFLICT (id) DO UPDATE SET config = $4, edge_status = $5, updated_at = $7",
        )
        .bind(&s.id)
        .bind(s.workspace_id.to_string())
        .bind(&s.name)
        .bind(s.config.to_string())
        .bind(s.edge_status.to_string())
        .bind(s.created_at.0)
        .bind(s.updated_at.0)
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn get_site(&self, id: &str) -> DomainResult<Option<Site>> {
        let r = sqlx::query("SELECT * FROM sites WHERE id = $1")
            .bind(id)
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(site_row).transpose()
    }

    async fn list_sites(&self) -> DomainResult<Vec<Site>> {
        let rows = sqlx::query("SELECT * FROM sites ORDER BY name").fetch_all(self.pool()).await.map_err(db_err)?;
        rows.iter().map(site_row).collect()
    }

    async fn delete_site(&self, id: &str) -> DomainResult<()> {
        sqlx::query("DELETE FROM sites WHERE id = $1").bind(id).execute(self.pool()).await.map_err(db_err)?;
        Ok(())
    }
}
