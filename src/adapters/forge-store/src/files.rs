use async_trait::async_trait;
use forge_core::files::{
    ApprovedRoot, FileGrant, FileGrantRepository, FilePath, FileRepository, FileUpload, Overwrite, RootKind, RootSpec,
};
use forge_core::*;
use sqlx::any::AnyRow;

use crate::row::*;
use crate::{Store, db_err};

fn spec_col(r: &AnyRow, col: &str) -> DomainResult<RootSpec> {
    serde_json::from_str(&text(r, col)?).map_err(DomainError::internal)
}

fn overwrite_col(r: &AnyRow, col: &str) -> DomainResult<Overwrite> {
    serde_json::from_str(&text(r, col)?).map_err(DomainError::internal)
}

fn kind_str(k: RootKind) -> &'static str {
    match k {
        RootKind::Node => "node",
        RootKind::Service => "service",
        RootKind::Container => "container",
    }
}

fn root_row(r: &AnyRow) -> DomainResult<ApprovedRoot> {
    let kind = match text(r, "kind")?.as_str() {
        "service" => RootKind::Service,
        "container" => RootKind::Container,
        _ => RootKind::Node,
    };
    Ok(ApprovedRoot {
        id: text(r, "id")?,
        workspace_id: parsed(r, "workspace_id")?,
        server_id: parsed(r, "server_id")?,
        kind,
        label: text(r, "label")?,
        spec: spec_col(r, "spec")?,
        read_only: flag(r, "read_only")?,
        service_id: opt_text(r, "service_id")?,
        created_by: text(r, "created_by")?,
        created_at: ts(r, "created_at")?,
    })
}

fn upload_row(r: &AnyRow) -> DomainResult<FileUpload> {
    let u64_col = |c: &str| int(r, c).map(|v| u64::try_from(v).unwrap_or(0));
    Ok(FileUpload {
        id: text(r, "id")?,
        workspace_id: parsed(r, "workspace_id")?,
        server_id: parsed(r, "server_id")?,
        root_id: text(r, "root_id")?,
        path: FilePath::parse(&text(r, "path")?).map_err(DomainError::Internal)?,
        size: u64_col("size")?,
        received: u64_col("received")?,
        overwrite: overwrite_col(r, "overwrite")?,
        created_by: text(r, "created_by")?,
        created_at: ts(r, "created_at")?,
        expires_at: ts(r, "expires_at")?,
    })
}

