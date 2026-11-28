//! Container shells through the agent link, control plane side (D-114).
//!
//! The console's WebSocket (`api::terminal`) does not change. Instead of an
//! SSH connection, the server signs a `ContainerExecOpen` for the node; the
//! node's root helper starts a Docker exec with a TTY, and the agent dials
//! back a streaming HTTP/2 request to `/agent/v1/exec/<session>` with a
//! single-use token (as tunnels do, D-091). The stream carries
//! [`forge_proto::exec_frame`] frames; [`NodeShellReader`] and
//! [`NodeShellWriter`] turn it into the terminal relay's `ShellReader` and
//! `ShellWriter`. No terminal account and no credential is involved.

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::{Body, BodyDataStream};
use axum::extract::{Extension, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use forge_core::remote::{ShellReader, ShellWriter, TermSize};
use forge_core::{DomainError, DomainResult, ServerId, Timestamp};
use forge_proto::exec_frame::{Decoder, Frame, encode_data};
use forge_proto::messages::ContainerExecOpen;
use futures::StreamExt;
use tokio::sync::{mpsc, oneshot};

use crate::agent_hub::AgentPeer;
use crate::state::AppState;

/// The header the node presents its stream token in.
pub const TOKEN_HEADER: &str = "x-forge-exec-token";
/// Frames queued towards the node before the browser side pauses.
const QUEUE: usize = 8;
/// A pending stream the node did not dial back within this long expires;
/// also the `expires_at_ms` signed into the command.
const PENDING_MS: i64 = 30_000;
/// The node ends a shell with no traffic for this long (the server's own
/// idle limit, 30 minutes, applies to the browser side).
const NODE_IDLE_SECS: u32 = 31 * 60;
/// Container shells at most, per user and per node.
pub const MAX_PER_USER: usize = 8;
pub const MAX_PER_NODE: usize = 8;

/// The node's half of a shell.
#[derive(Debug)]
pub struct NodeStream {
    pub from_node: Body,
    pub to_node: mpsc::Sender<Bytes>,
}

#[derive(Debug)]
struct Pending {
    server_id: ServerId,
    token_hash: String,
    created_ms: i64,
    tx: oneshot::Sender<NodeStream>,
}

/// Shells waiting for their node to dial back.
#[derive(Debug, Clone, Default)]
pub struct ExecStreams {
    inner: Arc<Mutex<HashMap<String, Pending>>>,
}

impl ExecStreams {
    /// A new session id and token, and the receiver of the node's stream.
    #[must_use]
    pub fn expect(&self, server_id: ServerId, now: Timestamp) -> (String, String, oneshot::Receiver<NodeStream>) {
        let session = hex::encode(forge_secrets::crypto::random_bytes::<16>());
        let token = hex::encode(forge_secrets::crypto::random_bytes::<32>());
        let (tx, rx) = oneshot::channel();

        if let Ok(mut m) = self.inner.lock() {
            m.retain(|_, p| now.0 - p.created_ms < PENDING_MS);
            m.insert(
                session.clone(),
                Pending {
                    server_id,
                    token_hash: forge_secrets::crypto::sha256_hex(token.as_bytes()),
                    created_ms: now.0,
                    tx,
                },
            );
        }

        (session, token, rx)
    }

    /// Drops a pending stream nobody dialled.
    pub fn forget(&self, session: &str) {
        if let Ok(mut m) = self.inner.lock() {
            m.remove(session);
        }
    }

    /// The node's dial-back: right node, right single-use token. The
    /// pending entry is taken whether or not it matches.
    fn accept(&self, session: &str, from: Option<ServerId>, token: &str) -> Option<oneshot::Sender<NodeStream>> {
        let p = self.inner.lock().ok()?.remove(session)?;
        let hash = forge_secrets::crypto::sha256_hex(token.as_bytes());
        let ok = from == Some(p.server_id)
            && forge_secrets::crypto::constant_time_eq(hash.as_bytes(), p.token_hash.as_bytes());

        ok.then_some(p.tx)
    }
}

/// `POST /agent/v1/exec/{session}`: the node's shell stream.
pub(crate) async fn hub_stream(
    State(state): State<AppState>,
    Extension(peer): Extension<AgentPeer>,
    Path(session): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let from = match crate::agent_hub::authenticated_server(&state, &peer).await {
        Ok(s) => Some(s.id),
        Err(r) => return *r,
    };

    let token = headers.get(TOKEN_HEADER).and_then(|v| v.to_str().ok()).unwrap_or_default();

    let Some(tx) = state.exec_streams.accept(&session, from, token) else {
        return StatusCode::FORBIDDEN.into_response();
    };

    let (to_node, mut rx) = mpsc::channel::<Bytes>(QUEUE);
    if tx.send(NodeStream { from_node: body, to_node }).is_err() {
        return StatusCode::GONE.into_response();
    }

    let stream = futures::stream::poll_fn(move |cx| rx.poll_recv(cx).map(|o| o.map(Ok::<Bytes, Infallible>)));

    Response::new(Body::from_stream(stream))
}

/// How a shell ended, as the node reported it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellEnd {
    Exit(i32),
    Error(String),
}

