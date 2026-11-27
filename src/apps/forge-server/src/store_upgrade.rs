//! The upgrade path of the Object Storage removal (D-180).
//!
//! Migration `0009` copied every managed Docker object store's container
//! (one row per member node) into `object_store_adoptions` before dropping
//! the object storage tables. This task adopts each of those containers as
//! a `container` Service through the ordinary adoption path
//! ([`crate::service_adopt`]) as soon as its node is online: same image,
//! environment, ports and mounts; the container is not restarted.
//!
//! * Environment: the store's, or the workspace's oldest when it had none.
//!   A workspace without environments keeps the row until one exists.
//! * Name: the store's slug, then `<slug>-2`, `-3`… on a clash.
//! * The store's console port becomes the service's **Open console** link.
//! * The root credential stays a plain credential vault item; it gains a
//!   link to the new service.
//!
//! A row is deleted once its service exists, or when the container cannot
//! be represented as a service (it is then left running untouched and the
//! failure is audited). Unreachable nodes and old agents are tried again.

use std::sync::Arc;
use std::time::Duration;

use forge_catalog::console_link::{ConsoleLink, ConsoleScheme};
use forge_core::model::{AgentStatus, AuditEvent, AuditOutcome, Environment, VaultLink};
use forge_core::ports::{
    AuditRecorder, PendingStoreAdoption, PendingStoreAdoptionRepository, ServerRepository, ServiceRepository,
    TenancyRepository, VaultRepository,
};
use forge_core::{DomainResult, EntityKind, EntityRef, EnvironmentId};
use serde_json::{Value, json};
use tokio::sync::watch;

use crate::service_adopt::{AUDIT_ADOPT, AdoptError, AdoptRequest, AdoptService};

/// How often pending containers are tried.
pub const TICK: Duration = Duration::from_secs(60);
/// The audit actor of the upgrade's adoptions.
pub const ACTOR: &str = "system:upgrade";
/// Audit: a store container could not be adopted and was left alone.
pub const AUDIT_SKIPPED: &str = "service.adopt.skipped";
/// The container of a store adopted from the App Store before D-170.
const LEGACY_CONTAINER: &str = "forge-rustfs";

/// The ports the upgrade uses.
#[derive(Debug, Clone)]
pub struct StoreUpgrade {
    pub pending: Arc<dyn PendingStoreAdoptionRepository>,
    pub adopt: Arc<AdoptService>,
    pub services: Arc<dyn ServiceRepository>,
    pub servers: Arc<dyn ServerRepository>,
    pub tenancy: Arc<dyn TenancyRepository>,
    pub vault: Arc<dyn VaultRepository>,
    pub audit: Arc<dyn AuditRecorder>,
}

/// What happened to one row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Adopted(String),
    /// Kept for a later pass, with the reason.
    Waiting(String),
    /// Dropped without a service, with the reason.
    Skipped(String),
}

/// The container a store ran in, from its settings JSON: a named container,
/// the App Store's own (adopted before D-170), or `forge-objstore-<slug>`.
#[must_use]
pub fn container_of(slug: &str, settings: Option<&str>) -> String {
    let v: Value = settings.and_then(|s| serde_json::from_str(s).ok()).unwrap_or(Value::Null);
    let named = v.get("container_name").and_then(Value::as_str).filter(|n| !n.is_empty());
    let adopted = v.get("adopted").and_then(Value::as_bool).unwrap_or(false);

    match named {
        Some(n) => n.to_owned(),
        None if adopted => LEGACY_CONTAINER.to_owned(),
        None => format!("forge-objstore-{slug}"),
    }
}

impl StoreUpgrade {
    /// One pass over the pending rows. Returns how many are left.
    pub async fn run_once(&self) -> DomainResult<usize> {
        let rows = self.pending.list_pending_store_adoptions().await?;
        let mut left = 0;

        for row in &rows {
            match self.adopt_row(row).await {
                Ok(Outcome::Adopted(name)) => {
                    tracing::info!(store = %row.id, service = %name, "object store container adopted as a service");
                    self.pending.delete_pending_store_adoption(&row.id).await?;
                }
                Ok(Outcome::Skipped(why)) => {
                    tracing::warn!(store = %row.id, reason = %why, "object store container left running, not adopted");
                    self.record(row, AUDIT_SKIPPED, AuditOutcome::Failure, json!({ "reason": why })).await;
                    self.pending.delete_pending_store_adoption(&row.id).await?;
                }
                Ok(Outcome::Waiting(why)) => {
                    tracing::debug!(store = %row.id, reason = %why, "object store container not adopted yet");
                    left += 1;
                }
                // One row's failure never stops the others.
                Err(e) => {
                    tracing::warn!(store = %row.id, error = %e, "adopting an object store container failed");
                    left += 1;
                }
            }
        }

        Ok(left)
    }

