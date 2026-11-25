//! Finds RustFS containers on nodes (D-170): every container whose image is
//! `rustfs/rustfs` (any registry, any tag) or that carries Forge's object
//! store label, whether Forge created it or not. Read-only: one container
//! `list`, then an `inspect` without environment values per match.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use forge_core::model::{AgentStatus, Server};
use forge_core::storage::ObjectStoreRepository;
use forge_core::storage_settings::StoreSettingsRepository;
use forge_core::{DomainResult, ObjectStoreId, ServerId};
use forge_proto::Command;
use forge_proto::messages::ContainerAction;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::jobs::storage::spec;
use crate::node_rpc::{NodeRpc, RpcError};

/// The label Forge puts on the containers it creates.
pub const FORGE_LABEL: &str = "org.orbyn.forge.objstore";
const QUICK: Duration = Duration::from_secs(20);

/// Docker on one node, as discovery saw it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DockerState {
    Available,
    NotInstalled,
    Unreachable,
    UnsupportedOs,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NodeDocker {
    pub server_id: ServerId,
    pub hostname: String,
    pub docker: DockerState,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MountType {
    Bind,
    Volume,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FoundMount {
    #[serde(rename = "type")]
    pub kind: MountType,
    pub source: String,
    pub destination: String,
    pub name: Option<String>,
}

/// How a container relates to Forge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FoundKind {
    /// A Forge store runs it.
    Managed,
    /// Forge's label, but no store owns it any more.
    Orphaned,
    /// Created outside Forge.
    Foreign,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FoundContainer {
    pub server_id: ServerId,
    pub hostname: String,
    pub id: String,
    pub name: String,
    pub image: String,
    pub version: String,
    pub state: String,
    pub status: String,
    pub ports: Vec<String>,
    pub mounts: Vec<FoundMount>,
    pub network: String,
    pub forge_label: Option<String>,
    pub store_id: Option<ObjectStoreId>,
    pub kind: FoundKind,
    /// Published host ports of the S3 API (container 9000) and console (9001).
    pub s3_port: Option<u16>,
    pub console_port: Option<u16>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct DiscoveryReport {
    pub nodes: Vec<NodeDocker>,
    pub containers: Vec<FoundContainer>,
}

/// One entry of the agent's container list.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ListedContainer {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub image: String,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub ports: Vec<String>,
    #[serde(default)]
    pub labels: Option<BTreeMap<String, String>>,
}

/// The agent's `inspect` answer (without environment values).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Inspected {
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub ports: Option<BTreeMap<String, Option<Vec<PortBinding>>>>,
    #[serde(default)]
    pub mounts: Option<Vec<InspectedMount>>,
    #[serde(default)]
    pub networks: Option<Vec<String>>,
    #[serde(default)]
    pub restart_policy: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct PortBinding {
    #[serde(rename = "HostPort", default)]
    pub host_port: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct InspectedMount {
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub destination: String,
}

/// Whether an image reference is RustFS: `rustfs/rustfs[:tag]`, also behind
/// a registry (`registry.example.test/rustfs/rustfs:1.0`).
#[must_use]
pub fn is_rustfs_image(image: &str) -> bool {
    let repo = image.split('@').next().unwrap_or_default();
    let repo = match repo.rsplit_once(':') {
        Some((r, tag)) if !tag.contains('/') => r,
        _ => repo,
    };

    repo == "rustfs/rustfs" || repo.ends_with("/rustfs/rustfs")
}

/// The tag of an image reference (`latest` when it has none).
#[must_use]
pub fn image_tag(image: &str) -> String {
    match image.rsplit_once(':') {
        Some((_, tag)) if !tag.contains('/') => tag.to_owned(),
        _ => "latest".into(),
    }
}

/// A mount as discovery shows it: Docker named volumes live under
/// `/var/lib/docker/volumes/<name>/_data`.
#[must_use]
pub fn mount_of(m: &InspectedMount) -> FoundMount {
    let name = m
        .source
        .strip_prefix("/var/lib/docker/volumes/")
        .and_then(|r| r.strip_suffix("/_data"))
        .filter(|n| !n.contains('/'))
        .map(str::to_owned);
    FoundMount {
        kind: if name.is_some() { MountType::Volume } else { MountType::Bind },
        source: m.source.clone(),
        destination: m.destination.clone(),
        name,
    }
}

/// The host port published for `container_port/tcp`.
#[must_use]
pub fn published(i: &Inspected, container_port: u16) -> Option<u16> {
    i.ports.as_ref()?.get(&format!("{container_port}/tcp"))?.as_ref()?.iter().find_map(|b| b.host_port.parse().ok())
}

/// Builds the report entry for one listed container and its inspect answer.
#[must_use]
pub fn found(server: &Server, c: &ListedContainer, i: &Inspected, owner: Option<ObjectStoreId>) -> FoundContainer {
    let label = c.labels.as_ref().and_then(|l| l.get(FORGE_LABEL)).cloned();
    let kind = match (owner, &label) {
        (Some(_), _) => FoundKind::Managed,
        (None, Some(_)) => FoundKind::Orphaned,
        (None, None) => FoundKind::Foreign,
    };

    let image = i.image.clone().unwrap_or_else(|| c.image.clone());
    let host = i.networks.as_ref().is_some_and(|n| n.iter().any(|x| x == "host"));

    FoundContainer {
        server_id: server.id,
        hostname: server.display_name.clone().unwrap_or_else(|| server.hostname.clone()),
        id: c.id.clone(),
        name: c.name.clone(),
        version: image_tag(&image),
        image,
        state: c.state.clone(),
        status: c.status.clone(),
        ports: c.ports.clone(),
        mounts: i.mounts.iter().flatten().map(mount_of).collect(),
        network: if host {
            "host".into()
        } else {
            i.networks.as_ref().and_then(|n| n.first().cloned()).unwrap_or_else(|| "bridge".into())
        },
        forge_label: label,
        store_id: owner,
        kind,
        s3_port: if host { Some(9000) } else { published(i, 9000) },
        console_port: if host { None } else { published(i, 9001) },
    }
}

/// Discovery over the agent link.
#[derive(Debug, Clone)]
pub struct Discovery {
    pub rpc: NodeRpc,
    pub stores: Arc<dyn ObjectStoreRepository>,
    pub settings: Arc<dyn StoreSettingsRepository>,
}

fn container(op: &str, id: &str) -> Command {
    Command::ContainerAction(ContainerAction {
        op: op.into(),
        id: id.into(),
        image: String::new(),
        tail: 0,
        since: String::new(),
        reveal_env: false,
    })
}

impl Discovery {
    /// Container names of the stores on `server`, by store.
    async fn units(&self, server: ServerId) -> DomainResult<BTreeMap<String, ObjectStoreId>> {
        let mut out = BTreeMap::new();

        for m in self.stores.list_node_store_members(server).await? {
            let Some(store) = self.stores.get_object_store(m.store_id).await? else { continue };
            if store.runtime != "docker" {
                continue;
            }

            let settings = self.settings.get_store_settings(store.id).await?;
            let all = self.stores.list_store_members(store.id).await?;

            // A store whose spec no longer validates still owns its container name.
            let unit = spec(&store, &m, &all, settings.as_ref())
                .map_or_else(|_| format!("forge-objstore-{}", store.slug), |s| s.unit());
            out.insert(unit, store.id);
        }

        Ok(out)
    }

    /// Scans one node. Errors become the node's Docker state; never fails
    /// the whole scan.
    pub async fn scan_node(&self, server: &Server, principal: &str) -> (NodeDocker, Vec<FoundContainer>) {
        let node = |docker, error: Option<String>| NodeDocker {
            server_id: server.id,
            hostname: server.display_name.clone().unwrap_or_else(|| server.hostname.clone()),
            docker,
            error,
        };

        if server.os != "linux" {
            return (node(DockerState::UnsupportedOs, None), Vec::new());
        }

        if server.status != AgentStatus::Online {
            return (node(DockerState::Unreachable, Some("the node is not connected".into())), Vec::new());
        }

        let listed = match self.rpc.call(server.id, principal, container("list", ""), QUICK).await {
            Ok(v) => v,
            Err(RpcError::Failed(e)) if e.contains(forge_catalog::apps::DOCKER_UNAVAILABLE) => {
                return (node(DockerState::NotInstalled, None), Vec::new());
            }
            Err(e) => return (node(DockerState::Unreachable, Some(e.to_string())), Vec::new()),
        };

        // An answer that is not a list reads as an empty node; the agent's own
        // format, so it never happens short of a bug.
        let listed: Vec<ListedContainer> = serde_json::from_value(listed).unwrap_or_default();

        let units = match self.units(server.id).await {
            Ok(u) => u,
            Err(e) => return (node(DockerState::Unreachable, Some(e.to_string())), Vec::new()),
        };

        let mut out = Vec::new();

        for c in listed {
            let labelled = c.labels.as_ref().is_some_and(|l| l.contains_key(FORGE_LABEL));
            if !labelled && !is_rustfs_image(&c.image) {
                continue;
            }

            // Mounts and ports are best effort: a failed inspect still lists it.
            let inspected: Inspected = self
                .rpc
                .call(server.id, principal, container("inspect", &c.id), QUICK)
                .await
                .ok()
                .and_then(|v: Value| serde_json::from_value(v).ok())
                .unwrap_or_default();
            out.push(found(server, &c, &inspected, units.get(&c.name).copied()));
        }

        (node(DockerState::Available, None), out)
    }

    /// Scans nodes concurrently.
    pub async fn scan(&self, servers: &[Server], principal: &str) -> DiscoveryReport {
        let results = futures::future::join_all(servers.iter().map(|s| self.scan_node(s, principal))).await;
        let mut report = DiscoveryReport::default();

        for (n, c) in results {
            report.nodes.push(n);
            report.containers.extend(c);
        }

        report
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // reason: test fixtures
mod tests {
    use super::*;

    #[test]
    fn recognises_rustfs_images() {
        for i in ["rustfs/rustfs", "rustfs/rustfs:1.0.6", "registry.example.test:5000/rustfs/rustfs:latest"] {
            assert!(is_rustfs_image(i), "{i}");
        }

        for i in ["minio/minio", "rustfs/rustfs-console", "myrustfs/rustfs2", "nginx:rustfs"] {
            assert!(!is_rustfs_image(i), "{i}");
        }

        assert_eq!(image_tag("rustfs/rustfs:1.0.6"), "1.0.6");
        assert_eq!(image_tag("registry.example.test:5000/rustfs/rustfs"), "latest");
    }

    #[test]
    fn parses_list_and_inspect_answers() {
        let list: Vec<ListedContainer> = serde_json::from_value(serde_json::json!([{
            "id": "35dfaf1f3aba", "name": "smoke", "image": "rustfs/rustfs:latest", "state": "running",
            "status": "Up", "ports": ["0.0.0.0:19000->9000/tcp"], "labels": {}
        }]))
        .unwrap();
        let inspect: Inspected = serde_json::from_value(serde_json::json!({
            "image": "rustfs/rustfs:1.0.6",
            "ports": { "9000/tcp": [{ "HostIp": "0.0.0.0", "HostPort": "19000" }], "9001/tcp": null },
            "mounts": [
                { "source": "/var/lib/docker/volumes/smoke-data/_data", "destination": "/data", "mode": "z" },
                { "source": "/srv/logs", "destination": "/logs", "mode": "" }
            ],
            "networks": ["bridge"], "env_keys": ["RUSTFS_ACCESS_KEY"]
        }))
        .unwrap();
        let server = Server {
            id: ServerId::new(),
            workspace_id: forge_core::WorkspaceId::new(),
            environment_id: None,
            cluster_id: None,
            role: forge_core::model::NodeRole::default(),
            display_name: None,
            hostname: "lab-c".into(),
            os: "linux".into(),
            arch: "x86_64".into(),
            agent_version: String::new(),
            status: AgentStatus::Online,
            last_seen_at: None,
            cert_serial: None,
            cert_not_after: None,
            created_at: forge_core::Timestamp(0),
        };
        let f = found(&server, &list[0], &inspect, None);
        assert_eq!(f.kind, FoundKind::Foreign);
        assert_eq!(f.version, "1.0.6");
        assert_eq!((f.s3_port, f.console_port), (Some(19000), None));
        assert_eq!(f.mounts[0].kind, MountType::Volume);
        assert_eq!(f.mounts[0].name.as_deref(), Some("smoke-data"));
        assert_eq!(f.mounts[1].kind, MountType::Bind);
        let mut labelled = list[0].clone();
        labelled.labels = Some(BTreeMap::from([(FORGE_LABEL.into(), "media".into())]));
        assert_eq!(found(&server, &labelled, &inspect, None).kind, FoundKind::Orphaned);
        assert_eq!(found(&server, &labelled, &inspect, Some(ObjectStoreId::new())).kind, FoundKind::Managed);
    }
}
