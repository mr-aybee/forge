use std::collections::{HashSet, VecDeque};
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use forge_core::Timestamp;
use forge_secrets::crypto::{random_bytes, sha256_hex};
use prost::Message;
use ring::rand::SystemRandom;
use ring::signature::{ED25519, Ed25519KeyPair, KeyPair, UnparsedPublicKey};

use crate::messages::{CommandBody, ServerKeys, SignedCommand, SignedServerKeys};
use crate::{Command, MAX_CLOCK_SKEW, MAX_COMMAND_TTL};

/// Signs commands for agents (control plane only).
pub struct CommandSigner {
    key: Ed25519KeyPair,
    key_id: String,
}

impl std::fmt::Debug for CommandSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CommandSigner").field("key_id", &self.key_id).finish_non_exhaustive()
    }
}

impl CommandSigner {
    pub fn from_pkcs8(pkcs8: &[u8]) -> Result<Self, VerifyError> {
        let key = Ed25519KeyPair::from_pkcs8(pkcs8).map_err(|_| VerifyError::Malformed)?;
        let key_id = sha256_hex(key.public_key().as_ref())[..16].to_owned();
        Ok(Self { key, key_id })
    }

    pub fn generate() -> Result<Self, VerifyError> {
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).map_err(|_| VerifyError::Malformed)?;
        Self::from_pkcs8(pkcs8.as_ref())
    }

    /// Loads the PKCS#8 key file, creating it on first start.
    pub fn load_or_create(path: &Path) -> std::io::Result<Self> {
        if !path.exists() {
            let pkcs8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new())
                .map_err(|_| std::io::Error::other("key generation failed"))?;
            forge_secrets::write_private(path, pkcs8.as_ref())?;
        }
        Self::from_pkcs8(&std::fs::read(path)?).map_err(|_| std::io::Error::other("invalid command key"))
    }

    #[must_use]
    pub fn public_key(&self) -> Vec<u8> {
        self.key.public_key().as_ref().to_vec()
    }

    /// Builds and signs a command for one agent.
    #[must_use]
    pub fn issue(
        &self,
        server_id: &str,
        principal: &str,
        command: Command,
        now: Timestamp,
        ttl: Duration,
    ) -> (CommandBody, SignedCommand) {
        let body = CommandBody {
            operation_id: uuid_like(),
            server_id: server_id.to_owned(),
            issued_at_ms: now.0,
            expires_at_ms: now.plus(ttl.min(MAX_COMMAND_TTL)).0,
            nonce: random_bytes::<16>().to_vec(),
            principal: principal.to_owned(),
            command: Some(command),
        };
        let signed = self.sign(&body);
        (body, signed)
    }

    /// Signs an already-built body.
    #[must_use]
    pub fn sign(&self, body: &CommandBody) -> SignedCommand {
        let bytes = body.encode_to_vec();
        let signature = self.key.sign(&bytes).as_ref().to_vec();
        SignedCommand { body: bytes, signature, key_id: self.key_id.clone() }
    }
}

/// Prefix of what is signed for [`ServerKeys`], so such a signature can never
/// pass as one over a command body.
const SERVER_KEYS_CONTEXT: &[u8] = b"forge-server-keys-v1\0";

/// How long a signed key announcement is accepted after it was issued.
pub const SERVER_KEYS_MAX_AGE: Duration = Duration::from_secs(3600);

fn server_keys_message(body: &[u8]) -> Vec<u8> {
    let mut m = Vec::with_capacity(SERVER_KEYS_CONTEXT.len() + body.len());
    m.extend_from_slice(SERVER_KEYS_CONTEXT);
    m.extend_from_slice(body);
    m
}

impl CommandSigner {
    /// Signs a control-plane key announcement (D-167).
    #[must_use]
    pub fn sign_server_keys(&self, keys: &ServerKeys) -> SignedServerKeys {
        let body = keys.encode_to_vec();
        let signature = self.key.sign(&server_keys_message(&body)).as_ref().to_vec();
        SignedServerKeys { body, signature }
    }
}

