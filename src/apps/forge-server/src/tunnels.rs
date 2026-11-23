//! Port tunnels, control plane side (D-091, D-100): the registry of open
//! tunnels (the live sockets) and the agent-hub route the node's data stream
//! arrives on. Rows for history and restart recovery live in the store
//! (`forge_core::tunnels`).
//!
//! A tunnel belongs to one user, one node and one port of one target. The CLI holds it
//! open with a control WebSocket; every TCP connection the CLI accepts
//! locally becomes one tunnel *connection*: a data WebSocket from the CLI,
//! a signed `TunnelOpen` to the node, and a streaming HTTP/2 request the node
//! dials back to `/agent/v1/tunnel/<connection_id>` with a single-use token.
//! The server splices the two. Revoking a tunnel drops every leg at once.

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::{Extension, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use forge_core::tunnels::TunnelTargetKind;
use forge_core::{EnvironmentId, ServerId, Timestamp, TunnelGrantId, UserId, WorkspaceId};
use tokio::sync::{mpsc, oneshot, watch};

use crate::agent_hub::AgentPeer;
use crate::state::AppState;

/// The header the node presents its stream token in.
pub const TOKEN_HEADER: &str = "x-forge-tunnel-token";
/// Frames queued towards the node before the CLI side pauses.
pub const QUEUE: usize = 8;
/// A tunnel with no control WebSocket after this long is dropped.
const UNCLAIMED_MS: i64 = 30_000;
/// A pending connection the node did not dial back within this long expires.
const PENDING_MS: i64 = 30_000;

/// What a tunnel points at, resolved from the service catalogue when it
/// was opened; the access re-check uses it without the catalogue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub kind: TunnelTargetKind,
    /// The stored reference (service id, `<stack id>/<service>`,
    /// installation id, server id).
    pub target_ref: String,
    /// The name people type (`site`, `seq-b/seq`, `postgresql`, the node).
    pub name: String,
    pub label: String,
    /// The service's environment (apps and stack services).
    pub environment_id: Option<EnvironmentId>,
    /// `http`, `db`, `ssh` or `tcp`.
    pub role: String,
    /// The address the agent dials on the node.
    pub dial_ip: String,
    /// The grant that allowed it, when no role did.
    pub grant_id: Option<TunnelGrantId>,
}

/// One open tunnel.
#[derive(Debug, Clone)]
pub struct Tunnel {
    pub id: String,
    pub workspace_id: WorkspaceId,
    pub user_id: UserId,
    pub user: String,
    pub server_id: ServerId,
    /// The node's label.
    pub server: String,
    pub target: Target,
    pub port: u16,
    /// The address the CLI listens on (as it reported it).
    pub bind: String,
    /// Connections go straight to the node's SSH (`--direct`, #35 phase 2)
    /// with a certificate per connection; nothing is relayed here.
    pub direct: bool,
    pub started_at: Timestamp,
    pub expires_at: Timestamp,
    /// A control WebSocket holds the tunnel.
    pub claimed: bool,
    pub connections: usize,
    pub bytes_up: Arc<AtomicU64>,
    pub bytes_down: Arc<AtomicU64>,
    /// Last byte in either direction (epoch ms).
    pub last_activity: Arc<AtomicI64>,
    /// `None` while open; the reason once closed.
    revoke: watch::Sender<Option<&'static str>>,
}

/// Everything [`Tunnels::open`] needs to register a tunnel.
#[derive(Debug, Clone)]
pub struct NewTunnel {
    pub workspace_id: WorkspaceId,
    pub user_id: UserId,
    pub user: String,
    pub server_id: ServerId,
    pub server: String,
    pub target: Target,
    pub port: u16,
    pub bind: String,
    pub direct: bool,
    pub now: Timestamp,
    pub expires_at: Timestamp,
}

impl Tunnel {
    /// Bytes from the user to the node, and back.
    #[must_use]
    pub fn bytes(&self) -> (u64, u64) {
        (self.bytes_up.load(Ordering::Relaxed), self.bytes_down.load(Ordering::Relaxed))
    }
}

/// The node's half of a connection, handed to the CLI side.
#[derive(Debug)]
pub struct NodeStream {
    /// Bytes from the node.
    pub from_node: Body,
    /// Bytes to the node; dropping it ends the stream.
    pub to_node: mpsc::Sender<Bytes>,
}

#[derive(Debug)]
struct Pending {
    tunnel_id: String,
    server_id: ServerId,
    token_hash: String,
    created_ms: i64,
    tx: oneshot::Sender<NodeStream>,
}

