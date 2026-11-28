//! Builds the maps the analytics worker attributes data with (D-141) and
//! pushes them when they change, and after every worker (re)connect.
//!
//! Four batch reads (nodes, services, placements, ingest key hashes) every
//! [`CHECK_EVERY`]; the result is hashed and only sent when the hash or the
//! worker changed.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use forge_core::analytics::{AnalyticsMaps, AppKey, KeyService, MAX_MAP_ENTRIES, NodeCert, Placement};
use forge_core::model::{AgentStatus, Server, Service};
use forge_core::ports::{LogKeyRepository, ServerRepository, ServiceRepository};
use forge_core::{DomainResult, ServerId};
use tokio::sync::{Notify, watch};

use super::client::{CallError, WorkerClient};

/// How often the maps are rebuilt and compared.
pub const CHECK_EVERY: Duration = Duration::from_secs(15);

/// The `forge.app` label value of a service: `<environment>/<name>`.
fn app_key(service: &Service) -> Option<String> {
    let env = service.spec.get("environment").and_then(serde_json::Value::as_str)?;
    Some(format!("{env}/{}", service.name))
}

/// The lower-case names an event may use for a node: hostname, its first
/// label, and the console name.
fn node_names(s: &Server) -> Vec<String> {
    let host = s.hostname.to_lowercase();
    let mut names = vec![host.clone()];

    if let Some(short) = host.split('.').next().filter(|x| *x != host) {
        names.push(short.to_owned());
    }

    if let Some(d) = s.display_name.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
        names.push(d.to_lowercase());
    }

    names
}

/// Wiring for [`MapsPublisher::new`].
#[derive(Debug)]
pub struct MapsDeps {
    pub servers: Arc<dyn ServerRepository>,
    pub services: Arc<dyn ServiceRepository>,
    pub keys: Arc<dyn LogKeyRepository>,
}

/// Builds and pushes the maps.
#[derive(Debug)]
pub struct MapsPublisher {
    d: MapsDeps,
    /// The worker generation and maps hash last pushed.
    last: Mutex<Option<(u64, String)>>,
    /// Wakes the loop early (a worker connected).
    wake: Notify,
    /// Per node: the ingest keys of the services placed there, sent to the
    /// node's log listener in heartbeat responses (D-142).
    node_keys: std::sync::RwLock<Arc<NodeKeys>>,
}

/// Per node: the hashes of the ingest keys its listener accepts.
pub type NodeKeys = HashMap<ServerId, Arc<Vec<forge_proto::messages::IngestKey>>>;

/// The keys each node accepts: a key works only where its service is placed.
fn node_keys(maps: &AnalyticsMaps) -> NodeKeys {
    let mut by_service: HashMap<forge_core::ServiceId, Vec<&KeyService>> = HashMap::new();

    for k in &maps.key_services {
        by_service.entry(k.service_id).or_default().push(k);
    }

    let mut out: HashMap<ServerId, Vec<forge_proto::messages::IngestKey>> = HashMap::new();

    for p in &maps.placements {
        let Some(keys) = by_service.get(&p.service_id) else { continue };
        let list = out.entry(p.server_id).or_default();

        for k in keys {
            if list.len() >= forge_proto::messages::MAX_INGEST_KEYS {
                break;
            }

            // Stored hashes are hex SHA-256; anything else is skipped.
            if let Ok(hash) = hex::decode(&k.token_hash)
                && hash.len() == 32
            {
                list.push(forge_proto::messages::IngestKey { hash, service_id: k.service_id.to_string() });
            }
        }
    }

    out.into_iter()
        .map(|(n, mut v)| {
            v.sort_by(|a, b| a.hash.cmp(&b.hash));
            v.dedup();
            (n, Arc::new(v))
        })
        .collect()
}

impl MapsPublisher {
    #[must_use]
    pub fn new(d: MapsDeps) -> Self {
        Self { d, last: Mutex::new(None), wake: Notify::new(), node_keys: std::sync::RwLock::default() }
    }

    /// Pushes on the next turn of the loop, whether or not anything changed.
    pub fn nudge(&self) {
        self.wake.notify_one();
    }