    async fn environment(&self, row: &PendingStoreAdoption) -> DomainResult<Option<Environment>> {
        if let Some(id) = row.environment_id {
            return self.tenancy.get_environment(id).await.map(Some);
        }

        let mut all = self.tenancy.list_environments(row.workspace_id, None).await?;
        all.sort_by_key(|e| (e.created_at, e.id.to_string()));

        Ok(all.into_iter().next())
    }

    /// The store's slug, or the first free `<slug>-<n>`.
    async fn free_name(&self, env: EnvironmentId, slug: &str) -> DomainResult<Option<String>> {
        let taken: Vec<String> = self.services.list_services(Some(env)).await?.into_iter().map(|s| s.name).collect();
        let base: String = slug.chars().take(36).collect();

        let found = std::iter::once(base.clone())
            .chain((2..100).map(|n| format!("{base}-{n}")))
            .find(|n| !taken.contains(n) && forge_catalog::apps::valid_slug(n, 40));

        Ok(found)
    }

    async fn adopt_row(&self, row: &PendingStoreAdoption) -> DomainResult<Outcome> {
        let container = container_of(&row.slug, row.settings.as_deref());

        // Adopted by an earlier pass that stopped before deleting the row.
        let done = self.services.list_server_services(row.server_id).await?;
        if let Some(s) = done.iter().find(|s| s.adopted_from.as_deref() == Some(container.as_str())) {
            return Ok(Outcome::Adopted(s.name.clone()));
        }

        let server = self.servers.get_server(row.server_id).await?;

        if server.status != AgentStatus::Online {
            return Ok(Outcome::Waiting("the node is not connected".into()));
        }

        let Some(env) = self.environment(row).await? else {
            tracing::warn!(store = %row.id, "the store's workspace has no environment to adopt its container into");
            return Ok(Outcome::Waiting("no environment".into()));
        };

        let Some(name) = self.free_name(env.id, &row.slug).await? else {
            return Ok(Outcome::Skipped(format!("no free service name for `{}`", row.slug)));
        };

        let req = AdoptRequest {
            environment_id: env.id,
            server_id: row.server_id,
            container: container.clone(),
            name: Some(name),
            principal: row.created_by.clone(),
            console: row.console_port.map(|port| ConsoleLink { port, path: "/".into(), scheme: ConsoleScheme::Http }),
        };

        let (service, _) = match self.adopt.adopt(&req).await {
            Ok(done) => done,
            Err(AdoptError::Invalid(m)) => return Ok(Outcome::Skipped(m)),
            Err(e @ (AdoptError::Conflict(_) | AdoptError::AgentTooOld)) => return Ok(Outcome::Waiting(e.to_string())),
            Err(AdoptError::Domain(e)) => return Err(e),
        };

        self.link_credential(row, service.id).await?;
        self.record(
            row,
            AUDIT_ADOPT,
            AuditOutcome::Success,
            json!({ "service": service.name, "service_id": service.id, "container": container, "environment_id": env.id }),
        )
        .await;

        Ok(Outcome::Adopted(service.name))
    }

    /// Links the store's root credential to the new service (idempotent).
    async fn link_credential(&self, row: &PendingStoreAdoption, service: forge_core::ServiceId) -> DomainResult<()> {
        let Some(mut item) = self.vault.get_vault_item(row.credential_id).await? else { return Ok(()) };
        let link = VaultLink { kind: "service".into(), id: service.to_string() };

        if item.links.contains(&link) {
            return Ok(());
        }

        item.links.push(link);

        self.vault.put_vault_item(&item).await
    }

    async fn record(&self, row: &PendingStoreAdoption, action: &str, outcome: AuditOutcome, details: Value) {
        let ev = AuditEvent::new(ACTOR, action, EntityRef::new(EntityKind::Server, row.server_id), outcome)
            .in_org(Some(row.workspace_id))
            .with_details(details);

        if let Err(e) = self.audit.record(ev).await {
            tracing::error!(error = %e, "audit write failed");
        }
    }

    /// Runs passes every [`TICK`] until nothing is pending or shutdown.
    pub async fn run(self: Arc<Self>, mut shutdown: watch::Receiver<bool>) {
        let mut tick = tokio::time::interval(TICK);

        loop {
            tokio::select! {
                _ = tick.tick() => {}
                _ = shutdown.changed() => break,
            }

            match self.run_once().await {
                Ok(0) => break,
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "object store upgrade pass failed"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_container_names() {
        assert_eq!(container_of("media", None), "forge-objstore-media");
        assert_eq!(container_of("media", Some(r#"{"image":"rustfs/rustfs"}"#)), "forge-objstore-media");
        assert_eq!(container_of("media", Some(r#"{"container_name":"smoke","adopted":true}"#)), "smoke");
        assert_eq!(container_of("media", Some(r#"{"adopted":true}"#)), "forge-rustfs");
        assert_eq!(container_of("media", Some("not json")), "forge-objstore-media");
    }
}
