use async_trait::async_trait;
use forge_core::model::{Installation, PackageRelease, PackageVersions};
use forge_core::ports::InstallationRepository;
use forge_core::*;
use sqlx::any::AnyRow;

use crate::row::*;
use crate::{Store, db_err};

fn map_json(r: &AnyRow, col: &str) -> DomainResult<std::collections::BTreeMap<String, String>> {
    serde_json::from_value(json(r, col)?).map_err(DomainError::internal)
}

fn row(r: &AnyRow) -> DomainResult<Installation> {
    Ok(Installation {
        id: parsed(r, "id")?,
        workspace_id: parsed(r, "workspace_id")?,
        server_id: parsed(r, "server_id")?,
        package: text(r, "package")?,
        version: text(r, "version")?,
        params: map_json(r, "params")?,
        status: text(r, "status")?,
        state: opt_text(r, "state")?,
        env: map_json(r, "env")?,
        last_job_id: opt_parsed(r, "last_job_id")?,
        last_error: opt_text(r, "last_error")?,
        versions: opt_text(r, "versions")?.and_then(|v| serde_json::from_str::<PackageVersions>(&v).ok()),
        update_policy: text(r, "update_policy")?,
        created_at: ts(r, "created_at")?,
        updated_at: ts(r, "updated_at")?,
    })
}

#[async_trait]
impl InstallationRepository for Store {
    async fn put_installation(&self, i: &Installation) -> DomainResult<()> {
        let params = serde_json::to_string(&i.params).map_err(DomainError::internal)?;
        let env = serde_json::to_string(&i.env).map_err(DomainError::internal)?;
        sqlx::query(
            "INSERT INTO installations (id, workspace_id, server_id, package, version, params, status, state, env,
                                        last_job_id, last_error, created_at, updated_at, versions, update_policy)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
             ON CONFLICT (id) DO UPDATE SET params = $6, status = $7, state = $8, env = $9, last_job_id = $10,
                                            last_error = $11, updated_at = $13, versions = $14,
                                            update_policy = $15",
        )
        .bind(i.id.to_string())
        .bind(i.workspace_id.to_string())
        .bind(i.server_id.to_string())
        .bind(&i.package)
        .bind(&i.version)
        .bind(params)
        .bind(&i.status)
        .bind(i.state.clone())
        .bind(env)
        .bind(i.last_job_id.map(|j| j.to_string()))
        .bind(i.last_error.clone())
        .bind(i.created_at.0)
        .bind(i.updated_at.0)
        .bind(i.versions.as_ref().map(serde_json::to_string).transpose().map_err(DomainError::internal)?)
        .bind(&i.update_policy)
        .execute(self.pool())
        .await
        .map_err(db_err)?;

        Ok(())
    }

    async fn get_installation(&self, id: InstallationId) -> DomainResult<Option<Installation>> {
        let r = sqlx::query("SELECT * FROM installations WHERE id = $1")
            .bind(id.to_string())
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(row).transpose()
    }

    async fn find_installation(
        &self,
        server: ServerId,
        package: &str,
        version: &str,
    ) -> DomainResult<Option<Installation>> {
        let r = sqlx::query("SELECT * FROM installations WHERE server_id = $1 AND package = $2 AND version = $3")
            .bind(server.to_string())
            .bind(package)
            .bind(version)
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(row).transpose()
    }

    async fn list_installations(&self, server: Option<ServerId>) -> DomainResult<Vec<Installation>> {
        let rows = match server {
            Some(s) => {
                sqlx::query("SELECT * FROM installations WHERE server_id = $1 ORDER BY package, version")
                    .bind(s.to_string())
                    .fetch_all(self.pool())
                    .await
            }
            None => sqlx::query("SELECT * FROM installations ORDER BY package, version").fetch_all(self.pool()).await,
        }
        .map_err(db_err)?;

        rows.iter().map(row).collect()
    }

    async fn delete_installation(&self, id: InstallationId) -> DomainResult<()> {
        sqlx::query("DELETE FROM installations WHERE id = $1")
            .bind(id.to_string())
            .execute(self.pool())
            .await
            .map_err(db_err)?;
        Ok(())
    }

    async fn put_releases(&self, product: &str, releases: &[PackageRelease]) -> DomainResult<()> {
        let mut tx = self.pool().begin().await.map_err(db_err)?;
        sqlx::query("DELETE FROM package_releases WHERE product = $1")
            .bind(product)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;

        for r in releases {
            sqlx::query(
                "INSERT INTO package_releases (product, line, latest, lts, released_at, latest_at, eol,
                                               support_until, fetched_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
            )
            .bind(product)
            .bind(&r.line)
            .bind(&r.latest)
            .bind(i64::from(r.lts))
            .bind(r.released_at.clone())
            .bind(r.latest_at.clone())
            .bind(r.eol.clone())
            .bind(r.support_until.clone())
            .bind(r.fetched_at.0)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        }

        tx.commit().await.map_err(db_err)
    }

    async fn list_releases(&self) -> DomainResult<Vec<PackageRelease>> {
        let rows = sqlx::query("SELECT * FROM package_releases ORDER BY product, line")
            .fetch_all(self.pool())
            .await
            .map_err(db_err)?;
        rows.iter()
            .map(|r| {
                Ok(PackageRelease {
                    product: text(r, "product")?,
                    line: text(r, "line")?,
                    latest: text(r, "latest")?,
                    lts: int(r, "lts")? != 0,
                    released_at: opt_text(r, "released_at")?,
                    latest_at: opt_text(r, "latest_at")?,
                    eol: opt_text(r, "eol")?,
                    support_until: opt_text(r, "support_until")?,
                    fetched_at: ts(r, "fetched_at")?,
                })
            })
            .collect()
    }
}
