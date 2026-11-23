//! The control socket (D-141). forge-server is the only client: every
//! connection must first present the secret forge-server gave the worker
//! on stdin, with the same protocol version. Connections are bounded; one
//! request is answered at a time per connection, and a request whose
//! connection closes is dropped (so an abandoned log batch is not written).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use forge_analytics_proto::config::Endpoint;
use forge_analytics_proto::{PROTOCOL, REQUEST_LIMITS, Request, Response, WorkerError, read_message, write_message};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};
use tokio::sync::{Semaphore, watch};

use crate::wiring::Worker;

/// Connections served at once; more are closed at once.
pub const MAX_CONNECTIONS: usize = 64;
/// Time a new connection has to authenticate.
const HELLO_WAIT: Duration = Duration::from_secs(2);

/// A bound control endpoint.
#[derive(Debug)]
pub enum Listener {
    #[cfg(unix)]
    Unix {
        listener: tokio::net::UnixListener,
        path: PathBuf,
    },
    Tcp(tokio::net::TcpListener),
}

impl Listener {
    /// Binds `endpoint`. A Unix socket is created in a directory only this
    /// user may enter and made mode 0600; a stale socket file is replaced.
    pub async fn bind(endpoint: &Endpoint) -> std::io::Result<Self> {
        match endpoint {
            #[cfg(unix)]
            Endpoint::Unix { path } => {
                use std::os::unix::fs::PermissionsExt;
                if let Some(dir) = path.parent() {
                    tokio::fs::create_dir_all(dir).await?;
                    tokio::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).await?;
                }
                match tokio::fs::remove_file(path).await {
                    Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
                    _ => {}
                }
                let listener = {
                    let short = forge_analytics_proto::socket_path::short(path)?;
                    tokio::net::UnixListener::bind(short.path())?
                };
                tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).await?;
                Ok(Self::Unix { listener, path: path.clone() })
            }
            #[cfg(not(unix))]
            Endpoint::Unix { .. } => {
                Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "Unix sockets are not available here"))
            }
            Endpoint::Loopback => Ok(Self::Tcp(tokio::net::TcpListener::bind("127.0.0.1:0").await?)),
        }
    }

    /// What forge-server connects to: the socket path or `127.0.0.1:<port>`.
    pub fn describe(&self) -> std::io::Result<String> {
        match self {
            #[cfg(unix)]
            Self::Unix { path, .. } => Ok(path.display().to_string()),
            Self::Tcp(l) => Ok(l.local_addr()?.to_string()),
        }
    }

    /// Removes the socket file.
    pub async fn cleanup(&self) {
        #[cfg(unix)]
        if let Self::Unix { path, .. } = self {
            let _ = tokio::fs::remove_file(path).await;
        }
    }
}

/// Accepts connections until shutdown.
pub async fn serve(listener: &Listener, worker: Arc<Worker>, secret: Arc<String>, mut shutdown: watch::Receiver<bool>) {
    let slots = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    loop {
        let accepted = tokio::select! {
            a = accept(listener) => a,
            _ = shutdown.changed() => return,
        };
        let conn = match accepted {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, "accepting a control connection failed");
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let Ok(slot) = slots.clone().try_acquire_owned() else {
            tracing::warn!("too many control connections; closing one");
            continue;
        };
        let (worker, secret) = (worker.clone(), secret.clone());
        tokio::spawn(async move {
            match conn {
                #[cfg(unix)]
                Conn::Unix(s) => connection(s, &worker, &secret).await,
                Conn::Tcp(s) => connection(s, &worker, &secret).await,
            }
            drop(slot);
        });
    }
}

enum Conn {
    #[cfg(unix)]
    Unix(tokio::net::UnixStream),
    Tcp(tokio::net::TcpStream),
}

async fn accept(l: &Listener) -> std::io::Result<Conn> {
    match l {
        #[cfg(unix)]
        Listener::Unix { listener, .. } => Ok(Conn::Unix(listener.accept().await?.0)),
        Listener::Tcp(t) => Ok(Conn::Tcp(t.accept().await?.0)),
    }
}

