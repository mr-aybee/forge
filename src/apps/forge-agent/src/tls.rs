//! rustls client configurations for the agent link (ring provider only).

use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms};
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};
use rustls_pki_types::pem::PemObject;
use rustls_pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn roots(ca_pem: &str) -> anyhow::Result<RootCertStore> {
    let mut roots = RootCertStore::empty();
    for cert in CertificateDer::pem_slice_iter(ca_pem.as_bytes()) {
        roots.add(cert?)?;
    }
    Ok(roots)
}

/// Checks the CA download against the fingerprint pinned in an enrollment
/// token and returns a PEM holding only that one certificate.
///
/// The response must contain exactly one certificate. Anything appended to it
/// (a second CA, an intermediate) would otherwise become a trust root that the
/// pin never covered, so such a response is refused rather than filtered.
pub fn pinned_ca_pem(ca_pem: &str, fingerprint_hex: &str) -> anyhow::Result<String> {
    use base64::Engine as _;
    let certs = CertificateDer::pem_slice_iter(ca_pem.as_bytes()).collect::<Result<Vec<_>, _>>()?;
    let [ca] = certs.as_slice() else {
        anyhow::bail!("control plane returned {} CA certificates; exactly one is required", certs.len());
    };
    let actual = forge_secrets::crypto::sha256_hex(ca.as_ref());
    if !forge_secrets::crypto::constant_time_eq(actual.as_bytes(), fingerprint_hex.as_bytes()) {
        anyhow::bail!("control plane CA does not match the fingerprint in the enrollment token");
    }
    let b64 = base64::engine::general_purpose::STANDARD.encode(ca.as_ref());
    let mut pem = String::from("-----BEGIN CERTIFICATE-----\n");
    for chunk in b64.as_bytes().chunks(64) {
        pem.push_str(&String::from_utf8_lossy(chunk));
        pem.push('\n');
    }
    pem.push_str("-----END CERTIFICATE-----\n");
    Ok(pem)
}

/// Server-authenticated TLS without a client certificate (enrollment).
pub fn anonymous(ca_pem: &str) -> anyhow::Result<Arc<ClientConfig>> {
    let mut cfg = ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()?
        .with_root_certificates(roots(ca_pem)?)
        .with_no_client_auth();
    cfg.alpn_protocols = vec![b"h2".to_vec()];
    Ok(Arc::new(cfg))
}

/// Mutual TLS with the agent's certificate.
pub fn mutual(ca_pem: &str, cert_pem: &str, key_pem: &str) -> anyhow::Result<Arc<ClientConfig>> {
    let chain = CertificateDer::pem_slice_iter(cert_pem.as_bytes()).collect::<Result<Vec<_>, _>>()?;
    let key = PrivateKeyDer::from_pem_slice(key_pem.as_bytes())?;
    let mut cfg = ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()?
        .with_root_certificates(roots(ca_pem)?)
        .with_client_auth_cert(chain, key)?;
    cfg.alpn_protocols = vec![b"h2".to_vec()];
    Ok(Arc::new(cfg))
}

/// Used for exactly one request: downloading the CA certificate, whose
/// SHA-256 is then checked against the pin in the enrollment token. Handshake
/// signatures are still verified; only chain validation is skipped.
pub fn ca_bootstrap() -> anyhow::Result<Arc<ClientConfig>> {
    let p = provider();
    let verifier = Arc::new(PinnedLater { algorithms: p.signature_verification_algorithms });
    let mut cfg = ClientConfig::builder_with_provider(p)
        .with_safe_default_protocol_versions()?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    cfg.alpn_protocols = vec![b"h2".to_vec()];
    Ok(Arc::new(cfg))
}

/// TLS for an HTTPS health check (HTTP/1.1, no client certificate). With
/// `verify`, the certificate must chain to a root this machine trusts and
/// match the name; without, only the handshake signature is checked (for
/// workloads with a self-signed certificate; the policy says so explicitly).
pub fn health_probe(verify: bool) -> anyhow::Result<Arc<ClientConfig>> {
    let p = provider();
    let builder = ClientConfig::builder_with_provider(p.clone()).with_safe_default_protocol_versions()?;
    let cfg = if verify {
        let mut roots = RootCertStore::empty();
        let found = rustls_native_certs::load_native_certs();
        let (added, _ignored) = roots.add_parsable_certificates(found.certs);
        if added == 0 {
            anyhow::bail!("this machine has no trusted root certificates");
        }
        builder.with_root_certificates(roots).with_no_client_auth()
    } else {
        let verifier = Arc::new(PinnedLater { algorithms: p.signature_verification_algorithms });
        builder.dangerous().with_custom_certificate_verifier(verifier).with_no_client_auth()
    };
    Ok(Arc::new(cfg))
}

#[derive(Debug)]
struct PinnedLater {
    algorithms: WebPkiSupportedAlgorithms,
}

impl ServerCertVerifier for PinnedLater {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use forge_core::Timestamp;
    use forge_pki::CertificateAuthority;

    const NOW: Timestamp = Timestamp(1_800_000_000_000);

    #[test]
    fn pinned_ca_pem_accepts_exactly_the_pinned_certificate() {
        let ca = CertificateAuthority::generate("pinned", NOW).unwrap();
        let pem = pinned_ca_pem(ca.cert_pem(), ca.fingerprint()).unwrap();
        let certs = CertificateDer::pem_slice_iter(pem.as_bytes()).collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(certs.len(), 1);
        assert_eq!(forge_secrets::crypto::sha256_hex(certs[0].as_ref()), ca.fingerprint());
        assert!(anonymous(&pem).is_ok());
    }

    #[test]
    fn pinned_ca_pem_refuses_an_appended_attacker_ca() {
        let ca = CertificateAuthority::generate("pinned", NOW).unwrap();
        let evil = CertificateAuthority::generate("attacker", NOW).unwrap();
        let bundle = format!("{}{}", ca.cert_pem(), evil.cert_pem());
        let err = pinned_ca_pem(&bundle, ca.fingerprint()).unwrap_err();
        assert!(err.to_string().contains("exactly one"), "{err}");
        // Attacker CA first, real CA appended: also refused.
        let bundle = format!("{}{}", evil.cert_pem(), ca.cert_pem());
        assert!(pinned_ca_pem(&bundle, ca.fingerprint()).is_err());
    }

    #[test]
    fn pinned_ca_pem_refuses_a_wrong_or_missing_certificate() {
        let ca = CertificateAuthority::generate("pinned", NOW).unwrap();
        let evil = CertificateAuthority::generate("attacker", NOW).unwrap();
        assert!(pinned_ca_pem(evil.cert_pem(), ca.fingerprint()).is_err());
        assert!(pinned_ca_pem("", ca.fingerprint()).is_err());
    }
}
