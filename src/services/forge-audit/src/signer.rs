use std::path::Path;

use forge_core::{DomainError, DomainResult};
use forge_secrets::crypto::{b64url, b64url_decode};
use ring::rand::SystemRandom;
use ring::signature::{ED25519, Ed25519KeyPair, KeyPair, UnparsedPublicKey};

/// Ed25519 key used to sign audit hashes.
pub struct AuditSigner {
    key: Ed25519KeyPair,
}

impl std::fmt::Debug for AuditSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuditSigner").field("public_key", &hex::encode(self.key.public_key())).finish()
    }
}

impl AuditSigner {
    /// Generates a new signing key.
    pub fn generate() -> DomainResult<Self> {
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).map_err(DomainError::internal)?;
        Self::from_pkcs8(pkcs8.as_ref())
    }

    /// Loads a signing key from PKCS#8 bytes.
    pub fn from_pkcs8(pkcs8: &[u8]) -> DomainResult<Self> {
        Ok(Self { key: Ed25519KeyPair::from_pkcs8(pkcs8).map_err(DomainError::internal)? })
    }

    /// Loads a PKCS#8 key file, creating one if missing.
    pub fn load_or_create(path: &Path) -> DomainResult<Self> {
        if !path.exists() {
            let pkcs8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).map_err(DomainError::internal)?;
            forge_secrets::write_private(path, pkcs8.as_ref()).map_err(DomainError::internal)?;
        }

        Self::from_pkcs8(&std::fs::read(path).map_err(DomainError::internal)?)
    }

    /// Signs a hex hash and returns the signature.
    #[must_use]
    pub fn sign(&self, hash_hex: &str) -> String {
        b64url(self.key.sign(hash_hex.as_bytes()).as_ref())
    }

    /// A verifier for this key's signatures.
    #[must_use]
    pub fn verifier(&self) -> AuditVerifier {
        AuditVerifier { public_key: self.key.public_key().as_ref().to_vec() }
    }
}

/// Verifies audit signatures with the public key only (can run off-host).
#[derive(Debug, Clone)]
pub struct AuditVerifier {
    public_key: Vec<u8>,
}

impl AuditVerifier {
    /// Creates a verifier from a public key.
    #[must_use]
    pub fn from_public_key(public_key: Vec<u8>) -> Self {
        Self { public_key }
    }

    /// The public key, hex-encoded.
    #[must_use]
    pub fn public_key_hex(&self) -> String {
        hex::encode(&self.public_key)
    }

    /// Whether `signature` is a valid signature of `hash_hex`.
    #[must_use]
    pub fn verify(&self, hash_hex: &str, signature: &str) -> bool {
        let Some(sig) = b64url_decode(signature) else { return false };
        UnparsedPublicKey::new(&ED25519, &self.public_key).verify(hash_hex.as_bytes(), &sig).is_ok()
    }
}
