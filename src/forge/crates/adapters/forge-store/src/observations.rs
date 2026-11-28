//! Observations, their history and the transition feed (migration 0018, D-085).

use async_trait::async_trait;
use forge_core::observation::*;
use forge_core::*;
use sqlx::any::AnyRow;

use crate::row::*;
use crate::{Store, db_err};

fn to_json<T: serde::Serialize>(v: &T) -> DomainResult<String> {
    serde_json::to_string(v).map_err(DomainError::internal)
}

fn from_json<T: serde::de::DeserializeOwned>(r: &AnyRow, col: &str) -> DomainResult<T> {
    serde_json::from_str(&text(r, col)?).map_err(|e| DomainError::Internal(format!("column {col}: {e}")))
}

fn opt_from_json<T: serde::de::DeserializeOwned>(r: &AnyRow, col: &str) -> DomainResult<Option<T>> {
    opt_text(r, col)?
        .map(|s| serde_json::from_str(&s).map_err(|e| DomainError::Internal(format!("column {col}: {e}"))))
        .transpose()
}

/// Key columns: `server_id` and `generation` are `''` when absent.
fn key_cols(k: &ObservationKey) -> (String, String, String, String) {
    (
        k.resource.kind.as_str().to_owned(),
        k.resource.id.clone(),
        k.server.map(|s| s.to_string()).unwrap_or_default(),
        k.subject.as_str().to_owned(),
    )
}

fn key_of(r: &AnyRow) -> DomainResult<ObservationKey> {
    let kind: EntityKind = parsed(r, "resource_type")?;
    let server = text(r, "server_id")?;

    Ok(ObservationKey {
        resource: EntityRef::new(kind, text(r, "resource_id")?),
        server: if server.is_empty() {
            None
        } else {
            Some(server.parse().map_err(|e| DomainError::Internal(format!("column server_id: {e}")))?)
        },
        subject: Subject::new(text(r, "subject")?)?,
    })
}

fn generation_of(r: &AnyRow) -> DomainResult<Option<String>> {
    Ok(Some(text(r, "generation")?).filter(|g| !g.is_empty()))
}

fn latest_of(r: &AnyRow, history: bool) -> DomainResult<Latest> {
    let received_at = ts(r, "received_at")?;
    Ok(Latest {
        key: key_of(r)?,
        value: from_json(r, "value")?,
        reason: opt_from_json(r, "reason")?,
        detail: opt_text(r, "detail")?,
        source: from_json(r, "source")?,
        executor: opt_parsed(r, "executor")?,
        generation: generation_of(r)?,
        observed_at: ts(r, "observed_at")?,
        received_at,
        fresh_until: ts(r, "fresh_until")?,
        skewed: flag(r, "skewed")?,
        // History rows have no change time of their own.
        changed_at: if history { received_at } else { ts(r, "changed_at")? },
    })
}

fn transition_of(r: &AnyRow) -> DomainResult<Transition> {
    Ok(Transition {
        seq: int(r, "seq")?,
        key: key_of(r)?,
        from: opt_from_json(r, "from_value")?,
        to: from_json(r, "to_value")?,
        reason: opt_from_json(r, "reason")?,
        generation: generation_of(r)?,
        at: ts(r, "at")?,
        evidence_at: ts(r, "evidence_at")?,
    })
}

fn dedupe(t: &Transition) -> DomainResult<String> {
    let (rt, rid, sid, subj) = key_cols(&t.key);
    Ok(format!(
        "{rt}|{rid}|{sid}|{subj}|{}|{}|{}|{}",
        t.generation.as_deref().unwrap_or_default(),
        t.from.as_ref().map(to_json).transpose()?.unwrap_or_default(),
        to_json(&t.to)?,
        t.evidence_at.0
    ))
}

const KEY_WHERE: &str = "resource_type = $1 AND resource_id = $2 AND server_id = $3 AND subject = $4";

