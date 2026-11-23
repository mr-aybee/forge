//! Sends sealed spool files to the analytics worker (D-142): oldest first,
//! one at a time, over mTLS with the agent's certificate. A file is deleted
//! only after the worker's ack names it. The worker answers 429 or 503 when
//! it is busy or its store is down: the file stays on disk and the uploader
//! backs off with jitter, so the spool absorbs the backlog.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use forge_core::Backoff;
use forge_core::spool::FILE_ID_HEADER;
use forge_proto::messages::SpoolAck;
use http::StatusCode;
use prost::Message;
use rustls::ClientConfig;
use tokio::sync::watch;

use crate::link::{Endpoint, Link, Outgoing};
use crate::spool::{ReadyFile, Spool};

const BACKOFF: Backoff = Backoff::new(Duration::from_secs(1), Duration::from_secs(60));
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(60);
/// Checks for work even without a notification (a missed wake-up costs
/// at most this).
const IDLE: Duration = Duration::from_secs(30);
/// Content type of an upload body.
pub const CONTENT_TYPE: &str = "application/x-ndjson+zstd";

/// What happened to one upload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The worker stored the file (now or before).
    Stored,
    /// Try again later (busy, store down, network).
    Retry(String),
    /// The worker will never take this file (too large, unreadable).
    Reject(String),
}

/// Where files go.
#[async_trait]
pub trait Sink: Send {
    async fn upload(&mut self, stem: &str, body: Vec<u8>) -> Outcome;
}

/// The worker's agent listener.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub url: String,
    /// Bumped when the agent's certificate is renewed, so the uploader
    /// reconnects with the new one.
    pub generation: u64,
}

/// Uploads over a [`Link`] to the worker.
pub struct WorkerSink {
    link: Link,
}

impl std::fmt::Debug for WorkerSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkerSink").field("link", &self.link).finish()
    }
}

impl WorkerSink {
    pub fn new(url: &str, tls: Arc<ClientConfig>) -> anyhow::Result<Self> {
        Ok(Self { link: Link::new(Endpoint::parse(url)?, tls, UPLOAD_TIMEOUT) })
    }
}

/// How the worker's answer to `stem` is read.
#[must_use]
pub fn outcome(stem: &str, status: StatusCode, body: &[u8]) -> Outcome {
    match status {
        StatusCode::OK => match SpoolAck::decode(body) {
            Ok(ack) if ack.file_id == stem => Outcome::Stored,
            Ok(ack) => Outcome::Retry(format!("ack for another file ({})", ack.file_id)),
            Err(e) => Outcome::Retry(format!("unreadable ack: {e}")),
        },
        StatusCode::PAYLOAD_TOO_LARGE | StatusCode::BAD_REQUEST | StatusCode::UNPROCESSABLE_ENTITY => {
            Outcome::Reject(format!("{status}: {}", String::from_utf8_lossy(body)))
        }
        _ => Outcome::Retry(format!("{status}: {}", String::from_utf8_lossy(body))),
    }
}

#[async_trait]
impl Sink for WorkerSink {
    async fn upload(&mut self, stem: &str, body: Vec<u8>) -> Outcome {
        let out = Outgoing {
            method: http::Method::POST,
            path: forge_proto::paths::SPOOL,
            content_type: CONTENT_TYPE,
            header: Some((FILE_ID_HEADER, stem)),
            body,
        };
        match self.link.send(out).await {
            Ok((status, body)) => outcome(stem, status, &body),
            Err(e) => Outcome::Retry(e.to_string()),
        }
    }
}

/// Builds the sink for a target (the TLS identity may change on renewal).
pub type MakeSink = Box<dyn Fn(&Target) -> anyhow::Result<Box<dyn Sink>> + Send + Sync>;

/// Uploads until `shutdown`. `target` is `None` until the server names the
/// worker; files wait on disk meanwhile.
pub async fn run(
    spool: Arc<Spool>,
    mut target: watch::Receiver<Option<Target>>,
    make: MakeSink,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut sink: Option<(Target, Box<dyn Sink>)> = None;
    let mut failures: u32 = 0;
    loop {
        if *shutdown.borrow() {
            return;
        }
        let current = target.borrow_and_update().clone();
        let file = spool.oldest();
        let (Some(t), Some(file)) = (current, file) else {
            tokio::select! {
                () = spool.sealed() => {}
                r = target.changed() => if r.is_err() { return },
                _ = shutdown.changed() => return,
                () = tokio::time::sleep(IDLE) => {}
            }
            continue;
        };
        if sink.as_ref().is_none_or(|(st, _)| *st != t) {
            match make(&t) {
                Ok(s) => sink = Some((t.clone(), s)),
                Err(e) => {
                    tracing::warn!(url = %t.url, error = %e, "bad analytics worker URL");
                    sink = None;
                    tokio::select! {
                        r = target.changed() => if r.is_err() { return },
                        _ = shutdown.changed() => return,
                    }
                    continue;
                }
            }
        }
        let Some((_, s)) = sink.as_mut() else { continue };
        let wait = match send_one(&spool, s.as_mut(), &file).await {
            Ok(()) => {
                failures = 0;
                continue;
            }
            Err(reason) => {
                let d = BACKOFF.delay(failures, crate::agent::unit_random());
                failures = failures.saturating_add(1);
                tracing::debug!(file = %file.stem, %reason, retry_in = ?d, "spool upload deferred");
                d
            }
        };
        tokio::select! {
            () = tokio::time::sleep(wait) => {}
            _ = shutdown.changed() => return,
            r = target.changed() => if r.is_err() { return },
        }
    }
}

/// Sends one file and acts on the answer. `Err` means try later.
async fn send_one(spool: &Arc<Spool>, sink: &mut dyn Sink, file: &ReadyFile) -> Result<(), String> {
    let finish = |acked: bool| {
        let (spool, stem) = (spool.clone(), file.stem.clone());
        async move {
            tokio::task::spawn_blocking(move || if acked { spool.ack(&stem) } else { spool.reject(&stem) })
                .await
                .map_err(|e| e.to_string())?
                .map_err(|e| e.to_string())
        }
    };
    let body = match tokio::fs::read(&file.path).await {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Dropped by the cap meanwhile.
            return finish(false).await;
        }
        Err(e) => return Err(e.to_string()),
    };
    match sink.upload(&file.stem, body).await {
        Outcome::Stored => finish(true).await,
        Outcome::Reject(reason) => {
            tracing::warn!(file = %file.stem, %reason, "the analytics worker refused a spool file; dropped");
            finish(false).await
        }
        Outcome::Retry(reason) => Err(reason),
    }
}
