//! Coalesces log writes into few large inserts (D-135), moved from
//! forge-server (D-141).
//!
//! Requests hand their parsed events to one writer task and wait for it:
//! the writer takes whatever is queued, lingers briefly for more, and writes
//! it all in one `INSERT`. Each request is answered only after its events
//! are stored (or the write failed), so a stored answer means stored.
//!
//! Bounded: the queue holds at most [`QUEUE_REQUESTS`] requests and
//! `ingest_max_queued_mb` of **decoded** events (what the events take in
//! memory, not the bytes received). When either is full, [`LogBatcher::write`]
//! answers [`BatchError::Busy`] at once and keeps nothing.
//!
//! A request whose waiter has gone (forge-server timed out and dropped the
//! connection) is skipped before the insert, so a batch the sender will
//! resend is not also written late.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use forge_analytics_proto::tuning::Tuning;
use forge_analytics_proto::views::QueueStats;
use forge_core::logs::LogEvent;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::{Instant, timeout_at};

use crate::analytics::Analytics;

/// Requests waiting for the writer before ingest answers "busy".
pub const QUEUE_REQUESTS: usize = 4_096;

/// Why a batch could not be stored.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BatchError {
    /// The queue is full: the sender should retry later.
    #[error("log ingest is busy; retry shortly")]
    Busy,
    /// The analytics database refused or failed the insert.
    #[error("{0}")]
    Write(String),
    /// The writer task ended (worker shutting down).
    #[error("the log writer stopped")]
    Stopped,
}

struct Pending {
    events: Vec<LogEvent>,
    bytes: u64,
    done: oneshot::Sender<Result<(), BatchError>>,
}

/// What is waiting for the writer.
#[derive(Debug, Default)]
struct Queue {
    bytes: AtomicU64,
    /// When each waiting request was queued, oldest first.
    since: Mutex<VecDeque<Instant>>,
}

/// Bytes `e` takes in memory, approximately: its strings and properties.
#[must_use]
pub fn decoded_size(e: &LogEvent) -> u64 {
    fn value(v: &serde_json::Value) -> usize {
        match v {
            serde_json::Value::String(s) => 24 + s.len(),
            serde_json::Value::Array(a) => 24 + a.iter().map(value).sum::<usize>(),
            serde_json::Value::Object(o) => 48 + o.iter().map(|(k, v)| 24 + k.len() + value(v)).sum::<usize>(),
            _ => 16,
        }
    }
    let opt = |s: &Option<String>| s.as_ref().map_or(0, String::len);
    let n = 256
        + e.message.len()
        + e.message_template.len()
        + opt(&e.exception)
        + opt(&e.trace_id)
        + opt(&e.span_id)
        + opt(&e.container_id)
        + opt(&e.container_name)
        + opt(&e.stack)
        + e.properties.iter().map(|(k, v)| 24 + k.len() + value(v)).sum::<usize>();
    u64::try_from(n).unwrap_or(u64::MAX)
}

/// Handle to the writer task. Cheap to share; the task ends when the last
/// handle is dropped.
#[derive(Debug, Clone)]
pub struct LogBatcher {
    tx: mpsc::Sender<Pending>,
    queue: Arc<Queue>,
    tuning: watch::Receiver<Tuning>,
}

impl LogBatcher {
    /// Starts the writer task on the current Tokio runtime.
    #[must_use]
    pub fn start(analytics: Arc<Analytics>, tuning: watch::Receiver<Tuning>) -> Self {
        let (tx, rx) = mpsc::channel(QUEUE_REQUESTS);
        let queue = Arc::new(Queue::default());
        tokio::spawn(run(analytics, rx, queue.clone(), tuning.clone()));
        Self { tx, queue, tuning }
    }

