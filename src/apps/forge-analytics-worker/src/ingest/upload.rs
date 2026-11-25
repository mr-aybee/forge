//! The agent upload listener (D-142): agents post sealed spool files here
//! over mTLS, one file per request.
//!
//! - **Who:** the client certificate must chain to the Forge CA (verified
//!   by rustls exactly like the agent hub), name a node that forge-server
//!   lists as active in its pushed maps, and be that node's current
//!   certificate (its serial is in the maps, D-145). The file name must name
//!   the same node.
//! - **Load:** a fixed number of files are imported at once; one more is
//!   answered 429 at once and nothing is held. Bodies are capped.
//! - **Once:** a file id stored before (the agent lost the ack) is
//!   acknowledged without a second write. Ids are kept on disk
//!   ([`SeenLog`]), so this holds across a worker restart.
//! - **Ack:** `200` with a [`SpoolAck`] only after the store confirmed.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use bytes::Bytes;
use forge_core::analytics::FileWrite;
use forge_core::spool::{FILE_ID_HEADER, FileName, MAX_FILE_BYTES};
use forge_core::{Clock, ServerId};
use forge_pki::PeerIdentity;
use forge_proto::messages::SpoolAck;
use http::{Method, Request, Response, StatusCode};
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Incoming;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use prost::Message;
use rustls::server::WebPkiClientVerifier;
use rustls::{RootCertStore, ServerConfig};
use rustls_pki_types::pem::PemObject;
use rustls_pki_types::{CertificateDer, PrivateKeyDer};
use tokio::net::TcpListener;
use tokio::sync::{Semaphore, watch};
use tokio_rustls::TlsAcceptor;

use super::seen::SeenLog;
use super::{ImportError, Ingest};
use forge_analytics_proto::WorkerError;

/// Most agent connections held at once.
const MAX_CONNECTIONS: usize = 20_000;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// The listener's TLS: the worker's certificate, and client certificates
/// required and verified against the Forge CA.
pub fn acceptor(ca_pem: &str, cert_pem: &str, key_pem: &str) -> anyhow::Result<TlsAcceptor> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut roots = RootCertStore::empty();

    for c in CertificateDer::pem_slice_iter(ca_pem.as_bytes()) {
        roots.add(c?)?;
    }

    let verifier = WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider.clone()).build()?;
    let mut chain = CertificateDer::pem_slice_iter(cert_pem.as_bytes()).collect::<Result<Vec<_>, _>>()?;
    chain.extend(CertificateDer::pem_slice_iter(ca_pem.as_bytes()).collect::<Result<Vec<_>, _>>()?);
    let key = PrivateKeyDer::from_pem_slice(key_pem.as_bytes())?;
    let mut cfg = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .with_client_cert_verifier(verifier)
        .with_single_cert(chain, key)?;
    cfg.alpn_protocols = vec![b"h2".to_vec()];

    Ok(TlsAcceptor::from(Arc::new(cfg)))
}

/// The listener's TLS, replaceable while it runs (D-145): forge-server
/// sends a new certificate before the current one expires. Each new
/// connection takes the current one; open connections keep theirs.
pub struct ListenerTls {
    ca_pem: String,
    current: std::sync::RwLock<TlsAcceptor>,
}

impl std::fmt::Debug for ListenerTls {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ListenerTls").finish_non_exhaustive()
    }
}

impl ListenerTls {
    /// The listener's TLS from the Forge CA and the worker's certificate.
    pub fn new(ca_pem: &str, cert_pem: &str, key_pem: &str) -> anyhow::Result<Self> {
        Ok(Self { ca_pem: ca_pem.to_owned(), current: std::sync::RwLock::new(acceptor(ca_pem, cert_pem, key_pem)?) })
    }

