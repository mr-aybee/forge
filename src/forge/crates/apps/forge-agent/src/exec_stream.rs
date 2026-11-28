//! Container shells, agent side (D-114): dial the control plane and relay
//! the helper's frames unchanged.
//!
//! The agent sends a `Ready` frame when the helper started the shell, or an
//! `Error` frame with the reason when it did not, so the console learns the
//! outcome at once. Bounded channels and the HTTP/2 window give backpressure:
//! a slow browser stalls reads up to the helper and the Docker socket.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use bytes::Bytes;
use forge_proto::exec_frame::{Frame, MAX_PAYLOAD};
use forge_proto::messages::ContainerExecOpen;
use http_body_util::BodyExt;
use rustls::ClientConfig;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::container_exec::{ExecPipe, Slot};
use crate::link::Endpoint;
use crate::tunnel::{ControlStream, dial_control, guard, now_ms};

/// The header carrying the stream token.
pub const TOKEN_HEADER: &str = "x-forge-exec-token";
/// How long the control plane gets to read an error before the stream drops.
const ERROR_GRACE: Duration = Duration::from_secs(5);
/// How long one direction waits for the other after it ended.
const DRAIN: Duration = Duration::from_secs(5);

/// Dials the control plane for one shell and relays it in the background.
/// `pipe` is the helper's answer: on error the reason goes to the console.
pub async fn run(
    x: ContainerExecOpen,
    pipe: Result<ExecPipe, String>,
    slot: Slot,
    endpoint: Endpoint,
    tls: Arc<ClientConfig>,
) -> Result<Value, String> {
    let path = format!("{}{}", forge_proto::paths::EXEC, x.session_id);
    let ControlStream { up, mut down, sender, conn_task } =
        dial_control(&endpoint, tls, &path, TOKEN_HEADER, &x.stream_token).await?;

    let pipe = match pipe {
        Ok(p) => p,
        Err(e) => {
            let _ = up.send(Bytes::from(Frame::Error(e.clone()).encode())).await;
            drop(up);

            // Let the control plane read it; it ends the stream then.
            let _ = tokio::time::timeout(ERROR_GRACE, async { while down.frame().await.is_some() {} }).await;
            conn_task.abort();

            return Err(e);
        }
    };

    if up.send(Bytes::from(Frame::Ready.encode())).await.is_err() {
        conn_task.abort();
        return Err("the control plane closed the stream".into());
    }

    let idle = Duration::from_secs(u64::from(x.idle_secs));
    let deadline_ms = now_ms().saturating_add(i64::from(x.lifetime_secs) * 1000);
    tokio::spawn(async move {
        let _slot = slot;
        let _sender = sender;
        let last = Arc::new(AtomicI64::new(now_ms()));
        let ExecPipe { mut reader, mut writer } = pipe;

        // Helper → control plane: frames as the helper wrote them.
        let up_last = last.clone();
        let up_fut = async move {
            let mut buf = vec![0u8; MAX_PAYLOAD];

            loop {
                match reader.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        up_last.store(now_ms(), Ordering::Relaxed);

                        if up.send(Bytes::copy_from_slice(&buf[..n])).await.is_err() {
                            break;
                        }
                    }
                }
            }
        };

        // Control plane → helper.
        let down_last = last.clone();
        let down_fut = async move {
            while let Some(Ok(frame)) = down.frame().await {
                if let Ok(data) = frame.into_data() {
                    down_last.store(now_ms(), Ordering::Relaxed);

                    if writer.write_all(&data).await.is_err() {
                        break;
                    }
                }
            }

            // The helper drops the shell's stream when the agent hangs up.
            let _ = writer.shutdown().await;
        };
        tokio::pin!(up_fut, down_fut);
        let both = async {
            tokio::select! {
                () = &mut up_fut => { let _ = tokio::time::timeout(DRAIN, &mut down_fut).await; }
                () = &mut down_fut => { let _ = tokio::time::timeout(DRAIN, &mut up_fut).await; }
            }
        };
        tokio::select! {
            () = both => {}
            () = guard(last, idle, deadline_ms) => {}
        }
        conn_task.abort();
    });

    Ok(json!({ "connected": true }))
}
