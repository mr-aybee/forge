//! The control-plane key agents pin (D-167).
//!
//! The agent listener and the analytics worker's upload listener present
//! certificates from the Forge CA for one long-lived key. Agents pin the
//! SHA-256 of that key's SubjectPublicKeyInfo instead of checking a host
//! name, so they reach the control plane by any address. Certificates are
//! renewed with the same key (hot-swapped into the listener, no restart),
//! so renewal never touches agents.
//!
//! **Rotation.** The server keeps the current key and a *next* key made
//! ahead of time; both fingerprints go to agents in every heartbeat reply,
//! signed with the command key ([`AgentKeys::announcement`]). The listeners
//! switch to the next key once it has been announced for `grace_days` and
//! every online agent that pins has acknowledged it, `rotate_days` after
//! the last switch or when an operator asked (`forge-server agent-key
//! rotate`); `--now` switches at once. The old key is kept sealed for one
//! more rotation, so a switch can be undone by hand.
//!
//! **Analytics worker.** The worker's upload listener has its own key
//! pair (current and next, rotated with the agent key), so the worker
//! process never holds the agent hub's key. Its fingerprints are announced
//! as extra pins labelled `analytics` and `analytics-next`; agents accept
//! any pinned key on both connections.
//!
//! **Replay and revocation.** Every change of the key set bumps a sequence
//! sent in the announcement; nodes refuse an older one. The sequence is
//! also at least the time of the change, so a database restored from a
//! backup moves forward again at its next change. `agent-key rotate --now
//! --revoke` drops every old key that does not survive the switch and
//! lists them as revoked, so nodes unpin them and never pin them again
//! from an announcement. It protects against one leaked key; the next key
//! is stored alongside the current one, so after a compromise of the key
//! store `--fresh` makes keys that were never announced, to pin by hand.
//!
//! **At rest.** The state lives in the settings table (`agent_tls.keys`):
//! fingerprints, times and the private keys sealed with the server's
//! envelope key, bound to their fingerprint. Only fingerprints are ever
//! logged, audited or shown. The CLI and a running server share it: the
//! server re-reads it on every tick ([`AgentKeys::tick`]).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::Duration;

use forge_core::ports::SettingsRepository;
use forge_core::{DomainError, Timestamp};
use forge_pki::{CertificateAuthority, ControlPlaneKey, PkiError};
use forge_proto::CommandSigner;
use forge_proto::messages::{ServerKey, ServerKeys, SignedServerKeys};
use forge_secrets::{Envelope, SecretError};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use rustls_pki_types::pem::PemObject;
use rustls_pki_types::{CertificateDer, PrivateKeyDer};
use serde::{Deserialize, Serialize};

use crate::config::AgentTlsSettings;

/// Prefix of this module's settings rows: internal, never listed or
/// edited through the generic settings API.
pub const SETTING_PREFIX: &str = "agent_tls.";
/// Settings key of the key state.
pub const SETTING_KEYS: &str = "agent_tls.keys";
/// Settings key of the agents' acknowledgements.
pub const SETTING_ACKS: &str = "agent_tls.acks";
/// Audit: the listeners switched to the next key.
pub const AUDIT_KEY_ROTATE: &str = "agent_key.rotate";
/// Audit: an operator asked for a rotation once agents have the next key.
pub const AUDIT_KEY_ROTATE_REQUEST: &str = "agent_key.rotate_request";
/// Audit: the first key pair was created (new install or upgrade).
pub const AUDIT_KEY_CREATE: &str = "agent_key.create";
/// Audit: an operator switched keys and revoked the old ones.
pub const AUDIT_KEY_REVOKE: &str = "agent_key.revoke";
/// Label of the analytics worker listener's key.
pub const LABEL_ANALYTICS: &str = "analytics";
/// Label of the analytics worker listener's next key.
pub const LABEL_ANALYTICS_NEXT: &str = "analytics-next";
/// An agent's acknowledgement that only drops keys is stored at most this
/// often; one that adds a key of this server is stored at once.
pub const ACK_MIN_INTERVAL_MS: i64 = 60_000;
/// Label of the key the listeners serve.
pub const LABEL_CURRENT: &str = "current";
/// Label of the key announced for the next switch.
pub const LABEL_NEXT: &str = "next";

const DAY_MS: i64 = 24 * 3_600_000;

/// What can go wrong with the key state.
#[derive(Debug, thiserror::Error)]
pub enum AgentKeyError {
    #[error("settings: {0}")]
    Store(#[from] DomainError),
    #[error("sealing: {0}")]
    Seal(#[from] SecretError),
    #[error("certificate: {0}")]
    Pki(#[from] PkiError),
    #[error("TLS: {0}")]
    Tls(#[from] rustls::Error),
    #[error("stored key state is unreadable: {0}")]
    Corrupt(#[from] serde_json::Error),
    #[error("stored key {0} does not match its fingerprint")]
    Mismatch(String),
}

/// One key as stored: its fingerprint, the sealed private key and when it
/// was made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct StoredKey {
    sha256: String,
    sealed: String,
    created_at_ms: i64,
}

/// The stored key state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct KeyState {
    current: StoredKey,
    next: StoredKey,
    /// The key before the last switch, kept for one rotation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    previous: Option<StoredKey>,
    /// When `current` started serving.
    current_since_ms: i64,
    /// An operator asked for a switch (`agent-key rotate` without `--now`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rotation_requested_at_ms: Option<i64>,
    /// Bumped on every change of the announced key set.
    #[serde(default)]
    sequence: u64,
    /// Revoked fingerprints, newest last (at most `MAX_REVOKED_KEYS`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    revoked: Vec<String>,
    /// The analytics worker listener's key and its successor (absent before
    /// the upgrade that added them; created at the next start).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    worker: Option<StoredKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    worker_next: Option<StoredKey>,
}

impl KeyState {
    /// Every fingerprint this server holds and announces.
    fn known(&self) -> Vec<&str> {
        [Some(&self.current), Some(&self.next), self.worker.as_ref(), self.worker_next.as_ref()]
            .into_iter()
            .flatten()
            .map(|k| k.sha256.as_str())
            .collect()
    }
}

/// What an agent last reported having pinned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentAck {
    pub keys: Vec<String>,
    /// When this set was first reported.
    pub since_ms: i64,
}

/// A key as shown to operators (never the private half).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KeyInfo {
    pub sha256: String,
    pub label: &'static str,
    pub created_at: Timestamp,
}

/// A switch to the next key, for the audit trail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Rotation {
    pub from: String,
    pub to: String,
    pub next: String,
    /// The analytics worker listener's key after the switch.
    pub worker: String,
    /// Online agents that pin keys, and how many had the new key.
    pub online: usize,
    pub acknowledged: usize,
    pub forced: bool,
    /// All keys were made new (`--fresh`); agents must pin them by hand.
    pub fresh: bool,
    /// Fingerprints revoked by this switch (`--revoke`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub revoked: Vec<String>,
    /// The key-set sequence after the switch.
    pub sequence: u64,
}