#[derive(Debug, Default)]
struct Inner {
    /// Last facts re-collection a direct open asked of each node (epoch ms).
    facts_refresh: HashMap<ServerId, i64>,
    tunnels: HashMap<String, Tunnel>,
    pending: HashMap<String, Pending>,
}

/// Open tunnels and connections waiting for their node.
#[derive(Debug, Clone, Default)]
pub struct Tunnels {
    inner: Arc<Mutex<Inner>>,
}

/// Why a tunnel or connection is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    PerUser(usize),
    PerNode(usize),
    Global(usize),
    Gone,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::PerUser(n) => write!(f, "you already have {n} open tunnels; close one first"),
            Refusal::PerNode(n) => write!(f, "this node already has {n} tunnel connections"),
            Refusal::Global(n) => write!(f, "the server already has {n} tunnel connections"),
            Refusal::Gone => write!(f, "the tunnel is closed"),
        }
    }
}

/// Limits from `[tunnels]`.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub per_user: usize,
    pub per_node: usize,
    pub global: usize,
}

/// Removes a connection from its tunnel's count when dropped.
#[derive(Debug)]
pub struct ConnectionSlot {
    tunnels: Tunnels,
    tunnel_id: String,
}

impl Drop for ConnectionSlot {
    fn drop(&mut self) {
        if let Ok(mut m) = self.tunnels.inner.lock()
            && let Some(t) = m.tunnels.get_mut(&self.tunnel_id)
        {
            t.connections = t.connections.saturating_sub(1);
        }
    }
}

impl Tunnels {
    /// Drops tunnels nobody claimed and connections their node never
    /// dialled back.
    fn sweep(inner: &mut Inner, now: i64) {
        inner.tunnels.retain(|_, t| t.claimed || now - t.started_at.0 < UNCLAIMED_MS);
        inner.pending.retain(|_, p| now - p.created_ms < PENDING_MS);
    }

    /// Whether a direct open may ask `server` for fresh facts now: at most
    /// once a minute per node, so repeated opens cannot flood it.
    #[must_use]
    pub fn may_refresh_facts(&self, server: ServerId, now: Timestamp) -> bool {
        let Ok(mut m) = self.inner.lock() else { return false };
        let last = m.facts_refresh.get(&server).copied();
        if last.is_some_and(|t| now.0 - t < 60_000) {
            return false;
        }
        m.facts_refresh.insert(server, now.0);
        true
    }

    /// Registers a new tunnel, unless the user is at their limit.
    pub fn open(&self, n: NewTunnel, limits: Limits) -> Result<Tunnel, Refusal> {
        let mut m = self.inner.lock().map_err(|_| Refusal::Gone)?;
        Self::sweep(&mut m, n.now.0);
        let mine = m.tunnels.values().filter(|t| t.user_id == n.user_id).count();
        if mine >= limits.per_user {
            return Err(Refusal::PerUser(limits.per_user));
        }
        let (revoke, _) = watch::channel(None);
        let now = n.now;
        let t = Tunnel {
            id: hex::encode(forge_secrets::crypto::random_bytes::<16>()),
            workspace_id: n.workspace_id,
            user_id: n.user_id,
            user: n.user,
            server_id: n.server_id,
            server: n.server,
            target: n.target,
            port: n.port,
            bind: n.bind,
            direct: n.direct,
            started_at: now,
            expires_at: n.expires_at,
            claimed: false,
            connections: 0,
            bytes_up: Arc::default(),
            bytes_down: Arc::default(),
            last_activity: Arc::new(AtomicI64::new(now.0)),
            revoke,
        };
        m.tunnels.insert(t.id.clone(), t.clone());
        Ok(t)
    }

    #[must_use]
    pub fn get(&self, id: &str) -> Option<Tunnel> {
        self.inner.lock().ok().and_then(|m| m.tunnels.get(id).cloned())
    }

    #[must_use]
    pub fn list(&self) -> Vec<Tunnel> {
        let mut v: Vec<Tunnel> = self.inner.lock().map(|m| m.tunnels.values().cloned().collect()).unwrap_or_default();
        v.sort_by_key(|t| std::cmp::Reverse(t.started_at.0));
        v
    }

