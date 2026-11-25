use std::sync::Arc;

use ring::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};

use crate::crypto::{b64url, b64url_decode, random_bytes};
use crate::{KeyProvider, SecretError};

const VERSION: &str = "v1";

/// Seals and opens secrets with per-secret data keys.
#[derive(Debug, Clone)]
pub struct Envelope {
    provider: Arc<dyn KeyProvider>,
}

impl Envelope {
    /// Creates an envelope that wraps keys with `provider`.
    #[must_use]
    pub fn new(provider: Arc<dyn KeyProvider>) -> Self {
        Self { provider }
    }

    /// Encrypts `plaintext`. `aad` binds the ciphertext to its context (for
    /// example `user:<id>:totp`) so it cannot be moved to another record.
    pub fn seal(&self, plaintext: &[u8], aad: &str) -> Result<String, SecretError> {
        let dek = random_bytes::<32>();
        let (key_id, wrapped) = self.provider.wrap(&dek)?;
        let key = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &dek).map_err(|_| SecretError::Crypto)?);
        let nonce = random_bytes::<12>();
        let mut buf = plaintext.to_vec();
        key.seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), Aad::from(aad.as_bytes()), &mut buf)
            .map_err(|_| SecretError::Crypto)?;

        Ok(format!("{VERSION}.{key_id}.{}.{}.{}", b64url(&wrapped), b64url(&nonce), b64url(&buf)))
    }

    /// Opens a sealed value bound to `aad`.
    pub fn open(&self, sealed: &str, aad: &str) -> Result<Vec<u8>, SecretError> {
        let parts: Vec<&str> = sealed.split('.').collect();

        let [version, key_id, wrapped, nonce, ct] = parts.as_slice() else {
            return Err(SecretError::Malformed);
        };

        if *version != VERSION {
            return Err(SecretError::Malformed);
        }

        let wrapped = b64url_decode(wrapped).ok_or(SecretError::Malformed)?;
        let nonce = b64url_decode(nonce).ok_or(SecretError::Malformed)?;
        let mut buf = b64url_decode(ct).ok_or(SecretError::Malformed)?;
        let dek = self.provider.unwrap(key_id, &wrapped)?;
        let key = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &dek).map_err(|_| SecretError::Crypto)?);
        let nonce = Nonce::try_assume_unique_for_key(&nonce).map_err(|_| SecretError::Malformed)?;
        let plain = key.open_in_place(nonce, Aad::from(aad.as_bytes()), &mut buf).map_err(|_| SecretError::Decrypt)?;

        Ok(plain.to_vec())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::LocalKeyProvider;

    fn envelope() -> Envelope {
        Envelope::new(Arc::new(LocalKeyProvider::from_bytes(&[7u8; 32]).unwrap()))
    }

    #[test]
    fn round_trip() {
        let e = envelope();
        let s = e.seal(b"hunter2", "user:1:totp").unwrap();
        assert!(!s.contains("hunter2"));
        assert_eq!(e.open(&s, "user:1:totp").unwrap(), b"hunter2");
    }

    #[test]
    fn wrong_aad_fails() {
        let e = envelope();
        let s = e.seal(b"x", "user:1").unwrap();
        assert!(matches!(e.open(&s, "user:2"), Err(SecretError::Decrypt)));
    }

    #[test]
    fn tampering_fails() {
        let e = envelope();
        let mut s = e.seal(b"secret", "a").unwrap();
        let last = s.pop().unwrap();
        s.push(if last == 'A' { 'B' } else { 'A' });
        assert!(e.open(&s, "a").is_err());
    }

    #[test]
    fn other_kek_cannot_open() {
        let s = envelope().seal(b"x", "a").unwrap();
        let other = Envelope::new(Arc::new(LocalKeyProvider::from_bytes(&[8u8; 32]).unwrap()));
        assert!(matches!(other.open(&s, "a"), Err(SecretError::UnknownKey(_))));
    }

    #[test]
    fn key_file_is_created_and_reloaded() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("master.key");
        let a = LocalKeyProvider::load_or_create(&path).unwrap();
        let b = LocalKeyProvider::load_or_create(&path).unwrap();
        assert_eq!(a.active_key_id(), b.active_key_id());
    }
}
