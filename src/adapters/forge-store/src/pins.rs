//! Per-user pins.

use async_trait::async_trait;
use forge_core::pins::{MAX_PINS, Pin, PinAdded, PinKind, PinRepository};
use forge_core::*;

use crate::row::*;
use crate::{Store, db_err};

fn limit(max: usize) -> i64 {
    i64::try_from(max).unwrap_or(i64::MAX)
}

#[async_trait]
impl PinRepository for Store {
    async fn list_pins(&self, user: UserId) -> DomainResult<Vec<Pin>> {
        let rows = sqlx::query(
            "SELECT kind, entity_id, position, created_at FROM user_pins WHERE user_id = $1 \
             ORDER BY position, created_at LIMIT $2",
        )
        .bind(user.to_string())
        .bind(limit(MAX_PINS))
        .fetch_all(self.pool())
        .await
        .map_err(db_err)?;
        rows.iter()
            .map(|r| {
                Ok(Pin {
                    user_id: user,
                    kind: parsed(r, "kind")?,
                    entity_id: text(r, "entity_id")?,
                    position: int(r, "position")?,
                    created_at: ts(r, "created_at")?,
                })
            })
            .collect()
    }

    async fn add_pin(
        &self,
        user: UserId,
        kind: PinKind,
        entity_id: &str,
        max: usize,
        now: Timestamp,
    ) -> DomainResult<PinAdded> {
        // One statement: the count, the next position and the insert see the
        // same snapshot, so concurrent adds cannot pass the cap together.
        let done = sqlx::query(
            "INSERT INTO user_pins (user_id, kind, entity_id, position, created_at) \
             SELECT $1, $2, $3, COALESCE(MAX(position), -1) + 1, $4 FROM user_pins WHERE user_id = $1 \
             HAVING COUNT(*) < $5 \
             ON CONFLICT (user_id, kind, entity_id) DO NOTHING",
        )
        .bind(user.to_string())
        .bind(kind.as_str())
        .bind(entity_id)
        .bind(now.0)
        .bind(limit(max))
        .execute(self.pool())
        .await
        .map_err(db_err)?;

        if done.rows_affected() > 0 {
            return Ok(PinAdded::Added);
        }

        let exists = sqlx::query("SELECT 1 AS one FROM user_pins WHERE user_id = $1 AND kind = $2 AND entity_id = $3")
            .bind(user.to_string())
            .bind(kind.as_str())
            .bind(entity_id)
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;

        Ok(if exists.is_some() { PinAdded::Exists } else { PinAdded::Full })
    }

    async fn remove_pin(&self, user: UserId, kind: PinKind, entity_id: &str) -> DomainResult<bool> {
        let done = sqlx::query("DELETE FROM user_pins WHERE user_id = $1 AND kind = $2 AND entity_id = $3")
            .bind(user.to_string())
            .bind(kind.as_str())
            .bind(entity_id)
            .execute(self.pool())
            .await
            .map_err(db_err)?;
        Ok(done.rows_affected() > 0)
    }

    async fn set_pin_order(&self, user: UserId, order: &[(PinKind, String)]) -> DomainResult<()> {
        // Atomic: every position changes in one transaction or none does.
        let mut tx = self.pool().begin().await.map_err(db_err)?;

        for (position, (kind, id)) in (0_i64..).zip(order) {
            sqlx::query("UPDATE user_pins SET position = $1 WHERE user_id = $2 AND kind = $3 AND entity_id = $4")
                .bind(position)
                .bind(user.to_string())
                .bind(kind.as_str())
                .bind(id)
                .execute(&mut *tx)
                .await
                .map_err(db_err)?;
        }

        tx.commit().await.map_err(db_err)?;

        Ok(())
    }
}
