//! Releases, deployments, their events and slots (#73).

use async_trait::async_trait;
use forge_core::deploy::{
    Deployment, DeploymentEvent, DeploymentKind, DeploymentRepository, DeploymentRequest, DeploymentState, Release,
    ReleaseRepository, SlotRecord,
};
use forge_core::model::EnvVar;
use forge_core::*;
use serde::{Deserialize, Serialize};
use sqlx::any::AnyRow;

use crate::row::*;
use crate::{Store, db_err};

/// A release's env as stored: unlike [`EnvVar`]'s own form, it keeps the
/// sealed values (they never leave the store through the API).
#[derive(Serialize, Deserialize)]
struct StoredEnv {
    key: String,
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    secret: bool,
    #[serde(default)]
    sealed: Option<String>,
}

fn env_db(env: &[EnvVar]) -> DomainResult<String> {
    let rows: Vec<StoredEnv> = env
        .iter()
        .map(|e| StoredEnv { key: e.key.clone(), value: e.value.clone(), secret: e.secret, sealed: e.sealed.clone() })
        .collect();
    serde_json::to_string(&rows).map_err(DomainError::internal)
}

fn env_col(r: &AnyRow) -> DomainResult<Vec<EnvVar>> {
    let rows: Vec<StoredEnv> = serde_json::from_str(&text(r, "env")?).map_err(DomainError::internal)?;
    Ok(rows.into_iter().map(|e| EnvVar { key: e.key, value: e.value, secret: e.secret, sealed: e.sealed }).collect())
}

fn release_row(r: &AnyRow, with_source: bool) -> DomainResult<Release> {
    Ok(Release {
        id: parsed(r, "id")?,
        service_id: parsed(r, "service_id")?,
        workspace_id: parsed(r, "workspace_id")?,
        number: int(r, "number")?,
        spec: json(r, "spec")?,
        env: env_col(r)?,
        artifact: json(r, "artifact")?,
        policy: json(r, "policy")?,
        source: if with_source { opt_text(r, "source")? } else { None },
        source_sha256: opt_text(r, "source_sha256")?,
        description: text(r, "description")?,
        created_by: parsed(r, "created_by")?,
        created_at: ts(r, "created_at")?,
    })
}

fn deployment_row(r: &AnyRow) -> DomainResult<Deployment> {
    let state = text(r, "state")?;
    let kind = text(r, "kind")?;

    Ok(Deployment {
        id: parsed(r, "id")?,
        service_id: parsed(r, "service_id")?,
        workspace_id: parsed(r, "workspace_id")?,
        release_id: parsed(r, "release_id")?,
        release_number: int(r, "release_number")?,
        from_release: int(r, "from_release")?,
        kind: DeploymentKind::parse(&kind).ok_or_else(|| DomainError::Internal(format!("deployment kind {kind}")))?,
        strategy: text(r, "strategy")?,
        state: DeploymentState::parse(&state)
            .ok_or_else(|| DomainError::Internal(format!("deployment state {state}")))?,
        request: opt_text(r, "request")?.as_deref().and_then(DeploymentRequest::parse),
        job_id: opt_parsed(r, "job_id")?,
        plan: json(r, "plan")?,
        step: int(r, "step")?,
        error: opt_text(r, "error")?,
        created_by: parsed(r, "created_by")?,
        created_at: ts(r, "created_at")?,
        updated_at: ts(r, "updated_at")?,
        finished_at: opt_ts(r, "finished_at")?,
    })
}

fn slot_row(r: &AnyRow) -> DomainResult<SlotRecord> {
    Ok(SlotRecord {
        service_id: parsed(r, "service_id")?,
        server_id: parsed(r, "server_id")?,
        slot: text(r, "slot")?,
        release_number: int(r, "release_number")?,
        weight: u32_col(r, "weight")?,
        state: text(r, "state")?,
        ports: json(r, "ports")?,
        updated_at: ts(r, "updated_at")?,
    })
}