    /// Marks a tunnel held by a control socket; returns its revoke signal.
    #[must_use]
    pub fn claim(&self, id: &str) -> Option<watch::Receiver<Option<&'static str>>> {
        let mut m = self.inner.lock().ok()?;
        let t = m.tunnels.get_mut(id)?;
        if t.claimed {
            return None;
        }
        t.claimed = true;
        Some(t.revoke.subscribe())
    }

    /// The revoke signal of a claimed tunnel (for its connections).
    #[must_use]
    pub fn watch(&self, id: &str) -> Option<watch::Receiver<Option<&'static str>>> {
        self.inner.lock().ok().and_then(|m| m.tunnels.get(id).filter(|t| t.claimed).map(|t| t.revoke.subscribe()))
    }

    /// Ends a tunnel (reason `closed`): every leg closes; it leaves the registry.
    pub fn close(&self, id: &str) -> Option<Tunnel> {
        self.close_with(id, "closed")
    }

    /// Ends a tunnel with a reason the control socket reports and records.
    pub fn close_with(&self, id: &str, reason: &'static str) -> Option<Tunnel> {
        let t = self.inner.lock().ok()?.tunnels.remove(id)?;
        let _ = t.revoke.send(Some(reason));
        Some(t)
    }

    /// Counts a new connection of a claimed tunnel against the limits.
    pub fn connect(&self, id: &str, limits: Limits) -> Result<ConnectionSlot, Refusal> {
        let mut m = self.inner.lock().map_err(|_| Refusal::Gone)?;
        let server = m.tunnels.get(id).filter(|t| t.claimed).map(|t| t.server_id).ok_or(Refusal::Gone)?;
        let total: usize = m.tunnels.values().map(|t| t.connections).sum();
        if total >= limits.global {
            return Err(Refusal::Global(limits.global));
        }
        let on_node: usize = m.tunnels.values().filter(|t| t.server_id == server).map(|t| t.connections).sum();
        if on_node >= limits.per_node {
            return Err(Refusal::PerNode(limits.per_node));
        }
        if let Some(t) = m.tunnels.get_mut(id) {
            t.connections += 1;
        }
        Ok(ConnectionSlot { tunnels: self.clone(), tunnel_id: id.to_owned() })
    }

    /// Waits for the node to dial back a connection. Returns the connection
    /// id and token to sign into `TunnelOpen`, and the receiver of the
    /// node's stream.
    #[must_use]
    pub fn expect(
        &self,
        tunnel_id: &str,
        server_id: ServerId,
        now: Timestamp,
    ) -> (String, String, oneshot::Receiver<NodeStream>) {
        let conn = hex::encode(forge_secrets::crypto::random_bytes::<16>());
        let token = hex::encode(forge_secrets::crypto::random_bytes::<32>());
        let (tx, rx) = oneshot::channel();
        if let Ok(mut m) = self.inner.lock() {
            Self::sweep(&mut m, now.0);
            m.pending.insert(
                conn.clone(),
                Pending {
                    tunnel_id: tunnel_id.to_owned(),
                    server_id,
                    token_hash: forge_secrets::crypto::sha256_hex(token.as_bytes()),
                    created_ms: now.0,
                    tx,
                },
            );
        }
        (conn, token, rx)
    }

    /// The node's dial-back: right node, right single-use token, a tunnel
    /// still open. Takes the pending entry either way it matches the id.
    fn accept(&self, conn: &str, from: Option<ServerId>, token: &str) -> Option<(oneshot::Sender<NodeStream>, String)> {
        let mut m = self.inner.lock().ok()?;
        let p = m.pending.remove(conn)?;
        let hash = forge_secrets::crypto::sha256_hex(token.as_bytes());
        let ok = from == Some(p.server_id)
            && forge_secrets::crypto::constant_time_eq(hash.as_bytes(), p.token_hash.as_bytes())
            && m.tunnels.get(&p.tunnel_id).is_some_and(|t| t.claimed);
        ok.then_some((p.tx, p.tunnel_id))
    }
}