/// How `forge-server agent-key rotate` switches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RotateMode {
    /// Once the grace period has passed and online agents have the key.
    WhenReady,
    /// At once (`--now`).
    Now,
    /// At once, revoking every old key that does not survive the switch
    /// (`--now --revoke`).
    NowRevoke,
    /// At once with brand-new keys that were never announced, revoking
    /// every old one (`--now --revoke --fresh`). Agents must be given the
    /// new fingerprints by hand: for a compromised key store.
    NowFresh,
}

/// What [`AgentKeys::tick`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TickOutcome {
    /// The listener certificate was renewed (same key).
    pub renewed: bool,
    /// This server switched keys.
    pub rotated: Option<Rotation>,
    /// Another process (the CLI) switched keys; this server now serves the
    /// new one. That process audited it.
    pub picked_up: bool,
}

/// Why a scheduled switch has not happened yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "waiting_for")]
pub enum RotationStatus {
    /// `rotate_days` is 0 and nobody asked.
    Manual,
    /// Not due before this time.
    Scheduled { at: Timestamp },
    /// Due, but the next key is announced for less than `grace_days`.
    Grace { until: Timestamp },
    /// Due, waiting for online agents to acknowledge the next key.
    Acknowledgements { online: usize, acknowledged: usize },
    /// Every condition holds; the next tick switches.
    Ready,
}

/// The listener certificate the TLS acceptor serves; swapped on renewal.
#[derive(Debug)]
pub struct KeyResolver(RwLock<Arc<CertifiedKey>>);

impl ResolvesServerCert for KeyResolver {
    fn resolve(&self, _hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        Some(self.0.read().unwrap_or_else(PoisonError::into_inner).clone())
    }
}

impl KeyResolver {
    fn set(&self, key: Arc<CertifiedKey>) {
        *self.0.write().unwrap_or_else(PoisonError::into_inner) = key;
    }

