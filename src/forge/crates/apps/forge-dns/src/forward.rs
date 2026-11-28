//! Forwarding of names outside the zones: the query goes to the first
//! upstream, then the next on a 1 s timeout. No caching.

use std::net::SocketAddr;
use std::time::Duration;

use hickory_proto::op::Message;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};
use tokio::time::timeout;

/// How long one upstream gets before the next is tried.
pub const UPSTREAM_TIMEOUT: Duration = Duration::from_secs(1);

/// Largest UDP reply read from an upstream.
const MAX_UDP: usize = 65_535;

/// Why one upstream gave no answer.
#[derive(Debug, thiserror::Error)]
pub enum ForwardError {
    #[error("i/o: {0}")]
    Io(#[from] std::io::Error),
    #[error("timed out")]
    Timeout,
    #[error("undecodable reply: {0}")]
    Decode(#[from] hickory_proto::serialize::binary::DecodeError),
    #[error("query too large for TCP")]
    TooLarge,
}

/// Sends the raw query to each upstream in order and returns the first reply.
pub async fn forward(upstreams: &[SocketAddr], query: &[u8]) -> Option<Message> {
    for upstream in upstreams {
        match timeout(UPSTREAM_TIMEOUT, ask(*upstream, query)).await {
            Ok(Ok(reply)) => return Some(reply),
            Ok(Err(e)) => tracing::debug!(%upstream, error = %e, "upstream failed"),
            Err(_) => tracing::debug!(%upstream, error = %ForwardError::Timeout, "upstream failed"),
        }
    }

    None
}

/// One upstream: UDP first, TCP when the UDP reply is truncated.
async fn ask(upstream: SocketAddr, query: &[u8]) -> Result<Message, ForwardError> {
    let reply = ask_udp(upstream, query).await?;

    if !reply.metadata.truncation {
        return Ok(reply);
    }

    ask_tcp(upstream, query).await
}

async fn ask_udp(upstream: SocketAddr, query: &[u8]) -> Result<Message, ForwardError> {
    let bind: SocketAddr = if upstream.is_ipv4() { ([0, 0, 0, 0], 0).into() } else { ([0u16; 8], 0).into() };
    let socket = UdpSocket::bind(bind).await?;
    socket.connect(upstream).await?;
    socket.send(query).await?;

    let id = query.get(..2);
    let mut buf = vec![0u8; MAX_UDP];

    // A connected socket only sees the upstream; skip stray replies to other ids.
    loop {
        let n = socket.recv(&mut buf).await?;

        if buf.get(..2) == id {
            return Ok(Message::from_vec(&buf[..n])?);
        }
    }
}

async fn ask_tcp(upstream: SocketAddr, query: &[u8]) -> Result<Message, ForwardError> {
    let len = u16::try_from(query.len()).map_err(|_| ForwardError::TooLarge)?;
    let mut stream = TcpStream::connect(upstream).await?;
    let mut framed = Vec::with_capacity(query.len() + 2);
    framed.extend_from_slice(&len.to_be_bytes());
    framed.extend_from_slice(query);
    stream.write_all(&framed).await?;

    let reply_len = stream.read_u16().await?;
    let mut buf = vec![0u8; usize::from(reply_len)];
    stream.read_exact(&mut buf).await?;

    Ok(Message::from_vec(&buf)?)
}
