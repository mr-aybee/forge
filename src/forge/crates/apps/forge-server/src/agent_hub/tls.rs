use std::sync::Arc;

use rustls::server::WebPkiClientVerifier;
use rustls::{RootCertStore, ServerConfig};
use rustls_pki_types::CertificateDer;
use rustls_pki_types::pem::PemObject;
use tokio_rustls::TlsAcceptor;

use crate::state::AppState;

/// TLS for the agent listener. Client certificates are *optional* at the TLS
/// layer (enrollment has none yet) and *required* by the handlers that need them.
/// The server certificate comes from the control-plane key's resolver
/// (D-167): renewals and key switches take effect without a restart.
pub(super) fn acceptor(state: &AppState) -> anyhow::Result<TlsAcceptor> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut roots = RootCertStore::empty();

    for c in CertificateDer::pem_slice_iter(state.ca.cert_pem().as_bytes()) {
        roots.add(c?)?;
    }

    let verifier = WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider.clone())
        .allow_unauthenticated()
        .build()?;
    let mut cfg = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .with_client_cert_verifier(verifier)
        .with_cert_resolver(state.agent_keys.resolver());
    cfg.alpn_protocols = vec![b"h2".to_vec()];

    Ok(TlsAcceptor::from(Arc::new(cfg)))
}
