use async_trait::async_trait;
use forge_core::model::{Job, JobFilter, JobStatus, JobStepRecord, NewJob};
use forge_core::ports::JobRepository;
use forge_core::*;
use serde_json::Value;
use sqlx::any::AnyRow;

use crate::row::*;
use crate::{Store, db_err};

const RUNNABLE: &str = "((status = 'queued' AND next_run_at <= $3) OR (status = 'running' AND lease_expires_at < $3))";

fn job_row(r: &AnyRow) -> DomainResult<Job> {
    Ok(Job {
        id: parsed(r, "id")?,
        kind: text(r, "kind")?,
        workspace_id: opt_parsed(r, "workspace_id")?,
        resource: text(r, "resource")?.parse().map_err(DomainError::Internal)?,
        principal: text(r, "principal")?.parse().map_err(DomainError::Internal)?,
        context: AuthzContext { mfa: flag(r, "authz_mfa")? },
        idempotency_key: opt_text(r, "idempotency_key")?,
        payload: json(r, "payload")?,
        state: json(r, "state")?,
        status: JobStatus::parse(&text(r, "status")?),
        current_step: u32_col(r, "current_step")?,
        attempts: u32_col(r, "attempts")?,
        max_attempts: u32_col(r, "max_attempts")?,
        next_run_at: ts(r, "next_run_at")?,
        lease_owner: opt_text(r, "lease_owner")?,
        lease_expires_at: opt_ts(r, "lease_expires_at")?,
        lease_token: int(r, "lease_token")?,
        last_error: opt_text(r, "last_error")?,
        created_at: ts(r, "created_at")?,
        updated_at: ts(r, "updated_at")?,
    })
}

fn lease_lost(id: JobId) -> DomainError {
    DomainError::LeaseLost(id.to_string())
}

impl Store {
    /// Fences a write: succeeds only if the caller still holds the lease.
    async fn fence(&self, id: JobId, token: i64, now: Timestamp) -> DomainResult<()> {
        let res =
            sqlx::query("UPDATE jobs SET updated_at = $1 WHERE id = $2 AND lease_token = $3 AND status = 'running'")
                .bind(now.0)
                .bind(id.to_string())
                .bind(token)
                .execute(self.pool())
                .await
                .map_err(db_err)?;
        if res.rows_affected() == 1 { Ok(()) } else { Err(lease_lost(id)) }
    }
}

#[async_trait]
impl JobRepository for Store {
    async fn insert_job(&self, job: NewJob, now: Timestamp) -> DomainResult<(Job, bool)> {
        let id = JobId::new();
        let res = sqlx::query(
            "INSERT INTO jobs (id, kind, workspace_id, resource, principal, authz_mfa, idempotency_key, payload, state, status,
                               current_step, attempts, max_attempts, next_run_at, lease_token, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $10, $6, $7, '{}', 'queued', 0, 0, $8, $9, 0, $9, $9)
             ON CONFLICT (kind, idempotency_key) DO NOTHING",
        )
        .bind(id.to_string())
        .bind(&job.kind)
        .bind(job.workspace_id.map(|o| o.to_string()))
        .bind(job.resource.to_string())
        .bind(job.principal.to_string())
        .bind(job.idempotency_key.clone())
        .bind(job.payload.to_string())
        .bind(i64::from(job.max_attempts.max(1)))
        .bind(now.0)
        .bind(b(job.context.mfa))
        .execute(self.pool())
        .await
        .map_err(db_err)?;

        if res.rows_affected() == 1 {
            return Ok((self.get_job(id).await?, true));
        }

        let key = job.idempotency_key.unwrap_or_default();
        let r = sqlx::query("SELECT * FROM jobs WHERE kind = $1 AND idempotency_key = $2")
            .bind(&job.kind)
            .bind(&key)
            .fetch_one(self.pool())
            .await
            .map_err(db_err)?;

