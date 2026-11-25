//! Ingest (D-141): service logs and heartbeat telemetry, forwarded by
//! forge-server as the sender sent them. This module decodes, attributes
//! and hands rows to the batch writer or the minute buffers. The agent
//! spool upload (one file per request) lands here later as a third path.

pub mod attribution;
pub mod dedupe;
pub mod seen;
pub mod spool_file;
pub mod upload;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use forge_analytics_proto::WorkerError;
use forge_analytics_proto::tuning::Tuning;
use forge_core::analytics::{ContainerMinute, FileWrite, SpoolFileWriter, SpoolRows};
use forge_core::logs::{ContainerContext, IngestContext, LogEvent, LogStream, container_event, parse_batch};
use forge_core::{ServerId, ServiceId, Timestamp};
use forge_proto::messages::{
    ContainerLogBatch, ContainerSample, ContainerStream, MAX_CONTAINER_FIELD_BYTES, MAX_CONTAINER_LINE_BYTES,
    MAX_CONTAINER_LOG_LINES, MAX_CONTAINER_SAMPLES, NodeMetrics,
};
use prost::Message;
use tokio::sync::watch;

use self::attribution::{attribute, attribute_node, clip};
use self::dedupe::{BatchKey, Stored};
use crate::analytics::Analytics;
use crate::batcher::{BatchError, LogBatcher};
use crate::maps::Maps;

/// `free_disk` before the first reading: unknown, so ingest is not refused.
pub const FREE_DISK_UNKNOWN: u64 = u64::MAX;
const MINUTE_MS: i64 = 60_000;

impl From<BatchError> for WorkerError {
    fn from(e: BatchError) -> Self {
        match e {
            BatchError::Busy => WorkerError::Busy,
            BatchError::Write(reason) => WorkerError::Unavailable { reason },
            BatchError::Stopped => WorkerError::Stopped,
        }
    }
}

/// Why a spool file was not stored.
#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    /// Not now: the store is down, busy or short of disk. Send it again.
    #[error(transparent)]
    Refused(WorkerError),
    /// Never: the file cannot be read.
    #[error(transparent)]
    File(spool_file::FileError),
}

/// Wiring for [`Ingest::new`].
#[derive(Debug)]
pub struct IngestDeps {
    pub analytics: Arc<Analytics>,
    pub batcher: LogBatcher,
    pub maps: Arc<Maps>,
    pub tuning: watch::Receiver<Tuning>,
    pub free_disk: Arc<AtomicU64>,
    /// Stores the rows of one spool file (exactly once where the store
    /// records file ids, D-143).
    pub spool: Arc<dyn SpoolFileWriter>,
}

/// What importing one spool file did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileImport {
    /// Rows read from the file.
    pub rows: u64,
    /// Rows skipped: unreadable, or an application key not valid here.
    pub skipped: u64,
    pub write: FileWrite,
}

/// Decodes, attributes and stores what forge-server forwards.
#[derive(Debug)]
pub struct Ingest {
    analytics: Arc<Analytics>,
    batcher: LogBatcher,
    maps: Arc<Maps>,
    tuning: watch::Receiver<Tuning>,
    /// The engine's free disk, bytes, refreshed by the usage sampler.
    free_disk: Arc<AtomicU64>,
    spool: Arc<dyn SpoolFileWriter>,
    stored: Mutex<Stored>,
}

impl Ingest {
    #[must_use]
    pub fn new(d: IngestDeps) -> Self {
        Self {
            analytics: d.analytics,
            batcher: d.batcher,
            maps: d.maps,
            tuning: d.tuning,
            free_disk: d.free_disk,
            spool: d.spool,
            stored: Mutex::new(Stored::default()),
        }
    }

    /// Refuses at once while the store is down or short of disk.
    pub fn accepting(&self) -> Result<(), WorkerError> {
        self.analytics.usable()?;
        let min_free = self.tuning.borrow().min_free_disk_bytes();
        let free = self.free_disk.load(Ordering::Relaxed);
        if free != FREE_DISK_UNKNOWN && free < min_free {
            return Err(WorkerError::DiskLow);
        }

        Ok(())
    }

