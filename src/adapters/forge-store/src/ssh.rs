use async_trait::async_trait;
use forge_core::model::SshEndpoint;
use forge_core::ports::SshEndpointRepository;
use forge_core::*;
use sqlx::any::AnyRow;

use crate::row::*;
use crate::{Store, db_err};

fn endpoint_row(r: &AnyRow) -> DomainResult<SshEndpoint> {
    Ok(SshEndpoint {
        server_id: parsed(r, "server_id")?,
        host: text(r, "host")?,
        port: u16::try_from(int(r, "port")?).map_err(DomainError::internal)?,
        host_key_fingerprint: text(r, "host_key")?,
        updated_at: ts(r, "updated_at")?,
    })
}

#[async_trait]
impl SshEndpointRepository for Store {
    async fn get_ssh_endpoint(&self, server: ServerId) -> DomainResult<Option<SshEndpoint>> {
        let r = sqlx::query("SELECT * FROM node_ssh WHERE server_id = $1")
            .bind(server.to_string())
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(endpoint_row).transpose()
    }

    async fn put_ssh_endpoint(&self, e: &SshEndpoint) -> DomainResult<()> {
        sqlx::query(
            "INSERT INTO node_ssh (server_id, host, port, host_key, updated_at) VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (server_id) DO UPDATE SET host = $2, port = $3, host_key = $4, updated_at = $5",
        )
        .bind(e.server_id.to_string())
        .bind(&e.host)
        .bind(i64::from(e.port))
        .bind(&e.host_key_fingerprint)
        .bind(e.updated_at.0)
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }
}
