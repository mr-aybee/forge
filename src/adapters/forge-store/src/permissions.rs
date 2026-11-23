//! Access profiles, their revisions, assignments and service accounts
//! (migration 0019, D-086).

use async_trait::async_trait;
use forge_core::permissions::{
    Assignee, Group, PermissionProfile, PermissionRepository, ProfileAssignment, ProfileRevision, ServiceAccount,
    load_actions,
};
use forge_core::*;
use sqlx::any::AnyRow;

use crate::row::*;
use crate::{Store, db_err};

fn profile_row(r: &AnyRow) -> DomainResult<PermissionProfile> {
    Ok(PermissionProfile {
        id: parsed(r, "id")?,
        name: text(r, "name")?,
        description: text(r, "description")?,
        builtin: flag(r, "builtin")?,
        revision: u32_col(r, "revision")?,
        archived_at: opt_ts(r, "archived_at")?,
        created_by: text(r, "created_by")?,
        created_at: ts(r, "created_at")?,
        updated_at: ts(r, "updated_at")?,
    })
}

fn revision_row(r: &AnyRow) -> DomainResult<ProfileRevision> {
    let names: Vec<String> = serde_json::from_str(&text(r, "actions")?).map_err(DomainError::internal)?;
    Ok(ProfileRevision {
        profile_id: parsed(r, "profile_id")?,
        revision: u32_col(r, "revision")?,
        // Stored lists were checked on the way in; re-check on the way out so
        // a hand-edited row can never grant an unknown action. Retired names
        // (removed actions) are dropped, so an upgrade still starts.
        actions: load_actions(&names)?,
        created_by: text(r, "created_by")?,
        created_at: ts(r, "created_at")?,
    })
}

fn assignment_row(r: &AnyRow) -> DomainResult<ProfileAssignment> {
    Ok(ProfileAssignment {
        id: parsed(r, "id")?,
        user_id: Some(parsed(r, "user_id")?),
        group_id: None,
        profile_id: parsed(r, "profile_id")?,
        scope: text(r, "scope")?.parse().map_err(DomainError::internal)?,
        expires_at: opt_ts(r, "expires_at")?,
        created_by: text(r, "created_by")?,
        created_at: ts(r, "created_at")?,
    })
}

fn group_assignment_row(r: &AnyRow) -> DomainResult<ProfileAssignment> {
    Ok(ProfileAssignment {
        id: parsed(r, "id")?,
        user_id: None,
        group_id: Some(parsed(r, "group_id")?),
        profile_id: parsed(r, "profile_id")?,
        scope: text(r, "scope")?.parse().map_err(DomainError::internal)?,
        expires_at: opt_ts(r, "expires_at")?,
        created_by: text(r, "created_by")?,
        created_at: ts(r, "created_at")?,
    })
}

fn group_row(r: &AnyRow) -> DomainResult<Group> {
    Ok(Group {
        id: parsed(r, "id")?,
        name: text(r, "name")?,
        description: text(r, "description")?,
        created_by: text(r, "created_by")?,
        created_at: ts(r, "created_at")?,
    })
}

fn service_row(r: &AnyRow) -> DomainResult<ServiceAccount> {
    Ok(ServiceAccount {
        user_id: parsed(r, "user_id")?,
        owner_id: parsed(r, "owner_id")?,
        description: text(r, "description")?,
        created_by: text(r, "created_by")?,
        created_at: ts(r, "created_at")?,
    })
}

fn actions_json(actions: &[Action]) -> DomainResult<String> {
    serde_json::to_string(&actions.iter().map(|a| a.as_str()).collect::<Vec<_>>()).map_err(DomainError::internal)
}

#[async_trait]
impl PermissionRepository for Store {
    async fn list_profiles(&self) -> DomainResult<Vec<PermissionProfile>> {
        let rows = sqlx::query("SELECT * FROM permission_profiles ORDER BY builtin DESC, name")
            .fetch_all(self.pool())
            .await
            .map_err(db_err)?;
        rows.iter().map(profile_row).collect()
    }

    async fn get_profile(&self, id: PermissionProfileId) -> DomainResult<Option<PermissionProfile>> {
        let row = sqlx::query("SELECT * FROM permission_profiles WHERE id = $1")
            .bind(id.to_string())
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        row.as_ref().map(profile_row).transpose()
    }

