//! Settings of a managed RustFS store after install (D-170): ports, console,
//! release and image, limits, restart policy, region and extra variables.
//! Saving records the new settings and returns what the node must do; the
//! `storage.action` job recreates the container (Docker) or reinstalls the
//! service in place (native), and restores the old settings when it fails.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use forge_catalog::storage::{valid_region, valid_version};
use forge_core::storage::{MODE_MANAGED, ObjectStore, ObjectStoreRepository, StoreMember};
use forge_core::storage_settings::{NetworkMode, RestartPolicy, RustfsSettings, StoreSettingsRepository};
use forge_core::{Clock, DomainError, DomainResult, ObjectStoreId};
use serde::{Deserialize, Serialize};

use crate::jobs::storage::{PreviousSettings, spec};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SettingsView {
    pub store_id: ObjectStoreId,
    pub runtime: String,
    pub editable: bool,
    /// Why the settings cannot be changed, when they cannot.
    pub reason: Option<String>,
    pub version: String,
    pub s3_port: u16,
    pub console_port: Option<u16>,
    pub region: String,
    pub docker: Option<RustfsSettings>,
}

/// The container settings that can change after install.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DockerUpdate {
    pub image: String,
    #[schemars(with = "String")]
    pub restart: RestartPolicy,
    #[serde(default)]
    pub memory_mb: Option<u32>,
    #[serde(default)]
    pub cpu_millis: Option<u32>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Fixed after install; accepted and ignored so a GET can be sent back.
    #[serde(default)]
    pub container_name: Option<String>,
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub network: Option<NetworkMode>,
    #[serde(default)]
    #[schemars(with = "Option<Vec<serde_json::Value>>")]
    pub volumes: Option<Vec<forge_core::storage_settings::DriveVolume>>,
    #[serde(default)]
    pub adopted: Option<bool>,
}

/// The full desired state.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SettingsUpdate {
    pub version: String,
    pub s3_port: u16,
    /// `null` turns the console off.
    #[serde(default)]
    pub console_port: Option<u16>,
    pub region: String,
    #[serde(default)]
    pub docker: Option<DockerUpdate>,
}

/// What saving changed.
#[derive(Debug, Clone)]
pub struct Applied {
    pub store: ObjectStore,
    pub previous: PreviousSettings,
    /// The `storage.action` each member runs.
    pub action: &'static str,
}

#[derive(Debug, Clone)]
pub struct StoreSettingsService {
    pub stores: Arc<dyn ObjectStoreRepository>,
    pub settings: Arc<dyn StoreSettingsRepository>,
    pub clock: Arc<dyn Clock>,
}

/// Why a store's settings cannot change, if they cannot.
fn not_editable(store: &ObjectStore, members: &[StoreMember], adopted: bool) -> Option<&'static str> {
    if store.mode != MODE_MANAGED || store.provider != "rustfs" {
        return Some("only managed RustFS stores have settings");
    }

    if members.iter().any(|m| m.data_paths == [forge_catalog::storage::LEGACY_DATA]) {
        return Some("the adopted App Store container is kept as it is");
    }

    if store.runtime == "docker" && adopted {
        return Some("an adopted container keeps the configuration it was found with; Forge does not recreate it");
    }

    if members.is_empty() {
        return Some("the store has no members");
    }

    None
}

fn invalid(m: impl Into<String>) -> DomainError {
    DomainError::Invalid(m.into())
}

impl StoreSettingsService {
    pub async fn view(&self, store: &ObjectStore, members: &[StoreMember]) -> DomainResult<SettingsView> {
        let docker = if store.runtime == "docker" {
            Some(self.settings.get_store_settings(store.id).await?.unwrap_or_default())
        } else {
            None
        };

        let adopted = docker.as_ref().is_some_and(|d| d.adopted);
        let reason = not_editable(store, members, adopted);

        Ok(SettingsView {
            store_id: store.id,
            runtime: store.runtime.clone(),
            editable: reason.is_none(),
            reason: reason.map(str::to_owned),
            version: store.version.clone(),
            s3_port: store.s3_port,
            console_port: store.console_port,
            region: store.region.clone(),
            docker,
        })
    }