/// Checks a key announcement against the command key: the signature, the
/// target node and its age (at most [`SERVER_KEYS_MAX_AGE`], with clock skew).
pub fn verify_server_keys(
    public_key: &[u8],
    signed: &SignedServerKeys,
    server_id: &str,
    now: Timestamp,
) -> Result<ServerKeys, VerifyError> {
    UnparsedPublicKey::new(&ED25519, public_key)
        .verify(&server_keys_message(&signed.body), &signed.signature)
        .map_err(|_| VerifyError::BadSignature)?;
    let keys = ServerKeys::decode(signed.body.as_slice()).map_err(|_| VerifyError::Malformed)?;
    if keys.server_id != server_id {
        return Err(VerifyError::WrongTarget);
    }
    let skew = i64::try_from(MAX_CLOCK_SKEW.as_millis()).unwrap_or(i64::MAX);
    let max_age = i64::try_from(SERVER_KEYS_MAX_AGE.as_millis()).unwrap_or(i64::MAX);
    if keys.issued_at_ms > now.0 + skew {
        return Err(VerifyError::NotYetValid);
    }
    if now.0 > keys.issued_at_ms + max_age + skew {
        return Err(VerifyError::Expired);
    }
    if keys.keys.len() > crate::messages::MAX_SERVER_KEYS {
        return Err(VerifyError::Malformed);
    }
    Ok(keys)
}

