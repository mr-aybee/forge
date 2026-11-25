//! Adopting any RustFS container a node runs (D-170): the store record is
//! built from what discovery read (image, ports, mounts, network); the
//! `adopt` job then reads the root credential on the node and seals it.

use std::sync::Arc;

use forge_core::storage::{MODE_MANAGED, ObjectStore, ObjectStoreRepository, StoreMember, TOPOLOGY_STANDALONE};
use forge_core::storage_settings::{DriveVolume, NetworkMode, RustfsSettings, StoreSettingsRepository};
use forge_core::{
    Clock, DomainError, DomainResult, EnvironmentId, ObjectStoreId, Principal, ServerId, VaultItemId, WorkspaceId,
};

use crate::jobs::storage::{busy_status, spec};
use crate::objstore::discovery::{FoundContainer, MountType};

/// What the operator chose for the adopted store.
#[derive(Debug, Clone)]
pub struct AdoptPlan {
    pub workspace: WorkspaceId,
    pub server: ServerId,
    pub name: String,
    pub owner: Option<EnvironmentId>,
    pub endpoint_host: String,
    /// Overrides for ports discovery cannot read (host network).
    pub s3_port: Option<u16>,
    pub console_port: Option<u16>,
    pub created_by: Principal,
}

#[derive(Debug, Clone)]
pub struct AdoptService {
    pub stores: Arc<dyn ObjectStoreRepository>,
    pub settings: Arc<dyn StoreSettingsRepository>,
    pub clock: Arc<dyn Clock>,
}

/// A slug from a container name: `[a-z][a-z0-9-]{0,30}`.
#[must_use]
pub fn slug_for(container: &str) -> String {
    let base = container.strip_prefix("forge-objstore-").unwrap_or(container);
    let mut s: String = base
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .skip_while(|c| !c.is_ascii_lowercase())
        .take(31)
        .collect();

    while s.ends_with('-') {
        s.pop();
    }

    if s.is_empty() { "rustfs".into() } else { s }
}

/// The settings and data paths of a found container: every mount except the
/// engine's log and temp directories is a drive.
pub fn adopted_layout(found: &FoundContainer) -> Result<(RustfsSettings, Vec<String>), String> {
    let drives: Vec<_> = found.mounts.iter().filter(|m| !matches!(m.destination.as_str(), "/logs" | "/tmp")).collect();
    if drives.is_empty() {
        return Err("the container has no data mount; Forge adopts containers whose data is on a volume".into());
    }

    let volumes = drives
        .iter()
        .map(|m| match (m.kind, &m.name) {
            (MountType::Volume, Some(name)) => {
                DriveVolume::Named { name: name.clone(), driver: String::new(), options: Default::default() }
            }
            _ => DriveVolume::Bind { host_path: m.source.clone() },
        })
        .collect();
    let repo = found.image.rsplit_once(':').filter(|(_, t)| !t.contains('/')).map_or(found.image.as_str(), |(r, _)| r);
    let settings = RustfsSettings {
        image: repo.to_owned(),
        container_name: found.name.clone(),
        network: if found.network == "host" { NetworkMode::Host } else { NetworkMode::Bridge },
        volumes,
        adopted: true,
        ..RustfsSettings::default()
    };

    Ok((settings, drives.iter().map(|m| m.destination.clone()).collect()))
}

