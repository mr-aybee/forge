//! The control plane's side of a node's Docker TLS endpoint (D-125).
//!
//! Every connection:
//! * trusts only that node's own Docker CA (never the system roots or
//!   Forge's agent CA), so a certificate from another node cannot answer;
//! * checks the daemon's certificate against the node's fixed DNS name
//!   (`<server_id>.docker.forge.internal`), whatever address it dials;
//! * presents a short-lived client certificate from the same CA, which
//!   dockerd requires (`--tlsverify`);
//! * uses TLS 1.2+ through `ring`, the one crypto provider.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use rustls::ClientConfig;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use rustls_pki_types::pem::PemObject;
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;

/// How long a TCP connect plus TLS handshake may take.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Where and how to reach one node's daemon.
#[derive(Clone)]
pub struct Target {
    /// `host:port` to dial.
    pub address: String,
    /// The name the daemon's certificate must carry.
    pub server_name: String,
    tls: Arc<ClientConfig>,
}

impl std::fmt::Debug for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Target").field("address", &self.address).field("server_name", &self.server_name).finish()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("TLS setup: {0}")]
    Tls(String),
    #[error("cannot reach the Docker endpoint {address}: {reason}")]
    Connect { address: String, reason: String },
    #[error("the Docker endpoint at {0} did not answer in time")]
    Timeout(String),
    #[error("Docker API: {0}")]
    Http(String),
}

fn tls_err(e: impl std::fmt::Display) -> ClientError {
    ClientError::Tls(e.to_string())
}

impl Target {
    /// A target that trusts `ca_pem` only and authenticates with the client
    /// certificate `cert_pem`/`key_pem`.
    pub fn new(
        address: String,
        server_name: String,
        ca_pem: &str,
        cert_pem: &str,
        key_pem: &str,
    ) -> Result<Self, ClientError> {
        let mut roots = rustls::RootCertStore::empty();

        for c in CertificateDer::pem_slice_iter(ca_pem.as_bytes()) {
            roots.add(c.map_err(tls_err)?).map_err(tls_err)?;
        }

        let chain =
            CertificateDer::pem_slice_iter(cert_pem.as_bytes()).collect::<Result<Vec<_>, _>>().map_err(tls_err)?;
        let key = PrivateKeyDer::from_pem_slice(key_pem.as_bytes()).map_err(tls_err)?;
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let cfg = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(tls_err)?
            .with_root_certificates(roots)
            .with_client_auth_cert(chain, key)
            .map_err(tls_err)?;

        Ok(Self { address, server_name, tls: Arc::new(cfg) })
    }

    /// Opens one verified TLS connection.
    pub async fn connect(&self) -> Result<TlsStream<TcpStream>, ClientError> {
        let name = ServerName::try_from(self.server_name.clone()).map_err(tls_err)?;
        let address = self.address.clone();
        let run = async {
            let tcp = TcpStream::connect(&address)
                .await
                .map_err(|e| ClientError::Connect { address: address.clone(), reason: e.to_string() })?;

            // Interactive streams (attach, exec) should not wait on Nagle.
            let _ = tcp.set_nodelay(true);
            TlsConnector::from(self.tls.clone())
                .connect(name, tcp)
                .await
                .map_err(|e| ClientError::Connect { address: address.clone(), reason: e.to_string() })
        };

        tokio::time::timeout(CONNECT_TIMEOUT, run).await.map_err(|_| ClientError::Timeout(self.address.clone()))?
    }

    /// Sends one request on a new connection and returns the response with
    /// its body still streaming. Upgrades (attach, exec) are taken from the
    /// response with `hyper::upgrade::on`.
    pub async fn send<B>(&self, req: Request<B>) -> Result<Response<Incoming>, ClientError>
    where
        B: hyper::body::Body + Send + 'static,
        B::Data: Send,
        B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        let tls = self.connect().await?;
        let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(tls))
            .await
            .map_err(|e| ClientError::Http(e.to_string()))?;
        tokio::spawn(conn.with_upgrades());

        sender.send_request(req).await.map_err(|e| ClientError::Http(e.to_string()))
    }

    /// A small JSON call (status checks): `GET path`, body up to 1 MiB.
    pub async fn get_json(&self, path: &str, timeout: Duration) -> Result<(u16, serde_json::Value), ClientError> {
        let req = Request::get(format!("http://{}{path}", self.server_name))
            .header("Host", "docker")
            .body(Full::new(Bytes::new()))
            .map_err(|e| ClientError::Http(e.to_string()))?;
        let run = async {
            let resp = self.send(req).await?;
            let status = resp.status().as_u16();
            let body = http_body_util::Limited::new(resp.into_body(), 1 << 20)
                .collect()
                .await
                .map_err(|e| ClientError::Http(e.to_string()))?
                .to_bytes();

            // A non-JSON answer (an error page from something else on the
            // port) is reported as null; callers check the status first.
            let v = serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
            Ok((status, v))
        };

        tokio::time::timeout(timeout, run).await.map_err(|_| ClientError::Timeout(self.address.clone()))?
    }
}
