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

/// Server-authenticated TLS without a client certificate, accepting the
/// control plane by its pinned key (enrollment, `pins fetch` checks).
pub fn anonymous_pinned(pins: &[String], endpoint: &str) -> anyhow::Result<Arc<ClientConfig>> {
    let p = provider();
    let verifier = Arc::new(PinnedKeyVerifier::new(pins, endpoint, p.signature_verification_algorithms));
    let mut cfg = ClientConfig::builder_with_provider(p)
        .with_safe_default_protocol_versions()?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    cfg.alpn_protocols = vec![b"h2".to_vec()];

    Ok(Arc::new(cfg))
}

/// Mutual TLS with the agent's certificate, accepting the control plane by
/// its pinned key (D-167): any address of the control plane works.
pub fn mutual_pinned(
    pins: &[String],
    endpoint: &str,
    cert_pem: &str,
    key_pem: &str,
) -> anyhow::Result<Arc<ClientConfig>> {
    let chain = CertificateDer::pem_slice_iter(cert_pem.as_bytes()).collect::<Result<Vec<_>, _>>()?;
    let key = PrivateKeyDer::from_pem_slice(key_pem.as_bytes())?;
    let p = provider();
    let verifier = Arc::new(PinnedKeyVerifier::new(pins, endpoint, p.signature_verification_algorithms));
    let mut cfg = ClientConfig::builder_with_provider(p)
        .with_safe_default_protocol_versions()?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_client_auth_cert(chain, key)?;
    cfg.alpn_protocols = vec![b"h2".to_vec()];

    Ok(Arc::new(cfg))
}

/// The agent's TLS to the control plane: by pinned key when the node has
/// pins (D-167), else the Forge CA and the host name, as enrolled before.
pub fn for_agent(
    pins: &[String],
    endpoint: &str,
    ca_pem: &str,
    cert_pem: &str,
    key_pem: &str,
) -> anyhow::Result<Arc<ClientConfig>> {
    if pins.is_empty() { mutual(ca_pem, cert_pem, key_pem) } else { mutual_pinned(pins, endpoint, cert_pem, key_pem) }
}

/// Accepts the control plane only when its certificate's key is pinned
/// (D-167): the SHA-256 of the leaf's SubjectPublicKeyInfo is in the set,
/// the certificate is valid now and meant for TLS servers. The name and
/// the issuer are not checked, so a node reaches Forge by any address; a
/// certificate from the Forge CA for another key (a node's Docker daemon)
/// is refused like any other. Handshake signatures are verified, proving
/// the peer holds the pinned key.
#[derive(Debug)]
pub struct PinnedKeyVerifier {
    pins: Vec<String>,
    /// `host:port`, for the mismatch message.
    endpoint: String,
    algorithms: WebPkiSupportedAlgorithms,
}

impl PinnedKeyVerifier {
    #[must_use]
    pub fn new(pins: &[String], endpoint: &str, algorithms: WebPkiSupportedAlgorithms) -> Self {
        Self { pins: pins.to_vec(), endpoint: endpoint.to_owned(), algorithms }
    }
}

fn unix_ms(now: UnixTime) -> forge_core::Timestamp {
    forge_core::Timestamp(i64::try_from(now.as_secs()).unwrap_or(i64::MAX).saturating_mul(1000))
}

impl ServerCertVerifier for PinnedKeyVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let leaf = forge_pki::inspect_server_leaf(end_entity.as_ref(), unix_ms(now))
            .map_err(|e| rustls::Error::General(format!("control plane certificate refused: {e}")))?;
        let pinned = self
            .pins
            .iter()
            .any(|p| forge_secrets::crypto::constant_time_eq(p.as_bytes(), leaf.key_fingerprint.as_bytes()));

        if !pinned {
            return Err(rustls::Error::General(crate::pins::mismatch_message(
                &leaf.key_fingerprint,
                &self.pins,
                &self.endpoint,
            )));
        }

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