    async fn create_profile(&self, p: &PermissionProfile, first: &ProfileRevision) -> DomainResult<()> {
        let mut tx = self.pool().begin().await.map_err(db_err)?;
        sqlx::query(
            "INSERT INTO permission_profiles
             (id, name, description, builtin, revision, archived_at, created_by, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(p.id.to_string())
        .bind(&p.name)
        .bind(&p.description)
        .bind(i64::from(p.builtin))
        .bind(i64::from(p.revision))
        .bind(p.archived_at.map(|t| t.0))
        .bind(&p.created_by)
        .bind(p.created_at.0)
        .bind(p.updated_at.0)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        sqlx::query(
            "INSERT INTO permission_profile_revisions (profile_id, revision, actions, created_by, created_at)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(first.profile_id.to_string())
        .bind(i64::from(first.revision))
        .bind(actions_json(&first.actions)?)
        .bind(&first.created_by)
        .bind(first.created_at.0)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        tx.commit().await.map_err(db_err)
    }

    async fn publish_revision(&self, rev: &ProfileRevision, at: Timestamp) -> DomainResult<()> {
        let mut tx = self.pool().begin().await.map_err(db_err)?;
        // Only the next number, and never on a built-in or archived profile:
        // a concurrent publish loses instead of overwriting.
        let r = sqlx::query(
            "UPDATE permission_profiles SET revision = $1, updated_at = $2
             WHERE id = $3 AND revision = $4 AND builtin = 0 AND archived_at IS NULL",
        )
        .bind(i64::from(rev.revision))
        .bind(at.0)
        .bind(rev.profile_id.to_string())
        .bind(i64::from(rev.revision.saturating_sub(1)))
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        if r.rows_affected() != 1 {
            return Err(DomainError::Conflict("the profile changed meanwhile, or it is built in or archived".into()));
        }
        sqlx::query(
            "INSERT INTO permission_profile_revisions (profile_id, revision, actions, created_by, created_at)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(rev.profile_id.to_string())
        .bind(i64::from(rev.revision))
        .bind(actions_json(&rev.actions)?)
        .bind(&rev.created_by)
        .bind(rev.created_at.0)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        tx.commit().await.map_err(db_err)
    }

    async fn get_revision(&self, id: PermissionProfileId, revision: u32) -> DomainResult<Option<ProfileRevision>> {
        let row = sqlx::query("SELECT * FROM permission_profile_revisions WHERE profile_id = $1 AND revision = $2")
            .bind(id.to_string())
            .bind(i64::from(revision))
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        row.as_ref().map(revision_row).transpose()
    }

    async fn list_revisions(&self, id: PermissionProfileId) -> DomainResult<Vec<ProfileRevision>> {
        let rows = sqlx::query("SELECT * FROM permission_profile_revisions WHERE profile_id = $1 ORDER BY revision")
            .bind(id.to_string())
            .fetch_all(self.pool())
            .await
            .map_err(db_err)?;
        rows.iter().map(revision_row).collect()
    }

    async fn archive_profile(&self, id: PermissionProfileId, at: Timestamp) -> DomainResult<()> {
        let r = sqlx::query(
            "UPDATE permission_profiles SET archived_at = $1, updated_at = $1
             WHERE id = $2 AND builtin = 0 AND archived_at IS NULL",
        )
        .bind(at.0)
        .bind(id.to_string())
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        if r.rows_affected() != 1 {
            return Err(DomainError::Conflict("the profile is built in or already archived".into()));
        }
        Ok(())
    }

    async fn sync_builtin(&self, p: &PermissionProfile, rev: &ProfileRevision) -> DomainResult<PermissionProfile> {
        let existing = sqlx::query("SELECT * FROM permission_profiles WHERE name = $1 AND builtin = 1")
            .bind(&p.name)
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        let Some(row) = existing else {
            self.create_profile(p, rev).await?;
            return Ok(p.clone());
        };
        let current = profile_row(&row)?;
        let latest = self.get_revision(current.id, current.revision).await?;
        if latest.is_some_and(|l| l.actions == rev.actions) {
            return Ok(current);
        }
        let next = current.revision + 1;
        let mut tx = self.pool().begin().await.map_err(db_err)?;
        sqlx::query(
            "INSERT INTO permission_profile_revisions (profile_id, revision, actions, created_by, created_at)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(current.id.to_string())
        .bind(i64::from(next))
        .bind(actions_json(&rev.actions)?)
        .bind(&rev.created_by)
        .bind(rev.created_at.0)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        sqlx::query("UPDATE permission_profiles SET revision = $1, updated_at = $2 WHERE id = $3")
            .bind(i64::from(next))
            .bind(rev.created_at.0)
            .bind(current.id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        tx.commit().await.map_err(db_err)?;
        Ok(PermissionProfile { revision: next, updated_at: rev.created_at, ..current })
    }

    async fn list_assignments(&self) -> DomainResult<Vec<ProfileAssignment>> {
        let rows = sqlx::query("SELECT * FROM profile_assignments ORDER BY created_at")
            .fetch_all(self.pool())
            .await
            .map_err(db_err)?;
        let mut out = rows.iter().map(assignment_row).collect::<DomainResult<Vec<_>>>()?;
        let rows = sqlx::query("SELECT * FROM group_assignments ORDER BY created_at")
            .fetch_all(self.pool())
            .await
            .map_err(db_err)?;
        for r in &rows {
            out.push(group_assignment_row(r)?);
        }
        out.sort_by_key(|a| a.created_at);
        Ok(out)
    }

    async fn create_assignment(&self, a: &ProfileAssignment, revision: u32) -> DomainResult<()> {
        // The group must still exist, like the profile: a concurrent delete
        // of the group wins instead of leaving an orphan grant.
        let (sql, who) = match a.assignee() {
            Some(Assignee::User(u)) => (
                "INSERT INTO profile_assignments (id, user_id, profile_id, scope, expires_at, created_by, created_at)
                 SELECT $1, $2, $3, $4, $5, $6, $7
                 WHERE EXISTS (SELECT 1 FROM permission_profiles
                               WHERE id = $3 AND archived_at IS NULL AND revision = $8)",
                u.to_string(),
            ),
            Some(Assignee::Group(g)) => (
                "INSERT INTO group_assignments (id, group_id, profile_id, scope, expires_at, created_by, created_at)
                 SELECT $1, $2, $3, $4, $5, $6, $7
                 WHERE EXISTS (SELECT 1 FROM permission_profiles
                               WHERE id = $3 AND archived_at IS NULL AND revision = $8)
                   AND EXISTS (SELECT 1 FROM access_groups WHERE id = $2)",
                g.to_string(),
            ),
            None => return Err(DomainError::Invalid("an assignment names exactly one user or group".into())),
        };
        let r = sqlx::query(sql)
            .bind(a.id.to_string())
            .bind(who)
            .bind(a.profile_id.to_string())
            .bind(a.scope.to_string())
            .bind(a.expires_at.map(|t| t.0))
            .bind(&a.created_by)
            .bind(a.created_at.0)
            .bind(i64::from(revision))
            .execute(self.pool())
            .await
            .map_err(db_err)?;
        if r.rows_affected() != 1 {
            return Err(DomainError::Conflict("the profile was archived or changed meanwhile; try again".into()));
        }
        Ok(())
    }

    async fn delete_assignment(&self, id: ProfileAssignmentId) -> DomainResult<bool> {
        let mut n = 0;
        for sql in ["DELETE FROM profile_assignments WHERE id = $1", "DELETE FROM group_assignments WHERE id = $1"] {
            let r = sqlx::query(sql).bind(id.to_string()).execute(self.pool()).await.map_err(db_err)?;
            n += r.rows_affected();
        }
        Ok(n > 0)
    }

    async fn list_service_accounts(&self) -> DomainResult<Vec<ServiceAccount>> {
        let rows = sqlx::query("SELECT * FROM service_accounts ORDER BY created_at")
            .fetch_all(self.pool())
            .await
            .map_err(db_err)?;
        rows.iter().map(service_row).collect()
    }

    async fn get_service_account(&self, user: UserId) -> DomainResult<Option<ServiceAccount>> {
        let row = sqlx::query("SELECT * FROM service_accounts WHERE user_id = $1")
            .bind(user.to_string())
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        row.as_ref().map(service_row).transpose()
    }

    async fn create_service_account(&self, user: &forge_core::model::User, sa: &ServiceAccount) -> DomainResult<()> {
        let mut tx = self.pool().begin().await.map_err(db_err)?;
        sqlx::query(
            "INSERT INTO users (id, username, email, display_name, password_hash, totp_secret_sealed, oidc_subject,
                                disabled, created_at, kind)
             VALUES ($1, $2, NULL, $3, NULL, NULL, NULL, $4, $5, 'service')",
        )
        .bind(user.id.to_string())
        .bind(user.username.as_deref().map(str::to_lowercase))
        .bind(&user.display_name)
        .bind(i64::from(user.disabled))
        .bind(user.created_at.0)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        sqlx::query(
            "INSERT INTO service_accounts (user_id, owner_id, description, created_by, created_at)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(sa.user_id.to_string())
        .bind(sa.owner_id.to_string())
        .bind(&sa.description)
        .bind(&sa.created_by)
        .bind(sa.created_at.0)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        tx.commit().await.map_err(db_err)
    }

    async fn set_service_account_disabled(&self, user: UserId, disabled: bool) -> DomainResult<bool> {
        let r = sqlx::query(
            "UPDATE users SET disabled = $1
             WHERE id = $2 AND kind = 'service' AND id IN (SELECT user_id FROM service_accounts)",
        )
        .bind(i64::from(disabled))
        .bind(user.to_string())
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(r.rows_affected() == 1)
    }

    async fn list_groups(&self) -> DomainResult<Vec<Group>> {
        let rows = sqlx::query("SELECT * FROM access_groups ORDER BY name_key")
            .fetch_all(self.pool())
            .await
            .map_err(db_err)?;
        rows.iter().map(group_row).collect()
    }

    async fn get_group(&self, id: GroupId) -> DomainResult<Option<Group>> {
        let row = sqlx::query("SELECT * FROM access_groups WHERE id = $1")
            .bind(id.to_string())
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        row.as_ref().map(group_row).transpose()
    }

    async fn create_group(&self, g: &Group) -> DomainResult<()> {
        sqlx::query(
            "INSERT INTO access_groups (id, name, name_key, description, created_by, created_at)
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(g.id.to_string())
        .bind(&g.name)
        .bind(g.name.to_lowercase())
        .bind(&g.description)
        .bind(&g.created_by)
        .bind(g.created_at.0)
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn update_group(&self, g: &Group) -> DomainResult<()> {
        let r = sqlx::query("UPDATE access_groups SET name = $1, name_key = $2, description = $3 WHERE id = $4")
            .bind(&g.name)
            .bind(g.name.to_lowercase())
            .bind(&g.description)
            .bind(g.id.to_string())
            .execute(self.pool())
            .await
            .map_err(db_err)?;
        if r.rows_affected() != 1 {
            return Err(DomainError::NotFound("group not found".into()));
        }
        Ok(())
    }

    async fn delete_group(&self, id: GroupId) -> DomainResult<Vec<ProfileAssignmentId>> {
        // Atomic: the group, its members and its grants go together.
        let mut tx = self.pool().begin().await.map_err(db_err)?;
        let rows = sqlx::query("SELECT id FROM group_assignments WHERE group_id = $1")
            .bind(id.to_string())
            .fetch_all(&mut *tx)
            .await
            .map_err(db_err)?;
        let ids = rows.iter().map(|r| parsed(r, "id")).collect::<DomainResult<Vec<ProfileAssignmentId>>>()?;
        for sql in [
            "DELETE FROM group_assignments WHERE group_id = $1",
            "DELETE FROM access_group_members WHERE group_id = $1",
            "DELETE FROM access_groups WHERE id = $1",
        ] {
            sqlx::query(sql).bind(id.to_string()).execute(&mut *tx).await.map_err(db_err)?;
        }
        tx.commit().await.map_err(db_err)?;
        Ok(ids)
    }

    async fn list_memberships(&self) -> DomainResult<Vec<(GroupId, UserId)>> {
        let rows = sqlx::query("SELECT group_id, user_id FROM access_group_members ORDER BY created_at")
            .fetch_all(self.pool())
            .await
            .map_err(db_err)?;
        rows.iter().map(|r| Ok((parsed(r, "group_id")?, parsed(r, "user_id")?))).collect()
    }

    async fn add_member(&self, group: GroupId, user: UserId, at: Timestamp) -> DomainResult<bool> {
        let r = sqlx::query(
            "INSERT INTO access_group_members (group_id, user_id, created_at)
             SELECT $1, $2, $3
             WHERE EXISTS (SELECT 1 FROM access_groups WHERE id = $1)
               AND NOT EXISTS (SELECT 1 FROM access_group_members WHERE group_id = $1 AND user_id = $2)",
        )
        .bind(group.to_string())
        .bind(user.to_string())
        .bind(at.0)
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(r.rows_affected() == 1)
    }

    async fn remove_member(&self, group: GroupId, user: UserId) -> DomainResult<bool> {
        let r = sqlx::query("DELETE FROM access_group_members WHERE group_id = $1 AND user_id = $2")
            .bind(group.to_string())
            .bind(user.to_string())
            .execute(self.pool())
            .await
            .map_err(db_err)?;
        Ok(r.rows_affected() == 1)
    }
}