    /// Queues `events` and waits until they are stored.
    pub async fn write(&self, events: Vec<LogEvent>) -> Result<(), BatchError> {
        let bytes: u64 = events.iter().map(decoded_size).sum();
        let max = self.tuning.borrow().max_queued_bytes();
        // Reserve the bytes first so concurrent senders cannot overshoot.
        let before = self.queue.bytes.fetch_add(bytes, Ordering::AcqRel);
        if before > 0 && before + bytes > max {
            self.queue.bytes.fetch_sub(bytes, Ordering::AcqRel);
            return Err(BatchError::Busy);
        }
        let (done, wait) = oneshot::channel();
        // Queued before sending, so the writer always finds an entry to pop.
        self.queue.since.lock().unwrap_or_else(PoisonError::into_inner).push_back(Instant::now());
        if let Err(e) = self.tx.try_send(Pending { events, bytes, done }) {
            self.queue.bytes.fetch_sub(bytes, Ordering::AcqRel);
            self.queue.since.lock().unwrap_or_else(PoisonError::into_inner).pop_back();
            return Err(match e {
                mpsc::error::TrySendError::Full(_) => BatchError::Busy,
                mpsc::error::TrySendError::Closed(_) => BatchError::Stopped,
            });
        }
        wait.await.map_err(|_| BatchError::Stopped)?
    }

    #[must_use]
    pub fn stats(&self) -> QueueStats {
        let since = self.queue.since.lock().unwrap_or_else(PoisonError::into_inner);
        QueueStats {
            queued_bytes: self.queue.bytes.load(Ordering::Acquire),
            queued_requests: since.len(),
            oldest_ms: since.front().map(|t| u64::try_from(t.elapsed().as_millis()).unwrap_or(u64::MAX)),
        }
    }
}

/// Takes the first queued request, then everything else that arrives within
/// the flush interval (up to the rows per insert), and writes it in one insert.
async fn run(
    analytics: Arc<Analytics>,
    mut rx: mpsc::Receiver<Pending>,
    queue: Arc<Queue>,
    tuning: watch::Receiver<Tuning>,
) {
    while let Some(first) = rx.recv().await {
        let (linger, max_rows) = {
            let t = tuning.borrow();
            (Duration::from_millis(u64::from(t.ingest_flush_interval_ms)), t.ingest_max_batch_rows as usize)
        };
        let mut rows = first.events.len();
        let mut batch = vec![first];
        let deadline = Instant::now() + linger;
        while rows < max_rows {
            match timeout_at(deadline, rx.recv()).await {
                Ok(Some(p)) => {
                    rows += p.events.len();
                    batch.push(p);
                }
                // Closed (shutdown) or the linger time is over: write what we have.
                Ok(None) | Err(_) => break,
            }
        }
        write_batch(&analytics, &queue, batch).await;
    }
}

