//! Append-only, hash-chained, signed audit log.
//!
//! Each entry stores `prev_hash` and `hash = SHA-256(canonical JSON of the
//! entry without hash/signature)`, and an Ed25519 signature over `hash` made
//! with the control plane's audit key. [`verify_chain`] recomputes every hash,
//! checks the links and the signatures, and reports the first broken entry.
//!
//! Editing any field, deleting an entry or reordering entries breaks the
//! chain. Re-signing a forged chain requires the private audit key, which is
//! why exported checkpoints (head hash) should also be stored off-host.

mod canonical;
mod histogram;
mod signer;

use std::sync::Arc;

use async_trait::async_trait;
use forge_core::model::{AuditEntry, AuditEvent, AuditFilter};
use forge_core::ports::{AuditRecorder, AuditRepository};
use forge_core::{Clock, DomainError, DomainResult};
use serde::Serialize;
use tokio::sync::Mutex;

pub use histogram::{AuditHistogram, AuditHistogramBucket, MAX_BUCKETS};
pub use signer::{AuditSigner, AuditVerifier};

/// `prev_hash` of the first entry.
pub const GENESIS_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

const MAX_APPEND_RETRIES: usize = 8;

/// Fields covered by the entry hash, in a fixed order.
#[derive(Serialize)]
struct Hashed<'a> {
    seq: i64,
    id: &'a str,
    at: i64,
    actor: &'a str,
    action: &'a str,
    resource: &'a str,
    workspace_id: Option<String>,
    outcome: &'a str,
    details: serde_json::Value,
    prev_hash: &'a str,
}

/// Computes the hash of an entry from its content and `prev_hash`.
#[must_use]
pub fn entry_hash(e: &AuditEntry) -> String {
    let h = Hashed {
        seq: e.seq,
        id: &e.id,
        at: e.at.0,
        actor: &e.actor,
        action: &e.action,
        resource: &e.resource,
        workspace_id: e.workspace_id.map(|o| o.to_string()),
        outcome: e.outcome.as_str(),
        details: canonical::sort_keys(&e.details),
        prev_hash: &e.prev_hash,
    };
    // Serializing a struct of strings/ints/Values cannot fail.
    let bytes = serde_json::to_vec(&h).unwrap_or_default();
    forge_secrets::crypto::sha256_hex(&bytes)
}

/// The audit log service.
#[derive(Debug)]
pub struct AuditLog {
    repo: Arc<dyn AuditRepository>,
    signer: Arc<AuditSigner>,
    clock: Arc<dyn Clock>,
    /// Serializes appends within this process; the `UNIQUE(seq)` constraint
    /// handles other processes.
    append_lock: Mutex<()>,
}

impl AuditLog {
    #[must_use]
    pub fn new(repo: Arc<dyn AuditRepository>, signer: Arc<AuditSigner>, clock: Arc<dyn Clock>) -> Self {
        Self { repo, signer, clock, append_lock: Mutex::new(()) }
    }

    #[must_use]
    pub fn verifier(&self) -> AuditVerifier {
        self.signer.verifier()
    }

    /// Appends one event and returns the persisted entry.
    pub async fn append(&self, event: AuditEvent) -> DomainResult<AuditEntry> {
        let _guard = self.append_lock.lock().await;
        for _ in 0..MAX_APPEND_RETRIES {
            let last = self.repo.last_entry().await?;
            let (seq, prev_hash) = match &last {
                Some(l) => (l.seq + 1, l.hash.clone()),
                None => (1, GENESIS_HASH.to_owned()),
            };
            let mut entry = AuditEntry {
                seq,
                id: uuid::Uuid::now_v7().to_string(),
                at: self.clock.now(),
                actor: event.actor.clone(),
                action: event.action.clone(),
                resource: event.resource.clone(),
                workspace_id: event.workspace_id,
                outcome: event.outcome,
                details: canonical::sort_keys(&event.details),
                prev_hash,
                hash: String::new(),
                signature: String::new(),
            };
            entry.hash = entry_hash(&entry);
            entry.signature = self.signer.sign(&entry.hash);
            match self.repo.insert_entry(&entry).await {
                Ok(()) => return Ok(entry),
                Err(DomainError::Conflict(_)) => {
                    tracing::debug!(seq, "audit seq taken by another writer, retrying");
                }
                Err(e) => return Err(e),
            }
        }
        Err(DomainError::Internal("audit append contention".into()))
    }

    /// Entries matching `filter` over time, by outcome, in at most
    /// `buckets` (clamped to 1..=[`MAX_BUCKETS`]) round-width buckets.
    pub async fn histogram(&self, filter: &AuditFilter, buckets: u32) -> DomainResult<AuditHistogram> {
        histogram::build(self.repo.as_ref(), filter, buckets, self.clock.now().0).await
    }

    /// Verifies the whole chain.
    pub async fn verify(&self) -> DomainResult<VerifyReport> {
        verify_chain(self.repo.as_ref(), &self.verifier()).await
    }
}

#[async_trait]
impl AuditRecorder for AuditLog {
    async fn record(&self, event: AuditEvent) -> DomainResult<AuditEntry> {
        self.append(event).await
    }
}

/// Outcome of a chain verification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VerifyReport {
    pub ok: bool,
    pub entries_checked: u64,
    pub head_hash: Option<String>,
    /// First failing sequence number and why.
    pub first_invalid: Option<(i64, String)>,
}

/// Walks the chain from seq 1 and checks hashes, links, ordering and signatures.
pub async fn verify_chain(repo: &dyn AuditRepository, verifier: &AuditVerifier) -> DomainResult<VerifyReport> {
    const PAGE: u32 = 500;
    let mut expected_prev = GENESIS_HASH.to_owned();
    let mut expected_seq = 1i64;
    let mut checked = 0u64;
    loop {
        let page = repo.entries_from(expected_seq, PAGE).await?;
        if page.is_empty() {
            break;
        }
        for e in &page {
            let fail = |why: &str| VerifyReport {
                ok: false,
                entries_checked: checked,
                head_hash: None,
                first_invalid: Some((e.seq, why.to_owned())),
            };
            if e.seq != expected_seq {
                return Ok(fail("sequence gap (entry deleted)"));
            }
            if e.prev_hash != expected_prev {
                return Ok(fail("prev_hash does not match previous entry"));
            }
            if entry_hash(e) != e.hash {
                return Ok(fail("content does not match hash (entry modified)"));
            }
            if !verifier.verify(&e.hash, &e.signature) {
                return Ok(fail("invalid signature"));
            }
            expected_prev.clone_from(&e.hash);
            expected_seq += 1;
            checked += 1;
        }
    }
    Ok(VerifyReport {
        ok: true,
        entries_checked: checked,
        head_hash: (checked > 0).then_some(expected_prev),
        first_invalid: None,
    })
}
