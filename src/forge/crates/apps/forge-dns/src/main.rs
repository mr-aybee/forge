//! `forge-dns`: the cluster DNS server (D-173).
//!
//! - Answers A, SRV and PTR for the zones in `names.json` from memory; AAAA
//!   for a known name is NOERROR without records; an unknown name in a zone
//!   is NXDOMAIN with the zone's SOA. A records rotate per query.
//! - Forwards every other name to the upstreams in order (1 s each), without
//!   caching.
//! - Polls the file every 2 s and swaps the map in whole; an invalid file
//!   keeps the previous map. Until a valid file is read, every query is
//!   REFUSED.
//! - Serves UDP (512 bytes, or the client's EDNS size) and TCP on every
//!   `--listen` address, and binds nothing else.

mod config;
mod forward;
mod handler;
mod watch;
mod zone;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use clap::Parser;
use hickory_server::Server;
use tokio::net::{TcpListener, UdpSocket};

use crate::config::{Args, RESOLV_CONF, default_upstreams};
use crate::handler::Handler;
use crate::watch::{Current, POLL_INTERVAL, Watcher};

/// Idle TCP connections are closed after this long.
const TCP_TIMEOUT: Duration = Duration::from_secs(5);
/// Responses buffered per TCP connection.
const TCP_BUFFER: usize = 32;

/// Binds UDP and TCP on every address and serves `handler` on them.
async fn bind(listen: &[SocketAddr], handler: Handler) -> anyhow::Result<Server<Handler>> {
    let mut server = Server::new(handler);

    for addr in listen {
        let udp = UdpSocket::bind(addr).await.with_context(|| format!("binding UDP {addr}"))?;
        let tcp = TcpListener::bind(addr).await.with_context(|| format!("binding TCP {addr}"))?;
        server.register_socket(udp);
        server.register_listener(tcp, TCP_TIMEOUT, TCP_BUFFER);
    }

    Ok(server)
}

async fn run(args: Args) -> anyhow::Result<()> {
    let upstreams = if args.upstream.is_empty() {
        let text = tokio::fs::read_to_string(RESOLV_CONF).await.with_context(|| format!("reading {RESOLV_CONF}"))?;

        default_upstreams(&text, &args.listen)
    } else {
        args.upstream.clone()
    };

    if upstreams.is_empty() {
        tracing::warn!("no upstream nameservers: names outside the zones get SERVFAIL");
    }

    let current = Arc::new(Current::default());
    let watcher = Watcher { path: args.file.clone(), ttl: args.ttl, current: current.clone(), interval: POLL_INTERVAL };
    tokio::spawn(watcher.run());

    let handler = Handler { current, upstreams: upstreams.clone().into() };
    let mut server = bind(&args.listen, handler).await?;
    tracing::info!(listen = ?args.listen, upstreams = ?upstreams, file = %args.file.display(), "forge-dns serving");

    tokio::select! {
        r = server.block_until_done() => r.context("serving DNS")?,
        r = tokio::signal::ctrl_c() => r.context("waiting for a signal")?,
    }

    Ok(())
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    let args = Args::parse();
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;

    runtime.block_on(run(args))
}

#[cfg(test)]
mod tests;