#[async_trait]
impl ObservationStore for Store {
    async fn latest_for(&self, keys: &[ObservationKey]) -> DomainResult<Vec<Latest>> {
        let mut out = Vec::with_capacity(keys.len());

        // One statement per 50 keys (200 placeholders), each key a lookup on
        // the primary key, instead of one statement per key.
        for chunk in keys.chunks(50) {
            let clauses: Vec<String> = (0..chunk.len())
                .map(|i| {
                    let n = i * 4;
                    format!(
                        "(resource_type = ${} AND resource_id = ${} AND server_id = ${} AND subject = ${})",
                        n + 1,
                        n + 2,
                        n + 3,
                        n + 4
                    )
                })
                .collect();
            let sql = format!("SELECT * FROM observations_latest WHERE {}", clauses.join(" OR "));
            let mut q = sqlx::query(&sql);

            for k in chunk {
                let (rt, rid, sid, subj) = key_cols(k);
                q = q.bind(rt).bind(rid).bind(sid).bind(subj);
            }

            for r in &q.fetch_all(self.pool()).await.map_err(db_err)? {
                out.push(latest_of(r, false)?);
            }
        }

        Ok(out)
    }

    async fn stale_since(&self, now: Timestamp, limit: u32) -> DomainResult<Vec<Latest>> {
        let rows = sqlx::query(
            "SELECT * FROM observations_latest WHERE stale_noted = 0 AND fresh_until <= $1
             ORDER BY fresh_until LIMIT $2",
        )
        .bind(now.0)
        .bind(i64::from(limit))
        .fetch_all(self.pool())
        .await
        .map_err(db_err)?;
        rows.iter().map(|r| latest_of(r, false)).collect()
    }

    async fn on_server(&self, server: ServerId, limit: u32) -> DomainResult<Vec<Latest>> {
        let id = server.to_string();
        let rows = sqlx::query(
            "SELECT * FROM observations_latest
             WHERE server_id = $1 OR (resource_type = 'server' AND resource_id = $1)
             ORDER BY resource_type, resource_id, subject LIMIT $2",
        )
        .bind(id)
        .bind(i64::from(limit))
        .fetch_all(self.pool())
        .await
        .map_err(db_err)?;

        rows.iter().map(|r| latest_of(r, false)).collect()
    }

