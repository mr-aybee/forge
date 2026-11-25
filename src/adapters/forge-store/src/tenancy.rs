use async_trait::async_trait;
use forge_core::model::{Environment, RoleBinding, Tenant, Workspace};
use forge_core::ports::{RoleBindingRepository, TenancyRepository};
use forge_core::*;
use sqlx::any::AnyRow;

use crate::row::*;
use crate::{Store, db_err};

fn org_row(r: &AnyRow) -> DomainResult<Workspace> {
    Ok(Workspace {
        id: parsed(r, "id")?,
        name: text(r, "name")?,
        slug: text(r, "slug")?,
        created_at: ts(r, "created_at")?,
    })
}

fn tenant_row(r: &AnyRow) -> DomainResult<Tenant> {
    Ok(Tenant {
        id: parsed(r, "id")?,
        workspace_id: parsed(r, "workspace_id")?,
        name: text(r, "name")?,
        created_at: ts(r, "created_at")?,
    })
}

fn env_row(r: &AnyRow) -> DomainResult<Environment> {
    Ok(Environment {
        id: parsed(r, "id")?,
        workspace_id: parsed(r, "workspace_id")?,
        project_id: opt_parsed(r, "project_id")?,
        cluster_id: opt_parsed(r, "cluster_id")?,
        name: text(r, "name")?,
        protected: flag(r, "protected")?,
        created_at: ts(r, "created_at")?,
    })
}

fn binding_row(r: &AnyRow) -> DomainResult<RoleBinding> {
    let kind: EntityKind = text(r, "scope_kind")?.parse().map_err(DomainError::Internal)?;
    Ok(RoleBinding {
        id: parsed(r, "id")?,
        user_id: parsed(r, "user_id")?,
        role: text(r, "role")?.parse().map_err(DomainError::Internal)?,
        scope: EntityRef::new(kind, text(r, "scope_id")?),
        created_at: ts(r, "created_at")?,
    })
}

fn not_found(what: &str, id: impl std::fmt::Display) -> DomainError {
    DomainError::NotFound(format!("{what} {id}"))
}

#[async_trait]
impl TenancyRepository for Store {
    async fn create_workspace(&self, org: &Workspace) -> DomainResult<()> {
        sqlx::query("INSERT INTO workspaces (id, name, slug, created_at) VALUES ($1, $2, $3, $4)")
            .bind(org.id.to_string())
            .bind(&org.name)
            .bind(&org.slug)
            .bind(org.created_at.0)
            .execute(self.pool())
            .await
            .map_err(db_err)?;
        Ok(())
    }

    async fn get_workspace(&self, id: WorkspaceId) -> DomainResult<Workspace> {
        let r = sqlx::query("SELECT * FROM workspaces WHERE id = $1")
            .bind(id.to_string())
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(org_row).transpose()?.ok_or_else(|| not_found("workspace", id))
    }

    async fn list_workspaces(&self) -> DomainResult<Vec<Workspace>> {
        let rows =
            sqlx::query("SELECT * FROM workspaces ORDER BY name").fetch_all(self.pool()).await.map_err(db_err)?;
        rows.iter().map(org_row).collect()
    }

    async fn create_tenant(&self, t: &Tenant) -> DomainResult<()> {
        sqlx::query("INSERT INTO tenants (id, workspace_id, name, created_at) VALUES ($1, $2, $3, $4)")
            .bind(t.id.to_string())
            .bind(t.workspace_id.to_string())
            .bind(&t.name)
            .bind(t.created_at.0)
            .execute(self.pool())
            .await
            .map_err(db_err)?;
        Ok(())
    }

    async fn get_tenant(&self, id: TenantId) -> DomainResult<Tenant> {
        let r = sqlx::query("SELECT * FROM tenants WHERE id = $1")
            .bind(id.to_string())
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(tenant_row).transpose()?.ok_or_else(|| not_found("tenant", id))
    }

