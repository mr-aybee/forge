//! Service logs (D-128): ingest keys and Seq-compatible ingest. Handlers in
//! `api/logs.rs` authorize and audit; this type holds the rules.
//!
//! Ingest checks the key (only its hash is stored) and hands the body, as
//! received, to the analytics worker (D-141), which parses, attributes and
//! stores it. A request is answered after its events are stored; when the
//! worker is down or busy the request fails with 503 and the sender (for
//! example Serilog's durable sink) retries. forge-server keeps nothing.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use forge_core::analytics::{AnalyticsError, TelemetrySink};
use forge_core::logs::LogKey;
use forge_core::model::Service;
use forge_core::ports::{LogKeyRepository, ServerRepository, ServiceRepository};
use forge_core::{Clock, DomainError, LogKeyId, ServerId, ServiceId, UserId};
use serde::Serialize;

/// Tokens start with this, so a leaked one is easy to recognise.
pub const TOKEN_PREFIX: &str = "flk_";
/// `last_used_at` is written at most this often per key.
const TOUCH_EVERY_MS: i64 = 60_000;
/// Keys whose last use is remembered; the map is cleared when full.
const MAX_TOUCHED: usize = 100_000;
/// Longest key name.
pub const MAX_KEY_NAME: usize = 80;

pub const AUDIT_KEY_CREATE: &str = "service.log_key.create";
pub const AUDIT_KEY_REVOKE: &str = "service.log_key.revoke";

/// Why logs cannot be written or read.
#[derive(Debug, thiserror::Error)]
pub enum LogsError {
    /// No such key, or the key was revoked.
    #[error("the log ingest key is missing or revoked")]
    BadKey,
    #[error(transparent)]
    Analytics(#[from] AnalyticsError),
    #[error("{0}")]
    Invalid(&'static str),
    #[error(transparent)]
    Domain(#[from] DomainError),
}

/// A new key and its token, which is shown once.
#[derive(Debug, Serialize)]
pub struct CreatedKey {
    pub key: LogKey,
    pub token: String,
}

/// The log service.
pub struct Logs {
    keys: Arc<dyn LogKeyRepository>,
    apps: Arc<dyn ServiceRepository>,
    servers: Arc<dyn ServerRepository>,
    sink: Arc<dyn TelemetrySink>,
    clock: Arc<dyn Clock>,
    /// When each key's `last_used_at` was last written.
    touched: Mutex<HashMap<LogKeyId, i64>>,
}

/// Wiring for [`Logs::new`].
#[derive(Debug)]
pub struct LogsDeps {
    pub keys: Arc<dyn LogKeyRepository>,
    pub apps: Arc<dyn ServiceRepository>,
    pub servers: Arc<dyn ServerRepository>,
    pub sink: Arc<dyn TelemetrySink>,
    pub clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for Logs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Logs").finish_non_exhaustive()
    }
}

fn hash(token: &str) -> String {
    forge_secrets::crypto::sha256_hex(token.as_bytes())
}

impl Logs {
    #[must_use]
    pub fn new(d: LogsDeps) -> Self {
        Self {
            keys: d.keys,
            apps: d.apps,
            servers: d.servers,
            sink: d.sink,
            clock: d.clock,
            touched: Mutex::new(HashMap::new()),
        }
    }

    /// Console names of `ids` (display name, else hostname). A node that no
    /// longer exists is left out: its events show no node.
    pub async fn node_names(&self, ids: &[ServerId]) -> HashMap<ServerId, String> {
        let mut out = HashMap::new();
        for id in ids {
            match self.servers.get_server(*id).await {
                Ok(s) => {
                    out.insert(*id, s.display_name.filter(|d| !d.trim().is_empty()).unwrap_or(s.hostname));
                }
                // Removed nodes are expected; anything else is worth a log line.
                Err(DomainError::NotFound(_)) => {}
                Err(e) => tracing::warn!(server = %id, error = %e, "could not name a log event's node"),
            }
        }
        out
    }

    /// Every node's id; the caller filters by permission.
    pub async fn node_ids(&self) -> Result<Vec<ServerId>, LogsError> {
        Ok(self.servers.list_servers(None).await?.into_iter().map(|s| s.id).collect())
    }

    /// Every service, or the one given; the caller filters by permission.
    pub async fn services(&self, one: Option<ServiceId>) -> Result<Vec<Service>, LogsError> {
        Ok(match one {
            Some(id) => self.apps.get_service(id).await?.into_iter().collect(),
            None => self.apps.list_services(None).await?,
        })
    }

    /// Creates a key for `service`. The token is returned once.
    pub async fn create_key(
        &self,
        service: ServiceId,
        name: &str,
        by: Option<UserId>,
    ) -> Result<CreatedKey, LogsError> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > MAX_KEY_NAME || name.chars().any(char::is_control) {
            return Err(LogsError::Invalid("a key name is 1 to 80 characters, without control characters"));
        }
        let token = format!("{TOKEN_PREFIX}{}", forge_secrets::crypto::random_token::<24>());
        let key = LogKey {
            id: LogKeyId::new(),
            service_id: service,
            name: name.to_string(),
            prefix: token.chars().take(10).collect(),
            created_at: self.clock.now(),
            created_by: by,
            last_used_at: None,
        };
        self.keys.create_log_key(&key, &hash(&token)).await?;
        Ok(CreatedKey { key, token })
    }

    pub async fn list_keys(&self, service: ServiceId) -> Result<Vec<LogKey>, LogsError> {
        Ok(self.keys.list_log_keys(service).await?)
    }

    pub async fn get_key(&self, id: LogKeyId) -> Result<Option<LogKey>, LogsError> {
        Ok(self.keys.get_log_key(id).await?)
    }

    pub async fn revoke_key(&self, id: LogKeyId) -> Result<(), LogsError> {
        self.keys.delete_log_key(id).await?;
        self.touched.lock().unwrap_or_else(PoisonError::into_inner).remove(&id);
        Ok(())
    }

    /// The key a token belongs to.
    async fn key_for(&self, token: &str) -> Result<LogKey, LogsError> {
        if !token.starts_with(TOKEN_PREFIX) || token.len() > 200 {
            return Err(LogsError::BadKey);
        }
        // Looked up by SHA-256, like API keys: only the hash is stored.
        self.keys.find_log_key(&hash(token)).await?.ok_or(LogsError::BadKey)
    }

    fn should_touch(&self, id: LogKeyId, now: i64) -> bool {
        let mut t = self.touched.lock().unwrap_or_else(PoisonError::into_inner);
        let due = t.get(&id).is_none_or(|last| now - last >= TOUCH_EVERY_MS);
        if due {
            if t.len() >= MAX_TOUCHED {
                t.clear();
            }
            t.insert(id, now);
        }
        due
    }

    /// Stores a batch sent with `token`; returns how many events it held.
    pub async fn ingest(&self, token: &str, body: &[u8]) -> Result<usize, LogsError> {
        let key = self.key_for(token).await?;
        let now = self.clock.now();
        let count = self.sink.ingest_logs(key.service_id, now, body).await?;
        if self.should_touch(key.id, now.0)
            && let Err(e) = self.keys.touch_log_key(key.id, now).await
        {
            // Only the "last used" column is stale; the events are stored.
            tracing::warn!(key = %key.id, error = %e, "could not record log key use");
        }
        Ok(count)
    }
}
