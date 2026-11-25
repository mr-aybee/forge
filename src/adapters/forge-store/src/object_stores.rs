use async_trait::async_trait;
use forge_core::storage::{ObjectStore, ObjectStoreRepository, StoreMember};
use forge_core::*;
use sqlx::any::AnyRow;

use crate::row::*;
use crate::{Store, db_err};

fn port(v: i64) -> DomainResult<u16> {
    u16::try_from(v).map_err(DomainError::internal)
}

fn store_row(r: &AnyRow) -> DomainResult<ObjectStore> {
    Ok(ObjectStore {
        id: parsed(r, "id")?,
        workspace_id: parsed(r, "workspace_id")?,
        environment_id: opt_parsed(r, "environment_id")?,
        name: text(r, "name")?,
        slug: text(r, "slug")?,
        provider: text(r, "provider")?,
        mode: text(r, "mode")?,
        region: text(r, "region")?,
        path_style: flag(r, "path_style")?,
        version: text(r, "version")?,
        deployed_version: opt_text(r, "deployed_version")?,
        runtime: text(r, "runtime")?,
        topology: text(r, "topology")?,
        s3_port: port(int(r, "s3_port")?)?,
        console_port: opt_int(r, "console_port")?.map(port).transpose()?,
        endpoint: text(r, "endpoint")?,
        credential_id: parsed(r, "credential_id")?,
        status: text(r, "status")?,
        last_error: opt_text(r, "last_error")?,
        last_job_id: opt_parsed(r, "last_job_id")?,
        created_by: text(r, "created_by")?.parse().map_err(DomainError::Internal)?,
        created_at: ts(r, "created_at")?,
        updated_at: ts(r, "updated_at")?,
    })
}

fn member_row(r: &AnyRow) -> DomainResult<StoreMember> {
    let bytes = |c: &str| opt_int(r, c).map(|v| v.and_then(|n| u64::try_from(n).ok()));
    Ok(StoreMember {
        store_id: parsed(r, "store_id")?,
        server_id: parsed(r, "server_id")?,
        data_paths: serde_json::from_value(json(r, "data_paths")?).map_err(DomainError::internal)?,
        endpoint_host: text(r, "endpoint_host")?,
        require_mount: flag(r, "require_mount")?,
        status: text(r, "status")?,
        capacity_bytes: bytes("capacity_bytes")?,
        free_bytes: bytes("free_bytes")?,
        dedicated: opt_int(r, "dedicated")?.map(|v| v != 0),
        observed_at: opt_ts(r, "observed_at")?,
    })
}

fn signed(v: Option<u64>) -> Option<i64> {
    v.map(|n| i64::try_from(n).unwrap_or(i64::MAX))
}