    async fn list_tenants(&self, org: WorkspaceId) -> DomainResult<Vec<Tenant>> {
        let rows = sqlx::query("SELECT * FROM tenants WHERE workspace_id = $1 ORDER BY name")
            .bind(org.to_string())
            .fetch_all(self.pool())
            .await
            .map_err(db_err)?;
        rows.iter().map(tenant_row).collect()
    }

    async fn create_environment(&self, e: &Environment) -> DomainResult<()> {
        let scope = self.environment_scope(e.workspace_id, e.cluster_id, e.created_at).await?;
        sqlx::query(
            "INSERT INTO environments (id, workspace_id, scope_id, cluster_id, project_id, name, protected, created_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(e.id.to_string())
        .bind(e.workspace_id.to_string())
        .bind(scope)
        .bind(e.cluster_id.map(|c| c.to_string()))
        .bind(e.project_id.map(|g| g.to_string()))
        .bind(&e.name)
        .bind(b(e.protected))
        .bind(e.created_at.0)
        .execute(self.pool())
        .await
        .map_err(db_err)?;

        Ok(())
    }

    async fn get_environment(&self, id: EnvironmentId) -> DomainResult<Environment> {
        let r = sqlx::query("SELECT * FROM environments WHERE id = $1")
            .bind(id.to_string())
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(env_row).transpose()?.ok_or_else(|| not_found("environment", id))
    }

    async fn list_environments(&self, ws: WorkspaceId, cluster: Option<ClusterId>) -> DomainResult<Vec<Environment>> {
        let rows = match cluster {
            Some(c) => {
                sqlx::query("SELECT * FROM environments WHERE workspace_id = $1 AND cluster_id = $2 ORDER BY name")
                    .bind(ws.to_string())
                    .bind(c.to_string())
                    .fetch_all(self.pool())
                    .await
            }
            None => {
                sqlx::query("SELECT * FROM environments WHERE workspace_id = $1 ORDER BY name")
                    .bind(ws.to_string())
                    .fetch_all(self.pool())
                    .await
            }
        }
        .map_err(db_err)?;

        rows.iter().map(env_row).collect()
    }

    async fn set_environment_cluster(&self, id: EnvironmentId, cluster: ClusterId) -> DomainResult<()> {
        let ns = self.get_environment(id).await?;
        let scope = self.environment_scope(ns.workspace_id, Some(cluster), ns.created_at).await?;

        // The scope moves with the cluster, so UNIQUE (scope_id, name)
        // refuses a name the target cluster already has.
        let r = sqlx::query("UPDATE environments SET cluster_id = $1, scope_id = $2 WHERE id = $3")
            .bind(cluster.to_string())
            .bind(scope)
            .bind(id.to_string())
            .execute(self.pool())
            .await
            .map_err(db_err)?;

        if r.rows_affected() == 0 {
            return Err(not_found("environment", id));
        }

        Ok(())
    }

    async fn list_environments_in_group(&self, ws: WorkspaceId, group: ProjectId) -> DomainResult<Vec<Environment>> {
        let rows = sqlx::query("SELECT * FROM environments WHERE workspace_id = $1 AND project_id = $2 ORDER BY name")
            .bind(ws.to_string())
            .bind(group.to_string())
            .fetch_all(self.pool())
            .await
            .map_err(db_err)?;
        rows.iter().map(env_row).collect()
    }

    async fn set_environment_project(&self, id: EnvironmentId, group: Option<ProjectId>) -> DomainResult<()> {
        let r = sqlx::query("UPDATE environments SET project_id = $1 WHERE id = $2")
            .bind(group.map(|g| g.to_string()))
            .bind(id.to_string())
            .execute(self.pool())
            .await
            .map_err(db_err)?;

        if r.rows_affected() == 0 {
            return Err(not_found("environment", id));
        }

        Ok(())
    }

    async fn rename_environment(&self, id: EnvironmentId, name: &str) -> DomainResult<()> {
        // UNIQUE (scope_id, name) refuses a name the cluster already has.
        let r = sqlx::query("UPDATE environments SET name = $1 WHERE id = $2")
            .bind(name)
            .bind(id.to_string())
            .execute(self.pool())
            .await
            .map_err(db_err)?;

        if r.rows_affected() == 0 {
            return Err(not_found("environment", id));
        }

        Ok(())
    }

    async fn delete_environment(&self, id: EnvironmentId) -> DomainResult<()> {
        let mut tx = self.pool().begin().await.map_err(db_err)?;

        // Nodes and clusters only point at an environment as a label.
        sqlx::query("UPDATE servers SET environment_id = NULL WHERE environment_id = $1")
            .bind(id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        sqlx::query("UPDATE clusters SET environment_id = NULL WHERE environment_id = $1")
            .bind(id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;

        // A store must never point at an environment that is gone (D-107). The
        // check runs inside this transaction, after its first write, so on
        // SQLite no owner can be assigned between the check and the delete.
        let owned = sqlx::query("SELECT COUNT(*) AS n FROM object_stores WHERE environment_id = $1")
            .bind(id.to_string())
            .fetch_one(&mut *tx)
            .await
            .map_err(db_err)?;
        let owned = int(&owned, "n")?;
        if owned > 0 {
            return Err(DomainError::Conflict(format!(
                "this environment owns {owned} object store(s); assign them to another environment or clear their owner first"
            )));
        }

        let r = sqlx::query("DELETE FROM environments WHERE id = $1")
            .bind(id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;

        if r.rows_affected() == 0 {
            return Err(not_found("environment", id));
        }

        tx.commit().await.map_err(db_err)
    }
}

impl Store {
    /// The internal uniqueness scope of an environment (migration 0008): its
    /// cluster, or its workspace while it has none. Created on first use.
    async fn environment_scope(
        &self,
        ws: WorkspaceId,
        cluster: Option<ClusterId>,
        at: Timestamp,
    ) -> DomainResult<String> {
        let (id, name) = match cluster {
            Some(c) => (c.to_string(), format!("cluster:{c}")),
            None => (ws.to_string(), "workspace".to_owned()),
        };

        sqlx::query(
            "INSERT INTO environment_scopes (id, workspace_id, tenant_id, name, created_at) VALUES ($1, $2, NULL, $3, $4)
             ON CONFLICT (id) DO NOTHING",
        )
        .bind(&id)
        .bind(ws.to_string())
        .bind(name)
        .bind(at.0)
        .execute(self.pool())
        .await
        .map_err(db_err)?;

        Ok(id)
    }
}

#[async_trait]
impl RoleBindingRepository for Store {
    async fn create_binding(&self, rb: &RoleBinding) -> DomainResult<()> {
        sqlx::query(
            "INSERT INTO role_bindings (id, user_id, role, scope_kind, scope_id, created_at) VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(rb.id.to_string())
        .bind(rb.user_id.to_string())
        .bind(rb.role.as_str())
        .bind(rb.scope.kind.as_str())
        .bind(&rb.scope.id)
        .bind(rb.created_at.0)
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn delete_binding(&self, id: RoleBindingId) -> DomainResult<()> {
        sqlx::query("DELETE FROM role_bindings WHERE id = $1")
            .bind(id.to_string())
            .execute(self.pool())
            .await
            .map_err(db_err)?;
        Ok(())
    }

    async fn list_bindings(&self) -> DomainResult<Vec<RoleBinding>> {
        let rows = sqlx::query("SELECT * FROM role_bindings ORDER BY created_at")
            .fetch_all(self.pool())
            .await
            .map_err(db_err)?;
        rows.iter().map(binding_row).collect()
    }

    async fn list_bindings_for_user(&self, user: UserId) -> DomainResult<Vec<RoleBinding>> {
        let rows = sqlx::query("SELECT * FROM role_bindings WHERE user_id = $1 ORDER BY created_at")
            .bind(user.to_string())
            .fetch_all(self.pool())
            .await
            .map_err(db_err)?;
        rows.iter().map(binding_row).collect()
    }
}
