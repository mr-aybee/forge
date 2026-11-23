use async_trait::async_trait;
use forge_core::model::{AgentStatus, EnrollmentToken, NodeRole, Server};
use forge_core::ports::{EnrollmentRepository, ServerRepository};
use forge_core::*;
use sqlx::Row;
use sqlx::any::AnyRow;

use crate::row::*;
use crate::{Store, db_err};

fn server_row(r: &AnyRow) -> DomainResult<Server> {
    Ok(Server {
        id: parsed(r, "id")?,
        workspace_id: parsed(r, "workspace_id")?,
        environment_id: opt_parsed(r, "environment_id")?,
        cluster_id: opt_parsed(r, "cluster_id")?,
        role: NodeRole::parse(&text(r, "role")?),
        display_name: opt_text(r, "display_name")?,
        hostname: text(r, "hostname")?,
        os: text(r, "os")?,
        arch: text(r, "arch")?,
        agent_version: text(r, "agent_version")?,
        status: AgentStatus::parse(&text(r, "status")?),
        last_seen_at: opt_ts(r, "last_seen_at")?,
        cert_serial: opt_text(r, "cert_serial")?,
        cert_not_after: opt_ts(r, "cert_not_after")?,
        created_at: ts(r, "created_at")?,
    })
}

fn token_row(r: &AnyRow) -> DomainResult<EnrollmentToken> {
    Ok(EnrollmentToken {
        id: text(r, "id")?,
        workspace_id: parsed(r, "workspace_id")?,
        environment_id: opt_parsed(r, "environment_id")?,
        token_hash: text(r, "token_hash")?,
        created_by: text(r, "created_by")?.parse().map_err(DomainError::Internal)?,
        expires_at: ts(r, "expires_at")?,
        used_at: opt_ts(r, "used_at")?,
        server_id: opt_parsed(r, "server_id")?,
    })
}