    /// The acceptor for a new connection.
    #[must_use]
    pub fn acceptor(&self) -> TlsAcceptor {
        self.current.read().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Serves `cert_pem` and `key_pem` from the next connection on. A pair
    /// that does not load, or a certificate not valid at `now`, is refused
    /// and the current one stays.
    pub fn replace(&self, cert_pem: &str, key_pem: &str, now: forge_core::Timestamp) -> anyhow::Result<()> {
        let leaf = CertificateDer::pem_slice_iter(cert_pem.as_bytes())
            .next()
            .ok_or_else(|| anyhow::anyhow!("no certificate"))??;
        let id = PeerIdentity::from_der(leaf.as_ref())?;
        anyhow::ensure!(id.not_before <= now && now < id.not_after, "the certificate is not valid now");
        let next = acceptor(&self.ca_pem, cert_pem, key_pem)?;
        *self.current.write().unwrap_or_else(PoisonError::into_inner) = next;

        Ok(())
    }
}

/// Handles uploads.
#[derive(Debug)]
pub struct Uploads {
    ingest: Arc<Ingest>,
    seen: Arc<Mutex<SeenLog>>,
    slots: Arc<Semaphore>,
    clock: Arc<dyn Clock>,
}

fn reply(status: StatusCode, body: impl Into<Bytes>) -> Response<Full<Bytes>> {
    let mut r = Response::new(Full::new(body.into()));
    *r.status_mut() = status;

    r
}

fn refused(e: &WorkerError) -> Response<Full<Bytes>> {
    let status = match e {
        WorkerError::Busy => StatusCode::TOO_MANY_REQUESTS,
        _ => StatusCode::SERVICE_UNAVAILABLE,
    };

    reply(status, e.to_string())
}

impl Uploads {
    #[must_use]
    pub fn new(ingest: Arc<Ingest>, seen: SeenLog, max_uploads: usize, clock: Arc<dyn Clock>) -> Self {
        Self { ingest, seen: Arc::new(Mutex::new(seen)), slots: Arc::new(Semaphore::new(max_uploads.max(1))), clock }
    }

    /// Drops remembered ids of nodes no longer active (on each maps push).
    pub async fn prune(&self) {
        let (seen, active) = (self.seen.clone(), self.ingest.maps().get());
        let r = tokio::task::spawn_blocking(move || {
            seen.lock().unwrap_or_else(PoisonError::into_inner).compact(Some(&active.active))
        })
        .await;

        if let Ok(Err(e)) | Err(e) = r.map_err(std::io::Error::other) {
            tracing::warn!(error = %e, "compacting the spool id log failed");
        }
    }