    /// The certificate served now (tests and diagnostics).
    #[must_use]
    pub fn current(&self) -> Arc<CertifiedKey> {
        self.0.read().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

/// The current key loaded, and the certificate served for it.
#[derive(Debug)]
struct Loaded {
    state: KeyState,
    key: Arc<ControlPlaneKey>,
    /// The analytics worker listener's key.
    worker_key: Arc<ControlPlaneKey>,
    not_before: Timestamp,
    not_after: Timestamp,
    /// Bumped on every switch, for the analytics worker's listener.
    generation: u64,
}

/// Wiring for [`AgentKeys::load_or_create`].
#[derive(Debug, Clone)]
pub struct AgentKeysDeps {
    pub settings: Arc<dyn SettingsRepository>,
    pub envelope: Envelope,
    pub ca: Arc<CertificateAuthority>,
    pub signer: Arc<CommandSigner>,
    /// Names in the listener certificates (`agent_hostnames`); optional for
    /// agents that pin, kept for tools and agents that check names.
    pub hostnames: Vec<String>,
    pub policy: AgentTlsSettings,
    /// An agent is online when it reported within this long.
    pub online_window: Duration,
}

/// The control-plane keys, the listener certificate and the rotation.
#[derive(Debug)]
pub struct AgentKeys {
    d: AgentKeysDeps,
    loaded: RwLock<Loaded>,
    resolver: Arc<KeyResolver>,
    /// Pins reported by agents, by server id, as last stored.
    acks: Mutex<BTreeMap<String, AgentAck>>,
    /// When each agent last reported (memory only: who is online).
    seen: Mutex<BTreeMap<String, i64>>,
    /// When each agent's acknowledgement was last stored (memory only).
    ack_written: Mutex<BTreeMap<String, i64>>,
}

fn aad(sha256: &str) -> String {
    format!("agent_tls.key:{sha256}")
}

fn seal(envelope: &Envelope, key: &ControlPlaneKey, now: Timestamp) -> Result<StoredKey, AgentKeyError> {
    let sealed = envelope.seal(key.key_pem().as_bytes(), &aad(key.fingerprint()))?;
    Ok(StoredKey { sha256: key.fingerprint().to_owned(), sealed, created_at_ms: now.0 })
}

fn open(envelope: &Envelope, stored: &StoredKey) -> Result<ControlPlaneKey, AgentKeyError> {
    let pem = envelope.open(&stored.sealed, &aad(&stored.sha256))?;
    let key = ControlPlaneKey::from_pem(&String::from_utf8_lossy(&pem))?;
    if key.fingerprint() != stored.sha256 {
        return Err(AgentKeyError::Mismatch(stored.sha256.clone()));
    }
    Ok(key)
}

/// The worker listener's key; a state without one (never after
/// [`AgentKeys::load_or_create`]) is refused rather than sharing the hub key.
fn open_worker(envelope: &Envelope, state: &KeyState) -> Result<ControlPlaneKey, AgentKeyError> {
    let w = state.worker.as_ref().ok_or_else(|| AgentKeyError::Mismatch("analytics worker key missing".into()))?;
    open(envelope, w)
}

fn certified(
    ca: &CertificateAuthority,
    key: &ControlPlaneKey,
    hostnames: &[String],
    now: Timestamp,
) -> Result<(Arc<CertifiedKey>, Timestamp, Timestamp), AgentKeyError> {
    let cert = ca.issue_server_certificate_for_key(key, hostnames, now)?;
    let mut chain = CertificateDer::pem_slice_iter(cert.cert_pem.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| PkiError::Parse(e.to_string()))?;
    chain.extend(
        CertificateDer::pem_slice_iter(ca.cert_pem().as_bytes())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| PkiError::Parse(e.to_string()))?,
    );
    let der = PrivateKeyDer::from_pem_slice(cert.key_pem.as_bytes()).map_err(|e| PkiError::Parse(e.to_string()))?;
    let signing = rustls::crypto::ring::default_provider().key_provider.load_private_key(der)?;
    Ok((Arc::new(CertifiedKey::new(chain, signing)), cert.not_before, cert.not_after))
}

impl AgentKeys {
    /// Loads the key state, creating the current and next keys on first
    /// start (`forge-server init`, or the first start after an upgrade).
    /// Returns whether keys were created, for the audit trail.
    pub async fn load_or_create(d: AgentKeysDeps, now: Timestamp) -> Result<(Self, bool), AgentKeyError> {
        let (mut state, created) = match read_state(d.settings.as_ref()).await? {
            Some(s) => (s, false),
            None => {
                let current = seal(&d.envelope, &ControlPlaneKey::generate()?, now)?;
                let next = seal(&d.envelope, &ControlPlaneKey::generate()?, now)?;
                let s = KeyState {
                    current,
                    next,
                    previous: None,
                    current_since_ms: now.0,
                    rotation_requested_at_ms: None,
                    sequence: 0,
                    revoked: Vec::new(),
                    worker: None,
                    worker_next: None,
                };
                (s, true)
            }
        };
        // New installs, and the first start after the upgrade that gave the
        // worker its own key: one more change of the key set.
        if created || state.worker.is_none() || state.worker_next.is_none() {
            if state.worker.is_none() {
                state.worker = Some(seal(&d.envelope, &ControlPlaneKey::generate()?, now)?);
            }
            if state.worker_next.is_none() {
                state.worker_next = Some(seal(&d.envelope, &ControlPlaneKey::generate()?, now)?);
            }
            state.sequence = next_sequence(state.sequence, now);
            write_state(d.settings.as_ref(), &state, now).await?;
        }
        let key = Arc::new(open(&d.envelope, &state.current)?);
        let worker_key = Arc::new(open_worker(&d.envelope, &state)?);
        let (cert, not_before, not_after) = certified(&d.ca, &key, &d.hostnames, now)?;
        let acks = read_acks(d.settings.as_ref()).await?;
        let keys = Self {
            loaded: RwLock::new(Loaded { state, key, worker_key, not_before, not_after, generation: 0 }),
            resolver: Arc::new(KeyResolver(RwLock::new(cert))),
            acks: Mutex::new(acks),
            seen: Mutex::new(BTreeMap::new()),
            ack_written: Mutex::new(BTreeMap::new()),
            d,
        };
        Ok((keys, created))
    }

    /// The certificate resolver for the agent listener.
    #[must_use]
    pub fn resolver(&self) -> Arc<KeyResolver> {
        self.resolver.clone()
    }

    /// SHA-256 of the key the listeners serve.
    #[must_use]
    pub fn current_fingerprint(&self) -> String {
        self.read().state.current.sha256.clone()
    }

    /// The current and next keys.
    #[must_use]
    pub fn keys(&self) -> Vec<KeyInfo> {
        let l = self.read();
        let mut out = vec![
            KeyInfo {
                sha256: l.state.current.sha256.clone(),
                label: LABEL_CURRENT,
                created_at: Timestamp(l.state.current.created_at_ms),
            },
            KeyInfo {
                sha256: l.state.next.sha256.clone(),
                label: LABEL_NEXT,
                created_at: Timestamp(l.state.next.created_at_ms),
            },
        ];
        for (k, label) in [(&l.state.worker, LABEL_ANALYTICS), (&l.state.worker_next, LABEL_ANALYTICS_NEXT)] {
            if let Some(k) = k {
                out.push(KeyInfo { sha256: k.sha256.clone(), label, created_at: Timestamp(k.created_at_ms) });
            }
        }
        if let Some(p) = &l.state.previous {
            out.push(KeyInfo { sha256: p.sha256.clone(), label: "previous", created_at: Timestamp(p.created_at_ms) });
        }
        out
    }

    /// Bumped on every key switch.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.read().generation
    }

    /// The key-set sequence nodes are told.
    #[must_use]
    pub fn sequence(&self) -> u64 {
        self.read().state.sequence
    }

    /// SHA-256 of the analytics worker listener's key.
    #[must_use]
    pub fn worker_fingerprint(&self) -> String {
        self.read().worker_key.fingerprint().to_owned()
    }

    /// A certificate for the analytics worker's listener: its own key,
    /// never the agent hub's.
    pub fn listener_certificate(&self, now: Timestamp) -> Result<forge_pki::ServerCertificate, AgentKeyError> {
        let key = self.read().worker_key.clone();
        Ok(self.d.ca.issue_server_certificate_for_key(&key, &self.d.hostnames, now)?)
    }

