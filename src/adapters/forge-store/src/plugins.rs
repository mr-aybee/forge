use async_trait::async_trait;
use forge_core::model::{PluginRecord, PluginState};
use forge_core::ports::PluginRepository;
use forge_core::*;
use sqlx::any::AnyRow;

use crate::row::*;
use crate::{Store, db_err};

fn plugin_row(r: &AnyRow) -> DomainResult<PluginRecord> {
    Ok(PluginRecord {
        plugin_id: text(r, "plugin_id")?,
        version: text(r, "version")?,
        publisher: text(r, "publisher")?,
        manifest_toml: text(r, "manifest_toml")?,
        artifact_sha256: text(r, "artifact_sha256")?,
        state: PluginState::parse(&text(r, "state")?),
        pinned: flag(r, "pinned")?,
        approved_permissions: serde_json::from_str(&text(r, "approved_permissions")?).map_err(DomainError::internal)?,
        fault_count: u32_col(r, "fault_count")?,
        quarantine_reason: opt_text(r, "quarantine_reason")?,
        installed_at: ts(r, "installed_at")?,
        updated_at: ts(r, "updated_at")?,
    })
}

#[async_trait]
impl PluginRepository for Store {
    async fn upsert_plugin(&self, p: &PluginRecord) -> DomainResult<()> {
        let perms = serde_json::to_string(&p.approved_permissions).map_err(DomainError::internal)?;
        sqlx::query(
            "INSERT INTO plugins (plugin_id, version, publisher, manifest_toml, artifact_sha256, state, pinned,
                                  approved_permissions, fault_count, quarantine_reason, installed_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
             ON CONFLICT (plugin_id) DO UPDATE SET
               version = $2, publisher = $3, manifest_toml = $4, artifact_sha256 = $5, state = $6, pinned = $7,
               approved_permissions = $8, fault_count = $9, quarantine_reason = $10, updated_at = $12",
        )
        .bind(&p.plugin_id)
        .bind(&p.version)
        .bind(&p.publisher)
        .bind(&p.manifest_toml)
        .bind(&p.artifact_sha256)
        .bind(p.state.as_str())
        .bind(b(p.pinned))
        .bind(perms)
        .bind(i64::from(p.fault_count))
        .bind(p.quarantine_reason.clone())
        .bind(p.installed_at.0)
        .bind(p.updated_at.0)
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn get_plugin(&self, plugin_id: &str) -> DomainResult<Option<PluginRecord>> {
        let r = sqlx::query("SELECT * FROM plugins WHERE plugin_id = $1")
            .bind(plugin_id)
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(plugin_row).transpose()
    }

    async fn list_plugins(&self) -> DomainResult<Vec<PluginRecord>> {
        let rows =
            sqlx::query("SELECT * FROM plugins ORDER BY plugin_id").fetch_all(self.pool()).await.map_err(db_err)?;
        rows.iter().map(plugin_row).collect()
    }

    async fn delete_plugin(&self, plugin_id: &str) -> DomainResult<()> {
        sqlx::query("DELETE FROM plugins WHERE plugin_id = $1")
            .bind(plugin_id)
            .execute(self.pool())
            .await
            .map_err(db_err)?;
        Ok(())
    }
}
