use async_trait::async_trait;
use forge_core::stacks::{Stack, StackKind, StackNode, StackRepository, StackRevision};
use forge_core::*;
use sqlx::any::AnyRow;

use crate::row::*;
use crate::{Store, db_err};

fn list_col(r: &AnyRow, col: &str) -> DomainResult<Vec<String>> {
    serde_json::from_str(&text(r, col)?).map_err(DomainError::internal)
}

fn list_db(v: &[String]) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "[]".into())
}

fn stack_row(r: &AnyRow) -> DomainResult<Stack> {
    Ok(Stack {
        id: parsed(r, "id")?,
        workspace_id: parsed(r, "workspace_id")?,
        environment_id: parsed(r, "environment_id")?,
        name: text(r, "name")?,
        kind: StackKind::parse(&text(r, "kind")?).unwrap_or(StackKind::Compose),
        default_server: parsed(r, "default_server_id")?,
        dir: opt_text(r, "dir")?,
        revision: int(r, "revision")?,
        deployed_revision: int(r, "deployed_revision")?,
        env_sealed: opt_text(r, "env_sealed")?,
        env_keys: list_col(r, "env_keys")?,
        created_by: parsed(r, "created_by")?,
        created_at: ts(r, "created_at")?,
        updated_at: ts(r, "updated_at")?,
    })
}

fn revision_row(r: &AnyRow, with_content: bool) -> DomainResult<StackRevision> {
    Ok(StackRevision {
        stack_id: parsed(r, "stack_id")?,
        revision: int(r, "revision")?,
        content: if with_content { text(r, "content")? } else { String::new() },
        sha256: text(r, "sha256")?,
        note: opt_text(r, "note")?,
        author: parsed(r, "author")?,
        created_at: ts(r, "created_at")?,
    })
}

fn node_row(r: &AnyRow) -> DomainResult<StackNode> {
    Ok(StackNode {
        stack_id: parsed(r, "stack_id")?,
        server_id: parsed(r, "server_id")?,
        services: list_col(r, "services")?,
        selected: list_col(r, "selected")?,
        revision: int(r, "revision")?,
        status: text(r, "status")?,
        state: json(r, "state")?,
        last_job_id: opt_parsed(r, "last_job_id")?,
        last_error: opt_text(r, "last_error")?,
        updated_at: ts(r, "updated_at")?,
    })
}

const INSERT_REVISION: &str =
    "INSERT INTO stack_revisions (stack_id, revision, content, sha256, note, author, created_at)
     VALUES ($1, $2, $3, $4, $5, $6, $7)";

#[async_trait]
impl StackRepository for Store {
    async fn create_stack(&self, s: &Stack, first: &StackRevision) -> DomainResult<()> {
        let mut tx = self.pool().begin().await.map_err(db_err)?;
        sqlx::query(
            "INSERT INTO stacks (id, workspace_id, environment_id, name, kind, default_server_id, dir, revision,
                                 deployed_revision, env_sealed, env_keys, created_by, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)",
        )
        .bind(s.id.to_string())
        .bind(s.workspace_id.to_string())
        .bind(s.environment_id.to_string())
        .bind(&s.name)
        .bind(s.kind.as_str())
        .bind(s.default_server.to_string())
        .bind(s.dir.clone())
        .bind(first.revision)
        .bind(s.deployed_revision)
        .bind(s.env_sealed.clone())
        .bind(list_db(&s.env_keys))
        .bind(s.created_by.to_string())
        .bind(s.created_at.0)
        .bind(s.updated_at.0)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        sqlx::query(INSERT_REVISION)
            .bind(s.id.to_string())
            .bind(first.revision)
            .bind(&first.content)
            .bind(&first.sha256)
            .bind(first.note.clone())
            .bind(first.author.to_string())
            .bind(first.created_at.0)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;

