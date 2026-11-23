use async_trait::async_trait;
use forge_core::model::FabricRecord;
use forge_core::ports::FabricRepository;
use forge_core::*;
use sqlx::any::AnyRow;

use crate::row::*;
use crate::{Store, db_err};

fn fabric_row(r: &AnyRow) -> DomainResult<FabricRecord> {
    Ok(FabricRecord {
        cluster_id: parsed(r, "cluster_id")?,
        spec: json(r, "spec")?,
        pins: json(r, "pins")?,
        leases: json(r, "leases")?,
        status: json(r, "status")?,
        updated_at: ts(r, "updated_at")?,
    })
}

#[async_trait]
impl FabricRepository for Store {
    async fn put_fabric(&self, f: &FabricRecord) -> DomainResult<()> {
        sqlx::query(
            "INSERT INTO cluster_fabrics (cluster_id, spec, pins, leases, status, updated_at) VALUES ($1, $2, $3, $4, $5, $6)
             ON CONFLICT (cluster_id) DO UPDATE SET spec = $2, pins = $3, leases = $4, status = $5, updated_at = $6",
        )
        .bind(f.cluster_id.to_string())
        .bind(f.spec.to_string())
        .bind(f.pins.to_string())
        .bind(f.leases.to_string())
        .bind(f.status.to_string())
        .bind(f.updated_at.0)
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn get_fabric(&self, cluster: ClusterId) -> DomainResult<Option<FabricRecord>> {
        let r = sqlx::query("SELECT * FROM cluster_fabrics WHERE cluster_id = $1")
            .bind(cluster.to_string())
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(fabric_row).transpose()
    }

    async fn list_fabrics(&self, workspace: Option<WorkspaceId>) -> DomainResult<Vec<FabricRecord>> {
        let rows = match workspace {
            Some(ws) => sqlx::query(
                "SELECT f.* FROM cluster_fabrics f JOIN clusters c ON c.id = f.cluster_id WHERE c.workspace_id = $1",
            )
            .bind(ws.to_string())
            .fetch_all(self.pool())
            .await,
            None => sqlx::query("SELECT * FROM cluster_fabrics").fetch_all(self.pool()).await,
        }
        .map_err(db_err)?;
        rows.iter().map(fabric_row).collect()
    }

    async fn delete_fabric(&self, cluster: ClusterId) -> DomainResult<()> {
        sqlx::query("DELETE FROM cluster_fabrics WHERE cluster_id = $1")
            .bind(cluster.to_string())
            .execute(self.pool())
            .await
            .map_err(db_err)?;
        Ok(())
    }
}
