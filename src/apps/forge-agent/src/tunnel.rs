//! Port tunnels, agent side (D-091, D-100).
//!
//! A signed `TunnelOpen` asks the agent to connect one tunnel connection:
//!
//! 1. Check the command: the port is in the signed allow-list, the
//!    connection id and token are well formed, it has not expired, the dial
//!    address is this node's own, and this node is under [`MAX_CONNECTIONS`].
//! 2. Dial `<dial_ip>:<port>`: `127.0.0.1` / `[::1]` when the command names
//!    no address, else one of this node's own interface addresses (a
//!    service bound to the cluster network address). Never a host name or a
//!    foreign address, so a tunnel cannot pivot elsewhere.
//! 3. Open a separate HTTP/2 connection to the control plane with the
//!    agent's mTLS identity and post a streaming request to
//!    `/agent/v1/tunnel/<connection_id>` with the single-use token. The
//!    response body streams the other way: one full-duplex stream, dialled
//!    from the node (works behind NAT).
//!
//! Bytes flow raw. Bounded channels and the HTTP/2 windows give
//! backpressure end to end. The connection ends when either side closes,
//! after `idle_secs` without traffic, at `expires_at_ms`, or when the
//! control plane drops the stream (revocation).

use std::convert::Infallible;
use std::net::IpAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use forge_proto::messages::TunnelOpen;
use http_body_util::BodyExt;
use hyper::body::{Body, Frame, SizeHint};
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use rustls::ClientConfig;
use rustls_pki_types::ServerName;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;

use crate::link::Endpoint;

/// Open tunnel connections on this node at most.
pub const MAX_CONNECTIONS: usize = 32;
/// HTTP/2 window for a tunnel stream: enough for tens of MB/s on a WAN.
const WINDOW: u32 = 2 * 1024 * 1024;
/// Bytes read from the local socket per frame.
const CHUNK: usize = 64 * 1024;
/// Frames queued towards the control plane before reading pauses.
const QUEUE: usize = 8;
const DIAL_TIMEOUT: Duration = Duration::from_secs(5);
const STREAM_TIMEOUT: Duration = Duration::from_secs(10);
/// The header carrying the stream token.
pub const TOKEN_HEADER: &str = "x-forge-tunnel-token";

static ACTIVE: AtomicUsize = AtomicUsize::new(0);

/// Holds one of the node's tunnel slots.
struct Slot;

impl Slot {
    fn take() -> Result<Self, String> {
        if ACTIVE.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
            ACTIVE.fetch_sub(1, Ordering::SeqCst);
            return Err(format!("this node already has {MAX_CONNECTIONS} tunnel connections"));
        }
        Ok(Self)
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        ACTIVE.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Where a connection dials: loopback (IPv4, then IPv6), or one address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dial {
    Loopback,
    Addr(IpAddr),
}

/// The addresses of this node's interfaces (loopback included).
#[must_use]
pub fn local_addresses() -> Vec<IpAddr> {
    sysinfo::Networks::new_with_refreshed_list()
        .values()
        .flat_map(|data| data.ip_networks().iter().map(|n| n.addr))
        .collect()
}

/// Checks a tunnel command against this node's own addresses; returns the
/// port and where to dial it.
pub fn validate(t: &TunnelOpen, now_ms: i64, local: &[IpAddr]) -> Result<(u16, Dial), String> {
    let port = u16::try_from(t.port).ok().filter(|p| *p > 0).ok_or("invalid port")?;
    let dial = if t.dial_ip.is_empty() {
        Dial::Loopback
    } else {
        let ip: IpAddr = t.dial_ip.parse().map_err(|_| format!("invalid dial address {:?}", t.dial_ip))?;
        let ip = ip.to_canonical();
        if ip.is_loopback() || ip.is_unspecified() {
            Dial::Loopback
        } else if local.iter().any(|l| l.to_canonical() == ip) {
            Dial::Addr(ip)
        } else {
            return Err(format!("{ip} is not an address of this node; tunnels dial the node itself only"));
        }
    };
    if !t.allowed_ports.contains(&t.port) {
        return Err(format!("port {port} is not allowed for tunnels on this node"));
    }
    let hex32 = |s: &str, n: usize| s.len() == n && s.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase());
    if !hex32(&t.connection_id, 32) {
        return Err("invalid connection id".into());
    }
    if !hex32(&t.stream_token, 64) {
        return Err("invalid stream token".into());
    }
    if !(1..=86_400).contains(&t.idle_secs) {
        return Err("invalid idle timeout".into());
    }
    if t.expires_at_ms <= now_ms {
        return Err("the tunnel connection has expired".into());
    }
    Ok((port, dial))
}

/// A request body fed from a channel.
pub(crate) struct ChannelBody(mpsc::Receiver<Bytes>);

impl Body for ChannelBody {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Bytes>, Infallible>>> {
        self.0.poll_recv(cx).map(|o| o.map(|b| Ok(Frame::data(b))))
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::default()
    }
}

