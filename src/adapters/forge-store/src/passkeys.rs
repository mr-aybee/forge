use async_trait::async_trait;
use forge_core::passkey::{Passkey, PasskeyRepository};
use forge_core::*;
use sqlx::any::AnyRow;

use crate::row::*;
use crate::{Store, db_err};

fn row(r: &AnyRow) -> DomainResult<Passkey> {
    Ok(Passkey {
        id: text(r, "id")?,
        user_id: parsed(r, "user_id")?,
        credential_id: text(r, "credential_id")?,
        public_key: text(r, "public_key")?,
        sign_count: u32_col(r, "sign_count")?,
        name: text(r, "name")?,
        created_at: ts(r, "created_at")?,
        last_used_at: opt_ts(r, "last_used_at")?,
    })
}

#[async_trait]
impl PasskeyRepository for Store {
    async fn add_passkey(&self, p: &Passkey) -> DomainResult<()> {
        sqlx::query(
            "INSERT INTO passkeys (id, user_id, credential_id, public_key, sign_count, name, created_at, last_used_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(&p.id)
        .bind(p.user_id.to_string())
        .bind(&p.credential_id)
        .bind(&p.public_key)
        .bind(i64::from(p.sign_count))
        .bind(&p.name)
        .bind(p.created_at.0)
        .bind(p.last_used_at.map(|t| t.0))
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn list_passkeys(&self, user: UserId) -> DomainResult<Vec<Passkey>> {
        let rows = sqlx::query("SELECT * FROM passkeys WHERE user_id = $1 ORDER BY created_at")
            .bind(user.to_string())
            .fetch_all(self.pool())
            .await
            .map_err(db_err)?;
        rows.iter().map(row).collect()
    }

    async fn passkey_by_credential(&self, credential_id: &str) -> DomainResult<Option<Passkey>> {
        let r = sqlx::query("SELECT * FROM passkeys WHERE credential_id = $1")
            .bind(credential_id)
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(row).transpose()
    }

    async fn touch_passkey(&self, id: &str, sign_count: u32, at: Timestamp) -> DomainResult<bool> {
        // The counter only moves forward (0 = the authenticator does not
        // count), so concurrent sign-ins cannot roll it back.
        let r = sqlx::query(
            "UPDATE passkeys SET sign_count = $2, last_used_at = $3
             WHERE id = $1 AND (sign_count < $2 OR $2 = 0)",
        )
        .bind(id)
        .bind(i64::from(sign_count))
        .bind(at.0)
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(r.rows_affected() > 0)
    }

    async fn rename_passkey(&self, id: &str, user: UserId, name: &str) -> DomainResult<bool> {
        let r = sqlx::query("UPDATE passkeys SET name = $3 WHERE id = $1 AND user_id = $2")
            .bind(id)
            .bind(user.to_string())
            .bind(name)
            .execute(self.pool())
            .await
            .map_err(db_err)?;
        Ok(r.rows_affected() > 0)
    }

    async fn delete_passkey(&self, id: &str, user: UserId) -> DomainResult<bool> {
        let r = sqlx::query("DELETE FROM passkeys WHERE id = $1 AND user_id = $2")
            .bind(id)
            .bind(user.to_string())
            .execute(self.pool())
            .await
            .map_err(db_err)?;
        Ok(r.rows_affected() > 0)
    }

    async fn delete_user_passkeys(&self, user: UserId) -> DomainResult<u64> {
        let r = sqlx::query("DELETE FROM passkeys WHERE user_id = $1")
            .bind(user.to_string())
            .execute(self.pool())
            .await
            .map_err(db_err)?;
        Ok(r.rows_affected())
    }

    async fn put_challenge(
        &self,
        hash: &str,
        user: Option<UserId>,
        purpose: &str,
        expires_at: Timestamp,
    ) -> DomainResult<()> {
        // Expired challenges go every 32nd insert (indexed on expires_at):
        // `expires_at` is now + the 5-minute lifetime, so anything older
        // than now has expired.
        static INSERTS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        if INSERTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed).is_multiple_of(32) {
            sqlx::query("DELETE FROM webauthn_challenges WHERE expires_at < $1")
                .bind(expires_at.0 - 300_000)
                .execute(self.pool())
                .await
                .map_err(db_err)?;
        }
        sqlx::query(
            "INSERT INTO webauthn_challenges (challenge_hash, user_id, purpose, expires_at) VALUES ($1, $2, $3, $4)",
        )
        .bind(hash)
        .bind(user.map(|u| u.to_string()))
        .bind(purpose)
        .bind(expires_at.0)
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn take_challenge(&self, hash: &str) -> DomainResult<Option<(Option<UserId>, String, Timestamp)>> {
        // One atomic statement: of two concurrent callers only one gets the
        // row (no SELECT-then-DELETE race on Postgres).
        let r = sqlx::query(
            "DELETE FROM webauthn_challenges WHERE challenge_hash = $1 RETURNING user_id, purpose, expires_at",
        )
        .bind(hash)
        .fetch_optional(self.pool())
        .await
        .map_err(db_err)?;
        match r {
            Some(r) => Ok(Some((opt_parsed(&r, "user_id")?, text(&r, "purpose")?, ts(&r, "expires_at")?))),
            None => Ok(None),
        }
    }
}
