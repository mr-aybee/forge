use async_trait::async_trait;
use forge_core::model::ClusterProfileRecord;
use forge_core::ports::ClusterProfileRepository;
use forge_core::*;

use crate::row::*;
use crate::{Store, db_err};

fn profile_row(r: &sqlx::any::AnyRow) -> DomainResult<ClusterProfileRecord> {
    Ok(ClusterProfileRecord {
        cluster_id: parsed(r, "cluster_id")?,
        profile: json(r, "profile")?,
        status: json(r, "status")?,
        updated_at: ts(r, "updated_at")?,
    })
}

#[async_trait]
impl ClusterProfileRepository for Store {
    async fn put_cluster_profile(&self, p: &ClusterProfileRecord) -> DomainResult<()> {
        sqlx::query(
            "INSERT INTO cluster_profiles (cluster_id, profile, status, updated_at) VALUES ($1, $2, $3, $4)
             ON CONFLICT (cluster_id) DO UPDATE SET profile = $2, status = $3, updated_at = $4",
        )
        .bind(p.cluster_id.to_string())
        .bind(p.profile.to_string())
        .bind(p.status.to_string())
        .bind(p.updated_at.0)
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn get_cluster_profile(&self, cluster: ClusterId) -> DomainResult<Option<ClusterProfileRecord>> {
        let r = sqlx::query("SELECT * FROM cluster_profiles WHERE cluster_id = $1")
            .bind(cluster.to_string())
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(profile_row).transpose()
    }

    async fn list_cluster_profiles(&self) -> DomainResult<Vec<ClusterProfileRecord>> {
        let rows = sqlx::query("SELECT * FROM cluster_profiles").fetch_all(self.pool()).await.map_err(db_err)?;
        rows.iter().map(profile_row).collect()
    }

    async fn delete_cluster_profile(&self, cluster: ClusterId) -> DomainResult<()> {
        sqlx::query("DELETE FROM cluster_profiles WHERE cluster_id = $1")
            .bind(cluster.to_string())
            .execute(self.pool())
            .await
            .map_err(db_err)?;
        Ok(())
    }
}
