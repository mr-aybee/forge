use std::path::Path;

use ring::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};

use crate::SecretError;
use crate::crypto::{random_bytes, sha256_hex};

/// Wraps and unwraps data keys. Implementations: local key file (Phase 1),
/// TPM, cloud KMS, OpenBao transit (later).
pub trait KeyProvider: Send + Sync + std::fmt::Debug {
    /// Id of the key currently used for wrapping.
    fn active_key_id(&self) -> &str;
    /// Wraps a data key; returns the key id and the wrapped key.
    fn wrap(&self, dek: &[u8; 32]) -> Result<(String, Vec<u8>), SecretError>;
    /// Unwraps a data key wrapped under `key_id`.
    fn unwrap(&self, key_id: &str, wrapped: &[u8]) -> Result<[u8; 32], SecretError>;
}

/// KEK stored in a local file (0600 on Unix). The key id is derived from the
/// key so a rotated file is detected.
pub struct LocalKeyProvider {
    key_id: String,
    kek: LessSafeKey,
}

impl std::fmt::Debug for LocalKeyProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalKeyProvider").field("key_id", &self.key_id).finish_non_exhaustive()
    }
}

impl LocalKeyProvider {
    /// Creates a provider from a 32-byte key.
    pub fn from_bytes(key: &[u8; 32]) -> Result<Self, SecretError> {
        let unbound = UnboundKey::new(&AES_256_GCM, key).map_err(|_| SecretError::Crypto)?;
        let key_id = format!("local-{}", &sha256_hex(key)[..12]);

        Ok(Self { key_id, kek: LessSafeKey::new(unbound) })
    }

    /// Loads the key file, creating it with a fresh random key if absent.
    pub fn load_or_create(path: &Path) -> Result<Self, SecretError> {
        if !path.exists() {
            let key = random_bytes::<32>();
            write_private(path, &key).map_err(|e| SecretError::KeyFile(e.to_string()))?;
        }

        let bytes = std::fs::read(path).map_err(|e| SecretError::KeyFile(e.to_string()))?;
        let key: [u8; 32] = bytes.as_slice().try_into().map_err(|_| SecretError::KeyFile("must be 32 bytes".into()))?;

        Self::from_bytes(&key)
    }
}

impl KeyProvider for LocalKeyProvider {
    fn active_key_id(&self) -> &str {
        &self.key_id
    }

    fn wrap(&self, dek: &[u8; 32]) -> Result<(String, Vec<u8>), SecretError> {
        let nonce = random_bytes::<12>();
        let mut buf = dek.to_vec();
        self.kek
            .seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), Aad::from(self.key_id.as_bytes()), &mut buf)
            .map_err(|_| SecretError::Crypto)?;
        let mut out = nonce.to_vec();
        out.extend_from_slice(&buf);

        Ok((self.key_id.clone(), out))
    }

    fn unwrap(&self, key_id: &str, wrapped: &[u8]) -> Result<[u8; 32], SecretError> {
        if key_id != self.key_id {
            return Err(SecretError::UnknownKey(key_id.to_owned()));
        }

        if wrapped.len() < 12 {
            return Err(SecretError::Malformed);
        }

        let (nonce, ct) = wrapped.split_at(12);
        let nonce = Nonce::try_assume_unique_for_key(nonce).map_err(|_| SecretError::Malformed)?;
        let mut buf = ct.to_vec();
        let plain = self
            .kek
            .open_in_place(nonce, Aad::from(self.key_id.as_bytes()), &mut buf)
            .map_err(|_| SecretError::Decrypt)?;

        plain.try_into().map_err(|_| SecretError::Malformed)
    }
}

/// Writes a file readable only by the current user (best effort on Windows,
/// where the data directory ACL is expected to be restricted by the installer).
pub fn write_private(path: &Path, data: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?;
        f.write_all(data)
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, data)
    }
}