    /// The keys an agent should pin, signed for `server_id`.
    #[must_use]
    pub fn announcement(&self, server_id: &str, now: Timestamp) -> SignedServerKeys {
        let l = self.read();
        let mut keys = vec![
            ServerKey { sha256: l.state.current.sha256.clone(), label: LABEL_CURRENT.into() },
            ServerKey { sha256: l.state.next.sha256.clone(), label: LABEL_NEXT.into() },
        ];
        for (k, label) in [(&l.state.worker, LABEL_ANALYTICS), (&l.state.worker_next, LABEL_ANALYTICS_NEXT)] {
            if let Some(k) = k {
                keys.push(ServerKey { sha256: k.sha256.clone(), label: label.into() });
            }
        }
        let keys = ServerKeys {
            server_id: server_id.to_owned(),
            issued_at_ms: now.0,
            keys,
            sequence: l.state.sequence,
            revoked: l.state.revoked.clone(),
        };
        drop(l);
        self.d.signer.sign_server_keys(&keys)
    }

    /// Records what an agent reported having pinned: only fingerprints this
    /// server holds; an empty result (an agent that pins none of them) is
    /// ignored. Stored only on a change, and a change that adds none of
    /// this server's keys at most every [`ACK_MIN_INTERVAL_MS`] per agent,
    /// so an agent cannot make the server rewrite the setting each beat.
    pub async fn record_ack(&self, server_id: &str, pinned: &[String], now: Timestamp) -> Result<(), AgentKeyError> {
        let known: Vec<String> = self.read().state.known().into_iter().map(str::to_owned).collect();
        let mut keys: Vec<String> =
            pinned.iter().filter_map(|k| forge_pki::parse_fingerprint(k)).filter(|k| known.contains(k)).collect();
        keys.sort();
        keys.dedup();
        if keys.is_empty() {
            return Ok(());
        }
        self.seen.lock().unwrap_or_else(PoisonError::into_inner).insert(server_id.to_owned(), now.0);
        let snapshot = {
            let mut acks = self.acks.lock().unwrap_or_else(PoisonError::into_inner);
            let previous = acks.get(server_id);
            if previous.is_some_and(|a| a.keys == keys) {
                return Ok(());
            }
            let adds = previous.is_none_or(|a| keys.iter().any(|k| !a.keys.contains(k)));
            let mut written = self.ack_written.lock().unwrap_or_else(PoisonError::into_inner);
            if !adds && written.get(server_id).is_some_and(|at| now.0 - at < ACK_MIN_INTERVAL_MS) {
                return Ok(());
            }
            written.insert(server_id.to_owned(), now.0);
            acks.insert(server_id.to_owned(), AgentAck { keys, since_ms: now.0 });
            acks.clone()
        };
        let json = serde_json::to_string(&snapshot)?;
        self.d.settings.put_setting(SETTING_ACKS, &json, now).await?;
        Ok(())
    }

    /// Forgets the acknowledgements of agents not in `existing` (deleted
    /// nodes). Returns how many were dropped.
    pub async fn prune_acks(
        &self,
        existing: &std::collections::HashSet<String>,
        now: Timestamp,
    ) -> Result<usize, AgentKeyError> {
        let (dropped, snapshot) = {
            let mut acks = self.acks.lock().unwrap_or_else(PoisonError::into_inner);
            let before = acks.len();
            acks.retain(|id, _| existing.contains(id));
            (before - acks.len(), acks.clone())
        };
        if dropped > 0 {
            self.seen.lock().unwrap_or_else(PoisonError::into_inner).retain(|id, _| existing.contains(id));
            self.ack_written.lock().unwrap_or_else(PoisonError::into_inner).retain(|id, _| existing.contains(id));
            self.d.settings.put_setting(SETTING_ACKS, &serde_json::to_string(&snapshot)?, now).await?;
        }
        Ok(dropped)
    }

