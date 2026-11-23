//! Package signatures.
//!
//! `signature = Ed25519(publisher_key, "forge-plugin-v1\n" || sha256(manifest) || sha256(artifact))`
//!
//! A valid signature proves *who* published a package, not that it is safe;
//! administrators still approve its permissions explicitly.

use std::collections::HashMap;

use forge_secrets::crypto::{b64url, b64url_decode, sha256};
use ring::rand::SystemRandom;
use ring::signature::{ED25519, Ed25519KeyPair, KeyPair, UnparsedPublicKey};

const DOMAIN: &[u8] = b"forge-plugin-v1\n";

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SignatureError {
    #[error("publisher `{0}` is not trusted")]
    UntrustedPublisher(String),
    #[error("signature does not match package")]
    Invalid,
    #[error("malformed key or signature")]
    Malformed,
}

fn message(manifest: &[u8], artifact: &[u8]) -> Vec<u8> {
    let mut m = DOMAIN.to_vec();
    m.extend_from_slice(&sha256(manifest));
    m.extend_from_slice(&sha256(artifact));
    m
}

/// A publisher signing key (kept by the publisher, never by Forge).
pub struct PublisherKey {
    key: Ed25519KeyPair,
}

impl std::fmt::Debug for PublisherKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PublisherKey").field("public", &self.public_key_b64()).finish()
    }
}

impl PublisherKey {
    /// Generates a key; returns it with its PKCS#8 encoding for storage.
    pub fn generate() -> Result<(Self, Vec<u8>), SignatureError> {
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).map_err(|_| SignatureError::Malformed)?;
        let key = Self::from_pkcs8(pkcs8.as_ref())?;
        Ok((key, pkcs8.as_ref().to_vec()))
    }

    pub fn from_pkcs8(pkcs8: &[u8]) -> Result<Self, SignatureError> {
        Ok(Self { key: Ed25519KeyPair::from_pkcs8(pkcs8).map_err(|_| SignatureError::Malformed)? })
    }

    #[must_use]
    pub fn public_key_b64(&self) -> String {
        b64url(self.key.public_key().as_ref())
    }

    /// Signs a package; returns the base64url signature.
    #[must_use]
    pub fn sign(&self, manifest: &[u8], artifact: &[u8]) -> String {
        b64url(self.key.sign(&message(manifest, artifact)).as_ref())
    }
}

/// Publisher → trusted public keys (several allow key rotation).
#[derive(Debug, Clone, Default)]
pub struct TrustStore {
    keys: HashMap<String, Vec<Vec<u8>>>,
}

impl TrustStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn trust(&mut self, publisher: &str, public_key_b64: &str) -> Result<(), SignatureError> {
        let key = b64url_decode(public_key_b64).filter(|k| k.len() == 32).ok_or(SignatureError::Malformed)?;
        self.keys.entry(publisher.to_owned()).or_default().push(key);
        Ok(())
    }

    /// Verifies that `publisher` signed exactly this manifest and artifact.
    pub fn verify(
        &self,
        publisher: &str,
        manifest: &[u8],
        artifact: &[u8],
        signature_b64: &str,
    ) -> Result<(), SignatureError> {
        let keys = self.keys.get(publisher).ok_or_else(|| SignatureError::UntrustedPublisher(publisher.to_owned()))?;
        let sig = b64url_decode(signature_b64).ok_or(SignatureError::Malformed)?;
        let msg = message(manifest, artifact);
        keys.iter()
            .any(|k| UnparsedPublicKey::new(&ED25519, k).verify(&msg, &sig).is_ok())
            .then_some(())
            .ok_or(SignatureError::Invalid)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn sign_and_verify() {
        let (key, _) = PublisherKey::generate().unwrap();
        let mut trust = TrustStore::new();
        trust.trust("orbyn", &key.public_key_b64()).unwrap();
        let sig = key.sign(b"manifest", b"wasm");
        assert!(trust.verify("orbyn", b"manifest", b"wasm", &sig).is_ok());
        assert_eq!(trust.verify("orbyn", b"manifest", b"tampered", &sig), Err(SignatureError::Invalid));
        assert_eq!(trust.verify("orbyn", b"manifest2", b"wasm", &sig), Err(SignatureError::Invalid));
        assert!(matches!(
            trust.verify("other", b"manifest", b"wasm", &sig),
            Err(SignatureError::UntrustedPublisher(_))
        ));
    }

    #[test]
    fn signature_from_untrusted_key_fails() {
        let (key, _) = PublisherKey::generate().unwrap();
        let (other, _) = PublisherKey::generate().unwrap();
        let mut trust = TrustStore::new();
        trust.trust("orbyn", &other.public_key_b64()).unwrap();
        assert_eq!(trust.verify("orbyn", b"m", b"a", &key.sign(b"m", b"a")), Err(SignatureError::Invalid));
    }
}