    async fn apply(&self, batch: &ObservationBatch) -> DomainResult<Vec<Transition>> {
        if batch.is_empty() {
            return Ok(Vec::new());
        }

        // Held until the commit: see `Store::observation_writes`.
        let _writes = self.observation_writes.lock().await;
        let mut tx = self.pool().begin().await.map_err(db_err)?;

        for l in &batch.latest {
            let (rt, rid, sid, subj) = key_cols(&l.key);
            sqlx::query(
                "INSERT INTO observations_latest (resource_type, resource_id, server_id, subject, value, reason,
                   detail, source, executor, generation, observed_at, received_at, fresh_until, skewed, changed_at,
                   stale_noted)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, 0)
                 ON CONFLICT (resource_type, resource_id, server_id, subject) DO UPDATE SET
                   value = excluded.value, reason = excluded.reason, detail = excluded.detail,
                   source = excluded.source, executor = excluded.executor, generation = excluded.generation,
                   observed_at = excluded.observed_at, received_at = excluded.received_at,
                   fresh_until = excluded.fresh_until, skewed = excluded.skewed, changed_at = excluded.changed_at,
                   stale_noted = 0",
            )
            .bind(rt)
            .bind(rid)
            .bind(sid)
            .bind(subj)
            .bind(to_json(&l.value)?)
            .bind(l.reason.as_ref().map(to_json).transpose()?)
            .bind(l.detail.clone())
            .bind(to_json(&l.source)?)
            .bind(l.executor.map(|s| s.to_string()))
            .bind(l.generation.clone().unwrap_or_default())
            .bind(l.observed_at.0)
            .bind(l.received_at.0)
            .bind(l.fresh_until.0)
            .bind(b(l.skewed))
            .bind(l.changed_at.0)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        }

        for h in &batch.history {
            let (rt, rid, sid, subj) = key_cols(&h.key);
            sqlx::query(
                "INSERT INTO observation_history (resource_type, resource_id, server_id, subject, generation,
                   observed_at, value, reason, detail, source, executor, received_at, fresh_until, skewed)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
                 ON CONFLICT (resource_type, resource_id, server_id, subject, generation, observed_at) DO NOTHING",
            )
            .bind(rt)
            .bind(rid)
            .bind(sid)
            .bind(subj)
            .bind(h.generation.clone().unwrap_or_default())
            .bind(h.observed_at.0)
            .bind(to_json(&h.value)?)
            .bind(h.reason.as_ref().map(to_json).transpose()?)
            .bind(h.detail.clone())
            .bind(to_json(&h.source)?)
            .bind(h.executor.map(|s| s.to_string()))
            .bind(h.received_at.0)
            .bind(h.fresh_until.0)
            .bind(b(h.skewed))
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        }

        for k in &batch.stale_noted {
            let (rt, rid, sid, subj) = key_cols(k);
            sqlx::query(&format!("UPDATE observations_latest SET stale_noted = 1 WHERE {KEY_WHERE}"))
                .bind(rt)
                .bind(rid)
                .bind(sid)
                .bind(subj)
                .execute(&mut *tx)
                .await
                .map_err(db_err)?;
        }

        let mut stored = Vec::with_capacity(batch.transitions.len());

        // The next number, read once: the lock above keeps it ours.
        let mut seq = if batch.transitions.is_empty() {
            0
        } else {
            let next = sqlx::query("SELECT COALESCE(MAX(seq), 0) + 1 AS n FROM observation_transitions")
                .fetch_one(&mut *tx)
                .await
                .map_err(db_err)?;
            int(&next, "n")?
        };

        for t in &batch.transitions {
            let (rt, rid, sid, subj) = key_cols(&t.key);
            let inserted = sqlx::query(
                "INSERT INTO observation_transitions (seq, resource_type, resource_id, server_id, subject,
                   from_value, to_value, reason, generation, at, evidence_at, dedupe)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
                 ON CONFLICT (dedupe) DO NOTHING",
            )
            .bind(seq)
            .bind(rt)
            .bind(rid)
            .bind(sid)
            .bind(subj)
            .bind(t.from.as_ref().map(to_json).transpose()?)
            .bind(to_json(&t.to)?)
            .bind(t.reason.as_ref().map(to_json).transpose()?)
            .bind(t.generation.clone().unwrap_or_default())
            .bind(t.at.0)
            .bind(t.evidence_at.0)
            .bind(dedupe(t)?)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?
            .rows_affected();

            if inserted == 1 {
                stored.push(Transition { seq, ..t.clone() });
                seq += 1;
            }
        }

        tx.commit().await.map_err(db_err)?;

        Ok(stored)
    }
}

#[async_trait]
impl ObservationQuery for Store {
    async fn latest(&self, filter: &ObservationFilter, limit: u32) -> DomainResult<Vec<Latest>> {
        let mut sql = String::from("SELECT * FROM observations_latest WHERE 1 = 1");
        let mut binds: Vec<String> = Vec::new();

        if let Some(r) = &filter.resource {
            binds.push(r.kind.as_str().to_owned());
            sql.push_str(&format!(" AND resource_type = ${}", binds.len()));
            binds.push(r.id.clone());
            sql.push_str(&format!(" AND resource_id = ${}", binds.len()));
        }

        if let Some(s) = filter.server {
            binds.push(s.to_string());
            sql.push_str(&format!(" AND server_id = ${}", binds.len()));
        }

        if let Some(p) = &filter.subject_prefix {
            // Subjects are validated to [a-z0-9._-], so no LIKE wildcards can
            // come from a stored subject; escape the prefix anyway.
            let escaped = p.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
            binds.push(format!("{escaped}%"));
            sql.push_str(&format!(" AND subject LIKE ${} ESCAPE '\\'", binds.len()));
        }

        sql.push_str(&format!(" ORDER BY resource_type, resource_id, server_id, subject LIMIT ${}", binds.len() + 1));
        let mut q = sqlx::query(&sql);

        for v in binds {
            q = q.bind(v);
        }

        let rows = q.bind(i64::from(limit)).fetch_all(self.pool()).await.map_err(db_err)?;

        rows.iter().map(|r| latest_of(r, false)).collect()
    }