#[async_trait]
impl ServerRepository for Store {
    async fn create_server(&self, s: &Server) -> DomainResult<()> {
        sqlx::query(
            "INSERT INTO servers (id, workspace_id, environment_id, hostname, os, arch, agent_version, status, last_seen_at,
                                  cert_serial, cert_not_after, created_at, cluster_id, role)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)",
        )
        .bind(s.id.to_string())
        .bind(s.workspace_id.to_string())
        .bind(s.environment_id.map(|e| e.to_string()))
        .bind(&s.hostname)
        .bind(&s.os)
        .bind(&s.arch)
        .bind(&s.agent_version)
        .bind(s.status.as_str())
        .bind(s.last_seen_at.map(|t| t.0))
        .bind(s.cert_serial.clone())
        .bind(s.cert_not_after.map(|t| t.0))
        .bind(s.created_at.0)
        .bind(s.cluster_id.map(|c| c.to_string()))
        .bind(s.role.as_str())
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn rename_server(&self, id: ServerId, display_name: Option<&str>) -> DomainResult<()> {
        let r = sqlx::query("UPDATE servers SET display_name = $1 WHERE id = $2")
            .bind(display_name.map(str::to_owned))
            .bind(id.to_string())
            .execute(self.pool())
            .await
            .map_err(db_err)?;
        if r.rows_affected() == 0 {
            return Err(DomainError::NotFound(format!("server {id}")));
        }
        Ok(())
    }

    async fn get_server(&self, id: ServerId) -> DomainResult<Server> {
        let r = sqlx::query("SELECT * FROM servers WHERE id = $1")
            .bind(id.to_string())
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(server_row).transpose()?.ok_or_else(|| DomainError::NotFound(format!("server {id}")))
    }

    async fn list_servers(&self, org: Option<WorkspaceId>) -> DomainResult<Vec<Server>> {
        let rows = match org {
            Some(o) => {
                sqlx::query("SELECT * FROM servers WHERE workspace_id = $1 ORDER BY hostname")
                    .bind(o.to_string())
                    .fetch_all(self.pool())
                    .await
            }
            None => sqlx::query("SELECT * FROM servers ORDER BY hostname").fetch_all(self.pool()).await,
        }
        .map_err(db_err)?;
        rows.iter().map(server_row).collect()
    }

    async fn find_server_by_hostname(&self, org: WorkspaceId, hostname: &str) -> DomainResult<Option<Server>> {
        let r = sqlx::query("SELECT * FROM servers WHERE workspace_id = $1 AND hostname = $2")
            .bind(org.to_string())
            .bind(hostname)
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(server_row).transpose()
    }

    async fn record_heartbeat(&self, id: ServerId, at: Timestamp, agent_version: &str) -> DomainResult<()> {
        sqlx::query(
            "UPDATE servers SET last_seen_at = $1, agent_version = $2, status = 'online'
             WHERE id = $3 AND status <> 'revoked'",
        )
        .bind(at.0)
        .bind(agent_version)
        .bind(id.to_string())
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn set_status(&self, id: ServerId, status: AgentStatus) -> DomainResult<()> {
        sqlx::query("UPDATE servers SET status = $1 WHERE id = $2")
            .bind(status.as_str())
            .bind(id.to_string())
            .execute(self.pool())
            .await
            .map_err(db_err)?;
        Ok(())
    }

    async fn delete_server(&self, id: ServerId, at: Timestamp) -> DomainResult<()> {
        let sid = id.to_string();
        let scope = format!("server:{sid}");
        let mut tx = self.pool().begin().await.map_err(db_err)?;
        let exists = sqlx::query("SELECT id FROM servers WHERE id = $1")
            .bind(&sid)
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_err)?;
        if exists.is_none() {
            return Err(DomainError::NotFound(format!("server {id}")));
        }
        // Stacks whose default node this is move to another of their nodes;
        // a stack on this node alone goes with it.
        sqlx::query(
            "UPDATE stacks SET default_server_id =
                 (SELECT n.server_id FROM stack_nodes n WHERE n.stack_id = stacks.id AND n.server_id <> $1
                  ORDER BY n.server_id LIMIT 1),
                 updated_at = $2
             WHERE default_server_id = $1
               AND EXISTS (SELECT 1 FROM stack_nodes n WHERE n.stack_id = stacks.id AND n.server_id <> $1)",
        )
        .bind(&sid)
        .bind(at.0)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        let orphan_stacks = "SELECT id FROM stacks WHERE default_server_id = $1";
        // Alarms of the node, with their history and dedupe trackers.
        let node_alarms = "SELECT id FROM alarms WHERE server_id = $1";
        let statements: Vec<String> = vec![
            format!("DELETE FROM stack_revisions WHERE stack_id IN ({orphan_stacks})"),
            format!("DELETE FROM stack_nodes WHERE stack_id IN ({orphan_stacks})"),
            format!("DELETE FROM stacks WHERE id IN ({orphan_stacks})"),
            "DELETE FROM stack_nodes WHERE server_id = $1".into(),
            format!("DELETE FROM alarm_events WHERE alarm_id IN ({node_alarms})"),
            format!("DELETE FROM alarm_transitions WHERE alarm_id IN ({node_alarms})"),
            "DELETE FROM alert_trackers WHERE fingerprint IN (SELECT fingerprint FROM alarms WHERE server_id = $1)"
                .into(),
            "DELETE FROM alarms WHERE server_id = $1".into(),
            "DELETE FROM node_ssh WHERE server_id = $1".into(),
            "DELETE FROM installations WHERE server_id = $1".into(),
            "DELETE FROM service_instances WHERE server_id = $1".into(),
            "DELETE FROM object_store_members WHERE server_id = $1".into(),
            "DELETE FROM file_uploads WHERE server_id = $1".into(),
            "DELETE FROM file_grants WHERE server_id = $1".into(),
            "DELETE FROM file_roots WHERE server_id = $1".into(),
            "DELETE FROM server_facts WHERE server_id = $1".into(),
            "DELETE FROM server_logins WHERE server_id = $1".into(),
            "DELETE FROM node_names WHERE server_id = $1".into(),
            "DELETE FROM observations_latest WHERE server_id = $1".into(),
            "DELETE FROM observation_history WHERE server_id = $1".into(),
            "DELETE FROM observation_transitions WHERE server_id = $1".into(),
            "DELETE FROM tunnels WHERE server_id = $1".into(),
            "DELETE FROM tunnel_grants WHERE server_id = $1".into(),
            "DELETE FROM docker_endpoints WHERE server_id = $1".into(),
            "DELETE FROM deploy_slots WHERE server_id = $1".into(),
            "DELETE FROM deployment_events WHERE server_id = $1".into(),
            "DELETE FROM entity_tags WHERE kind = 'server' AND entity_id = $1".into(),
            "DELETE FROM role_bindings WHERE scope_kind = 'server' AND scope_id = $1".into(),
            "UPDATE enrollment_tokens SET server_id = NULL WHERE server_id = $1".into(),
        ];
        for sql in &statements {
            sqlx::query(sql).bind(&sid).execute(&mut *tx).await.map_err(db_err)?;
        }
        for sql in
            ["DELETE FROM profile_assignments WHERE scope = $1", "DELETE FROM group_assignments WHERE scope = $1"]
        {
            sqlx::query(sql).bind(&scope).execute(&mut *tx).await.map_err(db_err)?;
        }
        sqlx::query(
            "UPDATE jobs SET status = 'cancelled', lease_owner = NULL, lease_expires_at = NULL,
                 last_error = 'node removed', updated_at = $1
             WHERE resource = $2 AND status = 'queued'",
        )
        .bind(at.0)
        .bind(&scope)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        sqlx::query("DELETE FROM servers WHERE id = $1").bind(&sid).execute(&mut *tx).await.map_err(db_err)?;
        tx.commit().await.map_err(db_err)?;
        Ok(())
    }

    async fn set_certificate(&self, id: ServerId, serial: &str, not_after: Timestamp) -> DomainResult<()> {
        sqlx::query("UPDATE servers SET cert_serial = $1, cert_not_after = $2 WHERE id = $3")
            .bind(serial)
            .bind(not_after.0)
            .bind(id.to_string())
            .execute(self.pool())
            .await
            .map_err(db_err)?;
        Ok(())
    }

    async fn mark_stale_offline(&self, cutoff: Timestamp) -> DomainResult<Vec<ServerId>> {
        let rows = sqlx::query(
            "UPDATE servers SET status = 'offline' WHERE status = 'online' AND last_seen_at < $1 RETURNING id",
        )
        .bind(cutoff.0)
        .fetch_all(self.pool())
        .await
        .map_err(db_err)?;
        rows.iter()
            .map(|r| {
                r.try_get::<String, _>("id").map_err(DomainError::internal)?.parse().map_err(DomainError::internal)
            })
            .collect()
    }

    async fn set_node_label(
        &self,
        id: ServerId,
        workspace: WorkspaceId,
        label: &str,
        explicit: bool,
        at: Timestamp,
    ) -> DomainResult<()> {
        sqlx::query(
            "INSERT INTO node_names (server_id, workspace_id, label, explicit, updated_at) VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (server_id) DO UPDATE SET workspace_id = $2, label = $3, explicit = $4, updated_at = $5",
        )
        .bind(id.to_string())
        .bind(workspace.to_string())
        .bind(label)
        .bind(i64::from(explicit))
        .bind(at.0)
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn claim_node_label(
        &self,
        id: ServerId,
        workspace: WorkspaceId,
        label: &str,
        at: Timestamp,
    ) -> DomainResult<bool> {
        let r = sqlx::query(
            "INSERT INTO node_names (server_id, workspace_id, label, explicit, updated_at) VALUES ($1, $2, $3, 0, $4)
             ON CONFLICT (server_id) DO NOTHING",
        )
        .bind(id.to_string())
        .bind(workspace.to_string())
        .bind(label)
        .bind(at.0)
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(r.rows_affected() == 1)
    }

    async fn list_node_labels(&self, workspace: Option<WorkspaceId>) -> DomainResult<Vec<(ServerId, String, bool)>> {
        let rows = match workspace {
            Some(w) => {
                sqlx::query("SELECT server_id, label, explicit FROM node_names WHERE workspace_id = $1")
                    .bind(w.to_string())
                    .fetch_all(self.pool())
                    .await
            }
            None => sqlx::query("SELECT server_id, label, explicit FROM node_names").fetch_all(self.pool()).await,
        }
        .map_err(db_err)?;
        rows.iter()
            .map(|r| {
                let id: String = r.try_get("server_id").map_err(DomainError::internal)?;
                let label: String = r.try_get("label").map_err(DomainError::internal)?;
                let explicit: i64 = r.try_get("explicit").map_err(DomainError::internal)?;
                Ok((id.parse().map_err(DomainError::internal)?, label, explicit != 0))
            })
            .collect()
    }
}

