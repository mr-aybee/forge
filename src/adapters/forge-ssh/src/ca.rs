//! SSH user certificate authority for browser terminal sessions.
//!
//! Nodes trust the CA's public key (`TrustedUserCAKeys`); each session gets a
//! fresh Ed25519 key and a certificate valid for a few minutes whose only
//! principal is the requested login. Nothing long-lived ever reaches a node.

use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use forge_core::{DomainError, DomainResult};
use forge_secrets::crypto::random_bytes;
use russh::keys::ssh_key::certificate::{Builder, CertType};
use russh::keys::ssh_key::private::{Ed25519Keypair, KeypairData};
use russh::keys::ssh_key::{LineEnding, PrivateKey};

/// Ed25519 user CA, stored as an OpenSSH private key file (mode 0600).
pub struct SshUserCa {
    key: PrivateKey,
}

impl std::fmt::Debug for SshUserCa {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SshUserCa").field("public", &self.public_openssh()).finish()
    }
}

/// A certificate and its ephemeral key, both OpenSSH-encoded.
pub struct IssuedCert {
    pub key_openssh: String,
    pub cert_openssh: String,
}

impl std::fmt::Debug for IssuedCert {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("IssuedCert(<redacted>)")
    }
}

fn new_ed25519(comment: &str) -> DomainResult<PrivateKey> {
    let pair = Ed25519Keypair::from_seed(&random_bytes::<32>());
    PrivateKey::new(KeypairData::Ed25519(pair), comment).map_err(DomainError::internal)
}

impl SshUserCa {
    /// Loads the CA key or creates it on first use.
    pub fn load_or_create(path: &Path) -> DomainResult<Self> {
        if let Ok(pem) = std::fs::read_to_string(path) {
            let key = PrivateKey::from_openssh(pem.as_bytes()).map_err(DomainError::internal)?;
            return Ok(Self { key });
        }

        let key = new_ed25519("orbyn-forge-user-ca")?;
        let pem = key.to_openssh(LineEnding::LF).map_err(DomainError::internal)?;

        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(DomainError::internal)?;
        }

        write_private(path, pem.as_bytes())?;

        Ok(Self { key })
    }

    /// `ssh-ed25519 AAAA… orbyn-forge-user-ca`, for `TrustedUserCAKeys`.
    #[must_use]
    pub fn public_openssh(&self) -> String {
        self.key.public_key().to_openssh().unwrap_or_default()
    }

    /// Issues a user certificate whose only principal is `principal` (the
    /// login, or `forge.<server_id>.<login>` for nodes that accept only
    /// certificates bound to them), valid from one minute ago (clock skew)
    /// for `ttl`.
    pub fn issue(&self, principal: &str, key_id: &str, ttl: Duration) -> DomainResult<IssuedCert> {
        self.sign(principal, key_id, ttl, &["permit-pty", "permit-agent-forwarding"])
    }

    /// Issues a certificate for one direct port tunnel (#35 phase 2): its
    /// only extension is `permit-port-forwarding` (no pty, no agent), and the
    /// node limits the login further from `key_id`
    /// (`forge-tunnel/<tunnel>/<address>/<port>`) to that one destination.
    pub fn issue_tunnel(&self, principal: &str, key_id: &str, ttl: Duration) -> DomainResult<IssuedCert> {
        self.sign(principal, key_id, ttl, &["permit-port-forwarding"])
    }

    fn sign(&self, principal: &str, key_id: &str, ttl: Duration, extensions: &[&str]) -> DomainResult<IssuedCert> {
        let session_key = new_ed25519(key_id)?;
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_err(DomainError::internal)?.as_secs();
        let mut b = Builder::new(
            random_bytes::<32>().to_vec(),
            session_key.public_key().key_data().clone(),
            now - 60,
            now + ttl.as_secs(),
        )
        .map_err(DomainError::internal)?;
        b.serial(u64::from_be_bytes(random_bytes::<8>()))
            .and_then(|b| b.key_id(key_id))
            .and_then(|b| b.cert_type(CertType::User))
            .and_then(|b| b.valid_principal(principal))
            .map_err(DomainError::internal)?;

        for ext in extensions {
            b.extension(*ext, "").map_err(DomainError::internal)?;
        }

        let cert = b.sign(&self.key).map_err(DomainError::internal)?;

        Ok(IssuedCert {
            key_openssh: session_key.to_openssh(LineEnding::LF).map_err(DomainError::internal)?.to_string(),
            cert_openssh: cert.to_openssh().map_err(DomainError::internal)?,
        })
    }
}

fn write_private(path: &Path, data: &[u8]) -> DomainResult<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .map_err(DomainError::internal)?;
        f.write_all(data).map_err(DomainError::internal)
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, data).map_err(DomainError::internal)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use russh::keys::ssh_key::Certificate;

    use super::*;

    #[test]
    fn issues_short_lived_user_certificates_for_one_login() {
        let dir = tempfile::tempdir().unwrap();
        let ca = SshUserCa::load_or_create(&dir.path().join("ssh_user_ca")).unwrap();
        let again = SshUserCa::load_or_create(&dir.path().join("ssh_user_ca")).unwrap();
        assert_eq!(ca.public_openssh(), again.public_openssh(), "key is persisted");

        let issued = ca.issue("forgeops", "forge:user:1", Duration::from_secs(300)).unwrap();
        let cert = Certificate::from_openssh(&issued.cert_openssh).unwrap();
        assert_eq!(cert.valid_principals(), ["forgeops"]);
        assert_eq!(cert.cert_type(), CertType::User);
        assert!(cert.valid_before() - cert.valid_after() <= 360);
        let ca_pub = russh::keys::PublicKey::from_openssh(&ca.public_openssh()).unwrap();
        let fp = ca_pub.fingerprint(russh::keys::HashAlg::Sha256);
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        cert.validate_at(now, [&fp]).expect("signed by the CA and currently valid");
        cert.validate_at(now + 3600, [&fp]).expect_err("expired after the TTL");
        assert!(PrivateKey::from_openssh(issued.key_openssh.as_bytes()).is_ok());
    }
}