    async fn history(&self, key: &ObservationKey, since: Timestamp, limit: u32) -> DomainResult<Vec<Latest>> {
        let (rt, rid, sid, subj) = key_cols(key);
        let rows = sqlx::query(&format!(
            "SELECT * FROM observation_history WHERE {KEY_WHERE} AND received_at >= $5
             ORDER BY received_at DESC, observed_at DESC LIMIT $6"
        ))
        .bind(rt)
        .bind(rid)
        .bind(sid)
        .bind(subj)
        .bind(since.0)
        .bind(i64::from(limit))
        .fetch_all(self.pool())
        .await
        .map_err(db_err)?;

        rows.iter().map(|r| latest_of(r, true)).collect()
    }
}

#[async_trait]
impl TransitionFeed for Store {
    async fn since(&self, after_seq: i64, limit: u32) -> DomainResult<Vec<Transition>> {
        let rows = sqlx::query("SELECT * FROM observation_transitions WHERE seq > $1 ORDER BY seq LIMIT $2")
            .bind(after_seq)
            .bind(i64::from(limit))
            .fetch_all(self.pool())
            .await
            .map_err(db_err)?;
        rows.iter().map(transition_of).collect()
    }

    async fn head(&self) -> DomainResult<i64> {
        let r = sqlx::query("SELECT COALESCE(MAX(seq), 0) AS n FROM observation_transitions")
            .fetch_one(self.pool())
            .await
            .map_err(db_err)?;
        int(&r, "n")
    }
}

#[async_trait]
impl ObservationRetention for Store {
    async fn sweep(&self, history_before: Timestamp, transitions_before_seq: i64) -> DomainResult<u64> {
        let h = sqlx::query("DELETE FROM observation_history WHERE received_at < $1")
            .bind(history_before.0)
            .execute(self.pool())
            .await
            .map_err(db_err)?
            .rows_affected();

        // The newest row always stays, so MAX(seq) + 1 never restarts and
        // consumers' cursors stay valid.
        let t = sqlx::query(
            "DELETE FROM observation_transitions
             WHERE seq < $1 AND seq < (SELECT MAX(seq) FROM observation_transitions)",
        )
        .bind(transitions_before_seq)
        .execute(self.pool())
        .await
        .map_err(db_err)?
        .rows_affected();

        Ok(h + t)
    }

    async fn delete_orphans(&self) -> DomainResult<u64> {
        let mut n = 0;

        for table in ["observations_latest", "observation_history"] {
            n += sqlx::query(&format!(
                "DELETE FROM {table} WHERE
                   (resource_type = 'server' AND resource_id NOT IN (SELECT id FROM servers))
                   OR (resource_type = 'service' AND resource_id NOT IN (SELECT id FROM services))
                   OR (server_id <> '' AND server_id NOT IN (SELECT id FROM servers))"
            ))
            .execute(self.pool())
            .await
            .map_err(db_err)?
            .rows_affected();
        }

        Ok(n)
    }
}

#[async_trait]
impl InstancePlacements for Store {
    async fn is_placed_on(&self, app: &str, server: ServerId) -> DomainResult<bool> {
        let r = sqlx::query("SELECT 1 AS one FROM service_instances WHERE service_id = $1 AND server_id = $2")
            .bind(app)
            .bind(server.to_string())
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        Ok(r.is_some())
    }
}