        Ok((job_row(&r)?, false))
    }

    async fn get_job(&self, id: JobId) -> DomainResult<Job> {
        let r = sqlx::query("SELECT * FROM jobs WHERE id = $1")
            .bind(id.to_string())
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(job_row).transpose()?.ok_or_else(|| DomainError::NotFound(format!("job {id}")))
    }

    async fn find_job_by_key(&self, kind: &str, key: &str) -> DomainResult<Option<Job>> {
        let r = sqlx::query("SELECT * FROM jobs WHERE kind = $1 AND idempotency_key = $2")
            .bind(kind)
            .bind(key)
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(job_row).transpose()
    }

    async fn list_jobs(&self, f: &JobFilter) -> DomainResult<Vec<Job>> {
        let limit = i64::from(if f.limit == 0 { 100 } else { f.limit.min(500) });
        let rows = sqlx::query(
            "SELECT * FROM jobs
             WHERE ($1 IS NULL OR workspace_id = $1) AND ($2 IS NULL OR status = $2)
             ORDER BY created_at DESC LIMIT $3",
        )
        .bind(f.workspace_id.map(|o| o.to_string()))
        .bind(f.status.map(|s| s.as_str().to_owned()))
        .bind(limit)
        .fetch_all(self.pool())
        .await
        .map_err(db_err)?;

        rows.iter().map(job_row).collect()
    }

    async fn list_steps(&self, id: JobId) -> DomainResult<Vec<JobStepRecord>> {
        let rows = sqlx::query("SELECT * FROM job_steps WHERE job_id = $1 ORDER BY step_index")
            .bind(id.to_string())
            .fetch_all(self.pool())
            .await
            .map_err(db_err)?;
        rows.iter()
            .map(|r| {
                Ok(JobStepRecord {
                    job_id: parsed(r, "job_id")?,
                    index: u32_col(r, "step_index")?,
                    name: text(r, "name")?,
                    status: text(r, "status")?,
                    started_at: ts(r, "started_at")?,
                    finished_at: opt_ts(r, "finished_at")?,
                    error: opt_text(r, "error")?,
                })
            })
            .collect()
    }

    async fn claim_job(&self, worker: &str, now: Timestamp, lease_until: Timestamp) -> DomainResult<Option<Job>> {
        // The runnable predicate is repeated in the outer WHERE so that two
        // workers racing on the same row cannot both win (optimistic claim).
        let sql = format!(
            "UPDATE jobs SET status = 'running', lease_owner = $1, lease_expires_at = $2,
                             lease_token = lease_token + 1, updated_at = $3
             WHERE id = (SELECT id FROM jobs WHERE {RUNNABLE} ORDER BY next_run_at, created_at LIMIT 1)
               AND {RUNNABLE}
             RETURNING *"
        );
        let r = sqlx::query(&sql)
            .bind(worker)
            .bind(lease_until.0)
            .bind(now.0)
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;

        r.as_ref().map(job_row).transpose()
    }

    async fn renew_lease(&self, id: JobId, token: i64, lease_until: Timestamp) -> DomainResult<()> {
        let res = sqlx::query(
            "UPDATE jobs SET lease_expires_at = $1 WHERE id = $2 AND lease_token = $3 AND status = 'running'",
        )
        .bind(lease_until.0)
        .bind(id.to_string())
        .bind(token)
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        if res.rows_affected() == 1 { Ok(()) } else { Err(lease_lost(id)) }
    }

    async fn start_step(&self, id: JobId, token: i64, index: u32, name: &str, now: Timestamp) -> DomainResult<()> {
        self.fence(id, token, now).await?;
        sqlx::query(
            "INSERT INTO job_steps (job_id, step_index, name, status, started_at, finished_at, error)
             VALUES ($1, $2, $3, 'running', $4, NULL, NULL)
             ON CONFLICT (job_id, step_index) DO UPDATE SET status = 'running', started_at = $4, error = NULL",
        )
        .bind(id.to_string())
        .bind(i64::from(index))
        .bind(name)
        .bind(now.0)
        .execute(self.pool())
        .await
        .map_err(db_err)?;

        Ok(())
    }

    async fn complete_step(
        &self,
        id: JobId,
        token: i64,
        index: u32,
        state: &Value,
        now: Timestamp,
    ) -> DomainResult<()> {
        let mut tx = self.pool().begin().await.map_err(db_err)?;
        let res = sqlx::query(
            "UPDATE jobs SET current_step = $1, state = $2, updated_at = $3
             WHERE id = $4 AND lease_token = $5 AND status = 'running'",
        )
        .bind(i64::from(index) + 1)
        .bind(state.to_string())
        .bind(now.0)
        .bind(id.to_string())
        .bind(token)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;

        if res.rows_affected() != 1 {
            return Err(lease_lost(id));
        }

        sqlx::query(
            "UPDATE job_steps SET status = 'succeeded', finished_at = $1 WHERE job_id = $2 AND step_index = $3",
        )
        .bind(now.0)
        .bind(id.to_string())
        .bind(i64::from(index))
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;

        tx.commit().await.map_err(db_err)
    }

    async fn finish_job(
        &self,
        id: JobId,
        token: i64,
        status: JobStatus,
        error: Option<&str>,
        now: Timestamp,
    ) -> DomainResult<()> {
        let mut tx = self.pool().begin().await.map_err(db_err)?;
        let res = sqlx::query(
            "UPDATE jobs SET status = $1, last_error = $2, lease_owner = NULL, lease_expires_at = NULL, updated_at = $3
             WHERE id = $4 AND lease_token = $5 AND status = 'running'",
        )
        .bind(status.as_str())
        .bind(error.map(str::to_owned))
        .bind(now.0)
        .bind(id.to_string())
        .bind(token)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;

        if res.rows_affected() != 1 {
            return Err(lease_lost(id));
        }

        if error.is_some() {
            sqlx::query(
                "UPDATE job_steps SET status = 'failed', finished_at = $1, error = $2
                 WHERE job_id = $3 AND status = 'running'",
            )
            .bind(now.0)
            .bind(error.map(str::to_owned))
            .bind(id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        }

        tx.commit().await.map_err(db_err)
    }

    async fn schedule_retry(
        &self,
        id: JobId,
        token: i64,
        next_run_at: Timestamp,
        error: &str,
        now: Timestamp,
    ) -> DomainResult<()> {
        let mut tx = self.pool().begin().await.map_err(db_err)?;
        let res = sqlx::query(
            "UPDATE jobs SET status = 'queued', next_run_at = $1, last_error = $2, attempts = attempts + 1,
                             lease_owner = NULL, lease_expires_at = NULL, updated_at = $3
             WHERE id = $4 AND lease_token = $5 AND status = 'running'",
        )
        .bind(next_run_at.0)
        .bind(error)
        .bind(now.0)
        .bind(id.to_string())
        .bind(token)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;

        if res.rows_affected() != 1 {
            return Err(lease_lost(id));
        }

        sqlx::query(
            "UPDATE job_steps SET status = 'retrying', finished_at = $1, error = $2 WHERE job_id = $3 AND status = 'running'",
        )
        .bind(now.0)
        .bind(error)
        .bind(id.to_string())
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;

        tx.commit().await.map_err(db_err)
    }

    async fn cancel_job(&self, id: JobId, now: Timestamp) -> DomainResult<bool> {
        let res = sqlx::query(
            "UPDATE jobs SET status = 'cancelled', lease_owner = NULL, lease_expires_at = NULL, updated_at = $1
             WHERE id = $2 AND status IN ('queued', 'running')",
        )
        .bind(now.0)
        .bind(id.to_string())
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(res.rows_affected() == 1)
    }
}
