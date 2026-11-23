//! Bucket grants (D-108): one person's access to one bucket of a store.
//! The rows of a store go with it (`delete_object_store`).

use async_trait::async_trait;
use forge_core::storage::{BucketAccess, BucketGrant, BucketGrantRepository};
use forge_core::*;
use sqlx::any::AnyRow;

use crate::row::*;
use crate::{Store, db_err};

fn access(r: &AnyRow) -> DomainResult<BucketAccess> {
    let s = text(r, "access")?;
    BucketAccess::parse(&s).ok_or_else(|| DomainError::internal(format!("unknown bucket access `{s}`")))
}

fn grant_row(r: &AnyRow) -> DomainResult<BucketGrant> {
    Ok(BucketGrant {
        id: parsed(r, "id")?,
        workspace_id: parsed(r, "workspace_id")?,
        user_id: parsed(r, "user_id")?,
        store_id: parsed(r, "store_id")?,
        bucket: text(r, "bucket")?,
        access: access(r)?,
        created_by: text(r, "created_by")?,
        created_at: ts(r, "created_at")?,
        expires_at: ts(r, "expires_at")?,
        revoked_at: opt_ts(r, "revoked_at")?,
        note: text(r, "note")?,
    })
}

#[async_trait]
impl BucketGrantRepository for Store {
    async fn put_bucket_grant(&self, g: &BucketGrant) -> DomainResult<()> {
        // A revocation is final: an existing `revoked_at` is never replaced
        // and never cleared, whatever the saved copy says.
        sqlx::query(
            "INSERT INTO bucket_grants (id, workspace_id, user_id, store_id, bucket, access, created_by, created_at,
                                        expires_at, revoked_at, note)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
             ON CONFLICT (id) DO UPDATE SET revoked_at = COALESCE(bucket_grants.revoked_at, $10)",
        )
        .bind(g.id.to_string())
        .bind(g.workspace_id.to_string())
        .bind(g.user_id.to_string())
        .bind(g.store_id.to_string())
        .bind(&g.bucket)
        .bind(g.access.as_str())
        .bind(&g.created_by)
        .bind(g.created_at.0)
        .bind(g.expires_at.0)
        .bind(g.revoked_at.map(|t| t.0))
        .bind(&g.note)
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn get_bucket_grant(&self, id: BucketGrantId) -> DomainResult<Option<BucketGrant>> {
        let r = sqlx::query("SELECT * FROM bucket_grants WHERE id = $1")
            .bind(id.to_string())
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(grant_row).transpose()
    }

    async fn list_live_bucket_grants(&self, user: UserId, now: Timestamp) -> DomainResult<Vec<BucketGrant>> {
        let rows = sqlx::query(
            "SELECT * FROM bucket_grants WHERE user_id = $1 AND revoked_at IS NULL AND expires_at > $2
             ORDER BY created_at DESC, id",
        )
        .bind(user.to_string())
        .bind(now.0)
        .fetch_all(self.pool())
        .await
        .map_err(db_err)?;
        rows.iter().map(grant_row).collect()
    }

    async fn list_live_grants_of_bucket(
        &self,
        store: ObjectStoreId,
        bucket: &str,
        now: Timestamp,
    ) -> DomainResult<Vec<BucketGrant>> {
        let rows = sqlx::query(
            "SELECT * FROM bucket_grants
             WHERE store_id = $1 AND bucket = $2 AND revoked_at IS NULL AND expires_at > $3
             ORDER BY created_at DESC, id",
        )
        .bind(store.to_string())
        .bind(bucket)
        .bind(now.0)
        .fetch_all(self.pool())
        .await
        .map_err(db_err)?;
        rows.iter().map(grant_row).collect()
    }

    async fn list_store_bucket_grants(
        &self,
        store: ObjectStoreId,
        live_at: Option<Timestamp>,
        limit: u32,
    ) -> DomainResult<Vec<BucketGrant>> {
        // `$2 < 0` stands for "every grant": no timestamp is negative.
        let rows = sqlx::query(
            "SELECT * FROM bucket_grants
             WHERE store_id = $1 AND ($2 < 0 OR (revoked_at IS NULL AND expires_at > $2))
             ORDER BY created_at DESC, id LIMIT $3",
        )
        .bind(store.to_string())
        .bind(live_at.map_or(-1, |t| t.0))
        .bind(i64::from(limit))
        .fetch_all(self.pool())
        .await
        .map_err(db_err)?;
        rows.iter().map(grant_row).collect()
    }
}
