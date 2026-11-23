//! `Idempotency-Key` records for mutating API requests (migration 0015).

use async_trait::async_trait;
use forge_core::model::{IdempotencyBegin, IdempotencyRecord};
use forge_core::ports::IdempotencyRepository;
use forge_core::*;
use sqlx::any::AnyRow;

use crate::row::*;
use crate::{Store, db_err};

fn record(r: &AnyRow) -> DomainResult<IdempotencyRecord> {
    Ok(IdempotencyRecord {
        principal: text(r, "principal")?,
        key: text(r, "idem_key")?,
        request_hash: text(r, "request_hash")?,
        done: text(r, "state")? == "done",
        status: opt_int(r, "status")?.and_then(|s| u16::try_from(s).ok()),
        content_type: opt_text(r, "content_type")?,
        body: opt_text(r, "body")?,
        created_at: ts(r, "created_at")?,
        expires_at: ts(r, "expires_at")?,
    })
}

#[async_trait]
impl IdempotencyRepository for Store {
    async fn begin_idempotent(
        &self,
        principal: &str,
        key: &str,
        request_hash: &str,
        now: Timestamp,
        expires_at: Timestamp,
        stale_pending_before: Timestamp,
    ) -> DomainResult<IdempotencyBegin> {
        // Expired rows go, and so does this key's pending row when its
        // request died with a crash or restart.
        sqlx::query("DELETE FROM idempotency_keys WHERE expires_at <= $1")
            .bind(now.0)
            .execute(self.pool())
            .await
            .map_err(db_err)?;
        sqlx::query(
            "DELETE FROM idempotency_keys
             WHERE principal = $1 AND idem_key = $2 AND state = 'pending' AND created_at < $3",
        )
        .bind(principal)
        .bind(key)
        .bind(stale_pending_before.0)
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        let inserted = sqlx::query(
            "INSERT INTO idempotency_keys (principal, idem_key, request_hash, state, created_at, expires_at)
             VALUES ($1, $2, $3, 'pending', $4, $5)
             ON CONFLICT (principal, idem_key) DO NOTHING",
        )
        .bind(principal)
        .bind(key)
        .bind(request_hash)
        .bind(now.0)
        .bind(expires_at.0)
        .execute(self.pool())
        .await
        .map_err(db_err)?
        .rows_affected();
        if inserted == 1 {
            return Ok(IdempotencyBegin::Started);
        }
        let r = sqlx::query("SELECT * FROM idempotency_keys WHERE principal = $1 AND idem_key = $2")
            .bind(principal)
            .bind(key)
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        match r {
            Some(r) => Ok(IdempotencyBegin::Existing(record(&r)?)),
            // Deleted between the insert and the read: treat as in flight.
            None => Err(DomainError::Conflict("the request with this Idempotency-Key is still running".into())),
        }
    }

    async fn finish_idempotent(
        &self,
        principal: &str,
        key: &str,
        claim: (&str, Timestamp),
        status: u16,
        content_type: Option<&str>,
        sealed_body: Option<&str>,
    ) -> DomainResult<()> {
        sqlx::query(
            "UPDATE idempotency_keys SET state = 'done', status = $3, content_type = $4, body = $5
             WHERE principal = $1 AND idem_key = $2 AND request_hash = $6 AND created_at = $7 AND state = 'pending'",
        )
        .bind(principal)
        .bind(key)
        .bind(i64::from(status))
        .bind(content_type.map(str::to_owned))
        .bind(sealed_body.map(str::to_owned))
        .bind(claim.0)
        .bind(claim.1.0)
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn release_idempotent(&self, principal: &str, key: &str, claim: (&str, Timestamp)) -> DomainResult<()> {
        sqlx::query(
            "DELETE FROM idempotency_keys
             WHERE principal = $1 AND idem_key = $2 AND state = 'pending' AND request_hash = $3 AND created_at = $4",
        )
        .bind(principal)
        .bind(key)
        .bind(claim.0)
        .bind(claim.1.0)
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn count_idempotent(&self, principal: &str, now: Timestamp) -> DomainResult<u64> {
        let r = sqlx::query("SELECT COUNT(*) AS n FROM idempotency_keys WHERE principal = $1 AND expires_at > $2")
            .bind(principal)
            .bind(now.0)
            .fetch_one(self.pool())
            .await
            .map_err(db_err)?;
        Ok(u64::try_from(int(&r, "n")?).unwrap_or(0))
    }
}