/// Output of a shell on a node.
pub struct NodeShellReader {
    body: BodyDataStream,
    dec: Decoder,
    end: Arc<Mutex<Option<ShellEnd>>>,
}

impl std::fmt::Debug for NodeShellReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NodeShellReader").finish_non_exhaustive()
    }
}

impl NodeShellReader {
    fn finish(&self, e: ShellEnd) {
        if let Ok(mut end) = self.end.lock() {
            end.get_or_insert(e);
        }
    }

    /// The next frame from the node; `None` when the stream ended or broke.
    async fn frame(&mut self) -> Option<Frame> {
        loop {
            match self.dec.next_frame() {
                Ok(Some(f)) => return Some(f),
                Ok(None) => {}
                Err(e) => {
                    self.finish(ShellEnd::Error(format!("the node sent a malformed stream: {e}")));
                    return None;
                }
            }

            match self.body.next().await {
                Some(Ok(b)) => self.dec.push(&b),
                _ => return None,
            }
        }
    }
}

#[async_trait::async_trait]
impl ShellReader for NodeShellReader {
    async fn recv(&mut self) -> Option<Vec<u8>> {
        loop {
            match self.frame().await? {
                Frame::Data(d) if d.is_empty() => {}
                Frame::Data(d) => return Some(d),
                Frame::Exit(code) => {
                    self.finish(ShellEnd::Exit(code));
                    return None;
                }
                Frame::Error(m) => {
                    self.finish(ShellEnd::Error(m));
                    return None;
                }
                Frame::Eof => return None,
                Frame::Ready | Frame::Resize { .. } => {}
            }
        }
    }
}

/// Input to a shell on a node.
#[derive(Debug)]
pub struct NodeShellWriter {
    tx: Mutex<Option<mpsc::Sender<Bytes>>>,
}

impl NodeShellWriter {
    fn sender(&self) -> DomainResult<mpsc::Sender<Bytes>> {
        self.tx
            .lock()
            .ok()
            .and_then(|t| t.clone())
            .ok_or_else(|| DomainError::Conflict("the container shell is closed".into()))
    }

    async fn send(&self, bytes: Vec<u8>) -> DomainResult<()> {
        self.sender()?
            .send(Bytes::from(bytes))
            .await
            .map_err(|_| DomainError::Conflict("the node closed the container shell".into()))
    }
}

#[async_trait::async_trait]
impl ShellWriter for NodeShellWriter {
    async fn write(&self, data: &[u8]) -> DomainResult<()> {
        self.send(encode_data(data)).await
    }

    async fn resize(&self, size: TermSize) -> DomainResult<()> {
        let s = size.clamped();
        let (cols, rows) = (u16::try_from(s.cols).unwrap_or(u16::MAX), u16::try_from(s.rows).unwrap_or(u16::MAX));

        self.send(Frame::Resize { cols, rows }.encode()).await
    }

    async fn eof(&self) -> DomainResult<()> {
        self.send(Frame::Eof.encode()).await
    }

    /// Ends the stream to the node: the agent hangs up the helper, which
    /// drops the shell's TTY.
    async fn close(&self) {
        if let Ok(mut t) = self.tx.lock() {
            t.take();
        }
    }
}

/// An open shell on a node.
#[derive(Debug)]
pub struct NodeShell {
    pub reader: NodeShellReader,
    pub writer: NodeShellWriter,
    /// Set when the node reports the exit code or an error.
    pub end: Arc<Mutex<Option<ShellEnd>>>,
}

/// Why a shell did not open.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OpenError {
    #[error("the node is offline")]
    Offline,
    #[error("the agent did not answer: update the agent (Software → Updates → Agents)")]
    NoAnswer,
    #[error("{0}")]
    Node(String),
}

/// What to open.
#[derive(Debug, Clone)]
pub struct OpenRequest {
    pub server_id: ServerId,
    pub container: String,
    pub size: TermSize,
    /// Who asks (signed into the command, shown in the agent's log).
    pub principal: String,
    pub lifetime: Duration,
}