    /// The maps from the operational database.
    pub async fn build(&self) -> DomainResult<AnalyticsMaps> {
        let servers = self.d.servers.list_servers(None).await?;
        let services = self.d.services.list_services(None).await?;
        let instances = self.d.services.list_all_instances(None).await?;
        let limit = u32::try_from(MAX_MAP_ENTRIES).unwrap_or(u32::MAX);
        let key_services: Vec<KeyService> = self.d.keys.list_log_key_hashes(limit).await?;
        let by_id: HashMap<ServerId, &Server> = servers.iter().map(|s| (s.id, s)).collect();
        let apps: HashMap<_, Option<String>> = services.iter().map(|s| (s.id, app_key(s))).collect();
        let active = || servers.iter().filter(|s| s.status != AgentStatus::Revoked);
        let mut maps = AnalyticsMaps {
            active_nodes: active().map(|s| s.id).collect(),
            key_services,
            app_keys: Vec::new(),
            placements: Vec::new(),
            // Only the current certificate uploads (D-145): renewal and
            // revocation replace or drop it here.
            node_certs: active()
                .filter_map(|s| {
                    let serial = forge_pki::canonical_serial(s.cert_serial.as_deref()?);
                    Some(NodeCert { server_id: s.id, serial })
                })
                .collect(),
        };

        for i in &instances {
            let Some(server) = by_id.get(&i.server_id) else { continue };

            if let Some(Some(app)) = apps.get(&i.service_id) {
                maps.app_keys.push(AppKey { server_id: i.server_id, app: app.clone(), service_id: i.service_id });
            }

            maps.placements.push(Placement {
                service_id: i.service_id,
                server_id: i.server_id,
                names: node_names(server),
            });
        }

        // Bounded: the worker refuses larger pushes, so cut here and say so.
        if !maps.within_limits() {
            tracing::warn!(limit = MAX_MAP_ENTRIES, "analytics maps are cut to the limit");
            maps.active_nodes.truncate(MAX_MAP_ENTRIES);
            maps.app_keys.truncate(MAX_MAP_ENTRIES);
            maps.placements.truncate(MAX_MAP_ENTRIES);
            maps.node_certs.truncate(MAX_MAP_ENTRIES);
        }

        Ok(maps)
    }

    /// The ingest keys `node`'s listener accepts.
    #[must_use]
    pub fn keys_for(&self, node: ServerId) -> Arc<Vec<forge_proto::messages::IngestKey>> {
        self.node_keys.read().unwrap_or_else(PoisonError::into_inner).get(&node).cloned().unwrap_or_default()
    }

    /// Rebuilds the maps and the per-node keys, and pushes the maps when
    /// they or the worker changed since the last push.
    pub async fn sync(&self, client: &WorkerClient) -> Result<bool, SyncError> {
        let maps = self.build().await?;
        *self.node_keys.write().unwrap_or_else(PoisonError::into_inner) = Arc::new(node_keys(&maps));

        if !client.connected() {
            return Ok(false);
        }

        let generation = client.generation();
        let json = serde_json::to_vec(&maps).map_err(|e| SyncError::Encode(e.to_string()))?;
        let hash = forge_secrets::crypto::sha256_hex(&json);
        let unchanged = self
            .last
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_some_and(|(g, h)| *g == generation && *h == hash);

        if unchanged {
            return Ok(false);
        }

        client.push_maps(maps).await?;
        *self.last.lock().unwrap_or_else(PoisonError::into_inner) = Some((generation, hash));

        Ok(true)
    }
}

/// Why the maps could not be pushed.
#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error(transparent)]
    Store(#[from] forge_core::DomainError),
    #[error("encoding the maps: {0}")]
    Encode(String),
    #[error(transparent)]
    Call(#[from] CallError),
}

/// Background loop: keeps the worker's maps current until shutdown.
pub async fn run(publisher: Arc<MapsPublisher>, client: Arc<WorkerClient>, mut shutdown: watch::Receiver<bool>) {
    if !client.enabled() {
        return;
    }

    loop {
        if let Err(e) = publisher.sync(&client).await {
            tracing::warn!(error = %e, "pushing the analytics maps failed; retrying");
        }

        tokio::select! {
            () = tokio::time::sleep(CHECK_EVERY) => {}
            () = publisher.wake.notified() => {}
            _ = shutdown.changed() => return,
        }
    }
}