/// Dials the local port: the given address, or IPv4 loopback, then IPv6
/// loopback.
async fn dial(port: u16, where_: Dial) -> Result<TcpStream, String> {
    let try_one = |addr: IpAddr| async move {
        tokio::time::timeout(DIAL_TIMEOUT, TcpStream::connect((addr, port)))
            .await
            .map_err(|_| "timed out".to_owned())?
            .map_err(|e| e.to_string())
    };
    match where_ {
        Dial::Addr(ip) => try_one(ip).await.map_err(|e| format!("nothing answers on {ip}:{port} of this node ({e})")),
        Dial::Loopback => match try_one(IpAddr::from([127, 0, 0, 1])).await {
            Ok(s) => Ok(s),
            Err(v4) => try_one(IpAddr::from([0, 0, 0, 0, 0, 0, 0, 1]))
                .await
                .map_err(|_| format!("nothing answers on port {port} of this node ({v4})")),
        },
    }
}

pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// Opens one tunnel connection and relays it in the background. Returns once
/// the local port answered and the control plane accepted the stream.
pub async fn open(t: TunnelOpen, endpoint: Endpoint, tls: Arc<ClientConfig>) -> Result<Value, String> {
    let (port, where_) = validate(&t, now_ms(), &local_addresses())?;
    let slot = Slot::take()?;
    let sock = dial(port, where_).await?;
    let _ = sock.set_nodelay(true);
    let path = format!("{}{}", forge_proto::paths::TUNNEL, t.connection_id);
    let ControlStream { up: up_tx, mut down, sender, conn_task } =
        dial_control(&endpoint, tls, &path, TOKEN_HEADER, &t.stream_token).await?;

    let idle = Duration::from_secs(u64::from(t.idle_secs));
    let deadline_ms = t.expires_at_ms;
    let last = Arc::new(AtomicI64::new(now_ms()));
    let (mut rd, mut wr) = sock.into_split();
    let up_last = last.clone();
    tokio::spawn(async move {
        let _slot = slot;
        let _sender = sender;
        // Local socket → control plane.
        let up = async {
            let mut buf = vec![0u8; CHUNK];
            loop {
                match rd.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        up_last.store(now_ms(), Ordering::Relaxed);
                        if up_tx.send(Bytes::copy_from_slice(&buf[..n])).await.is_err() {
                            break;
                        }
                    }
                }
            }
            // End of the request body: the control plane learns that the
            // service finished sending (half-close), and everything queued is
            // still delivered.
            drop(up_tx);
        };
        // Control plane → local socket.
        let down_last = last.clone();
        let down_fut = async {
            while let Some(frame) = down.frame().await {
                let Ok(frame) = frame else { break };
                if let Ok(data) = frame.into_data() {
                    down_last.store(now_ms(), Ordering::Relaxed);
                    if wr.write_all(&data).await.is_err() {
                        break;
                    }
                }
            }
            let _ = wr.shutdown().await;
        };
        // Both directions end on their own (a half-closed side keeps the
        // other one flowing); only then is the connection dropped, so no
        // queued bytes are lost. Idleness and the lifetime cut both.
        tokio::select! {
            _ = async { tokio::join!(up, down_fut) } => {}
            () = guard(last.clone(), idle, deadline_ms) => {}
        }
        conn_task.abort();
    });
    Ok(json!({ "connected": true, "port": port }))
}

/// Resolves when the stream was quiet for `idle` or `deadline_ms` passed.
pub(crate) async fn guard(last: Arc<AtomicI64>, idle: Duration, deadline_ms: i64) {
    loop {
        tokio::time::sleep(Duration::from_secs(5)).await;
        let now = now_ms();
        let quiet = now - last.load(Ordering::Relaxed);
        if now >= deadline_ms || quiet >= i64::try_from(idle.as_millis()).unwrap_or(i64::MAX) {
            break;
        }
    }
}

/// A full-duplex stream dialled to the control plane.
pub(crate) struct ControlStream {
    /// Bytes to the control plane; dropping it ends the request body.
    pub up: mpsc::Sender<Bytes>,
    /// Bytes from the control plane.
    pub down: hyper::body::Incoming,
    /// Keeps the HTTP/2 connection's sender alive.
    pub sender: hyper::client::conn::http2::SendRequest<ChannelBody>,
    pub conn_task: tokio::task::JoinHandle<()>,
}