    /// Stores a Seq/CLEF body sent with a key of `service`; returns how
    /// many events it held. Answered after the events are stored.
    pub async fn logs(&self, service: ServiceId, now: Timestamp, body: &[u8]) -> Result<usize, WorkerError> {
        self.accepting()?;
        let text =
            std::str::from_utf8(body).map_err(|_| WorkerError::BadBody { reason: "the body is not UTF-8".into() })?;
        let cx = IngestContext { service_id: service, server_id: None, node_ts: None, now };
        let mut events = parse_batch(text, cx).map_err(|e| WorkerError::BadBody { reason: e.to_string() })?;
        if events.iter().any(|e| e.server_id.is_none()) {
            let maps = self.maps.get();
            let nodes = maps.placements.get(&service).map_or(&[][..], Vec::as_slice);

            for e in events.iter_mut().filter(|e| e.server_id.is_none()) {
                e.server_id = attribute_node(&e.properties, nodes);
            }
        }

        let count = events.len();
        if count > 0 {
            self.batcher.write(events).await?;
        }

        Ok(count)
    }

    /// Records a heartbeat's metrics and stores its container lines.
    /// Returns the container batch `seq` to acknowledge, once stored.
    pub async fn telemetry(
        &self,
        server: ServerId,
        now: Timestamp,
        metrics: &[u8],
        logs: &[u8],
    ) -> Result<u64, WorkerError> {
        if !metrics.is_empty() {
            let m = NodeMetrics::decode(metrics).map_err(|e| WorkerError::BadBody { reason: e.to_string() })?;
            self.analytics.observe(server, now, &m);

            if !m.containers.is_empty() {
                let rows = self.minutes(server, now, &m.containers);
                self.analytics.push_container_minutes(rows);
            }
        }

        if logs.is_empty() {
            return Ok(0);
        }

        let batch = ContainerLogBatch::decode(logs).map_err(|e| WorkerError::BadBody { reason: e.to_string() })?;
        let key = BatchKey::of(server, &batch);
        if self.stored.lock().unwrap_or_else(PoisonError::into_inner).contains(&key) {
            // Stored before; its first answer did not reach the node.
            return Ok(batch.seq);
        }

        self.accepting()?;

        if batch.dropped > 0 {
            tracing::warn!(server = %server, dropped = batch.dropped, "the node dropped container log lines (buffer full)");
        }

        let events = self.events(server, now, &batch);
        if !events.is_empty() {
            self.batcher.write(events).await?;
        }

        self.stored.lock().unwrap_or_else(PoisonError::into_inner).insert(key);

        Ok(batch.seq)
    }

    /// Reads and stores one agent spool file of `node` (D-142); returns
    /// the rows stored. Answered after the store confirmed every insert.
    pub async fn spool_file(
        &self,
        node: ServerId,
        file_id: &str,
        now: Timestamp,
        bytes: bytes::Bytes,
    ) -> Result<FileImport, ImportError> {
        self.accepting().map_err(ImportError::Refused)?;
        let maps = self.maps.get();
        let imported = tokio::task::spawn_blocking(move || spool_file::read(&bytes, node, now, &maps))
            .await
            .map_err(|e| ImportError::Refused(WorkerError::Unavailable { reason: e.to_string() }))?
            .map_err(ImportError::File)?;
        self.analytics.usable().map_err(|e| ImportError::Refused(e.into()))?;
        let (rows, skipped) = (imported.rows(), imported.skipped);
        let file = SpoolRows {
            node,
            file_id: file_id.to_owned(),
            logs: imported.logs,
            minutes: imported.minutes,
            processes: imported.processes,
            containers: imported.containers,
        };
        let write = self
            .spool
            .write_spool_file(file)
            .await
            .map_err(|e| ImportError::Refused(WorkerError::Unavailable { reason: e.to_string() }))?;

        Ok(FileImport { rows, skipped, write })
    }

    /// The pushed maps.
    #[must_use]
    pub fn maps(&self) -> &Arc<Maps> {
        &self.maps
    }

    /// A node's batch as events (at most [`MAX_CONTAINER_LOG_LINES`]).
    fn events(&self, server: ServerId, now: Timestamp, batch: &ContainerLogBatch) -> Vec<LogEvent> {
        let maps = self.maps.get();
        let services = maps.apps.get(&server);

        batch
            .lines
            .iter()
            .take(MAX_CONTAINER_LOG_LINES)
            .map(|l| {
                let a = attribute(l.labels.as_ref(), services);
                let cx = ContainerContext {
                    service_id: a.service_id,
                    server_id: server,
                    container_id: clip(&l.container_id, 64).to_owned(),
                    container_name: clip(&l.container_name, MAX_CONTAINER_FIELD_BYTES).to_owned(),
                    stack: a.stack,
                    now,
                };

                let stream = match ContainerStream::try_from(l.stream) {
                    Ok(ContainerStream::Stderr) => LogStream::Stderr,
                    _ => LogStream::Stdout,
                };

                container_event(&cx, stream, l.ts_ms, clip(&l.line, MAX_CONTAINER_LINE_BYTES))
            })
            .collect()
    }