/// Checks the first frame: the secret and the protocol version.
fn authenticate(req: &Request, secret: &str) -> Result<(), WorkerError> {
    match req {
        Request::Hello { secret: s, protocol } => {
            if !forge_secrets::crypto::constant_time_eq(s.as_bytes(), secret.as_bytes()) {
                return Err(WorkerError::Unauthorized);
            }
            if *protocol != PROTOCOL {
                return Err(WorkerError::ProtocolMismatch { expected: PROTOCOL, got: *protocol });
            }
            Ok(())
        }
        _ => Err(WorkerError::Unauthorized),
    }
}

async fn connection<S>(stream: S, worker: &Worker, secret: &str)
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (mut rd, mut wr) = tokio::io::split(stream);
    let hello = tokio::time::timeout(HELLO_WAIT, read_message::<_, Request>(&mut rd, REQUEST_LIMITS)).await;
    let Ok(Ok((hello, _))) = hello else { return };
    let auth: Response = authenticate(&hello, secret).map(|()| forge_analytics_proto::Reply::Ok);
    let ok = auth.is_ok();
    if write_message(&mut wr, &auth, &[]).await.is_err() || !ok {
        if !ok {
            tracing::warn!("refused a control connection that failed to authenticate");
        }
        return;
    }
    loop {
        let (req, blob) = match read_message::<_, Request>(&mut rd, REQUEST_LIMITS).await {
            Ok(m) => m,
            Err(forge_analytics_proto::FrameError::Closed) => return,
            Err(e) => {
                tracing::warn!(error = %e, "malformed control message; closing the connection");
                return;
            }
        };
        // While the request runs, nothing else may arrive: a read that ends
        // means forge-server gave up, and the request is dropped with it.
        let mut probe = [0u8; 1];
        let response = tokio::select! {
            r = worker.handle(req, blob) => r,
            _ = rd.read(&mut probe) => return,
        };
        if write_message(&mut wr, &response, &[]).await.is_err() {
            return;
        }
    }
}

#[cfg(all(test, unix))]
#[allow(clippy::unwrap_used)] // reason: tests
mod tests {
    use super::*;
    use forge_analytics_proto::{RESPONSE_LIMITS, Reply};

    #[tokio::test]
    async fn only_the_secret_holder_with_this_protocol_is_served() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("w").join("control.sock");
        let config = forge_analytics_proto::config::WorkerConfig {
            store: forge_analytics_proto::config::StoreSelection::Memory,
            endpoint: Endpoint::Unix { path: path.clone() },
            tuning: crate::batcher::tests::tuning(),
            agent_listener: None,
        };
        let worker = Arc::new(Worker::build(&config, None, Arc::new(forge_core::SystemClock)).await.unwrap());
        let listener = Listener::bind(&config.endpoint).await.unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "the socket is owner-only");
        }
        let (tx, rx) = watch::channel(false);
        let server = tokio::spawn(async move {
            serve(&listener, worker, Arc::new("s3cret".into()), rx).await;
        });
        let hello = |secret: &str, protocol| Request::Hello { secret: secret.into(), protocol };
        for (h, want) in [
            (hello("wrong", PROTOCOL), Err(WorkerError::Unauthorized)),
            (
                hello("s3cret", PROTOCOL + 1),
                Err(WorkerError::ProtocolMismatch { expected: PROTOCOL, got: PROTOCOL + 1 }),
            ),
            (Request::Ping, Err(WorkerError::Unauthorized)),
        ] {
            let mut s = tokio::net::UnixStream::connect(&path).await.unwrap();
            write_message(&mut s, &h, &[]).await.unwrap();
            let (r, _): (Response, _) = read_message(&mut s, RESPONSE_LIMITS).await.unwrap();
            assert_eq!(r, want);
            // The connection is closed after a refusal.
            write_message(&mut s, &Request::Ping, &[]).await.ok();
            assert!(read_message::<_, Response>(&mut s, RESPONSE_LIMITS).await.is_err());
        }
        let mut s = tokio::net::UnixStream::connect(&path).await.unwrap();
        write_message(&mut s, &hello("s3cret", PROTOCOL), &[]).await.unwrap();
        let (r, _): (Response, _) = read_message(&mut s, RESPONSE_LIMITS).await.unwrap();
        assert_eq!(r, Ok(Reply::Ok));
        write_message(&mut s, &Request::Ping, &[]).await.unwrap();
        let (r, _): (Response, _) = read_message(&mut s, RESPONSE_LIMITS).await.unwrap();
        assert_eq!(r, Ok(Reply::Pong { pid: std::process::id() }));
        tx.send_replace(true);
        server.await.unwrap();
    }
}