/// Opens a separate HTTP/2 connection to the control plane with the
/// agent's mTLS identity and posts a streaming request to `path`,
/// presenting `token` in `header` (tunnels, D-091; container shells, D-114).
pub(crate) async fn dial_control(
    endpoint: &Endpoint,
    tls: Arc<ClientConfig>,
    path: &str,
    header: &str,
    token: &str,
) -> Result<ControlStream, String> {
    let tcp = tokio::time::timeout(STREAM_TIMEOUT, TcpStream::connect((endpoint.host.as_str(), endpoint.port)))
        .await
        .map_err(|_| "the control plane did not answer".to_owned())?
        .map_err(|e| format!("cannot reach the control plane: {e}"))?;
    let _ = tcp.set_nodelay(true);
    let name = ServerName::try_from(endpoint.host.clone()).map_err(|e| e.to_string())?;
    let tls_stream = tokio_rustls::TlsConnector::from(tls).connect(name, tcp).await.map_err(|e| e.to_string())?;
    let (mut sender, conn) = hyper::client::conn::http2::Builder::new(TokioExecutor::new())
        .timer(TokioTimer::new())
        .initial_stream_window_size(WINDOW)
        .initial_connection_window_size(WINDOW)
        .keep_alive_interval(Duration::from_secs(30))
        .keep_alive_timeout(Duration::from_secs(10))
        .handshake(TokioIo::new(tls_stream))
        .await
        .map_err(|e| e.to_string())?;
    let conn_task = tokio::spawn(async move {
        let _ = conn.await;
    });

    let (up_tx, up_rx) = mpsc::channel::<Bytes>(QUEUE);
    let uri = format!("https://{}:{}{}", endpoint.host, endpoint.port, path);
    let req = http::Request::builder()
        .method(http::Method::POST)
        .uri(uri)
        .header(header, token)
        .body(ChannelBody(up_rx))
        .map_err(|e| e.to_string())?;
    let resp = tokio::time::timeout(STREAM_TIMEOUT, sender.send_request(req))
        .await
        .map_err(|_| "the control plane did not accept the stream in time".to_owned())?
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        conn_task.abort();
        return Err(format!("the control plane refused the stream ({})", resp.status()));
    }
    Ok(ControlStream { up: up_tx, down: resp.into_body(), sender, conn_task })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn cmd() -> TunnelOpen {
        TunnelOpen {
            connection_id: "a".repeat(32),
            port: 5432,
            allowed_ports: vec![22, 5432],
            stream_token: "b".repeat(64),
            idle_secs: 900,
            expires_at_ms: 2_000,
            dial_ip: String::new(),
        }
    }

    const LOCAL: [IpAddr; 2] =
        [IpAddr::V4(std::net::Ipv4Addr::new(10, 64, 0, 17)), IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)];

    #[test]
    fn commands_are_checked_before_anything_is_dialled() {
        assert_eq!(validate(&cmd(), 1_000, &LOCAL), Ok((5432, Dial::Loopback)));
        let bad = |f: fn(&mut TunnelOpen)| {
            let mut c = cmd();
            f(&mut c);
            validate(&c, 1_000, &LOCAL)
        };
        assert!(bad(|c| c.port = 8080).unwrap_err().contains("not allowed"), "only signed ports");
        assert!(bad(|c| c.port = 0).is_err());
        assert!(bad(|c| c.port = 70_000).is_err());
        assert!(bad(|c| c.connection_id = "../x".into()).is_err());
        assert!(bad(|c| c.stream_token = "B".repeat(64)).is_err());
        assert!(bad(|c| c.idle_secs = 0).is_err());
        assert!(bad(|c| c.expires_at_ms = 1_000).unwrap_err().contains("expired"));
    }

    /// D-100: the dial address must be this node's own. Loopback and the
    /// unspecified address mean loopback; a foreign address is refused, so a
    /// tunnel cannot pivot to another host from the node.
    #[test]
    fn dial_addresses_are_the_nodes_own_only() {
        let with = |ip: &str| {
            let mut c = cmd();
            c.dial_ip = ip.into();
            validate(&c, 1_000, &LOCAL).map(|(_, d)| d)
        };
        assert_eq!(with("127.0.0.1"), Ok(Dial::Loopback));
        assert_eq!(with("::1"), Ok(Dial::Loopback));
        assert_eq!(with("0.0.0.0"), Ok(Dial::Loopback));
        assert_eq!(with("10.64.0.17"), Ok(Dial::Addr("10.64.0.17".parse().unwrap())), "the mesh address");
        assert_eq!(with("::ffff:10.64.0.17"), Ok(Dial::Addr("10.64.0.17".parse().unwrap())), "canonical form");
        assert!(with("10.64.0.18").unwrap_err().contains("not an address of this node"));
        assert!(with("192.0.2.1").is_err());
        assert!(with("db.example.com").unwrap_err().contains("invalid dial address"), "never a host name");
        assert!(with("127.0.0.1:5432").is_err());
    }

    #[test]
    fn slots_are_bounded_and_released() {
        let before = ACTIVE.load(Ordering::SeqCst);
        let held: Vec<Slot> = (before..MAX_CONNECTIONS).map(|_| Slot::take().unwrap()).collect();
        assert!(Slot::take().is_err(), "the node limit holds");
        drop(held);
        assert!(Slot::take().is_ok());
    }
}
