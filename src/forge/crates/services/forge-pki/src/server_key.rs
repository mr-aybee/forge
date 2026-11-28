//! The control plane's long-lived listener key (D-167) and the checks an
//! agent makes on the certificate the control plane presents.
//!
//! Agents pin the SHA-256 of the key's SubjectPublicKeyInfo, not a name or
//! a certificate: the listener certificate is renewed with the same key,
//! so renewal never touches agents, and any address of the control plane
//! works because the name in the certificate is not checked.

use std::time::Duration;

use forge_core::Timestamp;
use forge_secrets::crypto::sha256_hex;
use rcgen::{KeyPair, PublicKeyData};
use x509_parser::prelude::*;

use crate::PkiError;

/// How a fingerprint is shown to people: `sha256:` and lower-case hex.
pub const FINGERPRINT_PREFIX: &str = "sha256:";

/// A control-plane listener key. Its private half never leaves the server
/// unsealed except in memory.
pub struct ControlPlaneKey {
    key: KeyPair,
    fingerprint: String,
}

impl std::fmt::Debug for ControlPlaneKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ControlPlaneKey").field("fingerprint", &self.fingerprint).finish_non_exhaustive()
    }
}

impl ControlPlaneKey {
    /// A new ECDSA P-256 key.
    pub fn generate() -> Result<Self, PkiError> {
        Ok(Self::assemble(KeyPair::generate().map_err(|e| PkiError::Generate(e.to_string()))?))
    }

    /// Loads a key from its PKCS#8 PEM.
    pub fn from_pem(pem: &str) -> Result<Self, PkiError> {
        Ok(Self::assemble(KeyPair::from_pem(pem).map_err(|e| PkiError::Parse(e.to_string()))?))
    }

    fn assemble(key: KeyPair) -> Self {
        let fingerprint = sha256_hex(&key.subject_public_key_info());
        Self { key, fingerprint }
    }

    /// SHA-256 (hex) of the SubjectPublicKeyInfo: what agents pin.
    #[must_use]
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    /// The private key as PKCS#8 PEM, for a caller that seals it at rest.
    #[must_use]
    pub fn key_pem(&self) -> String {
        self.key.serialize_pem()
    }

    pub(crate) fn key_pair(&self) -> &KeyPair {
        &self.key
    }
}

/// Whether `s` is a SubjectPublicKeyInfo fingerprint: 64 hex digits, with
/// or without the `sha256:` prefix. Returns it in canonical form (lower
/// case, no prefix).
#[must_use]
pub fn parse_fingerprint(s: &str) -> Option<String> {
    let s = s.trim();
    let hex = s.strip_prefix(FINGERPRINT_PREFIX).unwrap_or(s);

    (hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit())).then(|| hex.to_ascii_lowercase())
}

/// `sha256:ab12cd34…ef56` for messages.
#[must_use]
pub fn short_fingerprint(fp: &str) -> String {
    match (fp.get(..8), fp.get(fp.len().saturating_sub(4)..)) {
        (Some(head), Some(tail)) if fp.len() > 12 => format!("{FINGERPRINT_PREFIX}{head}…{tail}"),
        _ => format!("{FINGERPRINT_PREFIX}{fp}"),
    }
}

/// Why a presented control-plane certificate was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LeafError {
    #[error("the certificate cannot be parsed: {0}")]
    Malformed(String),
    #[error("the certificate is not valid at this time (valid {not_before} to {not_after}, ms since epoch)")]
    Expired { not_before: i64, not_after: i64 },
    #[error("the certificate is not for TLS servers (no ServerAuth extended key usage)")]
    NotServerAuth,
}

/// What an agent learns from the control plane's certificate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerLeaf {
    /// SHA-256 (hex) of the SubjectPublicKeyInfo.
    pub key_fingerprint: String,
}

/// Grace for clock differences when checking a certificate's validity.
pub const LEAF_CLOCK_SKEW: Duration = Duration::from_secs(300);

