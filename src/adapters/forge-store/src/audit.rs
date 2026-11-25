use async_trait::async_trait;
use forge_core::model::{AuditActionCount, AuditBucketCount, AuditEntry, AuditFilter, AuditOutcome};
use forge_core::ports::AuditRepository;
use forge_core::*;
use sqlx::any::AnyRow;

use crate::row::*;
use crate::{Store, db_err};

fn entry_row(r: &AnyRow) -> DomainResult<AuditEntry> {
    Ok(AuditEntry {
        seq: int(r, "seq")?,
        id: text(r, "id")?,
        at: ts(r, "at")?,
        actor: text(r, "actor")?,
        action: text(r, "action")?,
        resource: text(r, "resource")?,
        workspace_id: opt_parsed(r, "workspace_id")?,
        outcome: AuditOutcome::parse(&text(r, "outcome")?),
        details: json(r, "details")?,
        prev_hash: text(r, "prev_hash")?,
        hash: text(r, "hash")?,
        signature: text(r, "signature")?,
    })
}

/// One bound value of a dynamically built audit query.
enum Arg {
    S(String),
    I(i64),
}

/// `LIKE` pattern text with `\`, `%` and `_` escaped (used with `ESCAPE '\'`).
fn like_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());

    for c in s.chars() {
        if matches!(c, '\\' | '%' | '_') {
            out.push('\\');
        }

        out.push(c);
    }

    out
}

/// The `WHERE` clause and its arguments for `f`; `with_cursor` adds `before`.
fn where_clause(f: &AuditFilter, with_cursor: bool) -> (String, Vec<Arg>) {
    let mut parts: Vec<String> = Vec::new();
    let mut args: Vec<Arg> = Vec::new();
    let push = |args: &mut Vec<Arg>, a: Arg| -> String {
        args.push(a);
        format!("${}", args.len())
    };

    if let Some(w) = f.workspace {
        let p = push(&mut args, Arg::S(w.to_string()));
        parts.push(format!("workspace_id = {p}"));
    }

    let actions: Vec<&String> = f.actions.iter().filter(|a| !a.is_empty()).collect();
    if !actions.is_empty() {
        let ors: Vec<String> = actions
            .iter()
            .map(|a| {
                let p = push(&mut args, Arg::S(format!("{}%", like_escape(a))));
                format!("action LIKE {p} ESCAPE '\\'")
            })
            .collect();
        parts.push(format!("({})", ors.join(" OR ")));
    }

    if let Some(a) = f.actor.as_deref().filter(|s| !s.is_empty()) {
        let p = push(&mut args, Arg::S(format!("%{}%", like_escape(&a.to_lowercase()))));
        parts.push(format!("LOWER(actor) LIKE {p} ESCAPE '\\'"));
    }

    if let Some(r) = f.resource.as_deref().filter(|s| !s.is_empty()) {
        let p = push(&mut args, Arg::S(format!("%{}%", like_escape(&r.to_lowercase()))));
        parts.push(format!("LOWER(resource) LIKE {p} ESCAPE '\\'"));
    }

    if let Some(t) = f.text.as_deref().filter(|s| !s.is_empty()) {
        let p = push(&mut args, Arg::S(format!("%{}%", like_escape(&t.to_lowercase()))));
        parts.push(format!(
            "(LOWER(action) LIKE {p} ESCAPE '\\' OR LOWER(actor) LIKE {p} ESCAPE '\\' \
             OR LOWER(resource) LIKE {p} ESCAPE '\\' OR LOWER(details) LIKE {p} ESCAPE '\\')"
        ));
    }

    if let Some(o) = f.outcome {
        let p = push(&mut args, Arg::S(o.as_str().to_owned()));
        parts.push(format!("outcome = {p}"));
    }

    if let Some(s) = f.since {
        let p = push(&mut args, Arg::I(s));
        parts.push(format!("at >= {p}"));
    }

    if let Some(u) = f.until {
        let p = push(&mut args, Arg::I(u));
        parts.push(format!("at < {p}"));
    }

    if with_cursor && let Some(b) = f.before {
        let p = push(&mut args, Arg::I(b));
        parts.push(format!("seq < {p}"));
    }

    let sql = if parts.is_empty() { String::new() } else { format!(" WHERE {}", parts.join(" AND ")) };

    (sql, args)
}

fn bind_all<'q>(
    mut q: sqlx::query::Query<'q, sqlx::Any, sqlx::any::AnyArguments<'q>>,
    args: Vec<Arg>,
) -> sqlx::query::Query<'q, sqlx::Any, sqlx::any::AnyArguments<'q>> {
    for a in args {
        q = match a {
            Arg::S(s) => q.bind(s),
            Arg::I(i) => q.bind(i),
        };
    }

    q
}

