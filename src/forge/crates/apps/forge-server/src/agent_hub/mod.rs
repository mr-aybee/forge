//! Agent hub: the mTLS HTTP/2 listener agents dial into.
//!
//! * `GET  /agent/v1/ca`        — CA PEM (public; the agent checks the pin)
//! * `POST /agent/v1/enroll`    — token + CSR → certificate (no client cert yet)
//! * `POST /agent/v1/heartbeat` — long-poll; carries results, returns commands
//! * `POST /agent/v1/renew`     — new CSR → new certificate
//!
//! The authenticated agent identity comes only from the verified client
//! certificate, never from the request body.

mod handlers;
pub(crate) use handlers::authenticated_server;
pub mod queue;
mod tls;

use std::net::SocketAddr;

use axum::Router;
use axum::extract::{DefaultBodyLimit, Extension};
use axum::routing::{get, post};
use forge_pki::PeerIdentity;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use hyper_util::service::TowerToHyperService;
use tokio::net::TcpListener;
use tokio::sync::watch;

pub use queue::CommandQueue;

/// Whether `server`'s agent asked this process for commands recently enough
/// (two long-poll holds and a margin) to pick up a new command now. The
/// stored node status cannot say that: it outlives a restart and is set by
/// hand in tests.
#[must_use]
pub fn linked(state: &AppState, server: forge_core::ServerId) -> bool {
    let hold = std::time::Duration::from_secs(u64::from(state.config.heartbeat_hold_secs));
    state.hub.polled_within(server, hold * 2 + std::time::Duration::from_secs(10))
}

use crate::state::AppState;

/// The verified client certificate of the connection, if any.
#[derive(Debug, Clone)]
pub struct AgentPeer(pub Option<PeerIdentity>);

fn router(state: AppState) -> Router {
    Router::new()
        .route(forge_proto::paths::CA, get(handlers::ca))
        .route(forge_proto::paths::ENROLL, post(handlers::enroll))
        .route(forge_proto::paths::HEARTBEAT, post(handlers::heartbeat))
        .route(forge_proto::paths::RENEW, post(handlers::renew))
        .route(forge_proto::paths::HEALTH_SECRETS, post(handlers::health_secrets))
        // Port tunnel data streams (D-091): unbounded, they stream for as long as
        // the connection lasts.
        .route("/agent/v1/tunnel/{connection}", post(crate::tunnels::hub_stream).layer(DefaultBodyLimit::disable()))
        // Container shell streams (D-114): unbounded like tunnels.
        .route("/agent/v1/exec/{session}", post(crate::container_exec::hub_stream).layer(DefaultBodyLimit::disable()))
        // Heartbeats carry results: a batch of file reads is several 350 KB answers.
        .layer(DefaultBodyLimit::max(8 * 1024 * 1024))
        .with_state(state)
}

/// TCP address of the connected agent.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PeerAddr(pub SocketAddr);

/// Binds the agent listener and serves until `shutdown`.
pub async fn serve(state: AppState, listener: TcpListener, mut shutdown: watch::Receiver<bool>) -> anyhow::Result<()> {
    let acceptor = tls::acceptor(&state)?;
    let app = router(state);
    tracing::info!(addr = %listener.local_addr()?, "agent hub listening (mTLS, HTTP/2)");

    loop {
        let (tcp, addr): (_, SocketAddr) = tokio::select! {
            r = listener.accept() => match r {
                Ok(v) => v,
                Err(e) => { tracing::warn!(error = %e, "accept failed"); continue; }
            },
            _ = shutdown.changed() => break,
        };
        let acceptor = acceptor.clone();
        let app = app.clone();
        tokio::spawn(async move {
            let _ = tcp.set_nodelay(true);

            let tls = match tokio::time::timeout(std::time::Duration::from_secs(10), acceptor.accept(tcp)).await {
                Ok(Ok(s)) => s,
                Ok(Err(e)) => {
                    tracing::debug!(%addr, error = %e, "TLS handshake failed");
                    return;
                }
                Err(_) => return,
            };

            let peer = tls
                .get_ref()
                .1
                .peer_certificates()
                .and_then(|c| c.first())
                .and_then(|c| PeerIdentity::from_der(c.as_ref()).ok());
            let svc = TowerToHyperService::new(app.layer(Extension(AgentPeer(peer))).layer(Extension(PeerAddr(addr))));
            let conn = hyper::server::conn::http2::Builder::new(TokioExecutor::new())
                .timer(TokioTimer::new())
                .max_concurrent_streams(16)
                // Tunnel streams need large windows for throughput (D-091).
                .initial_stream_window_size(2 * 1024 * 1024)
                .initial_connection_window_size(4 * 1024 * 1024)
                .keep_alive_interval(std::time::Duration::from_secs(60))
                .serve_connection(TokioIo::new(tls), svc);

            if let Err(e) = conn.await {
                tracing::debug!(%addr, error = %e, "agent connection ended");
            }
        });
    }

    Ok(())
}