impl AdoptService {
    /// Checks the container against the node's other stores and records the
    /// store, its member and settings. Multi-step write without a
    /// transaction: a failure part-way leaves a store in `adopting` without a
    /// job (nothing changed on the node), which the operator removes.
    pub async fn record(&self, found: &FoundContainer, plan: &AdoptPlan) -> DomainResult<(ObjectStore, StoreMember)> {
        if found.store_id.is_some() {
            return Err(DomainError::Conflict("that container already belongs to an object store".into()));
        }

        let (settings, data_paths) = adopted_layout(found).map_err(DomainError::Invalid)?;
        let s3_port = plan.s3_port.or(found.s3_port).unwrap_or(9000);
        let console_port = plan.console_port.or(found.console_port);
        let slug = slug_for(&found.name);

        for m in self.stores.list_node_store_members(plan.server).await? {
            if let Some(o) = self.stores.get_object_store(m.store_id).await?
                && (o.slug == slug || o.s3_port == s3_port || o.console_port.is_some_and(|p| Some(p) == console_port))
            {
                return Err(DomainError::Conflict(format!(
                    "another object store on this node uses the name `{slug}` or one of its ports"
                )));
            }
        }

        let now = self.clock.now();
        let id = ObjectStoreId::new();
        let version =
            if forge_catalog::storage::valid_version(&found.version) { found.version.clone() } else { "latest".into() };
        let store = ObjectStore {
            id,
            workspace_id: plan.workspace,
            environment_id: plan.owner,
            name: plan.name.clone(),
            slug,
            provider: "rustfs".into(),
            mode: MODE_MANAGED.into(),
            region: "us-east-1".into(),
            path_style: true,
            version,
            deployed_version: None,
            runtime: "docker".into(),
            topology: TOPOLOGY_STANDALONE.into(),
            s3_port,
            console_port,
            endpoint: format!("http://{}:{s3_port}", plan.endpoint_host),
            // Filled by the adopt job with the container's own credential.
            credential_id: VaultItemId::new(),
            status: busy_status("adopt").into(),
            last_error: None,
            last_job_id: None,
            created_by: plan.created_by.clone(),
            created_at: now,
            updated_at: now,
        };
        let member = StoreMember {
            store_id: id,
            server_id: plan.server,
            data_paths,
            endpoint_host: plan.endpoint_host.clone(),
            require_mount: false,
            status: busy_status("adopt").into(),
            capacity_bytes: None,
            free_bytes: None,
            dedicated: None,
            observed_at: None,
        };
        spec(&store, &member, std::slice::from_ref(&member), Some(&settings)).map_err(DomainError::Invalid)?;
        self.stores.put_object_store(&store).await?;
        self.stores.put_store_member(&member).await?;
        self.settings.put_store_settings(id, &settings, now).await?;

        Ok((store, member))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objstore::discovery::{FoundKind, FoundMount};

    #[test]
    fn slugs_come_from_container_names() {
        assert_eq!(slug_for("forge-objstore-media"), "media");
        assert_eq!(slug_for("My_RustFS"), "my-rustfs");
        assert_eq!(slug_for("123"), "rustfs");
        assert_eq!(slug_for(&"a".repeat(40)).len(), 31);
    }

    #[test]
    fn layout_keeps_volumes_and_skips_the_log_directory() {
        let mount = |kind, source: &str, destination: &str, name: Option<&str>| FoundMount {
            kind,
            source: source.into(),
            destination: destination.into(),
            name: name.map(str::to_owned),
        };
        let found = FoundContainer {
            server_id: ServerId::new(),
            hostname: "lab-c".into(),
            id: "abc".into(),
            name: "smoke".into(),
            image: "rustfs/rustfs:1.0.6".into(),
            version: "1.0.6".into(),
            state: "running".into(),
            status: "Up".into(),
            ports: vec![],
            mounts: vec![
                mount(MountType::Volume, "/var/lib/docker/volumes/d/_data", "/data", Some("d")),
                mount(MountType::Bind, "/srv/logs", "/logs", None),
            ],
            network: "bridge".into(),
            forge_label: None,
            store_id: None,
            kind: FoundKind::Foreign,
            s3_port: Some(19000),
            console_port: None,
        };
        let (settings, paths) = adopted_layout(&found).unwrap_or_default();
        assert_eq!(paths, ["/data"]);
        assert_eq!(settings.image, "rustfs/rustfs");
        assert_eq!(settings.container_name, "smoke");
        assert!(settings.adopted);
        assert!(matches!(&settings.volumes[0], DriveVolume::Named { name, .. } if name == "d"));
        let no_data = FoundContainer { mounts: vec![], ..found };
        assert!(adopted_layout(&no_data).is_err());
    }
}
