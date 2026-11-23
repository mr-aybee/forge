use std::sync::Arc;

use rustls::server::WebPkiClientVerifier;
use rustls::{RootCertStore, ServerConfig};
use rustls_pki_types::pem::PemObject;
use rustls_pki_types::{CertificateDer, PrivateKeyDer};
use tokio_rustls::TlsAcceptor;

use crate::state::AppState;

/// TLS for the agent listener. Client certificates are *optional* at the TLS
/// layer (enrollment has none yet) and *required* by the handlers that need them.
pub(super) fn acceptor(state: &AppState) -> anyhow::Result<TlsAcceptor> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut roots = RootCertStore::empty();
    for c in CertificateDer::pem_slice_iter(state.ca.cert_pem().as_bytes()) {
        roots.add(c?)?;
    }
    let verifier = WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider.clone())
        .allow_unauthenticated()
        .build()?;
    let server_cert = state.ca.issue_server_certificate(&state.config.agent_hostnames, state.clock.now())?;
    let mut chain = CertificateDer::pem_slice_iter(server_cert.cert_pem.as_bytes()).collect::<Result<Vec<_>, _>>()?;
    chain.extend(CertificateDer::pem_slice_iter(state.ca.cert_pem().as_bytes()).collect::<Result<Vec<_>, _>>()?);
    let key = PrivateKeyDer::from_pem_slice(server_cert.key_pem.as_bytes())?;
    let mut cfg = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .with_client_cert_verifier(verifier)
        .with_single_cert(chain, key)?;
    cfg.alpn_protocols = vec![b"h2".to_vec()];
    Ok(TlsAcceptor::from(Arc::new(cfg)))
}
