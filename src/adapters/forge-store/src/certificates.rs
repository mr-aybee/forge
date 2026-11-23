use async_trait::async_trait;
use forge_core::model::TlsCertificate;
use forge_core::ports::CertificateRepository;
use forge_core::*;
use sqlx::any::AnyRow;

use crate::row::*;
use crate::{Store, db_err};

fn cert_row(r: &AnyRow) -> DomainResult<TlsCertificate> {
    Ok(TlsCertificate {
        id: parsed(r, "id")?,
        workspace_id: parsed(r, "workspace_id")?,
        name: text(r, "name")?,
        domains: serde_json::from_value(json(r, "domains")?).map_err(DomainError::internal)?,
        issuer: text(r, "issuer")?,
        not_before: ts(r, "not_before")?,
        not_after: ts(r, "not_after")?,
        fingerprint: text(r, "fingerprint")?,
        chain_pem: text(r, "chain_pem")?,
        sealed_key: text(r, "sealed_key")?,
        created_by: text(r, "created_by")?.parse().map_err(DomainError::Internal)?,
        created_at: ts(r, "created_at")?,
        updated_at: ts(r, "updated_at")?,
    })
}

#[async_trait]
impl CertificateRepository for Store {
    async fn put_certificate(&self, c: &TlsCertificate) -> DomainResult<()> {
        sqlx::query(
            "INSERT INTO tls_certificates (id, workspace_id, name, domains, issuer, not_before, not_after, fingerprint, chain_pem, sealed_key, created_by, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
             ON CONFLICT (id) DO UPDATE SET name = $3, domains = $4, issuer = $5, not_before = $6, not_after = $7,
               fingerprint = $8, chain_pem = $9, sealed_key = $10, updated_at = $13",
        )
        .bind(c.id.to_string())
        .bind(c.workspace_id.to_string())
        .bind(&c.name)
        .bind(serde_json::to_string(&c.domains).map_err(DomainError::internal)?)
        .bind(&c.issuer)
        .bind(c.not_before.0)
        .bind(c.not_after.0)
        .bind(&c.fingerprint)
        .bind(&c.chain_pem)
        .bind(&c.sealed_key)
        .bind(c.created_by.to_string())
        .bind(c.created_at.0)
        .bind(c.updated_at.0)
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn get_certificate(&self, id: CertificateId) -> DomainResult<Option<TlsCertificate>> {
        let r = sqlx::query("SELECT * FROM tls_certificates WHERE id = $1")
            .bind(id.to_string())
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(cert_row).transpose()
    }

    async fn list_certificates(&self, workspace: Option<WorkspaceId>) -> DomainResult<Vec<TlsCertificate>> {
        let rows = match workspace {
            Some(ws) => {
                sqlx::query("SELECT * FROM tls_certificates WHERE workspace_id = $1 ORDER BY name")
                    .bind(ws.to_string())
                    .fetch_all(self.pool())
                    .await
            }
            None => sqlx::query("SELECT * FROM tls_certificates ORDER BY name").fetch_all(self.pool()).await,
        }
        .map_err(db_err)?;
        rows.iter().map(cert_row).collect()
    }

    async fn delete_certificate(&self, id: CertificateId) -> DomainResult<()> {
        sqlx::query("DELETE FROM tls_certificates WHERE id = $1")
            .bind(id.to_string())
            .execute(self.pool())
            .await
            .map_err(db_err)?;
        Ok(())
    }
}
