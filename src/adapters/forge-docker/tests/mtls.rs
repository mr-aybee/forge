//! The client against a fake dockerd that demands a client certificate.
#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use std::time::Duration;

use forge_core::Timestamp;
use forge_docker::Target;
use forge_pki::{AgentCsr, CertificateAuthority};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::WebPkiClientVerifier;
use rustls_pki_types::pem::PemObject;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const NOW: Timestamp = Timestamp(1_800_000_000_000);

fn now() -> Timestamp {
    Timestamp(
        i64::try_from(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis()).unwrap(),
    )
}

/// Starts a one-shot TLS server for `server_id` signed by `ca`; it answers
/// any request with `{"ApiVersion":"1.47"}`.
async fn fake_dockerd(ca: &CertificateAuthority, server_id: &str) -> String {
    let csr = AgentCsr::generate("x").unwrap();
    let ip = "127.0.0.1".parse().unwrap();
    let cert =
        ca.issue_docker_daemon_certificate(&csr.csr_pem, server_id, &[ip], now(), Duration::from_secs(3600)).unwrap();
    let chain = CertificateDer::pem_slice_iter(cert.cert_pem.as_bytes()).collect::<Result<Vec<_>, _>>().unwrap();
    let key = PrivateKeyDer::from_pem_slice(csr.key_pem.as_bytes()).unwrap();
    let mut roots = rustls::RootCertStore::empty();
    roots.add(CertificateDer::from_pem_slice(ca.cert_pem().as_bytes()).unwrap()).unwrap();
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider.clone()).build().unwrap();
    let cfg = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_client_cert_verifier(verifier)
        .with_single_cert(chain, key)
        .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(cfg));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    tokio::spawn(async move {
        while let Ok((tcp, _)) = listener.accept().await {
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(tcp).await else { return };
                let mut buf = vec![0u8; 4096];
                let _ = tls.read(&mut buf).await;
                let body = br#"{"ApiVersion":"1.47"}"#;
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
                    body.len()
                );
                let _ = tls.write_all(head.as_bytes()).await;
                let _ = tls.write_all(body).await;
                let _ = tls.shutdown().await;
            });
        }
    });

    addr
}

fn target(ca: &CertificateAuthority, addr: &str, server_id: &str) -> Target {
    let client = ca.issue_client_certificate("forge-server", now(), Duration::from_secs(3600)).unwrap();
    Target::new(addr.into(), forge_pki::docker_dns_name(server_id), ca.cert_pem(), &client.cert_pem, &client.key_pem)
        .unwrap()
}

#[tokio::test]
async fn talks_to_its_own_node_only() {
    let ca = CertificateAuthority::generate("Docker CA srv-a", NOW).unwrap();
    let addr = fake_dockerd(&ca, "srv-a").await;

    let (status, v) = target(&ca, &addr, "srv-a").get_json("/version", Duration::from_secs(5)).await.unwrap();
    assert_eq!(status, 200);
    assert_eq!(v["ApiVersion"], "1.47");

    // The daemon's certificate names srv-a: expecting srv-b fails.
    assert!(target(&ca, &addr, "srv-b").get_json("/version", Duration::from_secs(5)).await.is_err());

    // Another node's CA neither trusts this daemon nor is trusted by it.
    let other = CertificateAuthority::generate("Docker CA srv-b", NOW).unwrap();
    assert!(target(&other, &addr, "srv-a").get_json("/version", Duration::from_secs(5)).await.is_err());
}
