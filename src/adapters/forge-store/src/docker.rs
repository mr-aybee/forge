use async_trait::async_trait;
use forge_core::model::DockerEndpoint;
use forge_core::ports::DockerEndpointRepository;
use forge_core::*;
use sqlx::any::AnyRow;

use crate::row::*;
use crate::{Store, db_err};

fn endpoint_row(r: &AnyRow) -> DomainResult<DockerEndpoint> {
    Ok(DockerEndpoint {
        server_id: parsed(r, "server_id")?,
        host: text(r, "host")?,
        port: u16::try_from(int(r, "port")?).map_err(DomainError::internal)?,
        state: parsed(r, "state")?,
        ca_cert_pem: text(r, "ca_cert")?,
        ca_key_sealed: text(r, "ca_key")?,
        daemon_cert_not_after: ts(r, "daemon_cert_not_after")?,
        engine_version: opt_text(r, "engine_version")?,
        api_version: opt_text(r, "api_version")?,
        last_error: opt_text(r, "last_error")?,
        created_at: ts(r, "created_at")?,
        updated_at: ts(r, "updated_at")?,
    })
}

#[async_trait]
impl DockerEndpointRepository for Store {
    async fn get_docker_endpoint(&self, server: ServerId) -> DomainResult<Option<DockerEndpoint>> {
        let r = sqlx::query("SELECT * FROM docker_endpoints WHERE server_id = $1")
            .bind(server.to_string())
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(endpoint_row).transpose()
    }

    async fn put_docker_endpoint(&self, e: &DockerEndpoint) -> DomainResult<()> {
        sqlx::query(
            "INSERT INTO docker_endpoints (server_id, host, port, state, ca_cert, ca_key, daemon_cert_not_after,
                 engine_version, api_version, last_error, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
             ON CONFLICT (server_id) DO UPDATE SET host = $2, port = $3, state = $4, ca_cert = $5, ca_key = $6,
                 daemon_cert_not_after = $7, engine_version = $8, api_version = $9, last_error = $10,
                 updated_at = $12",
        )
        .bind(e.server_id.to_string())
        .bind(&e.host)
        .bind(i64::from(e.port))
        .bind(e.state.as_str())
        .bind(&e.ca_cert_pem)
        .bind(&e.ca_key_sealed)
        .bind(e.daemon_cert_not_after.0)
        .bind(e.engine_version.clone())
        .bind(e.api_version.clone())
        .bind(e.last_error.clone())
        .bind(e.created_at.0)
        .bind(e.updated_at.0)
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn delete_docker_endpoint(&self, server: ServerId) -> DomainResult<()> {
        sqlx::query("DELETE FROM docker_endpoints WHERE server_id = $1")
            .bind(server.to_string())
            .execute(self.pool())
            .await
            .map_err(db_err)?;
        Ok(())
    }
}