        tx.commit().await.map_err(db_err)
    }

    async fn put_stack(&self, s: &Stack) -> DomainResult<()> {
        sqlx::query(
            "UPDATE stacks SET name = $2, default_server_id = $3, dir = $4, deployed_revision = $5, env_sealed = $6,
                               env_keys = $7, updated_at = $8
             WHERE id = $1",
        )
        .bind(s.id.to_string())
        .bind(&s.name)
        .bind(s.default_server.to_string())
        .bind(s.dir.clone())
        .bind(s.deployed_revision)
        .bind(s.env_sealed.clone())
        .bind(list_db(&s.env_keys))
        .bind(s.updated_at.0)
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn claim_stack_edit(&self, id: StackId, seen: Timestamp, now: Timestamp) -> DomainResult<bool> {
        let r = sqlx::query("UPDATE stacks SET updated_at = $3 WHERE id = $1 AND updated_at = $2")
            .bind(id.to_string())
            .bind(seen.0)
            .bind(now.0)
            .execute(self.pool())
            .await
            .map_err(db_err)?;
        Ok(r.rows_affected() == 1)
    }

    async fn set_deployed_revision(&self, id: StackId, revision: i64) -> DomainResult<()> {
        sqlx::query("UPDATE stacks SET deployed_revision = $2 WHERE id = $1")
            .bind(id.to_string())
            .bind(revision)
            .execute(self.pool())
            .await
            .map_err(db_err)?;
        Ok(())
    }

    async fn get_stack(&self, id: StackId) -> DomainResult<Option<Stack>> {
        let r = sqlx::query("SELECT * FROM stacks WHERE id = $1")
            .bind(id.to_string())
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(stack_row).transpose()
    }

    async fn stack_by_name(&self, environment: EnvironmentId, name: &str) -> DomainResult<Option<Stack>> {
        let r = sqlx::query("SELECT * FROM stacks WHERE environment_id = $1 AND name = $2")
            .bind(environment.to_string())
            .bind(name)
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(stack_row).transpose()
    }

    async fn list_stacks(&self, environment: Option<EnvironmentId>) -> DomainResult<Vec<Stack>> {
        let rows = match environment {
            Some(ns) => {
                sqlx::query("SELECT * FROM stacks WHERE environment_id = $1 ORDER BY name")
                    .bind(ns.to_string())
                    .fetch_all(self.pool())
                    .await
            }
            None => sqlx::query("SELECT * FROM stacks ORDER BY name").fetch_all(self.pool()).await,
        }
        .map_err(db_err)?;

        rows.iter().map(stack_row).collect()
    }

    async fn delete_stack(&self, id: StackId) -> DomainResult<()> {
        let mut tx = self.pool().begin().await.map_err(db_err)?;

        for q in [
            "DELETE FROM stack_nodes WHERE stack_id = $1",
            "DELETE FROM stack_revisions WHERE stack_id = $1",
            "DELETE FROM stacks WHERE id = $1",
        ] {
            sqlx::query(q).bind(id.to_string()).execute(&mut *tx).await.map_err(db_err)?;
        }

        tx.commit().await.map_err(db_err)
    }

    async fn add_revision(&self, rev: &StackRevision) -> DomainResult<i64> {
        // Write first: the counter bump takes the write lock (SQLite) or the
        // row lock (PostgreSQL) before anything is read. A transaction that
        // reads first and then writes fails at once on SQLite with "database
        // is locked" when another connection commits in between (the busy
        // timeout does not apply to a read-to-write upgrade), and two
        // concurrent saves would race for the same number.
        let mut tx = self.pool().begin().await.map_err(db_err)?;
        let r =
            sqlx::query("UPDATE stacks SET revision = revision + 1, updated_at = $2 WHERE id = $1 RETURNING revision")
                .bind(rev.stack_id.to_string())
                .bind(rev.created_at.0)
                .fetch_optional(&mut *tx)
                .await
                .map_err(db_err)?
                .ok_or_else(|| DomainError::NotFound("stack".into()))?;
        let next = int(&r, "revision")?;
        sqlx::query(INSERT_REVISION)
            .bind(rev.stack_id.to_string())
            .bind(next)
            .bind(&rev.content)
            .bind(&rev.sha256)
            .bind(rev.note.clone())
            .bind(rev.author.to_string())
            .bind(rev.created_at.0)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        tx.commit().await.map_err(db_err)?;

        Ok(next)
    }

    async fn get_revision(&self, stack: StackId, revision: i64) -> DomainResult<Option<StackRevision>> {
        let r = sqlx::query("SELECT * FROM stack_revisions WHERE stack_id = $1 AND revision = $2")
            .bind(stack.to_string())
            .bind(revision)
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(|r| revision_row(r, true)).transpose()
    }

    async fn list_revisions(&self, stack: StackId) -> DomainResult<Vec<StackRevision>> {
        let rows = sqlx::query(
            "SELECT stack_id, revision, sha256, note, author, created_at FROM stack_revisions
             WHERE stack_id = $1 ORDER BY revision DESC",
        )
        .bind(stack.to_string())
        .fetch_all(self.pool())
        .await
        .map_err(db_err)?;
        rows.iter().map(|r| revision_row(r, false)).collect()
    }

    async fn put_node(&self, n: &StackNode) -> DomainResult<()> {
        sqlx::query(
            "INSERT INTO stack_nodes (stack_id, server_id, services, selected, revision, status, state, last_job_id,
                                      last_error, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
             ON CONFLICT (stack_id, server_id) DO UPDATE SET services = $3, selected = $4, revision = $5, status = $6,
                 state = $7, last_job_id = $8, last_error = $9, updated_at = $10",
        )
        .bind(n.stack_id.to_string())
        .bind(n.server_id.to_string())
        .bind(list_db(&n.services))
        .bind(list_db(&n.selected))
        .bind(n.revision)
        .bind(&n.status)
        .bind(n.state.to_string())
        .bind(n.last_job_id.map(|j| j.to_string()))
        .bind(n.last_error.clone())
        .bind(n.updated_at.0)
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn list_nodes(&self, stack: StackId) -> DomainResult<Vec<StackNode>> {
        let rows = sqlx::query("SELECT * FROM stack_nodes WHERE stack_id = $1 ORDER BY server_id")
            .bind(stack.to_string())
            .fetch_all(self.pool())
            .await
            .map_err(db_err)?;
        rows.iter().map(node_row).collect()
    }

    async fn nodes_on_server(&self, server: ServerId) -> DomainResult<Vec<StackNode>> {
        let rows = sqlx::query("SELECT * FROM stack_nodes WHERE server_id = $1 ORDER BY stack_id")
            .bind(server.to_string())
            .fetch_all(self.pool())
            .await
            .map_err(db_err)?;
        rows.iter().map(node_row).collect()
    }

    async fn delete_node(&self, stack: StackId, server: ServerId) -> DomainResult<()> {
        sqlx::query("DELETE FROM stack_nodes WHERE stack_id = $1 AND server_id = $2")
            .bind(stack.to_string())
            .bind(server.to_string())
            .execute(self.pool())
            .await
            .map_err(db_err)?;
        Ok(())
    }
}