fn signed(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

#[async_trait]
impl FileRepository for Store {
    async fn put_root(&self, root: &ApprovedRoot) -> DomainResult<()> {
        sqlx::query(
            "INSERT INTO file_roots (id, workspace_id, server_id, kind, label, spec, read_only, service_id,
                                     created_by, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
             ON CONFLICT (id) DO UPDATE SET label = $5, read_only = $7",
        )
        .bind(&root.id)
        .bind(root.workspace_id.to_string())
        .bind(root.server_id.to_string())
        .bind(kind_str(root.kind))
        .bind(&root.label)
        .bind(serde_json::to_string(&root.spec).map_err(DomainError::internal)?)
        .bind(i64::from(root.read_only))
        .bind(root.service_id.clone())
        .bind(&root.created_by)
        .bind(root.created_at.0)
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn get_root(&self, id: &str) -> DomainResult<Option<ApprovedRoot>> {
        let r = sqlx::query("SELECT * FROM file_roots WHERE id = $1")
            .bind(id)
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(root_row).transpose()
    }

    async fn list_roots(&self, server: ServerId) -> DomainResult<Vec<ApprovedRoot>> {
        let rows = sqlx::query("SELECT * FROM file_roots WHERE server_id = $1 ORDER BY label")
            .bind(server.to_string())
            .fetch_all(self.pool())
            .await
            .map_err(db_err)?;
        rows.iter().map(root_row).collect()
    }

    async fn delete_root(&self, id: &str) -> DomainResult<()> {
        sqlx::query("DELETE FROM file_roots WHERE id = $1").bind(id).execute(self.pool()).await.map_err(db_err)?;
        Ok(())
    }

    async fn put_upload(&self, u: &FileUpload) -> DomainResult<()> {
        sqlx::query(
            "INSERT INTO file_uploads (id, workspace_id, server_id, root_id, path, size, received, overwrite,
                                       created_by, created_at, expires_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
             ON CONFLICT (id) DO UPDATE SET received = $7, expires_at = $11",
        )
        .bind(&u.id)
        .bind(u.workspace_id.to_string())
        .bind(u.server_id.to_string())
        .bind(&u.root_id)
        .bind(u.path.to_string())
        .bind(signed(u.size))
        .bind(signed(u.received))
        .bind(serde_json::to_string(&u.overwrite).map_err(DomainError::internal)?)
        .bind(&u.created_by)
        .bind(u.created_at.0)
        .bind(u.expires_at.0)
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn get_upload(&self, id: &str) -> DomainResult<Option<FileUpload>> {
        let r = sqlx::query("SELECT * FROM file_uploads WHERE id = $1")
            .bind(id)
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(upload_row).transpose()
    }

    async fn delete_upload(&self, id: &str) -> DomainResult<()> {
        sqlx::query("DELETE FROM file_uploads WHERE id = $1").bind(id).execute(self.pool()).await.map_err(db_err)?;
        Ok(())
    }

    async fn expired_uploads(&self, now: Timestamp) -> DomainResult<Vec<FileUpload>> {
        let rows = sqlx::query("SELECT * FROM file_uploads WHERE expires_at < $1 ORDER BY expires_at")
            .bind(now.0)
            .fetch_all(self.pool())
            .await
            .map_err(db_err)?;
        rows.iter().map(upload_row).collect()
    }
}

fn strings_col(r: &AnyRow, col: &str) -> DomainResult<Vec<String>> {
    serde_json::from_str(&text(r, col)?).map_err(DomainError::internal)
}

fn grant_row(r: &AnyRow) -> DomainResult<FileGrant> {
    let unsigned = |v: Option<i64>| v.map(|n| u64::try_from(n).unwrap_or(0));
    Ok(FileGrant {
        id: text(r, "id")?,
        workspace_id: parsed(r, "workspace_id")?,
        server_id: parsed(r, "server_id")?,
        name: text(r, "name")?,
        roots: strings_col(r, "roots")?,
        write: flag(r, "can_write")?,
        delete: flag(r, "can_delete")?,
        token_hash: text(r, "token_hash")?,
        owner: opt_text(r, "owner")?.map_or_else(|| text(r, "created_by"), Ok)?,
        created_by: text(r, "created_by")?,
        created_at: ts(r, "created_at")?,
        expires_at: ts(r, "expires_at")?,
        revoked_at: opt_ts(r, "revoked_at")?,
        allowed_ips: strings_col(r, "allowed_ips")?,
        max_sessions: u32::try_from(int(r, "max_sessions")?).unwrap_or(1),
        bandwidth_kbps: opt_int(r, "bandwidth_kbps")?.and_then(|v| u32::try_from(v).ok()),
        transfer_limit: unsigned(opt_int(r, "transfer_limit")?),
        bytes_transferred: unsigned(Some(int(r, "bytes_transferred")?)).unwrap_or(0),
        last_used_at: opt_ts(r, "last_used_at")?,
    })
}

#[async_trait]
impl FileGrantRepository for Store {
    async fn put_grant(&self, g: &FileGrant) -> DomainResult<()> {
        let json = |v: &Vec<String>| serde_json::to_string(v).map_err(DomainError::internal);
        sqlx::query(
            "INSERT INTO file_grants (id, workspace_id, server_id, name, roots, can_write, can_delete, token_hash,
                                      created_by, created_at, expires_at, revoked_at, allowed_ips, max_sessions,
                                      bandwidth_kbps, transfer_limit, bytes_transferred, last_used_at, owner)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19)
             ON CONFLICT (id) DO UPDATE SET name = $4, roots = $5, can_write = $6, can_delete = $7,
                 token_hash = $8, expires_at = $11, revoked_at = $12, allowed_ips = $13, max_sessions = $14,
                 bandwidth_kbps = $15, transfer_limit = $16, owner = $19",
        )
        .bind(&g.id)
        .bind(g.workspace_id.to_string())
        .bind(g.server_id.to_string())
        .bind(&g.name)
        .bind(json(&g.roots)?)
        .bind(i64::from(g.write))
        .bind(i64::from(g.delete))
        .bind(&g.token_hash)
        .bind(&g.created_by)
        .bind(g.created_at.0)
        .bind(g.expires_at.0)
        .bind(g.revoked_at.map(|t| t.0))
        .bind(json(&g.allowed_ips)?)
        .bind(i64::from(g.max_sessions))
        .bind(g.bandwidth_kbps.map(i64::from))
        .bind(g.transfer_limit.map(signed))
        .bind(signed(g.bytes_transferred))
        .bind(g.last_used_at.map(|t| t.0))
        .bind(&g.owner)
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn get_grant(&self, id: &str) -> DomainResult<Option<FileGrant>> {
        let r = sqlx::query("SELECT * FROM file_grants WHERE id = $1")
            .bind(id)
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(grant_row).transpose()
    }

    async fn list_grants(&self, server: ServerId) -> DomainResult<Vec<FileGrant>> {
        let rows = sqlx::query("SELECT * FROM file_grants WHERE server_id = $1 ORDER BY created_at DESC")
            .bind(server.to_string())
            .fetch_all(self.pool())
            .await
            .map_err(db_err)?;
        rows.iter().map(grant_row).collect()
    }

    async fn list_all_grants(&self) -> DomainResult<Vec<FileGrant>> {
        let rows = sqlx::query("SELECT * FROM file_grants ORDER BY created_at DESC")
            .fetch_all(self.pool())
            .await
            .map_err(db_err)?;
        rows.iter().map(grant_row).collect()
    }

    async fn add_grant_usage(&self, id: &str, bytes: u64, at: Timestamp) -> DomainResult<()> {
        sqlx::query(
            "UPDATE file_grants SET bytes_transferred = bytes_transferred + $2, last_used_at = $3 WHERE id = $1",
        )
        .bind(id)
        .bind(signed(bytes))
        .bind(at.0)
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }
}