const RELEASE_COLS: &str = "id, service_id, workspace_id, number, spec, env, artifact, policy, source_sha256, description, \
                            created_by, created_at";

#[async_trait]
impl ReleaseRepository for Store {
    async fn create_release(&self, rel: &Release) -> DomainResult<i64> {
        // The number comes from the table in the same statement; a concurrent
        // insert of the same number fails on UNIQUE (service_id, number), and the
        // caller retries.
        let r = sqlx::query(
            "INSERT INTO releases (id, service_id, workspace_id, number, spec, env, artifact, policy, source, source_sha256,
                                   description, created_by, created_at)
             SELECT $1, $2, $3, COALESCE(MAX(number), 0) + 1, $4, $5, $6, $7, $8, $9, $10, $11, $12
             FROM releases WHERE service_id = $2
             RETURNING number",
        )
        .bind(rel.id.to_string())
        .bind(rel.service_id.to_string())
        .bind(rel.workspace_id.to_string())
        .bind(rel.spec.to_string())
        .bind(env_db(&rel.env)?)
        .bind(rel.artifact.to_string())
        .bind(rel.policy.to_string())
        .bind(rel.source.clone())
        .bind(rel.source_sha256.clone())
        .bind(&rel.description)
        .bind(rel.created_by.to_string())
        .bind(rel.created_at.0)
        .fetch_one(self.pool())
        .await
        .map_err(db_err)?;
        int(&r, "number")
    }

    async fn get_release(&self, id: ReleaseId) -> DomainResult<Option<Release>> {
        let r = sqlx::query("SELECT * FROM releases WHERE id = $1")
            .bind(id.to_string())
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(|r| release_row(r, true)).transpose()
    }

    async fn release_by_number(&self, app: ServiceId, number: i64) -> DomainResult<Option<Release>> {
        let r = sqlx::query("SELECT * FROM releases WHERE service_id = $1 AND number = $2")
            .bind(app.to_string())
            .bind(number)
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(|r| release_row(r, true)).transpose()
    }

    async fn list_releases(&self, app: ServiceId, limit: u32) -> DomainResult<Vec<Release>> {
        let rows = sqlx::query(&format!(
            "SELECT {RELEASE_COLS} FROM releases WHERE service_id = $1 ORDER BY number DESC LIMIT $2"
        ))
        .bind(app.to_string())
        .bind(i64::from(limit))
        .fetch_all(self.pool())
        .await
        .map_err(db_err)?;
        rows.iter().map(|r| release_row(r, false)).collect()
    }

    async fn set_release_artifact(&self, id: ReleaseId, artifact: &serde_json::Value) -> DomainResult<()> {
        sqlx::query("UPDATE releases SET artifact = $2 WHERE id = $1")
            .bind(id.to_string())
            .bind(artifact.to_string())
            .execute(self.pool())
            .await
            .map_err(db_err)?;
        Ok(())
    }

    async fn prune_releases(&self, app: ServiceId, keep: u32, except: &[i64]) -> DomainResult<u64> {
        let numbers: Vec<i64> = sqlx::query("SELECT number FROM releases WHERE service_id = $1 ORDER BY number DESC")
            .bind(app.to_string())
            .fetch_all(self.pool())
            .await
            .map_err(db_err)?
            .iter()
            .map(|r| int(r, "number"))
            .collect::<DomainResult<_>>()?;
        let mut deleted = 0;

        for n in numbers.into_iter().skip(keep as usize).filter(|n| !except.contains(n)) {
            let r = sqlx::query("DELETE FROM releases WHERE service_id = $1 AND number = $2")
                .bind(app.to_string())
                .bind(n)
                .execute(self.pool())
                .await
                .map_err(db_err)?;
            deleted += r.rows_affected();
        }

        Ok(deleted)
    }
}