    /// Ports other stores use on the member nodes.
    async fn taken_ports(&self, store: &ObjectStore, members: &[StoreMember]) -> DomainResult<HashSet<u16>> {
        let mut taken = HashSet::new();

        for m in members {
            for other in self.stores.list_node_store_members(m.server_id).await? {
                if other.store_id == store.id {
                    continue;
                }

                if let Some(o) = self.stores.get_object_store(other.store_id).await? {
                    taken.insert(o.s3_port);
                    taken.extend(o.console_port);
                }
            }
        }

        Ok(taken)
    }

    /// Validates `update` against every member and records it. Multi-step
    /// write: the store row, then the settings row; the job's failure path
    /// restores both from `previous` (explicit compensation).
    pub async fn apply(
        &self,
        store: &ObjectStore,
        members: &[StoreMember],
        update: &SettingsUpdate,
    ) -> DomainResult<Applied> {
        let current = self.settings.get_store_settings(store.id).await?;
        if let Some(reason) = not_editable(store, members, current.as_ref().is_some_and(|s| s.adopted)) {
            return Err(DomainError::Conflict(reason.into()));
        }

        if !valid_version(&update.version) {
            return Err(invalid("release: `latest` or an exact release such as 1.0.6"));
        }

        if !valid_region(&update.region) || update.region.is_empty() {
            return Err(invalid("region: lowercase letters, digits and -, up to 32"));
        }

        if update.s3_port < 1024 || update.console_port.is_some_and(|c| c < 1024 || c == update.s3_port) {
            return Err(invalid("ports: 1024-65535, the console different from the S3 port"));
        }

        let taken = self.taken_ports(store, members).await?;
        if taken.contains(&update.s3_port) || update.console_port.is_some_and(|p| taken.contains(&p)) {
            return Err(DomainError::Conflict("another object store on a member node uses that port".into()));
        }

        let new_settings = match (store.runtime.as_str(), &update.docker) {
            ("docker", Some(d)) => {
                let base = current.clone().unwrap_or_default();
                Some(RustfsSettings {
                    image: d.image.clone(),
                    restart: d.restart,
                    memory_mb: d.memory_mb,
                    cpu_millis: d.cpu_millis,
                    env: d.env.clone(),
                    ..base
                })
            }
            ("docker", None) => current.clone(),
            (_, Some(_)) => return Err(invalid("container settings apply to Docker stores only")),
            (_, None) => None,
        };

        let mut next = store.clone();
        next.version.clone_from(&update.version);
        next.s3_port = update.s3_port;
        next.console_port = update.console_port;
        next.region.clone_from(&update.region);
        next.endpoint = endpoint_with_port(&store.endpoint, update.s3_port);
        next.updated_at = self.clock.now();

        for m in members {
            spec(&next, m, members, new_settings.as_ref()).map_err(DomainError::Invalid)?;
        }

        let previous = PreviousSettings {
            version: store.version.clone(),
            s3_port: store.s3_port,
            console_port: store.console_port,
            region: store.region.clone(),
            endpoint: store.endpoint.clone(),
            settings: current,
        };
        self.stores.put_object_store(&next).await?;

        if let Some(s) = &new_settings {
            self.settings.put_store_settings(store.id, s, next.updated_at).await?;
        }

        let action = if store.runtime == "docker" { "reconfigure" } else { "install" };

        Ok(Applied { store: next, previous, action })
    }
}

/// `http://host:old` with the new port.
#[must_use]
pub fn endpoint_with_port(endpoint: &str, port: u16) -> String {
    match url::Url::parse(endpoint) {
        Ok(mut u) => match u.set_port(Some(port)) {
            Ok(()) => u.as_str().trim_end_matches('/').to_owned(),
            Err(()) => endpoint.to_owned(),
        },
        Err(_) => endpoint.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_follows_the_port() {
        assert_eq!(endpoint_with_port("http://192.0.2.10:9000", 9100), "http://192.0.2.10:9100");
        assert_eq!(endpoint_with_port("not a url", 9100), "not a url");
    }
}