#[async_trait]
impl ObjectStoreRepository for Store {
    async fn put_object_store(&self, s: &ObjectStore) -> DomainResult<()> {
        sqlx::query(
            "INSERT INTO object_stores (id, workspace_id, name, slug, provider, version, deployed_version, runtime,
                                        topology, s3_port, console_port, endpoint, credential_id, status, last_error,
                                        last_job_id, created_by, created_at, updated_at, mode, region, path_style,
                                        environment_id)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21,
                     $22, $23)
             ON CONFLICT (id) DO UPDATE SET name = $3, version = $6, deployed_version = $7, s3_port = $10,
                                            console_port = $11, endpoint = $12, credential_id = $13, status = $14,
                                            last_error = $15, last_job_id = $16, updated_at = $19, region = $21,
                                            path_style = $22",
        )
        .bind(s.id.to_string())
        .bind(s.workspace_id.to_string())
        .bind(&s.name)
        .bind(&s.slug)
        .bind(&s.provider)
        .bind(&s.version)
        .bind(s.deployed_version.clone())
        .bind(&s.runtime)
        .bind(&s.topology)
        .bind(i64::from(s.s3_port))
        .bind(s.console_port.map(i64::from))
        .bind(&s.endpoint)
        .bind(s.credential_id.to_string())
        .bind(&s.status)
        .bind(s.last_error.clone())
        .bind(s.last_job_id.map(|j| j.to_string()))
        .bind(s.created_by.to_string())
        .bind(s.created_at.0)
        .bind(s.updated_at.0)
        .bind(&s.mode)
        .bind(&s.region)
        .bind(b(s.path_style))
        .bind(s.environment_id.map(|n| n.to_string()))
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn set_object_store_owner(
        &self,
        id: ObjectStoreId,
        environment: Option<EnvironmentId>,
        at: Timestamp,
    ) -> DomainResult<()> {
        let done = match environment {
            // Only an environment that exists is recorded, in the same statement.
            Some(ns) => {
                sqlx::query(
                    "UPDATE object_stores SET environment_id = $2, updated_at = $3
                     WHERE id = $1 AND EXISTS (SELECT 1 FROM environments WHERE id = $2)",
                )
                .bind(id.to_string())
                .bind(ns.to_string())
                .bind(at.0)
                .execute(self.pool())
                .await
            }
            None => {
                sqlx::query("UPDATE object_stores SET environment_id = NULL, updated_at = $2 WHERE id = $1")
                    .bind(id.to_string())
                    .bind(at.0)
                    .execute(self.pool())
                    .await
            }
        }
        .map_err(db_err)?;

        if done.rows_affected() == 0 {
            return Err(DomainError::NotFound("object store or environment not found".into()));
        }

        Ok(())
    }

    async fn get_object_store(&self, id: ObjectStoreId) -> DomainResult<Option<ObjectStore>> {
        let r = sqlx::query("SELECT * FROM object_stores WHERE id = $1")
            .bind(id.to_string())
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(store_row).transpose()
    }

    async fn list_object_stores(&self, workspace: Option<WorkspaceId>) -> DomainResult<Vec<ObjectStore>> {
        let rows = match workspace {
            Some(w) => {
                sqlx::query("SELECT * FROM object_stores WHERE workspace_id = $1 ORDER BY name")
                    .bind(w.to_string())
                    .fetch_all(self.pool())
                    .await
            }
            None => sqlx::query("SELECT * FROM object_stores ORDER BY name").fetch_all(self.pool()).await,
        }
        .map_err(db_err)?;

        rows.iter().map(store_row).collect()
    }

    async fn delete_object_store(&self, id: ObjectStoreId) -> DomainResult<()> {
        let mut tx = self.pool().begin().await.map_err(db_err)?;

        // Grants name the store by id: they go first, and with it (D-108).
        sqlx::query("DELETE FROM bucket_grants WHERE store_id = $1")
            .bind(id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        sqlx::query("DELETE FROM object_store_members WHERE store_id = $1")
            .bind(id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        sqlx::query("DELETE FROM object_stores WHERE id = $1")
            .bind(id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;

        tx.commit().await.map_err(db_err)
    }

    async fn put_store_member(&self, m: &StoreMember) -> DomainResult<()> {
        sqlx::query(
            "INSERT INTO object_store_members (store_id, server_id, data_paths, status, capacity_bytes, free_bytes,
                                               dedicated, observed_at, require_mount, endpoint_host)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
             ON CONFLICT (store_id, server_id) DO UPDATE SET data_paths = $3, status = $4, capacity_bytes = $5,
                                                             free_bytes = $6, dedicated = $7, observed_at = $8,
                                                             require_mount = $9, endpoint_host = $10",
        )
        .bind(m.store_id.to_string())
        .bind(m.server_id.to_string())
        .bind(serde_json::to_string(&m.data_paths).map_err(DomainError::internal)?)
        .bind(&m.status)
        .bind(signed(m.capacity_bytes))
        .bind(signed(m.free_bytes))
        .bind(m.dedicated.map(b))
        .bind(m.observed_at.map(|t| t.0))
        .bind(b(m.require_mount))
        .bind(&m.endpoint_host)
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn list_store_members(&self, store: ObjectStoreId) -> DomainResult<Vec<StoreMember>> {
        let rows = sqlx::query("SELECT * FROM object_store_members WHERE store_id = $1 ORDER BY server_id")
            .bind(store.to_string())
            .fetch_all(self.pool())
            .await
            .map_err(db_err)?;
        rows.iter().map(member_row).collect()
    }

    async fn list_node_store_members(&self, server: ServerId) -> DomainResult<Vec<StoreMember>> {
        let rows = sqlx::query("SELECT * FROM object_store_members WHERE server_id = $1")
            .bind(server.to_string())
            .fetch_all(self.pool())
            .await
            .map_err(db_err)?;
        rows.iter().map(member_row).collect()
    }
}