/// The key fingerprint the server at `host:port` presents, without
/// trusting it (`forge-agent pins fetch`; the operator confirms it). The
/// handshake signature is still checked, so the peer holds that key.
pub async fn presented_key(host: &str, port: u16) -> anyhow::Result<String> {
    #[derive(Debug)]
    struct Capture(WebPkiSupportedAlgorithms, std::sync::Mutex<Option<String>>);
    impl ServerCertVerifier for Capture {
        fn verify_server_cert(
            &self,
            end_entity: &CertificateDer<'_>,
            _i: &[CertificateDer<'_>],
            _n: &ServerName<'_>,
            _o: &[u8],
            now: UnixTime,
        ) -> Result<ServerCertVerified, rustls::Error> {
            let leaf = forge_pki::inspect_server_leaf(end_entity.as_ref(), unix_ms(now))
                .map_err(|e| rustls::Error::General(format!("control plane certificate refused: {e}")))?;
            *self.1.lock().map_err(|_| rustls::Error::General("lock".into()))? = Some(leaf.key_fingerprint);

            Ok(ServerCertVerified::assertion())
        }
        fn verify_tls12_signature(
            &self,
            m: &[u8],
            c: &CertificateDer<'_>,
            d: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            rustls::crypto::verify_tls12_signature(m, c, d, &self.0)
        }
        fn verify_tls13_signature(
            &self,
            m: &[u8],
            c: &CertificateDer<'_>,
            d: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            rustls::crypto::verify_tls13_signature(m, c, d, &self.0)
        }
        fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
            self.0.supported_schemes()
        }
    }
    let p = provider();
    let capture = Arc::new(Capture(p.signature_verification_algorithms, std::sync::Mutex::new(None)));
    let cfg = ClientConfig::builder_with_provider(p)
        .with_safe_default_protocol_versions()?
        .dangerous()
        .with_custom_certificate_verifier(capture.clone())
        .with_no_client_auth();
    let tcp = tokio::time::timeout(std::time::Duration::from_secs(15), tokio::net::TcpStream::connect((host, port)))
        .await
        .map_err(|_| anyhow::anyhow!("connecting to {host}:{port} timed out"))??;
    let name = ServerName::try_from(host.to_owned())?;
    let _tls = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        tokio_rustls::TlsConnector::from(Arc::new(cfg)).connect(name, tcp),
    )
    .await
    .map_err(|_| anyhow::anyhow!("the TLS handshake with {host}:{port} timed out"))??;
    let fp = capture.1.lock().map_err(|_| anyhow::anyhow!("lock"))?.clone();

    fp.ok_or_else(|| anyhow::anyhow!("{host}:{port} presented no certificate"))
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
    use forge_core::Timestamp;
    use forge_pki::CertificateAuthority;

    use super::*;

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

    /// Completes a TLS handshake against a server presenting `cert_pem`
    /// (signed by `ca_pem`) with `key_pem`, as an agent pinning `pins`.
    async fn handshake(cert_pem: &str, key_pem: &str, ca_pem: &str, pins: &[String]) -> Result<(), String> {
        let mut chain = CertificateDer::pem_slice_iter(cert_pem.as_bytes()).collect::<Result<Vec<_>, _>>().unwrap();
        chain.extend(CertificateDer::pem_slice_iter(ca_pem.as_bytes()).map(Result::unwrap));
        let key = PrivateKeyDer::from_pem_slice(key_pem.as_bytes()).unwrap();
        let server = rustls::ServerConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(chain, key)
            .unwrap();
        let (a, b) = tokio::io::duplex(64 * 1024);
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(server));
        let srv = tokio::spawn(async move { acceptor.accept(b).await.map(|_| ()) });

        // An address the certificate does not name: no name is checked.
        let name = ServerName::try_from("198.51.100.7").unwrap();
        let r = tokio_rustls::TlsConnector::from(anonymous_pinned(pins, "198.51.100.7:3843").unwrap())
            .connect(name, a)
            .await
            .map(|_| ())
            .map_err(|e| e.to_string());
        let _ = srv.await;

        r
    }

    fn real_now() -> Timestamp {
        let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis();
        Timestamp(i64::try_from(ms).unwrap())
    }

    #[tokio::test]
    async fn the_pinned_key_is_accepted_under_any_name_and_renewed_certificate() {
        let now = real_now();
        let ca = CertificateAuthority::generate("forge", now).unwrap();
        let key = forge_pki::ControlPlaneKey::generate().unwrap();
        let pins = vec![key.fingerprint().to_owned()];
        let first = ca.issue_server_certificate_for_key(&key, &["forge.example".into()], now).unwrap();
        handshake(&first.cert_pem, &first.key_pem, ca.cert_pem(), &pins).await.unwrap();

        // Renewed a day later with the same key: still accepted.
        let renewed =
            ca.issue_server_certificate_for_key(&key, &[], now.minus(std::time::Duration::from_secs(86400))).unwrap();
        assert_ne!(renewed.cert_pem, first.cert_pem);
        handshake(&renewed.cert_pem, &renewed.key_pem, ca.cert_pem(), &pins).await.unwrap();

        // An announced next key works once the server switches to it.
        let next = forge_pki::ControlPlaneKey::generate().unwrap();
        let switched = ca.issue_server_certificate_for_key(&next, &["forge.example".into()], now).unwrap();
        assert!(handshake(&switched.cert_pem, &switched.key_pem, ca.cert_pem(), &pins).await.is_err());
        let both = vec![key.fingerprint().to_owned(), next.fingerprint().to_owned()];
        handshake(&switched.cert_pem, &switched.key_pem, ca.cert_pem(), &both).await.unwrap();
    }

    #[tokio::test]
    async fn other_keys_expired_and_non_server_certificates_are_refused() {
        let now = real_now();
        let ca = CertificateAuthority::generate("forge", now).unwrap();
        let key = forge_pki::ControlPlaneKey::generate().unwrap();
        let pins = vec![key.fingerprint().to_owned()];

        // A node's Docker daemon certificate from the same Forge CA: a valid
        // ServerAuth certificate for another key.
        let csr = forge_pki::AgentCsr::generate("n").unwrap();
        let ip: std::net::IpAddr = "198.51.100.7".parse().unwrap();
        let docker = ca
            .issue_docker_daemon_certificate(&csr.csr_pem, "srv-1", &[ip], now, std::time::Duration::from_secs(86400))
            .unwrap();
        let e = handshake(&docker.cert_pem, &csr.key_pem, ca.cert_pem(), &pins).await.unwrap_err();
        assert!(e.contains("is not pinned") && e.contains("198.51.100.7:3843"), "{e}");

        // Another CA altogether.
        let foreign = CertificateAuthority::generate("evil", now).unwrap();
        let f = foreign.issue_server_certificate(&["198.51.100.7".into()], now).unwrap();
        assert!(
            handshake(&f.cert_pem, &f.key_pem, foreign.cert_pem(), &pins).await.unwrap_err().contains("is not pinned")
        );

        // The pinned key, but an expired certificate.
        let old = ca
            .issue_server_certificate_for_key(&key, &[], now.minus(std::time::Duration::from_secs(200 * 86400)))
            .unwrap();
        let e = handshake(&old.cert_pem, &old.key_pem, ca.cert_pem(), &pins).await.unwrap_err();
        assert!(e.contains("not valid at this time"), "{e}");

        // A client certificate for a pinned key is not a server certificate.
        let client = ca.issue_client_certificate("forge-server", now, std::time::Duration::from_secs(3600)).unwrap();
        let client_pins = vec![forge_pki::server_key::cert_key_fingerprint(&client.cert_pem).unwrap()];
        let e = handshake(&client.cert_pem, &client.key_pem, ca.cert_pem(), &client_pins).await.unwrap_err();
        assert!(e.contains("ServerAuth"), "{e}");
    }
}
