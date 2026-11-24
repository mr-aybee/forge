//! Internal PKI for Orbyn Forge.
//!
//! * [`CertificateAuthority`] — self-signed root that issues the control
//!   plane's server certificate and short-lived agent client certificates.
//! * [`enrollment`] — one-time enrollment tokens that also pin the CA
//!   fingerprint (`<secret>.<ca-sha256>`), so a fresh agent can trust the
//!   control plane without a pre-installed root.
//! * [`PeerIdentity`] — parses an agent certificate presented over mTLS.
//! * [`ControlPlaneKey`] — the listener key agents pin (D-167).
//! * [`renewal_due`] — rotation policy (renew after ⅔ of the lifetime).
//!
//! Agent identity is never taken from the CSR: the CA overwrites the subject
//! with the server id assigned at enrollment.

mod ca;
pub mod enrollment;
mod identity;
pub mod server_key;

pub use ca::{AgentCsr, CertificateAuthority, IssuedCertificate, SERVER_CERT_VALIDITY, ServerCertificate};
pub use identity::PeerIdentity;
pub use server_key::{
    ControlPlaneKey, LeafError, ServerLeaf, inspect_server_leaf, parse_fingerprint, short_fingerprint,
};

use forge_core::Timestamp;

/// Agent certificate lifetime.
pub const AGENT_CERT_VALIDITY: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

/// DNS name placed in agent certificates.
#[must_use]
pub fn agent_dns_name(server_id: &str) -> String {
    format!("{server_id}.agents.forge.internal")
}

/// DNS name in a node's Docker daemon certificate (D-125); the control
/// plane verifies the daemon against it, whatever address it dials.
#[must_use]
pub fn docker_dns_name(server_id: &str) -> String {
    format!("{server_id}.docker.forge.internal")
}

/// SHA-256 (hex) of a PEM certificate's DER, or `None` when it is not PEM.
#[must_use]
pub fn cert_fingerprint(pem: &str) -> Option<String> {
    ca::pem_to_der(pem).ok().map(|der| forge_secrets::crypto::sha256_hex(&der))
}

#[derive(Debug, thiserror::Error)]
pub enum PkiError {
    #[error("certificate generation failed: {0}")]
    Generate(String),
    #[error("invalid CSR: {0}")]
    Csr(String),
    #[error("invalid certificate: {0}")]
    Parse(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// A certificate serial in hex as a peer presents it: lower case, without
/// the leading zero bytes DER drops (a `00` pair is kept only before a byte
/// of `80` or more, which needs it to stay positive). Serials issued before
/// D-145 were recorded with a leading `00` about once in 256; compare
/// serials through this.
#[must_use]
pub fn canonical_serial(hex: &str) -> String {
    let mut s = hex.trim().to_ascii_lowercase();
    while s.len() > 2 && s.starts_with("00") && s.as_bytes().get(2).is_some_and(|c| (b'0'..=b'7').contains(c)) {
        s.drain(..2);
    }
    s
}

/// Whether a certificate valid in `[not_before, not_after]` should be renewed at `now`.
#[must_use]
pub fn renewal_due(not_before: Timestamp, not_after: Timestamp, now: Timestamp) -> bool {
    let lifetime = not_after.0 - not_before.0;
    now.0 >= not_before.0 + lifetime * 2 / 3
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // reason: tests
mod tests {
    use super::*;

    #[test]
    fn serials_compare_in_their_der_form() {
        assert_eq!(canonical_serial("0021CB62"), "21cb62", "DER drops the zero byte");
        assert_eq!(canonical_serial("0080ab"), "0080ab", "kept: it keeps 80.. positive");
        assert_eq!(canonical_serial("00007f"), "7f");
        assert_eq!(canonical_serial("4a"), "4a");
        assert_eq!(canonical_serial("00"), "00");
    }

    #[test]
    fn issued_serials_are_the_ones_peers_present() {
        let now = Timestamp(1_800_000_000_000);
        let ca = CertificateAuthority::generate("t", now).unwrap();
        for _ in 0..1_000 {
            let csr = AgentCsr::generate("n").unwrap();
            let c = ca.issue_agent_certificate(&csr.csr_pem, "s", now, std::time::Duration::from_secs(60)).unwrap();
            let der = ca::pem_to_der(&c.cert_pem).unwrap();
            assert_eq!(PeerIdentity::from_der(&der).unwrap().serial_hex, c.serial_hex);
        }
    }

    #[test]
    fn renewal_at_two_thirds() {
        assert!(!renewal_due(Timestamp(0), Timestamp(300), Timestamp(199)));
        assert!(renewal_due(Timestamp(0), Timestamp(300), Timestamp(200)));
    }
}
