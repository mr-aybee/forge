//! Projects: the products that group environments (D-102).

use async_trait::async_trait;
use forge_core::model::Project;
use forge_core::ports::ProjectRepository;
use forge_core::*;
use sqlx::any::AnyRow;

use crate::row::*;
use crate::{Store, db_err};

fn group_row(r: &AnyRow) -> DomainResult<Project> {
    Ok(Project {
        id: parsed(r, "id")?,
        workspace_id: parsed(r, "workspace_id")?,
        name: text(r, "name")?,
        description: text(r, "description")?,
        created_at: ts(r, "created_at")?,
    })
}

fn not_found(id: ProjectId) -> DomainError {
    DomainError::NotFound(format!("project {id}"))
}

#[async_trait]
impl ProjectRepository for Store {
    async fn create_project(&self, g: &Project) -> DomainResult<()> {
        sqlx::query(
            "INSERT INTO projects (id, workspace_id, name, description, created_at) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(g.id.to_string())
        .bind(g.workspace_id.to_string())
        .bind(&g.name)
        .bind(&g.description)
        .bind(g.created_at.0)
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn get_project(&self, id: ProjectId) -> DomainResult<Project> {
        let r = sqlx::query("SELECT * FROM projects WHERE id = $1")
            .bind(id.to_string())
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(group_row).transpose()?.ok_or_else(|| not_found(id))
    }

    async fn list_projects(&self, ws: WorkspaceId) -> DomainResult<Vec<Project>> {
        let rows = sqlx::query("SELECT * FROM projects WHERE workspace_id = $1 ORDER BY name")
            .bind(ws.to_string())
            .fetch_all(self.pool())
            .await
            .map_err(db_err)?;
        rows.iter().map(group_row).collect()
    }

    async fn update_project(&self, id: ProjectId, name: &str, description: &str) -> DomainResult<()> {
        let r = sqlx::query("UPDATE projects SET name = $1, description = $2 WHERE id = $3")
            .bind(name)
            .bind(description)
            .bind(id.to_string())
            .execute(self.pool())
            .await
            .map_err(db_err)?;

        if r.rows_affected() == 0 {
            return Err(not_found(id));
        }

        Ok(())
    }

    async fn delete_project(&self, id: ProjectId) -> DomainResult<()> {
        let r = sqlx::query("DELETE FROM projects WHERE id = $1")
            .bind(id.to_string())
            .execute(self.pool())
            .await
            .map_err(db_err)?;

        if r.rows_affected() == 0 {
            return Err(not_found(id));
        }

        Ok(())
    }
}