/// Signs a `ContainerExecOpen` for the node and waits (at most
/// `container_shell_open_secs`) until its agent dialled back and the helper started the
/// shell, or reported why not.
pub async fn open(state: &AppState, r: &OpenRequest) -> Result<NodeShell, OpenError> {
    let now = state.clock.now();
    let (session, token, node_rx) = state.exec_streams.expect(r.server_id, now);
    let size = r.size.clamped();
    let cmd = forge_proto::Command::ContainerExecOpen(ContainerExecOpen {
        session_id: session.clone(),
        container: r.container.clone(),
        cols: size.cols,
        rows: size.rows,
        stream_token: token,
        idle_secs: NODE_IDLE_SECS,
        expires_at_ms: now.0 + PENDING_MS,
        lifetime_secs: u32::try_from(r.lifetime.as_secs()).unwrap_or(u32::MAX),
    });
    let (body, signed) = state.signer.issue(&r.server_id.to_string(), &r.principal, cmd, now, Duration::from_secs(60));
    if state.hub.enqueue(r.server_id, &body.operation_id, signed).is_err() {
        state.exec_streams.forget(&session);
        return Err(OpenError::Offline);
    }

    let open_wait = Duration::from_secs(u64::from(state.config.container_shell_open_secs));
    let deadline = tokio::time::Instant::now() + open_wait;
    let mut node_rx = node_rx;

    // The node dials back (with `Ready` or the error), or reports why it
    // could not (a refused command, a stream it could not open). An agent
    // that does not know the command does neither.
    let node = tokio::select! {
        r = tokio::time::timeout_at(deadline, &mut node_rx) => r.ok().and_then(Result::ok),
        res = state.hub.wait(&body.operation_id, open_wait) => match res {
            Some(res) if !res.ok => {
                state.exec_streams.forget(&session);
                return Err(OpenError::Node(res.error));
            }
            _ => tokio::time::timeout_at(deadline, node_rx).await.ok().and_then(Result::ok),
        },
    };

    let Some(node) = node else {
        state.exec_streams.forget(&session);
        return Err(OpenError::NoAnswer);
    };

    let end = Arc::new(Mutex::new(None));
    let mut reader = NodeShellReader { body: node.from_node.into_data_stream(), dec: Decoder::new(), end: end.clone() };
    let writer = NodeShellWriter { tx: Mutex::new(Some(node.to_node)) };

    match tokio::time::timeout_at(deadline, reader.frame()).await {
        Ok(Some(Frame::Ready)) => Ok(NodeShell { reader, writer, end }),
        Ok(Some(Frame::Error(m))) => Err(OpenError::Node(m)),
        Ok(_) => Err(OpenError::Node(end.lock().ok().and_then(|e| e.clone()).map_or_else(
            || "the node closed the container shell".to_owned(),
            |e| match e {
                ShellEnd::Error(m) => m,
                ShellEnd::Exit(c) => format!("the shell exited with {c}"),
            },
        ))),
        Err(_) => Err(OpenError::NoAnswer),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn the_node_must_present_its_own_token_once() {
        let s = ExecStreams::default();
        let node = ServerId::new();
        let (id, token, _rx) = s.expect(node, Timestamp(1_000));
        assert!(s.accept(&id, Some(ServerId::new()), &token).is_none(), "another node");
        assert!(s.accept(&id, Some(node), &token).is_none(), "taken by the failed attempt");
        let (id, _token, _rx) = s.expect(node, Timestamp(1_000));
        assert!(s.accept(&id, Some(node), &"0".repeat(64)).is_none(), "wrong token");
        let (id, token, _rx) = s.expect(node, Timestamp(1_000));
        assert!(s.accept(&id, None, &token).is_none(), "no client certificate");
        let (id, token, _rx) = s.expect(node, Timestamp(1_000));
        assert!(s.accept(&id, Some(node), &token).is_some());
        assert!(s.accept(&id, Some(node), &token).is_none(), "single use");

        // Expired entries are swept by the next expect.
        let (old, token, _rx) = s.expect(node, Timestamp(1_000));
        let _ = s.expect(node, Timestamp(1_000 + PENDING_MS));
        assert!(s.accept(&old, Some(node), &token).is_none(), "expired");
    }

    #[tokio::test]
    async fn frames_become_terminal_output_and_the_exit_code_is_kept() {
        let mut wire = Frame::Data(b"$ ".to_vec()).encode();
        wire.extend(Frame::Data(Vec::new()).encode());
        wire.extend(Frame::Resize { cols: 1, rows: 1 }.encode());
        wire.extend(Frame::Exit(3).encode());

        // Split mid-frame.
        let parts: Vec<Result<Bytes, Infallible>> =
            vec![Ok(Bytes::copy_from_slice(&wire[..3])), Ok(Bytes::copy_from_slice(&wire[3..]))];
        let end = Arc::new(Mutex::new(None));
        let mut r = NodeShellReader {
            body: Body::from_stream(futures::stream::iter(parts)).into_data_stream(),
            dec: Decoder::new(),
            end: end.clone(),
        };
        assert_eq!(r.recv().await, Some(b"$ ".to_vec()));
        assert_eq!(r.recv().await, None);
        assert_eq!(*end.lock().unwrap(), Some(ShellEnd::Exit(3)));

        let (tx, mut rx) = mpsc::channel(4);
        let w = NodeShellWriter { tx: Mutex::new(Some(tx)) };
        w.write(b"ls\r").await.unwrap();
        w.resize(TermSize { cols: 100, rows: 40 }).await.unwrap();
        w.close().await;
        assert!(w.write(b"x").await.is_err(), "closed");
        let mut dec = Decoder::new();

        while let Some(b) = rx.recv().await {
            dec.push(&b);
        }

        assert_eq!(dec.next_frame().unwrap(), Some(Frame::Data(b"ls\r".to_vec())));
        assert_eq!(dec.next_frame().unwrap(), Some(Frame::Resize { cols: 100, rows: 40 }));
    }
}
