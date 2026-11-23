//! Silences and maintenance windows (#11, D-112).

use async_trait::async_trait;
use forge_core::silences::{Silence, SilenceFilter, SilenceRepository, SilenceState, SilenceWindow};
use forge_core::*;
use sqlx::any::AnyRow;

use crate::row::*;
use crate::{Store, db_err};

fn from_json<T: serde::de::DeserializeOwned>(r: &AnyRow, col: &str) -> DomainResult<T> {
    serde_json::from_value(json(r, col)?).map_err(|e| DomainError::Internal(format!("column {col}: {e}")))
}

/// A column holding a serde enum's name, or NULL.
fn opt_named<T: serde::de::DeserializeOwned>(r: &AnyRow, col: &str) -> DomainResult<Option<T>> {
    opt_text(r, col)?
        .map(|s| {
            serde_json::from_value(serde_json::Value::String(s))
                .map_err(|e| DomainError::Internal(format!("column {col}: {e}")))
        })
        .transpose()
}

fn silence_row(r: &AnyRow) -> DomainResult<Silence> {
    Ok(Silence {
        id: parsed(r, "id")?,
        workspace_id: parsed(r, "workspace_id")?,
        scope: from_json(r, "scope")?,
        rules: from_json(r, "rules")?,
        min_severity: opt_named(r, "min_severity")?,
        max_severity: opt_named(r, "max_severity")?,
        starts_at: ts(r, "starts_at")?,
        ends_at: ts(r, "ends_at")?,
        reason: text(r, "reason")?,
        created_by: parsed(r, "created_by")?,
        created_at: ts(r, "created_at")?,
        updated_at: ts(r, "updated_at")?,
        ended_by: opt_parsed(r, "ended_by")?,
    })
}

#[async_trait]
impl SilenceRepository for Store {
    async fn create_silence(&self, s: &Silence) -> DomainResult<()> {
        sqlx::query(
            "INSERT INTO silences (id, workspace_id, scope, rules, min_severity, max_severity, starts_at, ends_at,
             reason, created_by, created_at, updated_at, ended_by)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
        )
        .bind(s.id.to_string())
        .bind(s.workspace_id.to_string())
        .bind(serde_json::to_string(&s.scope).map_err(DomainError::internal)?)
        .bind(serde_json::to_string(&s.rules).map_err(DomainError::internal)?)
        .bind(s.min_severity.map(|v| v.as_str().to_owned()))
        .bind(s.max_severity.map(|v| v.as_str().to_owned()))
        .bind(s.starts_at.0)
        .bind(s.ends_at.0)
        .bind(s.reason.clone())
        .bind(s.created_by.to_string())
        .bind(s.created_at.0)
        .bind(s.updated_at.0)
        .bind(s.ended_by.as_ref().map(ToString::to_string))
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn get_silence(&self, id: SilenceId) -> DomainResult<Option<Silence>> {
        let r = sqlx::query("SELECT * FROM silences WHERE id = $1")
            .bind(id.to_string())
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(silence_row).transpose()
    }

    async fn list_silences(&self, f: &SilenceFilter, limit: u32) -> DomainResult<Vec<Silence>> {
        // Numbered binds only; no values in the SQL text.
        enum B {
            S(String),
            I(i64),
        }
        let mut sql = String::from("SELECT * FROM silences WHERE 1 = 1");
        let mut binds: Vec<B> = Vec::new();
        if let Some(ws) = f.workspace {
            binds.push(B::S(ws.to_string()));
            sql.push_str(&format!(" AND workspace_id = ${}", binds.len()));
        }
        if let Some((state, now)) = f.state {
            binds.push(B::I(now.0));
            let n = binds.len();
            sql.push_str(&match state {
                SilenceState::Expired => format!(" AND ends_at <= ${n}"),
                SilenceState::Scheduled => format!(" AND ends_at > ${n} AND starts_at > ${n}"),
                SilenceState::Active => format!(" AND ends_at > ${n} AND starts_at <= ${n}"),
            });
        }
        if let Some((at, id)) = f.before {
            binds.push(B::I(at.0));
            let t = binds.len();
            binds.push(B::S(id.to_string()));
            sql.push_str(&format!(" AND (starts_at < ${t} OR (starts_at = ${t} AND id < ${}))", binds.len()));
        }
        sql.push_str(&format!(" ORDER BY starts_at DESC, id DESC LIMIT {}", limit.clamp(1, 1000)));
        let mut q = sqlx::query(&sql);
        for bind in binds {
            q = match bind {
                B::S(s) => q.bind(s),
                B::I(i) => q.bind(i),
            };
        }
        let rows = q.fetch_all(self.pool()).await.map_err(db_err)?;
        rows.iter().map(silence_row).collect()
    }

    async fn active_silences(&self, workspace: WorkspaceId, now: Timestamp) -> DomainResult<Vec<Silence>> {
        let rows = sqlx::query(
            "SELECT * FROM silences WHERE workspace_id = $1 AND starts_at <= $2 AND ends_at > $2
             ORDER BY ends_at DESC, id LIMIT 1000",
        )
        .bind(workspace.to_string())
        .bind(now.0)
        .fetch_all(self.pool())
        .await
        .map_err(db_err)?;
        rows.iter().map(silence_row).collect()
    }

    async fn count_open_silences(
        &self,
        workspace: WorkspaceId,
        now: Timestamp,
        created_by: Option<&Principal>,
    ) -> DomainResult<u64> {
        const ALL: &str = "SELECT COUNT(*) AS n FROM silences WHERE workspace_id = $1 AND ends_at > $2";
        const BY: &str =
            "SELECT COUNT(*) AS n FROM silences WHERE workspace_id = $1 AND ends_at > $2 AND created_by = $3";
        let q = sqlx::query(if created_by.is_some() { BY } else { ALL }).bind(workspace.to_string()).bind(now.0);
        let q = match created_by {
            Some(p) => q.bind(p.to_string()),
            None => q,
        };
        let r = q.fetch_one(self.pool()).await.map_err(db_err)?;
        u64::try_from(int(&r, "n")?).map_err(|_| DomainError::Internal("negative silence count".into()))
    }

    async fn set_silence_window(&self, id: SilenceId, w: &SilenceWindow, now: Timestamp) -> DomainResult<bool> {
        // One statement: the check that it has not ended and the change
        // cannot be separated by another writer.
        let done = sqlx::query(
            "UPDATE silences SET starts_at = $2, ends_at = $3, ended_by = COALESCE($4, ended_by), updated_at = $5
             WHERE id = $1 AND ends_at > $6",
        )
        .bind(id.to_string())
        .bind(w.starts_at.0)
        .bind(w.ends_at.0)
        .bind(w.ended_by.as_ref().map(ToString::to_string))
        .bind(w.updated_at.0)
        .bind(now.0)
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(done.rows_affected() > 0)
    }

    async fn prune_silences(&self, before: Timestamp) -> DomainResult<u64> {
        // A silence a delivery still names stays: the delivery's history
        // points at it. Deliveries are pruned first, on the same retention.
        let done = sqlx::query(
            "DELETE FROM silences WHERE ends_at < $1 AND NOT EXISTS (
                 SELECT 1 FROM notification_deliveries d WHERE d.silence_id = silences.id
             )",
        )
        .bind(before.0)
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(done.rows_affected())
    }
}