#[async_trait]
impl AuditRepository for Store {
    async fn last_entry(&self) -> DomainResult<Option<AuditEntry>> {
        let r = sqlx::query("SELECT * FROM audit_log ORDER BY seq DESC LIMIT 1")
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(entry_row).transpose()
    }

    async fn insert_entry(&self, e: &AuditEntry) -> DomainResult<()> {
        sqlx::query(
            "INSERT INTO audit_log (seq, id, at, actor, action, resource, workspace_id, outcome, details, prev_hash, hash, signature)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
        )
        .bind(e.seq)
        .bind(&e.id)
        .bind(e.at.0)
        .bind(&e.actor)
        .bind(&e.action)
        .bind(&e.resource)
        .bind(e.workspace_id.map(|o| o.to_string()))
        .bind(e.outcome.as_str())
        .bind(e.details.to_string())
        .bind(&e.prev_hash)
        .bind(&e.hash)
        .bind(&e.signature)
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn entries_from(&self, from_seq: i64, limit: u32) -> DomainResult<Vec<AuditEntry>> {
        let rows = sqlx::query("SELECT * FROM audit_log WHERE seq >= $1 ORDER BY seq LIMIT $2")
            .bind(from_seq)
            .bind(i64::from(limit))
            .fetch_all(self.pool())
            .await
            .map_err(db_err)?;
        rows.iter().map(entry_row).collect()
    }

    async fn recent_entries(&self, org: Option<WorkspaceId>, limit: u32) -> DomainResult<Vec<AuditEntry>> {
        let rows =
            sqlx::query("SELECT * FROM audit_log WHERE ($1 IS NULL OR workspace_id = $1) ORDER BY seq DESC LIMIT $2")
                .bind(org.map(|o| o.to_string()))
                .bind(i64::from(limit.clamp(1, 500)))
                .fetch_all(self.pool())
                .await
                .map_err(db_err)?;
        rows.iter().map(entry_row).collect()
    }

    async fn search_entries(&self, f: &AuditFilter) -> DomainResult<Vec<AuditEntry>> {
        let (clause, mut args) = where_clause(f, true);
        args.push(Arg::I(i64::from(f.limit.clamp(1, 500))));
        let sql = format!("SELECT * FROM audit_log{clause} ORDER BY seq DESC LIMIT ${}", args.len());
        let rows = bind_all(sqlx::query(&sql), args).fetch_all(self.pool()).await.map_err(db_err)?;

        rows.iter().map(entry_row).collect()
    }

    async fn count_entries(&self, f: &AuditFilter) -> DomainResult<i64> {
        let (clause, args) = where_clause(f, false);
        let sql = format!("SELECT COUNT(*) AS n FROM audit_log{clause}");
        let r = bind_all(sqlx::query(&sql), args).fetch_one(self.pool()).await.map_err(db_err)?;

        int(&r, "n")
    }

    async fn bucket_counts(&self, f: &AuditFilter, from: i64, step_ms: i64) -> DomainResult<Vec<AuditBucketCount>> {
        let f = AuditFilter { since: Some(f.since.map_or(from, |s| s.max(from))), ..f.clone() };
        let (clause, mut args) = where_clause(&f, false);
        args.push(Arg::I(from));
        let from_p = args.len();
        args.push(Arg::I(step_ms.max(1)));
        let step_p = args.len();
        let sql = format!(
            "SELECT (at - ${from_p}) / ${step_p} AS bucket, outcome, COUNT(*) AS n FROM audit_log{clause} \
             GROUP BY 1, 2 ORDER BY 1"
        );
        let rows = bind_all(sqlx::query(&sql), args).fetch_all(self.pool()).await.map_err(db_err)?;

        rows.iter()
            .map(|r| {
                Ok(AuditBucketCount {
                    bucket: int(r, "bucket")?,
                    outcome: AuditOutcome::parse(&text(r, "outcome")?),
                    count: int(r, "n")?,
                })
            })
            .collect()
    }

    async fn distinct_actions(&self, org: Option<WorkspaceId>) -> DomainResult<Vec<AuditActionCount>> {
        let rows = sqlx::query(
            "SELECT action, COUNT(*) AS n FROM audit_log WHERE ($1 IS NULL OR workspace_id = $1)
             GROUP BY action ORDER BY action",
        )
        .bind(org.map(|o| o.to_string()))
        .fetch_all(self.pool())
        .await
        .map_err(db_err)?;
        rows.iter().map(|r| Ok(AuditActionCount { action: text(r, "action")?, count: int(r, "n")? })).collect()
    }
}