    /// Answers one request from `peer` (the verified client certificate).
    pub async fn handle(&self, peer: Option<&PeerIdentity>, req: Request<Incoming>) -> Response<Full<Bytes>> {
        if req.uri().path() != forge_proto::paths::SPOOL {
            return reply(StatusCode::NOT_FOUND, "not found");
        }

        if req.method() != Method::POST {
            return reply(StatusCode::METHOD_NOT_ALLOWED, "POST only");
        }

        let Some((node, serial)) =
            peer.and_then(|p| Some((p.common_name.parse::<ServerId>().ok()?, p.serial_hex.as_str())))
        else {
            return reply(StatusCode::FORBIDDEN, "not an agent certificate");
        };

        // The node is active and this is its current certificate: one
        // replaced at renewal, or revoked, is refused (D-145).
        if !self.ingest.maps().get().admits(node, serial) {
            return reply(StatusCode::FORBIDDEN, "unknown or revoked node, or a replaced certificate");
        }

        let name = req.headers().get(FILE_ID_HEADER).and_then(|v| v.to_str().ok()).and_then(FileName::parse);
        let Some(name) = name else { return reply(StatusCode::BAD_REQUEST, "missing or malformed file id") };
        if name.node.to_string() != node.to_string() {
            return reply(StatusCode::FORBIDDEN, "the file belongs to another node");
        }

        let stem = name.stem();
        let ack = |rows: u64, duplicate: bool| {
            reply(StatusCode::OK, SpoolAck { file_id: stem.clone(), rows, duplicate }.encode_to_vec())
        };

        if self.seen.lock().unwrap_or_else(PoisonError::into_inner).contains(node, &name.id) {
            return ack(0, true);
        }

        // Over the import limit: refuse at once, hold nothing.
        let Ok(_slot) = self.slots.clone().try_acquire_owned() else {
            return reply(StatusCode::TOO_MANY_REQUESTS, "the analytics worker is busy");
        };

        let too_big = req
            .headers()
            .get(http::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok()?.parse::<u64>().ok())
            .is_some_and(|n| n > MAX_FILE_BYTES);

        if too_big {
            return reply(StatusCode::PAYLOAD_TOO_LARGE, "file too large");
        }

        let cap = usize::try_from(MAX_FILE_BYTES).unwrap_or(usize::MAX);
        let body = match Limited::new(req.into_body(), cap).collect().await {
            Ok(b) => b.to_bytes(),
            Err(e) if e.downcast_ref::<http_body_util::LengthLimitError>().is_some() => {
                return reply(StatusCode::PAYLOAD_TOO_LARGE, "file too large");
            }
            Err(e) => return reply(StatusCode::BAD_REQUEST, format!("reading the body: {e}")),
        };

        let imported = match self.ingest.spool_file(node, &name.id, self.clock.now(), body).await {
            Ok(i) => i,
            Err(ImportError::Refused(e)) => return refused(&e),
            Err(ImportError::File(e)) => {
                tracing::warn!(node = %node, file = %stem, error = %e, "unreadable spool file refused");
                return reply(StatusCode::UNPROCESSABLE_ENTITY, e.to_string());
            }
        };

        if imported.skipped > 0 {
            tracing::debug!(node = %node, file = %stem, skipped = imported.skipped, "spool rows skipped");
        }

        let (seen, id) = (self.seen.clone(), name.id.clone());
        let r =
            tokio::task::spawn_blocking(move || seen.lock().unwrap_or_else(PoisonError::into_inner).insert(node, &id))
                .await;

        if let Ok(Err(e)) | Err(e) = r.map_err(std::io::Error::other) {
            // Stored but not remembered here: a store without file ids
            // would store a resend again (D-142); DuckDB still skips it.
            tracing::warn!(error = %e, "recording a spool file id failed");
        }

        match imported.write {
            FileWrite::Stored => ack(imported.rows, false),
            FileWrite::AlreadyStored => ack(0, true),
        }
    }
}

/// Serves `listener` until `shutdown`.
pub async fn serve(
    listener: TcpListener,
    tls: Arc<ListenerTls>,
    uploads: Arc<Uploads>,
    mut shutdown: watch::Receiver<bool>,
) {
    let conns = Arc::new(Semaphore::new(MAX_CONNECTIONS));

    if let Ok(a) = listener.local_addr() {
        tracing::info!(addr = %a, "agent upload listener (mTLS, HTTP/2)");
    }

    loop {
        let (tcp, addr): (_, SocketAddr) = tokio::select! {
            r = listener.accept() => match r {
                Ok(v) => v,
                Err(e) => {
                    tracing::debug!(error = %e, "accept failed");
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    continue;
                }
            },
            _ = shutdown.changed() => return,
        };
        let Ok(permit) = conns.clone().try_acquire_owned() else { continue };
        let (acceptor, uploads, mut stop) = (tls.acceptor(), uploads.clone(), shutdown.clone());
        tokio::spawn(async move {
            let _permit = permit;

            let Ok(Ok(tls)) = tokio::time::timeout(HANDSHAKE_TIMEOUT, acceptor.accept(tcp)).await else {
                tracing::debug!(%addr, "agent TLS handshake failed");
                return;
            };

            let peer = tls
                .get_ref()
                .1
                .peer_certificates()
                .and_then(|c| c.first())
                .and_then(|c| PeerIdentity::from_der(c.as_ref()).ok());
            let peer = Arc::new(peer);
            let svc = hyper::service::service_fn(move |req| {
                let (uploads, peer) = (uploads.clone(), peer.clone());
                async move { Ok::<_, Infallible>(uploads.handle(peer.as_ref().as_ref(), req).await) }
            });
            let conn = hyper::server::conn::http2::Builder::new(TokioExecutor::new())
                .timer(TokioTimer::new())
                .max_concurrent_streams(4)
                .initial_stream_window_size(1024 * 1024)
                .initial_connection_window_size(1024 * 1024)
                .keep_alive_interval(Duration::from_secs(60))
                .serve_connection(TokioIo::new(tls), svc);
            tokio::select! {
                r = conn => if let Err(e) = r { tracing::debug!(%addr, error = %e, "agent upload connection ended") },
                _ = stop.changed() => {}
            }
        });
    }
}