#[async_trait]
impl EnrollmentRepository for Store {
    async fn create_token(&self, t: &EnrollmentToken) -> DomainResult<()> {
        sqlx::query(
            "INSERT INTO enrollment_tokens (id, workspace_id, environment_id, token_hash, created_by, expires_at, used_at, server_id)
             VALUES ($1, $2, $3, $4, $5, $6, NULL, NULL)",
        )
        .bind(&t.id)
        .bind(t.workspace_id.to_string())
        .bind(t.environment_id.map(|e| e.to_string()))
        .bind(&t.token_hash)
        .bind(t.created_by.to_string())
        .bind(t.expires_at.0)
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn consume_token(
        &self,
        token_hash: &str,
        now: Timestamp,
        server: ServerId,
    ) -> DomainResult<Option<EnrollmentToken>> {
        // Single statement: only one concurrent caller can flip `used_at`.
        let r = sqlx::query(
            "UPDATE enrollment_tokens SET used_at = $1, server_id = $2
             WHERE token_hash = $3 AND used_at IS NULL AND expires_at > $1
             RETURNING *",
        )
        .bind(now.0)
        .bind(server.to_string())
        .bind(token_hash)
        .fetch_optional(self.pool())
        .await
        .map_err(db_err)?;
        r.as_ref().map(token_row).transpose()
    }

    async fn revoke_certificate(&self, serial: &str, server: ServerId, at: Timestamp) -> DomainResult<()> {
        sqlx::query(
            "INSERT INTO revoked_certificates (serial, server_id, revoked_at) VALUES ($1, $2, $3)
             ON CONFLICT (serial) DO NOTHING",
        )
        .bind(serial)
        .bind(server.to_string())
        .bind(at.0)
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn get_token(&self, id: &str) -> DomainResult<Option<EnrollmentToken>> {
        let r = sqlx::query("SELECT * FROM enrollment_tokens WHERE id = $1")
            .bind(id)
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(token_row).transpose()
    }

    async fn put_enrollment_target(
        &self,
        token_id: &str,
        cluster: Option<ClusterId>,
        hostname: Option<&str>,
        at: Timestamp,
    ) -> DomainResult<()> {
        sqlx::query(
            "INSERT INTO enrollment_targets (token_id, cluster_id, hostname, created_at) VALUES ($1, $2, $3, $4)
             ON CONFLICT (token_id) DO UPDATE SET cluster_id = $2, hostname = $3",
        )
        .bind(token_id)
        .bind(cluster.map(|c| c.to_string()))
        .bind(hostname.map(str::to_owned))
        .bind(at.0)
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn get_enrollment_target(&self, token_id: &str) -> DomainResult<Option<(Option<ClusterId>, Option<String>)>> {
        let r = sqlx::query("SELECT cluster_id, hostname FROM enrollment_targets WHERE token_id = $1")
            .bind(token_id)
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(|r| Ok((opt_parsed(r, "cluster_id")?, opt_text(r, "hostname")?))).transpose()
    }

    async fn find_token_by_hash(&self, token_hash: &str) -> DomainResult<Option<EnrollmentToken>> {
        let r = sqlx::query("SELECT * FROM enrollment_tokens WHERE token_hash = $1")
            .bind(token_hash)
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(token_row).transpose()
    }

    async fn is_certificate_revoked(&self, serial: &str) -> DomainResult<bool> {
        let r = sqlx::query("SELECT serial FROM revoked_certificates WHERE serial = $1")
            .bind(serial)
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        Ok(r.is_some())
    }
}