async fn write_batch(analytics: &Analytics, queue: &Queue, batch: Vec<Pending>) {
    let requests = batch.len();
    let mut events = Vec::new();
    let mut waiting = Vec::with_capacity(batch.len());
    let (mut bytes, mut skipped) = (0, 0usize);
    for p in batch {
        bytes += p.bytes;
        // Nobody waits for it any more: the sender was told it failed and
        // will send it again, so writing it now would store it twice.
        if p.done.is_closed() {
            skipped += 1;
            continue;
        }
        events.extend(p.events);
        waiting.push(p.done);
    }
    let result = if events.is_empty() {
        Ok(())
    } else {
        match analytics.usable() {
            Ok(store) => store.write_logs(&events).await.map_err(|e| BatchError::Write(e.to_string())),
            Err(e) => Err(BatchError::Write(e.to_string())),
        }
    };
    match &result {
        Ok(()) => tracing::debug!(rows = events.len(), requests, skipped, "log batch stored"),
        Err(e) => tracing::warn!(rows = events.len(), requests, error = %e, "log batch write failed"),
    }
    queue.bytes.fetch_sub(bytes, Ordering::AcqRel);
    {
        let mut since = queue.since.lock().unwrap_or_else(PoisonError::into_inner);
        for _ in 0..requests {
            since.pop_front();
        }
    }
    for done in waiting {
        // The sender may have gone meanwhile; nothing to tell it.
        let _ = done.send(result.clone());
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // reason: tests
pub(crate) mod tests {
    use super::*;
    use crate::memory::MemoryStore;
    use forge_core::analytics::Retention;
    use forge_core::logs::{IngestContext, parse_batch};
    use forge_core::{ServiceId, Timestamp};

    pub(crate) fn tuning() -> Tuning {
        Tuning {
            node_metrics_days: 7,
            top_processes_days: 7,
            node_rollup_days: 365,
            app_logs_days: 30,
            cpu_quota_vcpu: 0.0,
            memory_budget_mb: 1024,
            search_max_concurrent: 8,
            search_timeout_secs: 20,
            search_max_lookback_days: 0,
            search_threads: 0,
            search_memory_mb: 0,
            ingest_max_queued_mb: 1,
            ingest_max_batch_rows: 20_000,
            ingest_flush_interval_ms: 10,
            min_free_disk_mb: 0,
        }
    }

    pub(crate) fn events(n: usize, size: usize) -> Vec<LogEvent> {
        let line = format!("{{\"@t\":\"2024-01-01T00:00:00Z\",\"@mt\":\"{}\"}}\n", "x".repeat(size));
        let cx = IngestContext { service_id: ServiceId::new(), server_id: None, node_ts: None, now: Timestamp(1) };
        parse_batch(&line.repeat(n), cx).unwrap()
    }

    pub(crate) async fn ready(store: Arc<MemoryStore>) -> Arc<Analytics> {
        let a = Arc::new(Analytics::new(
            store,
            Retention { node_metrics_days: 7, top_processes_days: 7, node_rollup_days: 365, app_logs_days: 30 },
        ));
        assert!(a.prepare().await);
        a
    }

    #[tokio::test]
    async fn a_full_queue_answers_busy_at_once_and_holds_nothing() {
        let store = Arc::new(MemoryStore::default());
        // Every insert waits until released, so the queue fills.
        let gate = store.hold_writes();
        let a = ready(store.clone()).await;
        let b = LogBatcher::start(a, watch::channel(tuning()).1);
        // About 0.6 MiB decoded: fits the 1 MiB cap once.
        let first = tokio::spawn({
            let b = b.clone();
            async move { b.write(events(20, 30_000)).await }
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while b.stats().queued_requests == 0 {
            assert!(Instant::now() < deadline, "the first batch never queued");
            tokio::task::yield_now().await;
        }
        let queued = b.stats().queued_bytes;
        assert!(queued > 500_000, "decoded size is counted: {queued}");
        let second = events(20, 30_000);
        let started = Instant::now();
        assert_eq!(b.write(second).await, Err(BatchError::Busy));
        assert!(started.elapsed() < Duration::from_millis(200), "busy is immediate");
        assert_eq!(b.stats().queued_bytes, queued, "the refused batch holds nothing");
        gate.release();
        assert_eq!(first.await.unwrap(), Ok(()));
        assert_eq!(store.log_count(), 20);
        assert_eq!(b.stats().queued_bytes, 0);
    }

    #[tokio::test]
    async fn a_batch_whose_waiter_left_is_not_written() {
        let store = Arc::new(MemoryStore::default());
        let gate = store.hold_writes();
        let a = ready(store.clone()).await;
        let b = LogBatcher::start(a, watch::channel(tuning()).1);
        // The first batch occupies the writer.
        let first = tokio::spawn({
            let b = b.clone();
            async move { b.write(events(1, 10)).await }
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while store.writes_started() == 0 {
            assert!(Instant::now() < deadline, "the writer never started");
            tokio::task::yield_now().await;
        }
        // The second waits behind it, then its waiter gives up.
        let late = b.write(events(5, 10));
        assert!(tokio::time::timeout(Duration::from_millis(50), late).await.is_err());
        gate.release();
        assert_eq!(first.await.unwrap(), Ok(()));
        let deadline = Instant::now() + Duration::from_secs(5);
        while b.stats().queued_requests > 0 {
            assert!(Instant::now() < deadline, "the queue never drained");
            tokio::task::yield_now().await;
        }
        assert_eq!(store.log_count(), 1, "the abandoned batch was skipped");
    }
}