    /// Every agent's last reported pins.
    #[must_use]
    pub fn acknowledgements(&self) -> BTreeMap<String, AgentAck> {
        self.acks.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Online agents that pin, and how many of them have `sha256`.
    #[must_use]
    pub fn ack_counts(&self, sha256: &str, now: Timestamp) -> (usize, usize) {
        let window = i64::try_from(self.d.online_window.as_millis()).unwrap_or(i64::MAX);
        let seen = self.seen.lock().unwrap_or_else(PoisonError::into_inner);
        let acks = self.acks.lock().unwrap_or_else(PoisonError::into_inner);
        // Without any report yet (a CLI process, or just after a restart),
        // every agent that ever reported pins counts: the cautious choice.
        let online: Vec<&String> = if seen.is_empty() {
            acks.keys().collect()
        } else {
            seen.iter().filter(|(_, at)| now.0 - **at <= window).map(|(id, _)| id).collect()
        };
        let acknowledged =
            online.iter().filter(|id| acks.get(**id).is_some_and(|a| a.keys.iter().any(|k| k == sha256))).count();
        (online.len(), acknowledged)
    }

    /// Where the scheduled switch stands.
    #[must_use]
    pub fn rotation_status(&self, now: Timestamp) -> RotationStatus {
        let l = self.read();
        let s = &l.state;
        let policy = self.d.policy;
        let scheduled = (policy.rotate_days > 0).then(|| s.current_since_ms + i64::from(policy.rotate_days) * DAY_MS);
        let due = s.rotation_requested_at_ms.is_some() || scheduled.is_some_and(|at| now.0 >= at);
        if !due {
            return scheduled.map_or(RotationStatus::Manual, |at| RotationStatus::Scheduled { at: Timestamp(at) });
        }
        let grace_until = s.next.created_at_ms + i64::from(policy.grace_days) * DAY_MS;
        if now.0 < grace_until {
            return RotationStatus::Grace { until: Timestamp(grace_until) };
        }
        let next = s.next.sha256.clone();
        drop(l);
        let (online, acknowledged) = self.ack_counts(&next, now);
        if acknowledged < online {
            return RotationStatus::Acknowledgements { online, acknowledged };
        }
        RotationStatus::Ready
    }

    /// Periodic work: picks up a switch made elsewhere (the CLI), renews the
    /// certificate before it expires, and switches keys when the rotation
    /// is ready.
    pub async fn tick(&self, now: Timestamp) -> Result<TickOutcome, AgentKeyError> {
        let mut out = TickOutcome::default();
        if let Some(stored) = read_state(self.d.settings.as_ref()).await? {
            let before = self.current_fingerprint();
            if stored.current.sha256 != before {
                let to = stored.current.sha256.clone();
                self.apply(stored, now)?;
                tracing::info!(from = %before, to = %to, "agent listener key switched elsewhere; now serving it");
                out.picked_up = true;
                return Ok(out);
            }
            self.write().state = stored;
        }
        if matches!(self.rotation_status(now), RotationStatus::Ready) {
            out.rotated = Some(self.switch(now, RotateMode::WhenReady).await?);
            return Ok(out);
        }
        let (nb, na) = {
            let l = self.read();
            (l.not_before, l.not_after)
        };
        if forge_pki::renewal_due(nb, na, now) {
            let key = self.read().key.clone();
            let (cert, not_before, not_after) = certified(&self.d.ca, &key, &self.d.hostnames, now)?;
            self.resolver.set(cert);
            let mut l = self.write();
            l.not_before = not_before;
            l.not_after = not_after;
            out.renewed = true;
        }
        Ok(out)
    }

    /// `forge-server agent-key rotate`: with [`RotateMode::Now`], switches at
    /// once; with [`RotateMode::NowRevoke`], also revokes the old keys;
    /// otherwise asks for a switch once the grace period has passed and
    /// every online agent has the next key. Returns the switch, if made.
    pub async fn rotate(&self, now: Timestamp, mode: RotateMode) -> Result<Option<Rotation>, AgentKeyError> {
        match mode {
            RotateMode::Now | RotateMode::NowRevoke | RotateMode::NowFresh => {
                return Ok(Some(self.switch(now, mode).await?));
            }
            RotateMode::WhenReady => {}
        }
        let state = {
            let mut l = self.write();
            l.state.rotation_requested_at_ms.get_or_insert(now.0);
            l.state.clone()
        };
        write_state(self.d.settings.as_ref(), &state, now).await?;
        Ok(None)
    }

    /// Switches the listeners (agent hub and analytics worker) to their
    /// next keys and makes new next keys. With [`RotateMode::NowRevoke`],
    /// every key that does not survive the switch (the old current, worker
    /// and previous keys) is revoked rather than kept; with
    /// [`RotateMode::NowFresh`], all four keys are made new and every old
    /// one is revoked. Store first, then memory: a failed write changes
    /// nothing.
    async fn switch(&self, now: Timestamp, mode: RotateMode) -> Result<Rotation, AgentKeyError> {
        let (old, next_sha) = {
            let l = self.read();
            (l.state.clone(), l.state.next.sha256.clone())
        };
        let (online, acknowledged) = self.ack_counts(&next_sha, now);
        let new_key =
            || -> Result<StoredKey, AgentKeyError> { seal(&self.d.envelope, &ControlPlaneKey::generate()?, now) };
        let fresh_set = mode == RotateMode::NowFresh;
        let (current, worker) = if fresh_set {
            (new_key()?, Some(new_key()?))
        } else {
            (old.next.clone(), old.worker_next.clone().or_else(|| old.worker.clone()))
        };
        let revoke = matches!(mode, RotateMode::NowRevoke | RotateMode::NowFresh);
        let mut state = KeyState {
            previous: (!revoke).then(|| old.current.clone()),
            current,
            next: new_key()?,
            current_since_ms: now.0,
            rotation_requested_at_ms: None,
            sequence: next_sequence(old.sequence, now),
            revoked: old.revoked.clone(),
            worker,
            worker_next: Some(new_key()?),
        };
        let mut revoked_now = Vec::new();
        if revoke {
            // Every fingerprint the old state held that the new one does not.
            let kept: Vec<String> = state.known().into_iter().map(str::to_owned).collect();
            for fp in old.known().into_iter().chain(old.previous.as_ref().map(|p| p.sha256.as_str())) {
                if !kept.iter().any(|k| k == fp) && !revoked_now.iter().any(|r| r == fp) {
                    revoked_now.push(fp.to_owned());
                }
            }
            state.revoked.retain(|r| !revoked_now.contains(r));
            state.revoked.extend(revoked_now.iter().cloned());
            let max = forge_proto::messages::MAX_REVOKED_KEYS;
            if state.revoked.len() > max {
                state.revoked.drain(..state.revoked.len() - max);
            }
        }
        write_state(self.d.settings.as_ref(), &state, now).await?;
        let rotation = Rotation {
            from: old.current.sha256,
            to: state.current.sha256.clone(),
            next: state.next.sha256.clone(),
            worker: state.worker.as_ref().map(|w| w.sha256.clone()).unwrap_or_default(),
            online,
            acknowledged,
            forced: mode != RotateMode::WhenReady,
            fresh: fresh_set,
            revoked: revoked_now,
            sequence: state.sequence,
        };
        self.apply(state, now)?;
        tracing::info!(from = %rotation.from, to = %rotation.to, online, acknowledged, forced = rotation.forced, "agent listener key switched");
        Ok(rotation)
    }

    /// Serves `state.current` from now on.
    fn apply(&self, state: KeyState, now: Timestamp) -> Result<(), AgentKeyError> {
        let key = Arc::new(open(&self.d.envelope, &state.current)?);
        let worker_key = Arc::new(open_worker(&self.d.envelope, &state)?);
        let (cert, not_before, not_after) = certified(&self.d.ca, &key, &self.d.hostnames, now)?;
        self.resolver.set(cert);
        let mut l = self.write();
        let generation = l.generation + 1;
        *l = Loaded { state, key, worker_key, not_before, not_after, generation };
        Ok(())
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, Loaded> {
        self.loaded.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, Loaded> {
        self.loaded.write().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The sequence after a change of the key set: one more than before, and
/// at least the time of the change (ms). A database restored from a backup
/// carries an older sequence that agents would refuse; taking the clock
/// moves the next announcement past anything they applied.
fn next_sequence(old: u64, now: Timestamp) -> u64 {
    old.saturating_add(1).max(u64::try_from(now.0).unwrap_or(0))
}

async fn setting(settings: &dyn SettingsRepository, key: &str) -> Result<Option<String>, AgentKeyError> {
    Ok(settings.all_settings().await?.into_iter().find(|(k, _)| k == key).map(|(_, v)| v))
}

async fn read_state(settings: &dyn SettingsRepository) -> Result<Option<KeyState>, AgentKeyError> {
    match setting(settings, SETTING_KEYS).await? {
        Some(v) => Ok(Some(serde_json::from_str(&v)?)),
        None => Ok(None),
    }
}

async fn write_state(settings: &dyn SettingsRepository, s: &KeyState, now: Timestamp) -> Result<(), AgentKeyError> {
    settings.put_setting(SETTING_KEYS, &serde_json::to_string(s)?, now).await?;
    Ok(())
}

async fn read_acks(settings: &dyn SettingsRepository) -> Result<BTreeMap<String, AgentAck>, AgentKeyError> {
    match setting(settings, SETTING_ACKS).await? {
        Some(v) => Ok(serde_json::from_str(&v)?),
        None => Ok(BTreeMap::new()),
    }
}

/// The analytics worker's listener certificates, for the current key.
#[derive(Debug)]
pub struct WorkerListenerCerts(pub Arc<AgentKeys>);

impl crate::analytics_worker::ListenerCerts for WorkerListenerCerts {
    fn issue(&self, now: Timestamp) -> Result<crate::analytics_worker::ListenerCert, PkiError> {
        let c = self.0.listener_certificate(now).map_err(|e| PkiError::Generate(e.to_string()))?;
        Ok(crate::analytics_worker::ListenerCert {
            tls: forge_analytics_proto::config::AgentTls { cert_pem: c.cert_pem, key_pem: c.key_pem },
            not_before: c.not_before,
            not_after: c.not_after,
        })
    }

    fn generation(&self) -> u64 {
        self.0.generation()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // reason: tests
mod tests {
    use super::*;

    const T0: Timestamp = Timestamp(1_800_000_000_000);

    #[derive(Debug, Default)]
    struct Settings(Mutex<BTreeMap<String, String>>);

    #[async_trait::async_trait]
    impl SettingsRepository for Settings {
        async fn all_settings(&self) -> forge_core::DomainResult<Vec<(String, String)>> {
            Ok(self.0.lock().unwrap().iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        }
        async fn put_setting(&self, key: &str, value: &str, _now: Timestamp) -> forge_core::DomainResult<()> {
            self.0.lock().unwrap().insert(key.into(), value.into());
            Ok(())
        }
        async fn delete_setting(&self, key: &str) -> forge_core::DomainResult<()> {
            self.0.lock().unwrap().remove(key);
            Ok(())
        }
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        deps: AgentKeysDeps,
        settings: Arc<Settings>,
    }

    fn fixture(policy: AgentTlsSettings) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let provider = forge_secrets::LocalKeyProvider::load_or_create(&dir.path().join("master.key")).unwrap();
        let settings = Arc::new(Settings::default());
        let deps = AgentKeysDeps {
            settings: settings.clone(),
            envelope: Envelope::new(Arc::new(provider)),
            ca: Arc::new(CertificateAuthority::generate("t", T0).unwrap()),
            signer: Arc::new(CommandSigner::generate().unwrap()),
            hostnames: vec!["forge.test".into()],
            policy,
            online_window: Duration::from_secs(60),
        };
        Fixture { _dir: dir, deps, settings }
    }

    fn served_key(keys: &AgentKeys, now: Timestamp) -> String {
        let leaf = keys.resolver().current();
        forge_pki::inspect_server_leaf(leaf.cert[0].as_ref(), now).unwrap().key_fingerprint
    }

    fn day(n: i64) -> Timestamp {
        Timestamp(T0.0 + n * DAY_MS)
    }

    #[tokio::test]
    async fn keys_are_created_once_sealed_and_reloaded() {
        let f = fixture(AgentTlsSettings::default());
        let (a, created) = AgentKeys::load_or_create(f.deps.clone(), T0).await.unwrap();
        assert!(created);
        let stored = f.settings.0.lock().unwrap().get(SETTING_KEYS).cloned().unwrap();
        assert!(!stored.contains("PRIVATE KEY"), "the private key is sealed at rest");
        let (b, created) = AgentKeys::load_or_create(f.deps.clone(), T0).await.unwrap();
        assert!(!created);
        assert_eq!(a.keys(), b.keys());
        assert_eq!(served_key(&b, T0), a.current_fingerprint());
        // The announcement is signed for the node and carries current and next.
        let signed = a.announcement("srv-1", T0);
        let ann = forge_proto::verify_server_keys(&f.deps.signer.public_key(), &signed, "srv-1", T0).unwrap();
        let fps: Vec<&str> = ann.keys.iter().map(|k| k.sha256.as_str()).collect();
        let listed: Vec<String> = a.keys().into_iter().take(4).map(|k| k.sha256).collect();
        assert_eq!(fps, listed);
        assert_eq!(ann.sequence, u64::try_from(T0.0).unwrap(), "at least the time of the change");
        assert!(ann.revoked.is_empty());
    }

    #[tokio::test]
    async fn the_worker_listener_has_its_own_key_announced_as_analytics() {
        let f = fixture(AgentTlsSettings::default());
        let (keys, _) = AgentKeys::load_or_create(f.deps.clone(), T0).await.unwrap();
        let worker = keys.worker_fingerprint();
        assert_ne!(worker, keys.current_fingerprint(), "never the agent hub's key");
        let cert = keys.listener_certificate(T0).unwrap();
        let der = rustls_pki_types::CertificateDer::pem_slice_iter(cert.cert_pem.as_bytes()).next().unwrap().unwrap();
        assert_eq!(forge_pki::inspect_server_leaf(der.as_ref(), T0).unwrap().key_fingerprint, worker);
        let ann =
            forge_proto::verify_server_keys(&f.deps.signer.public_key(), &keys.announcement("srv-1", T0), "srv-1", T0)
                .unwrap();
        assert!(ann.keys.iter().any(|k| k.sha256 == worker && k.label == LABEL_ANALYTICS));
        // An upgraded state without worker keys gets them, with a new sequence.
        let mut old: serde_json::Value =
            serde_json::from_str(&f.settings.0.lock().unwrap()[SETTING_KEYS].clone()).unwrap();
        old.as_object_mut().unwrap().remove("worker");
        old.as_object_mut().unwrap().remove("worker_next");
        old["sequence"] = serde_json::json!(0);
        f.settings.0.lock().unwrap().insert(SETTING_KEYS.into(), old.to_string());
        let (up, created) = AgentKeys::load_or_create(f.deps.clone(), T0).await.unwrap();
        assert!(!created);
        assert_eq!(up.sequence(), u64::try_from(T0.0).unwrap());
        assert_ne!(up.worker_fingerprint(), up.current_fingerprint());
    }

    #[tokio::test]
    async fn rotating_with_revoke_drops_and_announces_every_old_key() {
        let f = fixture(AgentTlsSettings::default());
        let (keys, _) = AgentKeys::load_or_create(f.deps.clone(), T0).await.unwrap();
        // A plain switch keeps the previous key and bumps the sequence.
        let seq = keys.sequence();
        let r = keys.rotate(day(1), RotateMode::Now).await.unwrap().unwrap();
        assert!(r.revoked.is_empty() && r.sequence > seq);
        assert!(keys.keys().iter().any(|k| k.label == "previous"));
        let (previous, old, old_worker, seq) =
            (r.from.clone(), keys.current_fingerprint(), keys.worker_fingerprint(), keys.sequence());
        let r = keys.rotate(day(2), RotateMode::NowRevoke).await.unwrap().unwrap();
        assert_eq!(r.revoked, [old.clone(), old_worker.clone(), previous.clone()]);
        assert!(r.sequence > seq);
        assert!(keys.keys().iter().all(|k| k.sha256 != old && k.label != "previous"), "not kept");
        let ann = forge_proto::verify_server_keys(
            &f.deps.signer.public_key(),
            &keys.announcement("srv-1", day(2)),
            "srv-1",
            day(2),
        )
        .unwrap();
        assert_eq!(ann.revoked, [old, old_worker, previous]);
        assert_eq!(ann.sequence, r.sequence);
    }

    #[tokio::test]
    async fn rotating_with_fresh_keys_revokes_everything_announced_before() {
        let f = fixture(AgentTlsSettings::default());
        let (keys, _) = AgentKeys::load_or_create(f.deps.clone(), T0).await.unwrap();
        let before: Vec<String> = keys.keys().into_iter().map(|k| k.sha256).collect();
        let r = keys.rotate(day(1), RotateMode::NowFresh).await.unwrap().unwrap();
        assert!(r.fresh);
        assert_eq!(r.revoked.len(), before.len(), "current, next, worker and worker-next");
        assert!(before.iter().all(|b| r.revoked.contains(b)));
        assert!(keys.keys().iter().all(|k| !before.contains(&k.sha256)), "nothing old survives");
        assert_eq!(served_key(&keys, day(1)), r.to);
        assert_eq!(keys.worker_fingerprint(), r.worker);
    }

    #[tokio::test]
    async fn a_restored_lower_sequence_moves_past_what_agents_applied() {
        let f = fixture(AgentTlsSettings::default());
        let (keys, _) = AgentKeys::load_or_create(f.deps.clone(), T0).await.unwrap();
        let backup = f.settings.0.lock().unwrap()[SETTING_KEYS].clone();
        // Agents applied announcements made after the backup.
        keys.rotate(day(1), RotateMode::Now).await.unwrap();
        keys.rotate(day(1), RotateMode::Now).await.unwrap();
        let applied = keys.sequence();
        // The database is restored; the server restarts with the old state.
        f.settings.0.lock().unwrap().insert(SETTING_KEYS.into(), backup);
        let (restored, _) = AgentKeys::load_or_create(f.deps.clone(), day(2)).await.unwrap();
        assert!(restored.sequence() < applied, "an agent would refuse this as stale");
        let r = restored.rotate(day(2), RotateMode::Now).await.unwrap().unwrap();
        assert!(r.sequence > applied, "the next change is accepted again");
        let ann = forge_proto::verify_server_keys(
            &f.deps.signer.public_key(),
            &restored.announcement("srv-1", day(2)),
            "srv-1",
            day(2),
        )
        .unwrap();
        assert!(ann.sequence > applied);
    }

    #[tokio::test]
    async fn acknowledgements_are_checked_rate_limited_and_pruned() {
        let f = fixture(AgentTlsSettings::default());
        let (keys, _) = AgentKeys::load_or_create(f.deps.clone(), T0).await.unwrap();
        let (current, next) = (keys.keys()[0].sha256.clone(), keys.keys()[1].sha256.clone());
        let writes = || f.settings.0.lock().unwrap().get(SETTING_ACKS).cloned();
        // Unknown fingerprints are dropped; none known: nothing stored.
        keys.record_ack("a", &["ab".repeat(32)], T0).await.unwrap();
        assert!(writes().is_none());
        keys.record_ack("a", &[current.clone(), "ab".repeat(32)], T0).await.unwrap();
        assert_eq!(keys.acknowledgements()["a"].keys, std::slice::from_ref(&current));
        // Adding a known key is stored at once.
        let t1 = Timestamp(T0.0 + 1_000);
        keys.record_ack("a", &[current.clone(), next.clone()], t1).await.unwrap();
        assert_eq!(keys.acknowledgements()["a"].keys.len(), 2);
        // Flapping back (a drop only) within a minute is not stored...
        let t2 = Timestamp(T0.0 + 2_000);
        let before = writes();
        keys.record_ack("a", std::slice::from_ref(&current), t2).await.unwrap();
        assert_eq!(writes(), before);
        // ...after a minute it is.
        let t3 = Timestamp(t1.0 + ACK_MIN_INTERVAL_MS);
        keys.record_ack("a", std::slice::from_ref(&current), t3).await.unwrap();
        assert_eq!(keys.acknowledgements()["a"].keys, std::slice::from_ref(&current));
        // Deleted servers are pruned.
        keys.record_ack("b", std::slice::from_ref(&current), t3).await.unwrap();
        let existing: std::collections::HashSet<String> = ["b".to_owned()].into();
        assert_eq!(keys.prune_acks(&existing, t3).await.unwrap(), 1);
        assert!(!keys.acknowledgements().contains_key("a"));
        assert!(!writes().unwrap().contains("\"a\""));
        assert_eq!(keys.prune_acks(&existing, t3).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn the_certificate_is_renewed_with_the_same_key_in_place() {
        let f = fixture(AgentTlsSettings { rotate_days: 0, grace_days: 30 });
        let (keys, _) = AgentKeys::load_or_create(f.deps, T0).await.unwrap();
        let before = keys.resolver().current();
        assert_eq!(keys.tick(day(10)).await.unwrap(), TickOutcome::default());
        let out = keys.tick(day(61)).await.unwrap();
        assert!(out.renewed && out.rotated.is_none());
        let after = keys.resolver().current();
        assert_ne!(before.cert[0], after.cert[0], "a new certificate is served");
        assert_eq!(served_key(&keys, day(61)), keys.current_fingerprint(), "for the same key");
        assert_eq!(keys.generation(), 0, "renewal is not a key change");
    }

    #[tokio::test]
    async fn no_switch_before_grace_and_acknowledgements() {
        let f = fixture(AgentTlsSettings { rotate_days: 365, grace_days: 30 });
        let (keys, _) = AgentKeys::load_or_create(f.deps, T0).await.unwrap();
        let (current, next) = (keys.keys()[0].sha256.clone(), keys.keys()[1].sha256.clone());
        // Two agents pin; one has the next key.
        let now = day(400);
        keys.record_ack("a", std::slice::from_ref(&current), now).await.unwrap();
        keys.record_ack("b", &[current.clone(), next.clone()], now).await.unwrap();
        assert_eq!(keys.rotation_status(day(100)), RotationStatus::Scheduled { at: day(365) });
        assert_eq!(keys.rotation_status(now), RotationStatus::Acknowledgements { online: 2, acknowledged: 1 });
        assert!(keys.tick(now).await.unwrap().rotated.is_none());
        assert_eq!(keys.current_fingerprint(), current);
        // An agent that went offline no longer holds the switch back...
        let later = Timestamp(now.0 + 120_000);
        keys.record_ack("b", &[current.clone(), next.clone()], later).await.unwrap();
        assert_eq!(keys.rotation_status(later), RotationStatus::Ready);
        let r = keys.tick(later).await.unwrap().rotated.unwrap();
        assert_eq!(
            (r.from.as_str(), r.to.as_str(), r.online, r.acknowledged, r.forced),
            (&*current, &*next, 1, 1, false)
        );
        assert_eq!(keys.current_fingerprint(), next);
        assert_eq!(served_key(&keys, later), next);
        assert_eq!(keys.generation(), 1);
        // ...the old key is kept for one rotation, and a fresh next key is made.
        let labels: Vec<(&str, String)> = keys.keys().into_iter().map(|k| (k.label, k.sha256)).collect();
        assert!(labels.contains(&("previous", current)));
        assert_eq!(r.next, labels[1].1);
        // The fresh next key waits out its own grace period.
        assert!(matches!(keys.rotation_status(later), RotationStatus::Scheduled { .. }));
    }

    #[tokio::test]
    async fn a_requested_rotation_waits_and_now_switches_at_once() {
        let f = fixture(AgentTlsSettings { rotate_days: 0, grace_days: 30 });
        let (keys, _) = AgentKeys::load_or_create(f.deps.clone(), T0).await.unwrap();
        assert_eq!(keys.rotation_status(day(5)), RotationStatus::Manual);
        assert!(keys.rotate(day(5), RotateMode::WhenReady).await.unwrap().is_none());
        assert_eq!(keys.rotation_status(day(5)), RotationStatus::Grace { until: day(30) });
        assert!(keys.tick(day(5)).await.unwrap().rotated.is_none());
        // `--now` ignores grace and acknowledgements.
        let current = keys.current_fingerprint();
        let next = keys.keys()[1].sha256.clone();
        keys.record_ack("a", std::slice::from_ref(&current), day(6)).await.unwrap();
        let r = keys.rotate(day(6), RotateMode::Now).await.unwrap().unwrap();
        assert!(r.forced && r.to == next && r.acknowledged == 0 && r.online == 1);
        // A running server picks up a switch another process (the CLI) made.
        let (server, _) = {
            let f2 = AgentKeysDeps { settings: f.settings.clone(), ..f.deps.clone() };
            // Loaded before the switch it would serve the old key; reload the
            // old state to simulate that.
            let (s, c) = AgentKeys::load_or_create(f2, day(6)).await.unwrap();
            (s, c)
        };
        assert_eq!(server.current_fingerprint(), next);
        let cli = keys;
        let r2 = cli.rotate(day(7), RotateMode::Now).await.unwrap().unwrap();
        let out = server.tick(day(7)).await.unwrap();
        assert!(out.picked_up && out.rotated.is_none());
        assert_eq!(server.current_fingerprint(), r2.to);
        assert_eq!(served_key(&server, day(7)), r2.to);
    }
}
