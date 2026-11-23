use async_trait::async_trait;
use forge_core::swarm::{ClusterSwarm, SwarmMember, SwarmRepository};
use forge_core::*;
use sqlx::any::AnyRow;

use crate::row::*;
use crate::{Store, db_err};

fn swarm_row(r: &AnyRow) -> DomainResult<ClusterSwarm> {
    Ok(ClusterSwarm {
        cluster_id: parsed(r, "cluster_id")?,
        swarm_id: text(r, "swarm_id")?,
        port: u16::try_from(int(r, "port")?).map_err(DomainError::internal)?,
        default_addr_pool: serde_json::from_str(&text(r, "default_addr_pool")?).map_err(DomainError::internal)?,
        autolock: flag(r, "autolock")?,
        tokens_sealed: opt_text(r, "tokens_sealed")?,
        updated_at: ts(r, "updated_at")?,
    })
}

fn member_row(r: &AnyRow) -> DomainResult<SwarmMember> {
    Ok(SwarmMember {
        server_id: parsed(r, "server_id")?,
        cluster_id: parsed(r, "cluster_id")?,
        node_id: text(r, "node_id")?,
        role: text(r, "role")?.parse()?,
        advertise_addr: text(r, "advertise_addr")?,
        manager_addr: text(r, "manager_addr")?,
        state: text(r, "state")?.parse()?,
        error: text(r, "error")?,
        updated_at: ts(r, "updated_at")?,
    })
}

#[async_trait]
impl SwarmRepository for Store {
    async fn get_swarm(&self, cluster: ClusterId) -> DomainResult<Option<ClusterSwarm>> {
        let r = sqlx::query("SELECT * FROM cluster_swarms WHERE cluster_id = $1")
            .bind(cluster.to_string())
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(swarm_row).transpose()
    }

    async fn put_swarm(&self, s: &ClusterSwarm) -> DomainResult<()> {
        let pool = serde_json::to_string(&s.default_addr_pool).map_err(DomainError::internal)?;
        sqlx::query(
            "INSERT INTO cluster_swarms (cluster_id, swarm_id, port, default_addr_pool, autolock, tokens_sealed, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             ON CONFLICT (cluster_id) DO UPDATE SET swarm_id = $2, port = $3, default_addr_pool = $4, autolock = $5,
                 tokens_sealed = $6, updated_at = $7",
        )
        .bind(s.cluster_id.to_string())
        .bind(&s.swarm_id)
        .bind(i64::from(s.port))
        .bind(pool)
        .bind(i64::from(s.autolock))
        .bind(s.tokens_sealed.clone())
        .bind(s.updated_at.0)
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn delete_swarm(&self, cluster: ClusterId) -> DomainResult<()> {
        sqlx::query("DELETE FROM cluster_swarms WHERE cluster_id = $1")
            .bind(cluster.to_string())
            .execute(self.pool())
            .await
            .map_err(db_err)?;
        Ok(())
    }

    async fn list_members(&self, cluster: ClusterId) -> DomainResult<Vec<SwarmMember>> {
        let rows = sqlx::query("SELECT * FROM swarm_members WHERE cluster_id = $1 ORDER BY updated_at")
            .bind(cluster.to_string())
            .fetch_all(self.pool())
            .await
            .map_err(db_err)?;
        rows.iter().map(member_row).collect()
    }

    async fn get_member(&self, server: ServerId) -> DomainResult<Option<SwarmMember>> {
        let r = sqlx::query("SELECT * FROM swarm_members WHERE server_id = $1")
            .bind(server.to_string())
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(member_row).transpose()
    }

    async fn put_member(&self, m: &SwarmMember) -> DomainResult<()> {
        sqlx::query(
            "INSERT INTO swarm_members (server_id, cluster_id, node_id, role, advertise_addr, manager_addr, state, error, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
             ON CONFLICT (server_id) DO UPDATE SET cluster_id = $2, node_id = $3, role = $4, advertise_addr = $5,
                 manager_addr = $6, state = $7, error = $8, updated_at = $9",
        )
        .bind(m.server_id.to_string())
        .bind(m.cluster_id.to_string())
        .bind(&m.node_id)
        .bind(m.role.as_str())
        .bind(&m.advertise_addr)
        .bind(&m.manager_addr)
        .bind(m.state.as_str())
        .bind(&m.error)
        .bind(m.updated_at.0)
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn delete_member(&self, server: ServerId) -> DomainResult<()> {
        sqlx::query("DELETE FROM swarm_members WHERE server_id = $1")
            .bind(server.to_string())
            .execute(self.pool())
            .await
            .map_err(db_err)?;
        Ok(())
    }
}