    /// Per-minute rows of a node's samples (at most [`MAX_CONTAINER_SAMPLES`]).
    fn minutes(&self, server: ServerId, now: Timestamp, samples: &[ContainerSample]) -> Vec<ContainerMinute> {
        let maps = self.maps.get();
        let services = maps.apps.get(&server);
        let minute = Timestamp(now.0 - now.0.rem_euclid(MINUTE_MS));

        samples
            .iter()
            .take(MAX_CONTAINER_SAMPLES)
            .filter(|s| !s.container_name.is_empty())
            .map(|s| {
                let a = attribute(s.labels.as_ref(), services);
                ContainerMinute {
                    server_id: server,
                    minute,
                    container_name: clip(&s.container_name, MAX_CONTAINER_FIELD_BYTES).to_owned(),
                    container_id: clip(&s.container_id, 64).to_owned(),
                    image: clip(&s.image, MAX_CONTAINER_FIELD_BYTES).to_owned(),
                    service_id: a.service_id,
                    stack: a.stack,
                    cpu: if s.cpu_percent.is_finite() { s.cpu_percent.max(0.0) } else { 0.0 },
                    mem_used: s.mem_used_bytes,
                    mem_limit: s.mem_limit_bytes,
                    rx: s.net_rx_bytes_per_sec,
                    tx: s.net_tx_bytes_per_sec,
                    block_read: s.block_read_bytes_per_sec,
                    block_write: s.block_write_bytes_per_sec,
                    restarts: s.restarts,
                }
            })
            .collect()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // reason: tests
mod tests {
    use super::*;
    use crate::batcher::tests::{ready, tuning};
    use crate::memory::MemoryStore;
    use forge_core::analytics::{AnalyticsMaps, AppKey};
    use forge_proto::messages::{ContainerLabels, ContainerLogLine};
    use std::time::Duration;

    fn ingest(analytics: Arc<Analytics>, maps: Arc<Maps>) -> Ingest {
        let rx = watch::channel(tuning()).1;
        Ingest::new(IngestDeps {
            batcher: LogBatcher::start(analytics.clone(), rx.clone()),
            spool: Arc::new(forge_core::analytics::PerTableSpoolWriter(analytics.configured_store().clone())),
            analytics,
            maps,
            tuning: rx,
            free_disk: Arc::new(AtomicU64::new(FREE_DISK_UNKNOWN)),
        })
    }

    fn line(i: usize, stream: ContainerStream, text: &str) -> ContainerLogLine {
        ContainerLogLine {
            container_id: "0123456789ab".into(),
            container_name: format!("c{i}"),
            image: "img".into(),
            labels: Some(ContainerLabels {
                forge_app: "prod/api".into(),
                forge_stack: String::new(),
                compose_project: "shop".into(),
                compose_service: String::new(),
            }),
            stream: stream as i32,
            ts_ms: 1_000 + i64::try_from(i).unwrap(),
            line: text.into(),
        }
    }

    #[tokio::test]
    async fn batches_are_bounded_and_unplaced_services_stay_unattributed() {
        let maps = Arc::new(Maps::default());
        let t = ingest(ready(Arc::new(MemoryStore::default())).await, maps.clone());
        let node = ServerId::new();
        let mut lines: Vec<_> =
            (0..MAX_CONTAINER_LOG_LINES + 10).map(|i| line(i, ContainerStream::Stdout, "ok")).collect();
        lines[0] = line(0, ContainerStream::Stderr, "careful");
        lines[0].line = "x".repeat(MAX_CONTAINER_LINE_BYTES + 100);
        let batch = ContainerLogBatch { seq: 1, lines, dropped: 0 };
        let events = t.events(node, Timestamp(5), &batch);
        assert_eq!(events.len(), MAX_CONTAINER_LOG_LINES, "at most one batch's worth of lines");
        let e = &events[0];
        assert_eq!(e.server_id, Some(node), "the node comes from the link");
        assert_eq!(e.service_id, None, "prod/api is not placed on this node");
        assert_eq!(e.stack.as_deref(), Some("shop"));
        assert_eq!(e.level, forge_core::logs::LogLevel::Warning, "stderr without a level");
        assert!(e.message.len() <= MAX_CONTAINER_LINE_BYTES + 4, "long lines are cut");

        // Once the service is placed there, lines are attributed to it.
        let svc = ServiceId::new();
        maps.replace(AnalyticsMaps {
            app_keys: vec![AppKey { server_id: node, app: "prod/api".into(), service_id: svc }],
            ..AnalyticsMaps::default()
        })
        .unwrap();
        assert_eq!(t.events(node, Timestamp(5), &batch)[1].service_id, Some(svc));
        let samples: Vec<ContainerSample> = (0..MAX_CONTAINER_SAMPLES + 5)
            .map(|i| ContainerSample { container_name: format!("c{i}"), cpu_percent: f32::NAN, ..Default::default() })
            .collect();
        let rows = t.minutes(node, Timestamp(125_000), &samples);
        assert_eq!(rows.len(), MAX_CONTAINER_SAMPLES);
        assert_eq!(rows[0].minute, Timestamp(120_000));
        assert!(rows[0].cpu.abs() < f32::EPSILON, "a bad number is stored as 0");
    }

    /// The node's ack was lost (forge-server timed out while the insert ran):
    /// the batch it sends again is acknowledged, not stored a second time.
    #[tokio::test]
    async fn a_batch_sent_again_after_an_ack_timeout_is_not_written_twice() {
        let store = Arc::new(MemoryStore::default());
        let gate = store.hold_writes();
        let t = Arc::new(ingest(ready(store.clone()).await, Arc::new(Maps::default())));
        let node = ServerId::new();
        let batch = ContainerLogBatch {
            seq: 7,
            lines: (0..3).map(|i| line(i, ContainerStream::Stdout, "hello")).collect(),
            dropped: 0,
        };
        let bytes = batch.encode_to_vec();

        // The first attempt times out on the server side while the insert runs.
        let first = tokio::spawn({
            let (t, bytes) = (t.clone(), bytes.clone());
            async move { t.telemetry(node, Timestamp(1), &[], &bytes).await }
        });
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);

        while store.writes_started() == 0 {
            assert!(tokio::time::Instant::now() < deadline, "the insert never started");
            tokio::task::yield_now().await;
        }

        gate.release();
        assert_eq!(first.await.unwrap(), Ok(7));

        // The node never saw the ack and sends the same batch again.
        assert_eq!(t.telemetry(node, Timestamp(2), &[], &bytes).await, Ok(7));
        assert_eq!(store.log_count(), 3, "stored once");
        assert_eq!(store.writes_started(), 1);

        // A different batch with a reused seq (the helper restarted) is stored.
        let again = ContainerLogBatch { seq: 7, lines: vec![line(9, ContainerStream::Stdout, "new")], dropped: 0 };
        assert_eq!(t.telemetry(node, Timestamp(3), &[], &again.encode_to_vec()).await, Ok(7));
        assert_eq!(store.log_count(), 4);
    }

    #[tokio::test]
    async fn clef_events_are_attributed_to_placed_nodes_and_bad_bodies_refused() {
        let store = Arc::new(MemoryStore::default());
        let maps = Arc::new(Maps::default());
        let t = ingest(ready(store.clone()).await, maps.clone());
        let (svc, node) = (ServiceId::new(), ServerId::new());
        maps.replace(AnalyticsMaps {
            placements: vec![forge_core::analytics::Placement {
                service_id: svc,
                server_id: node,
                names: vec!["web-1".into()],
            }],
            ..AnalyticsMaps::default()
        })
        .unwrap();
        let body = br#"{"@t":"2024-01-01T00:00:00Z","@mt":"hi","MachineName":"WEB-1"}"#;
        assert_eq!(t.logs(svc, Timestamp(1), body).await, Ok(1));
        assert_eq!(store.log_count(), 1);
        assert!(matches!(t.logs(svc, Timestamp(1), b"not json").await, Err(WorkerError::BadBody { .. })));
        assert!(matches!(t.logs(svc, Timestamp(1), &[0xff, 0xfe]).await, Err(WorkerError::BadBody { .. })));
    }
}
