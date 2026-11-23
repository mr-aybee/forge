use async_trait::async_trait;
use forge_core::model::Cluster;
use forge_core::ports::ClusterRepository;
use forge_core::*;
use sqlx::Row;
use sqlx::any::AnyRow;

use crate::row::*;
use crate::{Store, db_err};

fn cluster_row(r: &AnyRow) -> DomainResult<Cluster> {
    Ok(Cluster {
        id: parsed(r, "id")?,
        workspace_id: parsed(r, "workspace_id")?,
        name: text(r, "name")?,
        description: text(r, "description")?,
        environment_id: opt_parsed(r, "environment_id")?,
        abbr: text(r, "abbr")?,
        icon: r.try_get::<Option<String>, _>("icon").map_err(db_err)?,
        created_at: ts(r, "created_at")?,
    })
}

#[async_trait]
impl ClusterRepository for Store {
    async fn create_cluster(&self, c: &Cluster) -> DomainResult<()> {
        sqlx::query(
            "INSERT INTO clusters (id, workspace_id, name, description, environment_id, abbr, icon, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(c.id.to_string())
        .bind(c.workspace_id.to_string())
        .bind(&c.name)
        .bind(&c.description)
        .bind(c.environment_id.map(|e| e.to_string()))
        .bind(&c.abbr)
        .bind(c.icon.clone())
        .bind(c.created_at.0)
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn get_cluster(&self, id: ClusterId) -> DomainResult<Cluster> {
        let r = sqlx::query("SELECT * FROM clusters WHERE id = $1")
            .bind(id.to_string())
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(cluster_row).transpose()?.ok_or_else(|| DomainError::NotFound(format!("cluster {id}")))
    }

    async fn list_clusters(&self, workspace: WorkspaceId) -> DomainResult<Vec<Cluster>> {
        let rows = sqlx::query("SELECT * FROM clusters WHERE workspace_id = $1 ORDER BY name")
            .bind(workspace.to_string())
            .fetch_all(self.pool())
            .await
            .map_err(db_err)?;
        rows.iter().map(cluster_row).collect()
    }

    async fn update_cluster(&self, c: &Cluster) -> DomainResult<()> {
        sqlx::query(
            "UPDATE clusters SET name = $1, description = $2, environment_id = $3, abbr = $4, icon = $5 WHERE id = $6",
        )
        .bind(&c.name)
        .bind(&c.description)
        .bind(c.environment_id.map(|e| e.to_string()))
        .bind(&c.abbr)
        .bind(c.icon.clone())
        .bind(c.id.to_string())
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn delete_cluster(&self, id: ClusterId) -> DomainResult<()> {
        let mut tx = self.pool().begin().await.map_err(db_err)?;
        sqlx::query("UPDATE servers SET cluster_id = NULL WHERE cluster_id = $1")
            .bind(id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        sqlx::query("DELETE FROM clusters WHERE id = $1")
            .bind(id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        tx.commit().await.map_err(db_err)
    }

    async fn assign_server(
        &self,
        server: ServerId,
        cluster: Option<ClusterId>,
        environment: Option<EnvironmentId>,
    ) -> DomainResult<()> {
        sqlx::query("UPDATE servers SET cluster_id = $1, environment_id = $2 WHERE id = $3")
            .bind(cluster.map(|c| c.to_string()))
            .bind(environment.map(|e| e.to_string()))
            .bind(server.to_string())
            .execute(self.pool())
            .await
            .map_err(db_err)?;
        Ok(())
    }
}
