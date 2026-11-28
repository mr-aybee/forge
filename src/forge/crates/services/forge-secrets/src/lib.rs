//! Envelope encryption and small crypto helpers.
//!
//! Each secret is encrypted with a fresh 256-bit data key (DEK) using
//! AES-256-GCM; the DEK is wrapped by a key-encryption key (KEK) held by a
//! [`KeyProvider`]. Phase 1 ships [`LocalKeyProvider`] (a key file); TPM,
//! cloud KMS and OpenBao providers implement the same trait later.
//!
//! Sealed format: `v1.<key_id>.<wrapped_dek>.<nonce>.<ciphertext>` (base64url).

pub mod crypto;
mod envelope;
pub mod pow;
mod provider;

pub use envelope::Envelope;
pub use provider::{KeyProvider, LocalKeyProvider, write_private};

/// Errors from sealing and opening secrets.
#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    #[error("malformed sealed secret")]
    Malformed,
    #[error("unknown key id `{0}`")]
    UnknownKey(String),
    #[error("decryption failed")]
    Decrypt,
    #[error("crypto failure")]
    Crypto,
    #[error("key file: {0}")]
    KeyFile(String),
}
