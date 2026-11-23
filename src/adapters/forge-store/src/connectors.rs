//! MCP connectors: dynamically registered OAuth clients and the connections
//! people approved (issue #6, migration 0013).

use async_trait::async_trait;
use forge_core::model::{AgentConnection, AgentMode, OAuthClientRecord};
use forge_core::ports::ConnectorRepository;
use forge_core::*;
use sqlx::any::AnyRow;

use crate::row::*;
use crate::{Store, db_err};

fn client_row(r: &AnyRow) -> DomainResult<OAuthClientRecord> {
    Ok(OAuthClientRecord {
        client_id: text(r, "client_id")?,
        name: text(r, "name")?,
        redirect_uris: serde_json::from_str(&text(r, "redirect_uris")?).map_err(DomainError::internal)?,
        created_at: ts(r, "created_at")?,
        created_ip: opt_text(r, "created_ip")?,
        revoked_at: opt_ts(r, "revoked_at")?,
        last_used_at: opt_ts(r, "last_used_at")?,
    })
}

fn connection_row(r: &AnyRow) -> DomainResult<AgentConnection> {
    let mode = text(r, "mode")?;
    Ok(AgentConnection {
        id: text(r, "id")?,
        user_id: parsed(r, "user_id")?,
        client_id: text(r, "client_id")?,
        client_name: text(r, "client_name")?,
        redirect_host: text(r, "redirect_host")?,
        resource: text(r, "resource")?,
        mode: AgentMode::parse(&mode).unwrap_or(AgentMode::ReadOnly),
        created_at: ts(r, "created_at")?,
        last_used_at: opt_ts(r, "last_used_at")?,
        revoked_at: opt_ts(r, "revoked_at")?,
    })
}

#[async_trait]
impl ConnectorRepository for Store {
    async fn create_client(&self, c: &OAuthClientRecord) -> DomainResult<()> {
        sqlx::query(
            "INSERT INTO oauth_clients (client_id, name, redirect_uris, created_at, created_ip, revoked_at, last_used_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(&c.client_id)
        .bind(&c.name)
        .bind(serde_json::to_string(&c.redirect_uris).map_err(DomainError::internal)?)
        .bind(c.created_at.0)
        .bind(c.created_ip.clone())
        .bind(c.revoked_at.map(|t| t.0))
        .bind(c.last_used_at.map(|t| t.0))
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn get_client(&self, client_id: &str) -> DomainResult<Option<OAuthClientRecord>> {
        let r = sqlx::query("SELECT * FROM oauth_clients WHERE client_id = $1")
            .bind(client_id)
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(client_row).transpose()
    }

    async fn list_clients(&self) -> DomainResult<Vec<OAuthClientRecord>> {
        let rows = sqlx::query("SELECT * FROM oauth_clients ORDER BY created_at DESC")
            .fetch_all(self.pool())
            .await
            .map_err(db_err)?;
        rows.iter().map(client_row).collect()
    }

    async fn count_clients_since(&self, since: Timestamp) -> DomainResult<u64> {
        let r = sqlx::query("SELECT COUNT(*) AS n FROM oauth_clients WHERE created_at >= $1")
            .bind(since.0)
            .fetch_one(self.pool())
            .await
            .map_err(db_err)?;
        Ok(u64::try_from(int(&r, "n")?).unwrap_or(0))
    }

    async fn revoke_client(&self, client_id: &str, at: Timestamp) -> DomainResult<()> {
        let mut tx = self.pool().begin().await.map_err(db_err)?;
        sqlx::query("UPDATE oauth_clients SET revoked_at = $1 WHERE client_id = $2 AND revoked_at IS NULL")
            .bind(at.0)
            .bind(client_id)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        sqlx::query(
            "DELETE FROM sessions WHERE connection_id IN (SELECT id FROM agent_connections WHERE client_id = $1)",
        )
        .bind(client_id)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        sqlx::query("UPDATE agent_connections SET revoked_at = $1 WHERE client_id = $2 AND revoked_at IS NULL")
            .bind(at.0)
            .bind(client_id)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        tx.commit().await.map_err(db_err)
    }

    async fn create_connection(&self, c: &AgentConnection) -> DomainResult<()> {
        sqlx::query(
            "INSERT INTO agent_connections (id, user_id, client_id, client_name, redirect_host, resource, mode,
                                            created_at, last_used_at, revoked_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
        )
        .bind(&c.id)
        .bind(c.user_id.to_string())
        .bind(&c.client_id)
        .bind(&c.client_name)
        .bind(&c.redirect_host)
        .bind(&c.resource)
        .bind(c.mode.as_str())
        .bind(c.created_at.0)
        .bind(c.last_used_at.map(|t| t.0))
        .bind(c.revoked_at.map(|t| t.0))
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn get_connection(&self, id: &str) -> DomainResult<Option<AgentConnection>> {
        let r = sqlx::query("SELECT * FROM agent_connections WHERE id = $1")
            .bind(id)
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(connection_row).transpose()
    }

    async fn list_connections(&self, user: Option<UserId>) -> DomainResult<Vec<AgentConnection>> {
        let rows = match user {
            Some(u) => {
                sqlx::query("SELECT * FROM agent_connections WHERE user_id = $1 ORDER BY created_at DESC")
                    .bind(u.to_string())
                    .fetch_all(self.pool())
                    .await
            }
            None => {
                sqlx::query("SELECT * FROM agent_connections ORDER BY created_at DESC").fetch_all(self.pool()).await
            }
        }
        .map_err(db_err)?;
        rows.iter().map(connection_row).collect()
    }

    async fn revoke_connection(&self, id: &str, at: Timestamp) -> DomainResult<()> {
        let mut tx = self.pool().begin().await.map_err(db_err)?;
        sqlx::query("UPDATE agent_connections SET revoked_at = $1 WHERE id = $2 AND revoked_at IS NULL")
            .bind(at.0)
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        sqlx::query("DELETE FROM sessions WHERE connection_id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        tx.commit().await.map_err(db_err)
    }

    async fn touch_connection(&self, id: &str, at: Timestamp) -> DomainResult<()> {
        sqlx::query("UPDATE agent_connections SET last_used_at = $1 WHERE id = $2")
            .bind(at.0)
            .bind(id)
            .execute(self.pool())
            .await
            .map_err(db_err)?;
        Ok(())
    }
}