#[async_trait]
impl DeploymentRepository for Store {
    async fn create_deployment(&self, d: &Deployment) -> DomainResult<()> {
        // The lock row and the deployment in one transaction: a second
        // deployment of the same service fails on the lock's primary key.
        let mut tx = self.pool().begin().await.map_err(db_err)?;
        sqlx::query("INSERT INTO deployment_locks (service_id, deployment_id) VALUES ($1, $2)")
            .bind(d.service_id.to_string())
            .bind(d.id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(|e| match db_err(e) {
                DomainError::Conflict(_) => {
                    DomainError::Conflict("the service already has a deployment running".into())
                }
                other => other,
            })?;
        sqlx::query(
            "INSERT INTO deployments (id, service_id, workspace_id, release_id, release_number, from_release, kind, strategy,
                                      state, request, job_id, plan, step, error, created_by, created_at, updated_at,
                                      finished_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18)",
        )
        .bind(d.id.to_string())
        .bind(d.service_id.to_string())
        .bind(d.workspace_id.to_string())
        .bind(d.release_id.to_string())
        .bind(d.release_number)
        .bind(d.from_release)
        .bind(d.kind.as_str())
        .bind(&d.strategy)
        .bind(d.state.as_str())
        .bind(d.request.map(|r| r.as_str()))
        .bind(d.job_id.map(|j| j.to_string()))
        .bind(d.plan.to_string())
        .bind(d.step)
        .bind(d.error.clone())
        .bind(d.created_by.to_string())
        .bind(d.created_at.0)
        .bind(d.updated_at.0)
        .bind(d.finished_at.map(|t| t.0))
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;

        tx.commit().await.map_err(db_err)
    }

    async fn put_deployment(&self, d: &Deployment) -> DomainResult<()> {
        // State and lock in one transaction: a finished deployment frees the service.
        let mut tx = self.pool().begin().await.map_err(db_err)?;
        sqlx::query(
            "UPDATE deployments SET state = $2, job_id = $3, plan = $4, step = $5, error = $6, updated_at = $7,
                                    finished_at = $8
             WHERE id = $1",
        )
        .bind(d.id.to_string())
        .bind(d.state.as_str())
        .bind(d.job_id.map(|j| j.to_string()))
        .bind(d.plan.to_string())
        .bind(d.step)
        .bind(d.error.clone())
        .bind(d.updated_at.0)
        .bind(d.finished_at.map(|t| t.0))
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;

        if !d.state.active() {
            sqlx::query("DELETE FROM deployment_locks WHERE service_id = $1 AND deployment_id = $2")
                .bind(d.service_id.to_string())
                .bind(d.id.to_string())
                .execute(&mut *tx)
                .await
                .map_err(db_err)?;
        }

        tx.commit().await.map_err(db_err)
    }

    async fn get_deployment(&self, id: DeploymentId) -> DomainResult<Option<Deployment>> {
        let r = sqlx::query("SELECT * FROM deployments WHERE id = $1")
            .bind(id.to_string())
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(deployment_row).transpose()
    }

    async fn list_deployments(&self, app: ServiceId, limit: u32) -> DomainResult<Vec<Deployment>> {
        let rows =
            sqlx::query("SELECT * FROM deployments WHERE service_id = $1 ORDER BY created_at DESC, id DESC LIMIT $2")
                .bind(app.to_string())
                .bind(i64::from(limit))
                .fetch_all(self.pool())
                .await
                .map_err(db_err)?;
        rows.iter().map(deployment_row).collect()
    }

    async fn active_deployment(&self, app: ServiceId) -> DomainResult<Option<Deployment>> {
        let r = sqlx::query(
            "SELECT d.* FROM deployments d JOIN deployment_locks l ON l.deployment_id = d.id WHERE l.service_id = $1",
        )
        .bind(app.to_string())
        .fetch_optional(self.pool())
        .await
        .map_err(db_err)?;
        r.as_ref().map(deployment_row).transpose()
    }