fn uuid_like() -> String {
    hex::encode(random_bytes::<16>())
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VerifyError {
    #[error("malformed command")]
    Malformed,
    #[error("bad signature")]
    BadSignature,
    #[error("command addressed to another agent")]
    WrongTarget,
    #[error("command expired")]
    Expired,
    #[error("command not yet valid")]
    NotYetValid,
    #[error("command validity window too long")]
    TtlTooLong,
    #[error("command issued before agent start")]
    IssuedBeforeStart,
    #[error("replayed command")]
    Replay,
    #[error("replay cache full")]
    CacheFull,
}

/// Agent-side verification. Holds a bounded replay cache of operation ids;
/// entries leave the cache once their command has expired, so the bound is
/// never hit under normal load.
#[derive(Debug)]
pub struct CommandVerifier {
    public_key: Vec<u8>,
    server_id: String,
    started_at: Timestamp,
    seen: Mutex<ReplayCache>,
}

#[derive(Debug, Default)]
struct ReplayCache {
    order: VecDeque<(i64, String)>,
    ids: HashSet<String>,
}

const REPLAY_CAPACITY: usize = 4096;

impl CommandVerifier {
    #[must_use]
    pub fn new(public_key: Vec<u8>, server_id: String, started_at: Timestamp) -> Self {
        Self { public_key, server_id, started_at, seen: Mutex::new(ReplayCache::default()) }
    }

    /// Verifies and returns the decoded body. Checks run cheapest-first, but
    /// nothing from the body is trusted until the signature has passed.
    pub fn verify(&self, cmd: &SignedCommand, now: Timestamp) -> Result<CommandBody, VerifyError> {
        UnparsedPublicKey::new(&ED25519, &self.public_key)
            .verify(&cmd.body, &cmd.signature)
            .map_err(|_| VerifyError::BadSignature)?;
        let body = CommandBody::decode(cmd.body.as_slice()).map_err(|_| VerifyError::Malformed)?;
        if body.command.is_none() || body.operation_id.is_empty() {
            return Err(VerifyError::Malformed);
        }
        if body.server_id != self.server_id {
            return Err(VerifyError::WrongTarget);
        }
        let skew = i64::try_from(MAX_CLOCK_SKEW.as_millis()).unwrap_or(i64::MAX);
        let max_ttl = i64::try_from(MAX_COMMAND_TTL.as_millis()).unwrap_or(i64::MAX);
        if body.expires_at_ms - body.issued_at_ms > max_ttl {
            return Err(VerifyError::TtlTooLong);
        }
        if now.0 > body.expires_at_ms + skew {
            return Err(VerifyError::Expired);
        }
        if body.issued_at_ms > now.0 + skew {
            return Err(VerifyError::NotYetValid);
        }
        if body.issued_at_ms + skew < self.started_at.0 {
            return Err(VerifyError::IssuedBeforeStart);
        }
        self.remember(&body, now)?;
        Ok(body)
    }

    /// Operation ids still inside their validity window, with their expiry
    /// (ms). A verifier that restarts can [`Self::restore`] them, so a
    /// command seen before the restart cannot be replayed after it.
    #[must_use]
    pub fn remembered(&self) -> Vec<(i64, String)> {
        self.seen.lock().map(|c| c.order.iter().cloned().collect()).unwrap_or_default()
    }

    /// Loads ids saved by [`Self::remembered`]; expired ones are dropped on
    /// the next verification, and at most the cache's capacity is kept.
    pub fn restore(&self, entries: impl IntoIterator<Item = (i64, String)>) {
        let Ok(mut cache) = self.seen.lock() else { return };
        for (exp, id) in entries {
            if cache.ids.len() >= REPLAY_CAPACITY {
                break;
            }
            if cache.ids.insert(id.clone()) {
                cache.order.push_back((exp, id));
            }
        }
    }

    fn remember(&self, body: &CommandBody, now: Timestamp) -> Result<(), VerifyError> {
        let mut cache = self.seen.lock().map_err(|_| VerifyError::Malformed)?;
        let skew = i64::try_from(MAX_CLOCK_SKEW.as_millis()).unwrap_or(i64::MAX);
        while cache.order.front().is_some_and(|(exp, _)| exp + skew < now.0) {
            if let Some((_, id)) = cache.order.pop_front() {
                cache.ids.remove(&id);
            }
        }
        if cache.ids.contains(&body.operation_id) {
            return Err(VerifyError::Replay);
        }
        if cache.ids.len() >= REPLAY_CAPACITY {
            return Err(VerifyError::CacheFull);
        }
        cache.ids.insert(body.operation_id.clone());
        cache.order.push_back((body.expires_at_ms, body.operation_id.clone()));
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::messages::{Ping, ServiceStatus};

    const T0: Timestamp = Timestamp(1_000_000);

    fn pair() -> (CommandSigner, CommandVerifier) {
        let s = CommandSigner::generate().unwrap();
        let v = CommandVerifier::new(s.public_key(), "srv-1".into(), T0);
        (s, v)
    }

    fn ttl() -> Duration {
        Duration::from_secs(60)
    }

    #[test]
    fn valid_command_verifies_once() {
        let (s, v) = pair();
        let (_, cmd) = s.issue("srv-1", "user:a", Command::Ping(Ping {}), T0, ttl());
        assert!(v.verify(&cmd, T0).is_ok());
        assert_eq!(v.verify(&cmd, T0), Err(VerifyError::Replay));
    }

    #[test]
    fn tampered_body_fails() {
        let (s, v) = pair();
        let (mut body, _) = s.issue("srv-1", "user:a", Command::Ping(Ping {}), T0, ttl());
        let mut cmd = s.sign(&body);
        body.command = Some(Command::ServiceStatus(ServiceStatus { name: "sshd".into() }));
        cmd.body = body.encode_to_vec();
        assert_eq!(v.verify(&cmd, T0), Err(VerifyError::BadSignature));
    }

    #[test]
    fn other_signer_fails() {
        let (_, v) = pair();
        let (other, _) = pair();
        let (_, cmd) = other.issue("srv-1", "user:a", Command::Ping(Ping {}), T0, ttl());
        assert_eq!(v.verify(&cmd, T0), Err(VerifyError::BadSignature));
    }

    #[test]
    fn wrong_target_fails() {
        let (s, v) = pair();
        let (_, cmd) = s.issue("srv-2", "user:a", Command::Ping(Ping {}), T0, ttl());
        assert_eq!(v.verify(&cmd, T0), Err(VerifyError::WrongTarget));
    }

    #[test]
    fn expired_and_future_commands_fail() {
        let (s, v) = pair();
        let (_, cmd) = s.issue("srv-1", "user:a", Command::Ping(Ping {}), T0, ttl());
        assert_eq!(v.verify(&cmd, T0.plus(Duration::from_secs(120))), Err(VerifyError::Expired));
        let (_, future) = s.issue("srv-1", "user:a", Command::Ping(Ping {}), T0.plus(Duration::from_secs(600)), ttl());
        assert_eq!(v.verify(&future, T0), Err(VerifyError::NotYetValid));
    }

    #[test]
    fn overlong_ttl_is_rejected() {
        let (s, v) = pair();
        let (mut body, _) = s.issue("srv-1", "user:a", Command::Ping(Ping {}), T0, ttl());
        body.expires_at_ms = T0.plus(Duration::from_secs(3600)).0;
        assert_eq!(v.verify(&s.sign(&body), T0), Err(VerifyError::TtlTooLong));
    }

    #[test]
    fn commands_from_before_restart_are_rejected() {
        let s = CommandSigner::generate().unwrap();
        let (_, cmd) = s.issue("srv-1", "user:a", Command::Ping(Ping {}), T0, ttl());
        // Agent restarted 40 s after the command was issued (beyond skew).
        let restarted = CommandVerifier::new(s.public_key(), "srv-1".into(), T0.plus(Duration::from_secs(40)));
        assert_eq!(restarted.verify(&cmd, T0.plus(Duration::from_secs(41))), Err(VerifyError::IssuedBeforeStart));
    }

    #[test]
    fn a_restored_cache_still_refuses_replays() {
        let (s, v) = pair();
        let (_, cmd) = s.issue("srv-1", "u", Command::Ping(Ping {}), T0, Duration::from_secs(60));
        v.verify(&cmd, T0).unwrap();
        let saved = v.remembered();
        let restarted = CommandVerifier::new(s.public_key(), "srv-1".into(), T0);
        restarted.restore(saved);
        assert_eq!(restarted.verify(&cmd, T0).unwrap_err(), VerifyError::Replay);
    }

    #[test]
    fn replay_cache_forgets_expired_entries() {
        let (s, v) = pair();
        for i in 0..10 {
            let now = T0.plus(Duration::from_secs(i * 100));
            let (_, cmd) = s.issue("srv-1", "user:a", Command::Ping(Ping {}), now, ttl());
            v.verify(&cmd, now).unwrap();
        }
        assert!(v.seen.lock().unwrap().ids.len() <= 2);
    }

    #[test]
    fn server_key_announcements_are_signed_for_one_node_and_fresh() {
        let (s, _) = pair();
        let keys = ServerKeys {
            server_id: "srv-1".into(),
            issued_at_ms: T0.0,
            keys: vec![crate::messages::ServerKey { sha256: "ab".repeat(32), label: "current".into() }],
        };
        let signed = s.sign_server_keys(&keys);
        assert_eq!(verify_server_keys(&s.public_key(), &signed, "srv-1", T0), Ok(keys.clone()));
        assert_eq!(verify_server_keys(&s.public_key(), &signed, "srv-2", T0), Err(VerifyError::WrongTarget));
        let late = T0.plus(SERVER_KEYS_MAX_AGE + MAX_CLOCK_SKEW + Duration::from_secs(1));
        assert_eq!(verify_server_keys(&s.public_key(), &signed, "srv-1", late), Err(VerifyError::Expired));
        let other = CommandSigner::generate().unwrap();
        assert_eq!(verify_server_keys(&other.public_key(), &signed, "srv-1", T0), Err(VerifyError::BadSignature));
        let mut tampered = signed.clone();
        tampered.body.push(0);
        assert_eq!(verify_server_keys(&s.public_key(), &tampered, "srv-1", T0), Err(VerifyError::BadSignature));
        // A command signature never passes as an announcement.
        let (_, cmd) = s.issue("srv-1", "u", Command::Ping(Ping {}), T0, ttl());
        let forged = SignedServerKeys { body: cmd.body, signature: cmd.signature };
        assert_eq!(verify_server_keys(&s.public_key(), &forged, "srv-1", T0), Err(VerifyError::BadSignature));
    }
}