/// `POST /agent/v1/tunnel/{connection_id}`: the node's data stream. The
/// request body carries bytes from the node; the response body carries
/// bytes to it, for as long as the CLI side holds the connection.
pub(crate) async fn hub_stream(
    State(state): State<AppState>,
    Extension(peer): Extension<AgentPeer>,
    Path(conn): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    // The same certificate checks as every agent request (revoked
    // certificates and nodes are refused).
    let from = match crate::agent_hub::authenticated_server(&state, &peer).await {
        Ok(s) => Some(s.id),
        Err(r) => return *r,
    };
    let token = headers.get(TOKEN_HEADER).and_then(|v| v.to_str().ok()).unwrap_or_default();
    let Some((tx, _tunnel)) = state.tunnels.accept(&conn, from, token) else {
        return StatusCode::FORBIDDEN.into_response();
    };
    let (to_node, mut rx) = mpsc::channel::<Bytes>(QUEUE);
    if tx.send(NodeStream { from_node: body, to_node }).is_err() {
        return StatusCode::GONE.into_response();
    }
    let stream = futures::stream::poll_fn(move |cx| rx.poll_recv(cx).map(|o| o.map(Ok::<Bytes, Infallible>)));
    Response::new(Body::from_stream(stream))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const LIMITS: Limits = Limits { per_user: 2, per_node: 2, global: 3 };

    fn new(user: UserId, node: ServerId, now: i64) -> NewTunnel {
        NewTunnel {
            workspace_id: WorkspaceId::new(),
            user_id: user,
            user: "u".into(),
            server_id: node,
            server: "n".into(),
            target: Target {
                kind: TunnelTargetKind::Package,
                target_ref: "inst-1".into(),
                name: "postgresql".into(),
                label: "postgresql on n".into(),
                environment_id: None,
                role: "db".into(),
                dial_ip: "127.0.0.1".into(),
                grant_id: None,
            },
            port: 5432,
            direct: false,
            bind: "127.0.0.1".into(),
            now: Timestamp(now),
            expires_at: Timestamp(now + 3_600_000),
        }
    }

    fn open(t: &Tunnels, user: UserId, node: ServerId) -> Result<Tunnel, Refusal> {
        t.open(new(user, node, 1_000), LIMITS)
    }

    #[test]
    fn limits_hold_per_user_node_and_server() {
        let t = Tunnels::default();
        let (u, n) = (UserId::new(), ServerId::new());
        let a = open(&t, u, n).unwrap();
        let b = open(&t, u, n).unwrap();
        assert_eq!(open(&t, u, n).unwrap_err(), Refusal::PerUser(2));
        assert!(t.connect(&a.id, LIMITS).is_err(), "not claimed yet");
        let _ra = t.claim(&a.id).unwrap();
        assert!(t.claim(&a.id).is_none(), "one control socket");
        let c1 = t.connect(&a.id, LIMITS).unwrap();
        let _c2 = t.connect(&a.id, LIMITS).unwrap();
        let _rb = t.claim(&b.id).unwrap();
        assert_eq!(t.connect(&b.id, LIMITS).unwrap_err(), Refusal::PerNode(2));
        drop(c1);
        let _cb = t.connect(&b.id, LIMITS).expect("a closed connection frees its place");
        // Global (3): another user on another node gets one more, then no more.
        let other = open(&t, UserId::new(), ServerId::new()).unwrap();
        let _ro = t.claim(&other.id).unwrap();
        let _co = t.connect(&other.id, LIMITS).unwrap();
        assert_eq!(t.connect(&other.id, LIMITS).unwrap_err(), Refusal::Global(3));
    }

    #[test]
    fn closing_signals_every_leg_and_unclaimed_tunnels_expire() {
        let t = Tunnels::default();
        let x = open(&t, UserId::new(), ServerId::new()).unwrap();
        let rx = t.claim(&x.id).unwrap();
        let conn_rx = t.watch(&x.id).unwrap();
        assert!(t.close_with(&x.id, "access_withdrawn").is_some());
        assert_eq!(*rx.borrow(), Some("access_withdrawn"));
        assert_eq!(*conn_rx.borrow(), Some("access_withdrawn"));
        assert!(t.get(&x.id).is_none());
        let y = open(&t, UserId::new(), ServerId::new()).unwrap();
        // 31 s later another open sweeps the unclaimed one.
        let _ = t.open(new(UserId::new(), ServerId::new(), 32_000), LIMITS);
        assert!(t.get(&y.id).is_none());
    }

    #[test]
    fn the_node_must_present_its_own_token_once() {
        let t = Tunnels::default();
        let node = ServerId::new();
        let x = open(&t, UserId::new(), node).unwrap();
        let _rx = t.claim(&x.id).unwrap();
        let (conn, token, _wait) = t.expect(&x.id, node, Timestamp(1_000));
        assert!(t.accept(&conn, Some(ServerId::new()), &token).is_none(), "another node");
        let (conn, token, _wait) = t.expect(&x.id, node, Timestamp(1_000));
        assert!(t.accept(&conn, Some(node), "0".repeat(64).as_str()).is_none(), "wrong token");
        let (conn, token2, _wait) = t.expect(&x.id, node, Timestamp(1_000));
        assert!(t.accept(&conn, Some(node), &token2).is_some());
        assert!(t.accept(&conn, Some(node), &token2).is_none(), "single use");
        let _ = token;
    }
}