    async fn request_deployment(
        &self,
        id: DeploymentId,
        request: DeploymentRequest,
        now: Timestamp,
    ) -> DomainResult<()> {
        sqlx::query("UPDATE deployments SET request = $2, updated_at = $3 WHERE id = $1")
            .bind(id.to_string())
            .bind(request.as_str())
            .bind(now.0)
            .execute(self.pool())
            .await
            .map_err(db_err)?;
        Ok(())
    }

    async fn clear_request(&self, id: DeploymentId) -> DomainResult<()> {
        sqlx::query("UPDATE deployments SET request = NULL WHERE id = $1")
            .bind(id.to_string())
            .execute(self.pool())
            .await
            .map_err(db_err)?;
        Ok(())
    }

    async fn add_event(&self, e: &DeploymentEvent) -> DomainResult<()> {
        // `seq` is the next one for the deployment; a retried step adds a new
        // event rather than overwriting one.
        sqlx::query(
            "INSERT INTO deployment_events (deployment_id, seq, at, level, server_id, message)
             SELECT $1, COALESCE(MAX(seq), 0) + 1, $2, $3, $4, $5 FROM deployment_events WHERE deployment_id = $1",
        )
        .bind(e.deployment_id.to_string())
        .bind(e.at.0)
        .bind(&e.level)
        .bind(e.server_id.map(|s| s.to_string()))
        .bind(&e.message)
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn list_events(&self, id: DeploymentId, after_seq: i64, limit: u32) -> DomainResult<Vec<DeploymentEvent>> {
        let rows =
            sqlx::query("SELECT * FROM deployment_events WHERE deployment_id = $1 AND seq > $2 ORDER BY seq LIMIT $3")
                .bind(id.to_string())
                .bind(after_seq)
                .bind(i64::from(limit))
                .fetch_all(self.pool())
                .await
                .map_err(db_err)?;
        rows.iter()
            .map(|r| {
                Ok(DeploymentEvent {
                    deployment_id: parsed(r, "deployment_id")?,
                    seq: int(r, "seq")?,
                    at: ts(r, "at")?,
                    level: text(r, "level")?,
                    server_id: opt_parsed(r, "server_id")?,
                    message: text(r, "message")?,
                })
            })
            .collect()
    }

    async fn put_slot(&self, s: &SlotRecord) -> DomainResult<()> {
        sqlx::query(
            "INSERT INTO deploy_slots (service_id, server_id, slot, release_number, weight, state, ports, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
             ON CONFLICT (service_id, server_id, slot) DO UPDATE SET release_number = $4, weight = $5, state = $6,
                 ports = $7, updated_at = $8",
        )
        .bind(s.service_id.to_string())
        .bind(s.server_id.to_string())
        .bind(&s.slot)
        .bind(s.release_number)
        .bind(i64::from(s.weight))
        .bind(&s.state)
        .bind(s.ports.to_string())
        .bind(s.updated_at.0)
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn replace_slots(&self, app: ServiceId, rows: &[SlotRecord]) -> DomainResult<()> {
        let mut tx = self.pool().begin().await.map_err(db_err)?;
        sqlx::query("DELETE FROM deploy_slots WHERE service_id = $1")
            .bind(app.to_string())
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;

        for s in rows.iter().filter(|s| s.service_id == app) {
            sqlx::query(
                "INSERT INTO deploy_slots (service_id, server_id, slot, release_number, weight, state, ports, updated_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
            )
            .bind(s.service_id.to_string())
            .bind(s.server_id.to_string())
            .bind(&s.slot)
            .bind(s.release_number)
            .bind(i64::from(s.weight))
            .bind(&s.state)
            .bind(s.ports.to_string())
            .bind(s.updated_at.0)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        }

        tx.commit().await.map_err(db_err)
    }

    async fn delete_slot(&self, app: ServiceId, server: ServerId, slot: &str) -> DomainResult<()> {
        sqlx::query("DELETE FROM deploy_slots WHERE service_id = $1 AND server_id = $2 AND slot = $3")
            .bind(app.to_string())
            .bind(server.to_string())
            .bind(slot)
            .execute(self.pool())
            .await
            .map_err(db_err)?;
        Ok(())
    }

    async fn list_slots(&self, app: ServiceId) -> DomainResult<Vec<SlotRecord>> {
        let rows = sqlx::query("SELECT * FROM deploy_slots WHERE service_id = $1 ORDER BY server_id, slot")
            .bind(app.to_string())
            .fetch_all(self.pool())
            .await
            .map_err(db_err)?;
        rows.iter().map(slot_row).collect()
    }

    async fn list_slots_of(&self, apps: &[ServiceId]) -> DomainResult<Vec<SlotRecord>> {
        let mut out = Vec::new();

        for chunk in apps.chunks(200) {
            let marks: Vec<String> = (1..=chunk.len()).map(|i| format!("${i}")).collect();
            let sql = format!(
                "SELECT * FROM deploy_slots WHERE service_id IN ({}) ORDER BY service_id, server_id, slot",
                marks.join(", ")
            );
            let mut q = sqlx::query(&sql);

            for a in chunk {
                q = q.bind(a.to_string());
            }

            for r in q.fetch_all(self.pool()).await.map_err(db_err)? {
                out.push(slot_row(&r)?);
            }
        }

        Ok(out)
    }

    async fn slot_ports(&self, app: ServiceId, slot: &str, count: u32, range: (u16, u16)) -> DomainResult<Vec<u16>> {
        let have = |rows: Vec<AnyRow>| -> DomainResult<Vec<u16>> {
            rows.iter().map(|r| int(r, "port").map(|p| u16::try_from(p).unwrap_or(0))).collect()
        };
        let current = have(
            sqlx::query("SELECT port FROM deploy_ports WHERE service_id = $1 AND slot = $2 ORDER BY idx")
                .bind(app.to_string())
                .bind(slot)
                .fetch_all(self.pool())
                .await
                .map_err(db_err)?,
        )?;
        let mut ports = current;
        let mut idx = ports.len() as i64;

        // Lowest free port first; a port taken concurrently fails on UNIQUE
        // and the next one is tried.
        let mut candidate = i64::from(range.0);

        while (ports.len() as u32) < count {
            if candidate > i64::from(range.1) {
                return Err(DomainError::Conflict("no free slot ports left".into()));
            }

            let taken = sqlx::query("SELECT 1 AS one FROM deploy_ports WHERE port = $1")
                .bind(candidate)
                .fetch_optional(self.pool())
                .await
                .map_err(db_err)?
                .is_some();

            if !taken {
                let r = sqlx::query("INSERT INTO deploy_ports (service_id, slot, idx, port) VALUES ($1, $2, $3, $4)")
                    .bind(app.to_string())
                    .bind(slot)
                    .bind(idx)
                    .bind(candidate)
                    .execute(self.pool())
                    .await;

                match r {
                    Ok(_) => {
                        ports.push(u16::try_from(candidate).unwrap_or(0));
                        idx += 1;
                    }
                    Err(e) => match db_err(e) {
                        DomainError::Conflict(_) => {}
                        other => return Err(other),
                    },
                }
            }

            candidate += 1;
        }

        ports.truncate(count as usize);

        Ok(ports)
    }

    async fn current_release(&self, app: ServiceId) -> DomainResult<i64> {
        let r = sqlx::query(
            "SELECT release_number FROM deployments WHERE service_id = $1 AND state = 'succeeded'
             ORDER BY finished_at DESC, created_at DESC LIMIT 1",
        )
        .bind(app.to_string())
        .fetch_optional(self.pool())
        .await
        .map_err(db_err)?;
        r.as_ref().map_or(Ok(0), |r| int(r, "release_number"))
    }
}