/// Reads the key fingerprint of a control-plane certificate and checks that
/// it is valid at `now` (with [`LEAF_CLOCK_SKEW`]) and meant for TLS
/// servers. The name and the issuer are not checked: the key is what is
/// pinned.
pub fn inspect_server_leaf(der: &[u8], now: Timestamp) -> Result<ServerLeaf, LeafError> {
    let (_, cert) = X509Certificate::from_der(der).map_err(|e| LeafError::Malformed(e.to_string()))?;
    let v = cert.validity();
    let (not_before, not_after) = (v.not_before.timestamp() * 1000, v.not_after.timestamp() * 1000);
    let skew = i64::try_from(LEAF_CLOCK_SKEW.as_millis()).unwrap_or(i64::MAX);
    if now.0 + skew < not_before || now.0 - skew > not_after {
        return Err(LeafError::Expired { not_before, not_after });
    }

    let server_auth = matches!(cert.extended_key_usage(), Ok(Some(eku)) if eku.value.server_auth);
    if !server_auth {
        return Err(LeafError::NotServerAuth);
    }

    Ok(ServerLeaf { key_fingerprint: sha256_hex(cert.public_key().raw) })
}

/// The key fingerprint of a PEM certificate (any purpose), or `None`.
#[must_use]
pub fn cert_key_fingerprint(pem: &str) -> Option<String> {
    let der = crate::ca::pem_to_der(pem).ok()?;
    let (_, cert) = X509Certificate::from_der(&der).ok()?;

    Some(sha256_hex(cert.public_key().raw))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // reason: tests
mod tests {
    use super::*;
    use crate::{AgentCsr, CertificateAuthority};

    const NOW: Timestamp = Timestamp(1_800_000_000_000);

    fn der(pem: &str) -> Vec<u8> {
        crate::ca::pem_to_der(pem).unwrap()
    }

    #[test]
    fn the_certificate_carries_the_pinned_key_across_renewals() {
        let ca = CertificateAuthority::generate("t", NOW).unwrap();
        let key = ControlPlaneKey::generate().unwrap();
        let a = ca.issue_server_certificate_for_key(&key, &["forge".into()], NOW).unwrap();
        let later = NOW.plus(Duration::from_secs(60 * 86400));
        let b = ca.issue_server_certificate_for_key(&key, &["other".into()], later).unwrap();
        assert_ne!(a.cert_pem, b.cert_pem);
        assert_eq!(inspect_server_leaf(&der(&a.cert_pem), NOW).unwrap().key_fingerprint, key.fingerprint());
        assert_eq!(inspect_server_leaf(&der(&b.cert_pem), later).unwrap().key_fingerprint, key.fingerprint());
        assert_eq!(cert_key_fingerprint(&a.cert_pem).as_deref(), Some(key.fingerprint()));

        // The sealed form round-trips to the same fingerprint.
        assert_eq!(ControlPlaneKey::from_pem(&key.key_pem()).unwrap().fingerprint(), key.fingerprint());
    }

    #[test]
    fn expired_and_client_certificates_are_refused() {
        let ca = CertificateAuthority::generate("t", NOW).unwrap();
        let key = ControlPlaneKey::generate().unwrap();
        let c = ca.issue_server_certificate_for_key(&key, &["forge".into()], NOW).unwrap();
        let after = NOW.plus(crate::SERVER_CERT_VALIDITY).plus(Duration::from_secs(3600));
        assert!(matches!(inspect_server_leaf(&der(&c.cert_pem), after), Err(LeafError::Expired { .. })));
        let before = NOW.minus(Duration::from_secs(3600));
        assert!(matches!(inspect_server_leaf(&der(&c.cert_pem), before), Err(LeafError::Expired { .. })));
        let client = ca.issue_client_certificate("forge-server", NOW, Duration::from_secs(3600)).unwrap();
        assert_eq!(inspect_server_leaf(&der(&client.cert_pem), NOW), Err(LeafError::NotServerAuth));
        let csr = AgentCsr::generate("n").unwrap();
        let agent = ca.issue_agent_certificate(&csr.csr_pem, "s", NOW, Duration::from_secs(60)).unwrap();
        assert_eq!(inspect_server_leaf(&der(&agent.cert_pem), NOW), Err(LeafError::NotServerAuth));
    }

    #[test]
    fn fingerprints_parse_and_shorten() {
        let fp = "AB".repeat(32);
        assert_eq!(parse_fingerprint(&fp), Some("ab".repeat(32)));
        assert_eq!(parse_fingerprint(&format!("sha256:{fp}")), Some("ab".repeat(32)));
        assert_eq!(parse_fingerprint("sha256:abc"), None);
        assert_eq!(parse_fingerprint(&"zz".repeat(32)), None);
        assert_eq!(short_fingerprint(&"ab".repeat(32)), "sha256:abababab…abab");
    }
}
