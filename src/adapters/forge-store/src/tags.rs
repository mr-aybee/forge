//! Tags and node facts (migration 0010).

use std::collections::HashMap;

use async_trait::async_trait;
use forge_core::tags::{FactsRepository, NodeLoginRepository, TagKind, TagRepository};
use forge_core::*;

use crate::row::*;
use crate::{Store, db_err};

#[async_trait]
impl TagRepository for Store {
    async fn set_tags(&self, kind: TagKind, id: &str, tags: &[String], now: Timestamp) -> DomainResult<()> {
        let mut tx = self.pool().begin().await.map_err(db_err)?;
        sqlx::query("DELETE FROM entity_tags WHERE kind = $1 AND entity_id = $2")
            .bind(kind.as_str())
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;

        for t in tags {
            sqlx::query("INSERT INTO entity_tags (kind, entity_id, tag, created_at) VALUES ($1, $2, $3, $4)")
                .bind(kind.as_str())
                .bind(id)
                .bind(t)
                .bind(now.0)
                .execute(&mut *tx)
                .await
                .map_err(db_err)?;
        }

        tx.commit().await.map_err(db_err)?;

        Ok(())
    }

    async fn tags_of(&self, kind: TagKind, id: &str) -> DomainResult<Vec<String>> {
        let rows = sqlx::query("SELECT tag FROM entity_tags WHERE kind = $1 AND entity_id = $2 ORDER BY tag")
            .bind(kind.as_str())
            .bind(id)
            .fetch_all(self.pool())
            .await
            .map_err(db_err)?;
        rows.iter().map(|r| text(r, "tag")).collect()
    }

    async fn tags_of_kind(&self, kind: TagKind) -> DomainResult<HashMap<String, Vec<String>>> {
        let rows = sqlx::query("SELECT entity_id, tag FROM entity_tags WHERE kind = $1 ORDER BY tag")
            .bind(kind.as_str())
            .fetch_all(self.pool())
            .await
            .map_err(db_err)?;
        let mut out: HashMap<String, Vec<String>> = HashMap::new();

        for r in &rows {
            out.entry(text(r, "entity_id")?).or_default().push(text(r, "tag")?);
        }

        Ok(out)
    }

    async fn distinct_tags(&self, kind: TagKind) -> DomainResult<Vec<(String, u64)>> {
        let rows = sqlx::query("SELECT tag, COUNT(*) AS n FROM entity_tags WHERE kind = $1 GROUP BY tag ORDER BY tag")
            .bind(kind.as_str())
            .fetch_all(self.pool())
            .await
            .map_err(db_err)?;
        rows.iter().map(|r| Ok((text(r, "tag")?, u64::try_from(int(r, "n")?).unwrap_or(0)))).collect()
    }
}

#[async_trait]
impl NodeLoginRepository for Store {
    async fn set_logins(&self, server: ServerId, logins: &[String], now: Timestamp) -> DomainResult<()> {
        let mut tx = self.pool().begin().await.map_err(db_err)?;
        sqlx::query("DELETE FROM server_logins WHERE server_id = $1")
            .bind(server.to_string())
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;

        for l in logins {
            sqlx::query("INSERT INTO server_logins (server_id, login, created_at) VALUES ($1, $2, $3)")
                .bind(server.to_string())
                .bind(l)
                .bind(now.0)
                .execute(&mut *tx)
                .await
                .map_err(db_err)?;
        }

        tx.commit().await.map_err(db_err)?;

        Ok(())
    }

    async fn logins_of(&self, server: ServerId) -> DomainResult<Vec<String>> {
        let rows = sqlx::query("SELECT login FROM server_logins WHERE server_id = $1 ORDER BY created_at, login")
            .bind(server.to_string())
            .fetch_all(self.pool())
            .await
            .map_err(db_err)?;
        rows.iter().map(|r| text(r, "login")).collect()
    }
}

#[async_trait]
impl FactsRepository for Store {
    async fn put_facts(&self, server: ServerId, facts: &serde_json::Value, now: Timestamp) -> DomainResult<()> {
        let body = serde_json::to_string(facts).map_err(DomainError::internal)?;
        let mut tx = self.pool().begin().await.map_err(db_err)?;
        sqlx::query("DELETE FROM server_facts WHERE server_id = $1")
            .bind(server.to_string())
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        sqlx::query("INSERT INTO server_facts (server_id, facts, collected_at) VALUES ($1, $2, $3)")
            .bind(server.to_string())
            .bind(body)
            .bind(now.0)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        tx.commit().await.map_err(db_err)?;

        Ok(())
    }

    async fn all_facts(&self) -> DomainResult<HashMap<ServerId, (serde_json::Value, Timestamp)>> {
        let rows = sqlx::query("SELECT server_id, facts, collected_at FROM server_facts")
            .fetch_all(self.pool())
            .await
            .map_err(db_err)?;
        let mut out = HashMap::new();

        for r in &rows {
            let id: ServerId = parsed(r, "server_id")?;
            let facts = serde_json::from_str(&text(r, "facts")?).map_err(DomainError::internal)?;
            out.insert(id, (facts, ts(r, "collected_at")?));
        }

        Ok(out)
    }

    async fn facts_of(&self, server: ServerId) -> DomainResult<Option<(serde_json::Value, Timestamp)>> {
        let row = sqlx::query("SELECT facts, collected_at FROM server_facts WHERE server_id = $1")
            .bind(server.to_string())
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        row.map(|r| {
            let facts = serde_json::from_str(&text(&r, "facts")?).map_err(DomainError::internal)?;
            Ok((facts, ts(&r, "collected_at")?))
        })
        .transpose()
    }
}
