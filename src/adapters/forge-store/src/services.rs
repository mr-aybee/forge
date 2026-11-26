use async_trait::async_trait;
use forge_core::model::{EnvVar, Service, ServiceInstance};
use forge_core::ports::{HealthAuthority, HealthAuthorityRepository, ServiceRepository};
use forge_core::*;
use serde_json::{Value, json};
use sqlx::any::AnyRow;

use crate::row::*;
use crate::{Store, db_err};

/// Storage form of the environment: keeps sealed values (the domain type
/// does not serialize them, so they never reach API responses).
fn env_to_db(env: &[EnvVar]) -> String {
    Value::Array(
        env.iter()
            .map(|e| json!({ "key": e.key, "value": if e.secret { Value::Null } else { json!(e.value) }, "secret": e.secret, "sealed": e.sealed }))
            .collect(),
    )
    .to_string()
}

fn env_from_db(v: &Value) -> Vec<EnvVar> {
    v.as_array()
        .map(|a| {
            a.iter()
                .map(|e| EnvVar {
                    key: e["key"].as_str().unwrap_or_default().to_owned(),
                    value: e["value"].as_str().map(str::to_owned),
                    secret: e["secret"].as_bool().unwrap_or(false),
                    sealed: e["sealed"].as_str().map(str::to_owned),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn app_row(r: &AnyRow) -> DomainResult<Service> {
    Ok(Service {
        id: parsed(r, "id")?,
        workspace_id: parsed(r, "workspace_id")?,
        environment_id: parsed(r, "environment_id")?,
        name: text(r, "name")?,
        runtime: text(r, "runtime")?,
        spec: json(r, "spec")?,
        env: env_from_db(&json(r, "env")?),
        created_by: parsed(r, "created_by")?,
        created_at: ts(r, "created_at")?,
        updated_at: ts(r, "updated_at")?,
        version: int(r, "version")?,
    })
}

fn instance_row(r: &AnyRow) -> DomainResult<ServiceInstance> {
    Ok(ServiceInstance {
        service_id: parsed(r, "service_id")?,
        server_id: parsed(r, "server_id")?,
        role: text(r, "role")?,
        status: text(r, "status")?,
        state: opt_text(r, "state")?,
        last_job_id: opt_parsed(r, "last_job_id")?,
        last_error: opt_text(r, "last_error")?,
        updated_at: ts(r, "updated_at")?,
    })
}

#[async_trait]
impl ServiceRepository for Store {
    async fn put_service(&self, a: &Service) -> DomainResult<()> {
        sqlx::query(
            "INSERT INTO services (id, workspace_id, environment_id, name, runtime, spec, env, created_by, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
             ON CONFLICT (id) DO UPDATE SET runtime = $5, spec = $6, env = $7, updated_at = $10,
                 version = services.version + 1",
        )
        .bind(a.id.to_string())
        .bind(a.workspace_id.to_string())
        .bind(a.environment_id.to_string())
        .bind(&a.name)
        .bind(&a.runtime)
        .bind(a.spec.to_string())
        .bind(env_to_db(&a.env))
        .bind(a.created_by.to_string())
        .bind(a.created_at.0)
        .bind(a.updated_at.0)
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn update_service_if(&self, a: &Service, expected: i64) -> DomainResult<bool> {
        let done = sqlx::query(
            "UPDATE services SET runtime = $2, spec = $3, env = $4, updated_at = $5, version = version + 1
             WHERE id = $1 AND version = $6",
        )
        .bind(a.id.to_string())
        .bind(&a.runtime)
        .bind(a.spec.to_string())
        .bind(env_to_db(&a.env))
        .bind(a.updated_at.0)
        .bind(expected)
        .execute(self.pool())
        .await
        .map_err(db_err)?;

        Ok(done.rows_affected() == 1)
    }

    async fn get_service(&self, id: ServiceId) -> DomainResult<Option<Service>> {
        let r = sqlx::query("SELECT * FROM services WHERE id = $1")
            .bind(id.to_string())
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(app_row).transpose()
    }

    async fn get_services(&self, ids: &[ServiceId]) -> DomainResult<Vec<Service>> {
        let mut out = Vec::with_capacity(ids.len());

        // One statement per 200 ids keeps the placeholder count small.
        for chunk in ids.chunks(200) {
            let marks: Vec<String> = (1..=chunk.len()).map(|i| format!("${i}")).collect();
            let sql = format!("SELECT * FROM services WHERE id IN ({})", marks.join(", "));
            let mut q = sqlx::query(&sql);

            for id in chunk {
                q = q.bind(id.to_string());
            }

            for r in &q.fetch_all(self.pool()).await.map_err(db_err)? {
                out.push(app_row(r)?);
            }
        }

        Ok(out)
    }

    async fn list_services(&self, environment: Option<EnvironmentId>) -> DomainResult<Vec<Service>> {
        let rows = match environment {
            Some(n) => {
                sqlx::query("SELECT * FROM services WHERE environment_id = $1 ORDER BY name")
                    .bind(n.to_string())
                    .fetch_all(self.pool())
                    .await
            }
            None => sqlx::query("SELECT * FROM services ORDER BY name").fetch_all(self.pool()).await,
        }
        .map_err(db_err)?;

        rows.iter().map(app_row).collect()
    }

    async fn delete_service(&self, id: ServiceId) -> DomainResult<()> {
        // Releases, deployments and slots (#73) go with the service.
        for q in [
            "DELETE FROM deployment_events WHERE deployment_id IN (SELECT id FROM deployments WHERE service_id = $1)",
            "DELETE FROM deployment_locks WHERE service_id = $1",
            "DELETE FROM deployments WHERE service_id = $1",
            "DELETE FROM releases WHERE service_id = $1",
            "DELETE FROM deploy_slots WHERE service_id = $1",
            "DELETE FROM deploy_ports WHERE service_id = $1",
            "DELETE FROM service_instances WHERE service_id = $1",
        ] {
            sqlx::query(q).bind(id.to_string()).execute(self.pool()).await.map_err(db_err)?;
        }

        sqlx::query("DELETE FROM app_health_authority WHERE service_id = $1")
            .bind(id.to_string())
            .execute(self.pool())
            .await
            .map_err(db_err)?;
        sqlx::query("DELETE FROM services WHERE id = $1")
            .bind(id.to_string())
            .execute(self.pool())
            .await
            .map_err(db_err)?;

        Ok(())
    }

    async fn put_instance(&self, i: &ServiceInstance) -> DomainResult<()> {
        sqlx::query(
            "INSERT INTO service_instances (service_id, server_id, role, status, state, last_job_id, last_error, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
             ON CONFLICT (service_id, server_id) DO UPDATE SET role = $3, status = $4, state = $5, last_job_id = $6,
                                                          last_error = $7, updated_at = $8",
        )
        .bind(i.service_id.to_string())
        .bind(i.server_id.to_string())
        .bind(&i.role)
        .bind(&i.status)
        .bind(i.state.clone())
        .bind(i.last_job_id.map(|j| j.to_string()))
        .bind(i.last_error.clone())
        .bind(i.updated_at.0)
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn list_all_instances(&self, environment: Option<EnvironmentId>) -> DomainResult<Vec<ServiceInstance>> {
        let rows = match environment {
            Some(ns) => {
                sqlx::query(
                    "SELECT i.* FROM service_instances i JOIN services a ON a.id = i.service_id
                     WHERE a.environment_id = $1 ORDER BY i.service_id, i.role, i.server_id",
                )
                .bind(ns.to_string())
                .fetch_all(self.pool())
                .await
            }
            None => {
                sqlx::query("SELECT * FROM service_instances ORDER BY service_id, role, server_id")
                    .fetch_all(self.pool())
                    .await
            }
        }
        .map_err(db_err)?;

        rows.iter().map(instance_row).collect()
    }

    async fn list_server_services(&self, server: ServerId) -> DomainResult<Vec<Service>> {
        let rows = sqlx::query(
            "SELECT a.* FROM services a JOIN service_instances i ON i.service_id = a.id
             WHERE i.server_id = $1 ORDER BY a.name",
        )
        .bind(server.to_string())
        .fetch_all(self.pool())
        .await
        .map_err(db_err)?;
        rows.iter().map(app_row).collect()
    }

    async fn list_cluster_services(&self, cluster: ClusterId) -> DomainResult<(Vec<Service>, Vec<ServiceInstance>)> {
        let apps = sqlx::query(
            "SELECT a.* FROM services a JOIN environments n ON n.id = a.environment_id WHERE n.cluster_id = $1 ORDER BY a.name",
        )
        .bind(cluster.to_string())
        .fetch_all(self.pool())
        .await
        .map_err(db_err)?;
        let inst = sqlx::query(
            "SELECT i.* FROM service_instances i JOIN services a ON a.id = i.service_id JOIN environments n ON n.id = a.environment_id
             WHERE n.cluster_id = $1 ORDER BY i.service_id, i.role, i.server_id",
        )
        .bind(cluster.to_string())
        .fetch_all(self.pool())
        .await
        .map_err(db_err)?;

        Ok((
            apps.iter().map(app_row).collect::<DomainResult<_>>()?,
            inst.iter().map(instance_row).collect::<DomainResult<_>>()?,
        ))
    }

    async fn list_instances(&self, app: ServiceId) -> DomainResult<Vec<ServiceInstance>> {
        let rows = sqlx::query("SELECT * FROM service_instances WHERE service_id = $1 ORDER BY role, server_id")
            .bind(app.to_string())
            .fetch_all(self.pool())
            .await
            .map_err(db_err)?;
        rows.iter().map(instance_row).collect()
    }

    async fn delete_instance(&self, app: ServiceId, server: ServerId) -> DomainResult<()> {
        sqlx::query("DELETE FROM deploy_slots WHERE service_id = $1 AND server_id = $2")
            .bind(app.to_string())
            .bind(server.to_string())
            .execute(self.pool())
            .await
            .map_err(db_err)?;
        sqlx::query("DELETE FROM service_instances WHERE service_id = $1 AND server_id = $2")
            .bind(app.to_string())
            .bind(server.to_string())
            .execute(self.pool())
            .await
            .map_err(db_err)?;

        Ok(())
    }
}

#[async_trait]
impl HealthAuthorityRepository for Store {
    async fn save_health_policy(&self, a: &Service, authority: Option<&HealthAuthority>) -> DomainResult<()> {
        // One transaction: a changed policy never exists without the
        // record of who saved it, nor that record without the policy.
        let mut tx = self.pool().begin().await.map_err(db_err)?;
        sqlx::query(
            "UPDATE services SET runtime = $2, spec = $3, env = $4, updated_at = $5, version = version + 1 WHERE id = $1",
        )
            .bind(a.id.to_string())
            .bind(&a.runtime)
            .bind(a.spec.to_string())
            .bind(env_to_db(&a.env))
            .bind(a.updated_at.0)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;

        match authority {
            Some(h) => {
                sqlx::query(
                    "INSERT INTO app_health_authority (service_id, saved_by, saved_at, revision) VALUES ($1, $2, $3, $4)
                     ON CONFLICT (service_id) DO UPDATE SET saved_by = $2, saved_at = $3, revision = $4",
                )
                .bind(a.id.to_string())
                .bind(h.saved_by.to_string())
                .bind(h.saved_at.0)
                .bind(h.revision)
                .execute(&mut *tx)
                .await
                .map_err(db_err)?;
            }
            None => {
                sqlx::query("DELETE FROM app_health_authority WHERE service_id = $1")
                    .bind(a.id.to_string())
                    .execute(&mut *tx)
                    .await
                    .map_err(db_err)?;
            }
        }

        tx.commit().await.map_err(db_err)
    }

    async fn get_health_authority(&self, app: ServiceId) -> DomainResult<Option<HealthAuthority>> {
        let r = sqlx::query(
            "SELECT service_id, saved_by, saved_at, revision FROM app_health_authority WHERE service_id = $1",
        )
        .bind(app.to_string())
        .fetch_optional(self.pool())
        .await
        .map_err(db_err)?;
        r.as_ref()
            .map(|r| {
                Ok(HealthAuthority {
                    service_id: parsed(r, "service_id")?,
                    saved_by: parsed(r, "saved_by")?,
                    saved_at: ts(r, "saved_at")?,
                    revision: int(r, "revision")?,
                })
            })
            .transpose()
    }
}
