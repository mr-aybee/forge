//! Port tunnels (D-100): tunnel history and explicit tunnel grants.

use async_trait::async_trait;
use forge_core::tunnels::{TunnelGrant, TunnelGrantRepository, TunnelRecord, TunnelRepository, TunnelTargetKind};
use forge_core::*;
use sqlx::any::AnyRow;

use crate::row::*;
use crate::{Store, db_err};

/// Every port of the target.
const ALL_PORTS: &str = "*";

fn ports_to_text(ports: &[u16]) -> String {
    if ports.is_empty() {
        ALL_PORTS.to_owned()
    } else {
        ports.iter().map(ToString::to_string).collect::<Vec<_>>().join(",")
    }
}

fn ports_from_text(s: &str) -> Vec<u16> {
    if s == ALL_PORTS {
        return Vec::new();
    }

    s.split(',').filter_map(|p| p.trim().parse().ok()).collect()
}

fn kind(r: &AnyRow) -> DomainResult<TunnelTargetKind> {
    text(r, "target_kind")?.parse().map_err(DomainError::Internal)
}

fn unsigned(v: i64) -> u64 {
    u64::try_from(v).unwrap_or(0)
}

fn signed(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

fn grant_row(r: &AnyRow) -> DomainResult<TunnelGrant> {
    Ok(TunnelGrant {
        id: parsed(r, "id")?,
        workspace_id: parsed(r, "workspace_id")?,
        user_id: parsed(r, "user_id")?,
        target_kind: kind(r)?,
        target_ref: text(r, "target_ref")?,
        server_id: opt_parsed(r, "server_id")?,
        ports: ports_from_text(&text(r, "ports")?),
        created_by: text(r, "created_by")?,
        created_at: ts(r, "created_at")?,
        expires_at: ts(r, "expires_at")?,
        revoked_at: opt_ts(r, "revoked_at")?,
        note: text(r, "note")?,
    })
}

fn tunnel_row(r: &AnyRow) -> DomainResult<TunnelRecord> {
    Ok(TunnelRecord {
        id: text(r, "id")?,
        workspace_id: parsed(r, "workspace_id")?,
        user_id: parsed(r, "user_id")?,
        server_id: parsed(r, "server_id")?,
        target_kind: kind(r)?,
        target_ref: text(r, "target_ref")?,
        label: text(r, "label")?,
        port: u16::try_from(int(r, "port")?).unwrap_or(0),
        dial_ip: text(r, "dial_ip")?,
        bind: text(r, "bind")?,
        grant_id: opt_parsed(r, "grant_id")?,
        opened_at: ts(r, "opened_at")?,
        expires_at: ts(r, "expires_at")?,
        closed_at: opt_ts(r, "closed_at")?,
        close_reason: opt_text(r, "close_reason")?,
        bytes_up: unsigned(int(r, "bytes_up")?),
        bytes_down: unsigned(int(r, "bytes_down")?),
    })
}

#[async_trait]
impl TunnelGrantRepository for Store {
    async fn put_tunnel_grant(&self, g: &TunnelGrant) -> DomainResult<()> {
        sqlx::query(
            "INSERT INTO tunnel_grants (id, workspace_id, user_id, target_kind, target_ref, server_id, ports,
                                        created_by, created_at, expires_at, revoked_at, note)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
             ON CONFLICT (id) DO UPDATE SET ports = $7, expires_at = $10, revoked_at = $11, note = $12",
        )
        .bind(g.id.to_string())
        .bind(g.workspace_id.to_string())
        .bind(g.user_id.to_string())
        .bind(g.target_kind.as_str())
        .bind(&g.target_ref)
        .bind(g.server_id.map(|s| s.to_string()))
        .bind(ports_to_text(&g.ports))
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

    async fn get_tunnel_grant(&self, id: TunnelGrantId) -> DomainResult<Option<TunnelGrant>> {
        let r = sqlx::query("SELECT * FROM tunnel_grants WHERE id = $1")
            .bind(id.to_string())
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(grant_row).transpose()
    }

    async fn list_tunnel_grants(&self, user: Option<UserId>) -> DomainResult<Vec<TunnelGrant>> {
        let rows =
            sqlx::query("SELECT * FROM tunnel_grants WHERE ($1 = '' OR user_id = $1) ORDER BY created_at DESC, id")
                .bind(user.map(|u| u.to_string()).unwrap_or_default())
                .fetch_all(self.pool())
                .await
                .map_err(db_err)?;
        rows.iter().map(grant_row).collect()
    }
}

#[async_trait]
impl TunnelRepository for Store {
    async fn put_tunnel(&self, t: &TunnelRecord) -> DomainResult<()> {
        sqlx::query(
            "INSERT INTO tunnels (id, workspace_id, user_id, server_id, target_kind, target_ref, label, port, dial_ip,
                                  bind, grant_id, opened_at, expires_at, closed_at, close_reason, bytes_up, bytes_down)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17)
             ON CONFLICT (id) DO UPDATE SET expires_at = $13, closed_at = $14, close_reason = $15,
                 bytes_up = $16, bytes_down = $17",
        )
        .bind(&t.id)
        .bind(t.workspace_id.to_string())
        .bind(t.user_id.to_string())
        .bind(t.server_id.to_string())
        .bind(t.target_kind.as_str())
        .bind(&t.target_ref)
        .bind(&t.label)
        .bind(i64::from(t.port))
        .bind(&t.dial_ip)
        .bind(&t.bind)
        .bind(t.grant_id.map(|g| g.to_string()))
        .bind(t.opened_at.0)
        .bind(t.expires_at.0)
        .bind(t.closed_at.map(|c| c.0))
        .bind(t.close_reason.clone())
        .bind(signed(t.bytes_up))
        .bind(signed(t.bytes_down))
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn get_tunnel(&self, id: &str) -> DomainResult<Option<TunnelRecord>> {
        let r = sqlx::query("SELECT * FROM tunnels WHERE id = $1")
            .bind(id)
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(tunnel_row).transpose()
    }

    async fn list_tunnels(&self, user: Option<UserId>, limit: u32) -> DomainResult<Vec<TunnelRecord>> {
        // Open rows (closed_at NULL) first, then the newest closed ones.
        let rows = sqlx::query(
            "SELECT * FROM tunnels WHERE ($1 = '' OR user_id = $1)
             ORDER BY CASE WHEN closed_at IS NULL THEN 0 ELSE 1 END, opened_at DESC, id
             LIMIT $2",
        )
        .bind(user.map(|u| u.to_string()).unwrap_or_default())
        .bind(i64::from(limit))
        .fetch_all(self.pool())
        .await
        .map_err(db_err)?;
        rows.iter().map(tunnel_row).collect()
    }

    async fn close_tunnel(
        &self,
        id: &str,
        at: Timestamp,
        reason: &str,
        bytes_up: u64,
        bytes_down: u64,
    ) -> DomainResult<()> {
        sqlx::query(
            "UPDATE tunnels SET closed_at = $2, close_reason = $3, bytes_up = $4, bytes_down = $5
             WHERE id = $1 AND closed_at IS NULL",
        )
        .bind(id)
        .bind(at.0)
        .bind(reason)
        .bind(signed(bytes_up))
        .bind(signed(bytes_down))
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn close_open_tunnels(&self, at: Timestamp, reason: &str) -> DomainResult<u64> {
        let r = sqlx::query("UPDATE tunnels SET closed_at = $1, close_reason = $2 WHERE closed_at IS NULL")
            .bind(at.0)
            .bind(reason)
            .execute(self.pool())
            .await
            .map_err(db_err)?;
        Ok(r.rows_affected())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_lists_round_trip_and_star_means_all() {
        assert_eq!(ports_to_text(&[]), "*");
        assert_eq!(ports_from_text("*"), Vec::<u16>::new());
        assert_eq!(ports_to_text(&[80, 5341]), "80,5341");
        assert_eq!(ports_from_text("80, 5341,x"), vec![80, 5341]);
    }
}
