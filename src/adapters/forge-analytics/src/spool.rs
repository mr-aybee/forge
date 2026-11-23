//! Spool file writes to ClickHouse and the embedded engine (D-145).
//!
//! - **Group commit.** One task writes. It takes the first queued file and
//!   every file already queued behind it (up to `batch_rows` rows and
//!   [`MAX_FILES`] files) and stores them with one insert per table, the
//!   four tables at once. Nothing waits on a timer: a lone file is written
//!   at once, and under load the files that arrive while a batch is being
//!   written form the next one, so inserts grow with the load while their
//!   number per second stays bounded by the engine's write time. Each file
//!   is answered only once every table it has rows in confirmed the insert.
//! - **Once per table.** Every insert carries an
//!   `insert_deduplication_token` derived from the file ids it holds and the
//!   table: `<file id>:<table>` for one file, a hash of the ids for a group.
//!   The tables keep a deduplication window, so an insert sent again with
//!   the same token is acknowledged without writing. A failed insert is
//!   retried at once with the same rows and token, so a timeout whose insert
//!   did land is not written twice. When a table still fails, the tables
//!   that succeeded are remembered per file, and the file sent again by its
//!   agent is written only to the tables it is missing.
//!
//! What is not covered: the worker stops (crash or restart) between two
//! table inserts of a group of several files. The agents then send those
//! files again, they form other groups with other tokens, and the tables
//! already written get their rows twice. A file stored alone (the usual case
//! at light load) is covered by its token even then.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use forge_core::analytics::{FileWrite, SpoolFileWriter, SpoolRows};
use forge_core::{DomainError, DomainResult, ServerId};
use tokio::sync::{mpsc, oneshot};

use crate::ClickHouse;
use crate::clickhouse::{
    APP_LOGS, CONTAINER_MINUTES, ContainerRow, LogRow, MinuteRow, NODE_MINUTES, ProcRow, TOP_PROCESSES, ndjson,
};

/// Most files in one batch.
pub const MAX_FILES: usize = 256;
/// Files waiting for the writer; senders wait beyond this (the upload slots
/// already bound how many there are).
const QUEUE: usize = 64;
/// Files whose stored tables are remembered.
const REMEMBERED: usize = 8_192;
/// Delays before the retries of a failed insert (same rows, same token).
const RETRIES: [Duration; 2] = [Duration::from_millis(250), Duration::from_secs(1)];

/// The spool tables, in a fixed order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Table {
    Logs,
    NodeMinutes,
    Processes,
    Containers,
}

impl Table {
    pub(crate) const ALL: [Table; 4] = [Table::Logs, Table::NodeMinutes, Table::Processes, Table::Containers];

    pub(crate) fn name(self) -> &'static str {
        match self {
            Table::Logs => APP_LOGS,
            Table::NodeMinutes => NODE_MINUTES,
            Table::Processes => TOP_PROCESSES,
            Table::Containers => CONTAINER_MINUTES,
        }
    }

    fn bit(self) -> u8 {
        match self {
            Table::Logs => 1,
            Table::NodeMinutes => 2,
            Table::Processes => 4,
            Table::Containers => 8,
        }
    }

    fn rows(self, f: &SpoolRows) -> usize {
        match self {
            Table::Logs => f.logs.len(),
            Table::NodeMinutes => f.minutes.len(),
            Table::Processes => f.processes.len(),
            Table::Containers => f.containers.len(),
        }
    }

    /// Appends `f`'s rows of this table to `out` as JSONEachRow.
    fn encode(self, f: &SpoolRows, out: &mut String) -> DomainResult<()> {
        match self {
            Table::Logs => ndjson(f.logs.iter().map(LogRow::from), out),
            Table::NodeMinutes => ndjson(f.minutes.iter().map(MinuteRow::from), out),
            Table::Processes => ndjson(f.processes.iter().map(ProcRow::from), out),
            Table::Containers => ndjson(f.containers.iter().map(ContainerRow::from), out),
        }
    }
}

/// Engine settings of a spool insert with `token`. Synchronous: the worker
/// already groups files, so the engine need not; the answer comes once the
/// rows are written. Deduplication also covers the materialized views that
/// build the hourly rollups. `async_insert_deduplicate` keeps the token
/// working where a user profile turns asynchronous inserts on.
#[must_use]
pub fn insert_settings(token: &str) -> [(&'static str, String); 5] {
    [
        ("insert_deduplicate", "1".to_owned()),
        ("insert_deduplication_token", token.to_owned()),
        ("deduplicate_blocks_in_dependent_materialized_views", "1".to_owned()),
        ("async_insert", "0".to_owned()),
        ("async_insert_deduplicate", "1".to_owned()),
    ]
}

/// The deduplication token of one insert into `table` holding the files
/// `ids` (`(node, file id)`, in batch order).
#[must_use]
pub fn token(table: &str, ids: &[(ServerId, &str)]) -> String {
    match ids {
        [(_, id)] => format!("{id}:{table}"),
        _ => {
            let mut all = String::with_capacity(ids.len() * 70);
            for (node, id) in ids {
                all.push_str(&node.to_string());
                all.push('/');
                all.push_str(id);
                all.push('\n');
            }
            format!("g{}:{table}", forge_secrets::crypto::sha256_hex(all.as_bytes()))
        }
    }
}

/// Where the writer's inserts go: ClickHouse, or a fake in tests.
#[async_trait]
pub(crate) trait TableSink: Send + Sync + 'static {
    async fn insert_once(&self, table: Table, body: String, token: &str) -> DomainResult<()>;
}

#[async_trait]
impl TableSink for ClickHouse {
    async fn insert_once(&self, table: Table, body: String, token: &str) -> DomainResult<()> {
        ClickHouse::insert_once(self, table.name(), body, token).await
    }
}

struct Job {
    file: SpoolRows,
    reply: oneshot::Sender<DomainResult<FileWrite>>,
}

/// [`SpoolFileWriter`] for ClickHouse and the embedded engine.
#[derive(Debug, Clone)]
pub struct ClickHouseSpoolWriter {
    tx: mpsc::Sender<Job>,
}

impl std::fmt::Debug for Job {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Job").field("file", &self.file.file_id).finish_non_exhaustive()
    }
}

fn stopped() -> DomainError {
    DomainError::Internal("analytics: the spool writer stopped".into())
}

impl ClickHouseSpoolWriter {
    /// Starts the writer task (inside a Tokio runtime); batches hold up to
    /// `batch_rows` rows. It stops when the last handle is dropped.
    #[must_use]
    pub fn start(store: ClickHouse, batch_rows: usize) -> Self {
        Self::with_sink(Arc::new(store), batch_rows, &RETRIES)
    }

    pub(crate) fn with_sink(sink: Arc<dyn TableSink>, batch_rows: usize, retries: &'static [Duration]) -> Self {
        let (tx, rx) = mpsc::channel(QUEUE);
        tokio::spawn(run(sink, rx, batch_rows.max(1), retries));
        Self { tx }
    }
}

#[async_trait]
impl SpoolFileWriter for ClickHouseSpoolWriter {
    async fn write_spool_file(&self, file: SpoolRows) -> DomainResult<FileWrite> {
        let (reply, rx) = oneshot::channel();
        self.tx.send(Job { file, reply }).await.map_err(|_| stopped())?;
        rx.await.map_err(|_| stopped())?
    }
}

/// The tables already stored per file: files whose insert failed in some
/// table, and files stored in full (a file sent twice at once). Bounded;
/// the oldest is forgotten first.
#[derive(Debug, Default)]
struct Stored {
    tables: HashMap<(ServerId, String), u8>,
    order: VecDeque<(ServerId, String)>,
}

impl Stored {
    fn get(&self, node: ServerId, id: &str) -> u8 {
        self.tables.get(&(node, id.to_owned())).copied().unwrap_or(0)
    }

    fn add(&mut self, node: ServerId, id: &str, mask: u8) {
        let key = (node, id.to_owned());
        if let Some(m) = self.tables.get_mut(&key) {
            *m |= mask;
            return;
        }
        if self.order.len() >= REMEMBERED
            && let Some(old) = self.order.pop_front()
        {
            self.tables.remove(&old);
        }
        self.tables.insert(key.clone(), mask);
        self.order.push_back(key);
    }
}

async fn run(sink: Arc<dyn TableSink>, mut rx: mpsc::Receiver<Job>, batch_rows: usize, retries: &'static [Duration]) {
    let mut stored = Stored::default();
    while let Some(first) = rx.recv().await {
        let mut rows = first.file.rows();
        let mut batch = vec![first];
        while batch.len() < MAX_FILES && rows < batch_rows as u64 {
            match rx.try_recv() {
                Ok(j) => {
                    rows += j.file.rows();
                    batch.push(j);
                }
                Err(_) => break,
            }
        }
        write_batch(sink.as_ref(), &mut stored, batch, retries).await;
    }
}

/// One table's insert of a batch.
struct Insert {
    table: Table,
    token: String,
    body: String,
}

/// The tables `f` has rows in.
fn needed(f: &SpoolRows) -> u8 {
    Table::ALL.iter().filter(|t| t.rows(f) > 0).fold(0, |m, t| m | t.bit())
}

async fn write_batch(sink: &dyn TableSink, stored: &mut Stored, batch: Vec<Job>, retries: &'static [Duration]) {
    // Per job: the tables it still needs; a file twice in one batch is
    // written once and both are answered alike.
    let mut first_of: HashMap<(ServerId, &str), usize> = HashMap::new();
    let mut todo = vec![0u8; batch.len()];
    let mut dup = vec![None; batch.len()];
    for (i, j) in batch.iter().enumerate() {
        let f = &j.file;
        if let Some(&k) = first_of.get(&(f.node, f.file_id.as_str())) {
            dup[i] = Some(k);
            continue;
        }
        first_of.insert((f.node, f.file_id.as_str()), i);
        todo[i] = needed(f) & !stored.get(f.node, &f.file_id);
    }
    let inserts = match build(&batch, &todo) {
        Ok(v) => v,
        Err(e) => {
            for j in batch {
                let _ = j.reply.send(Err(DomainError::Internal(e.to_string())));
            }
            return;
        }
    };
    let results = futures::future::join_all(inserts.iter().map(|ins| attempt(sink, ins, retries))).await;
    let mut ok_mask = 0u8;
    let mut error: Option<String> = None;
    for (ins, r) in inserts.iter().zip(results) {
        match r {
            Ok(()) => ok_mask |= ins.table.bit(),
            Err(e) => {
                tracing::warn!(table = ins.table.name(), error = %e, "storing spool rows failed");
                error.get_or_insert_with(|| e.to_string());
            }
        }
    }
    let outcome: Vec<Result<FileWrite, String>> = batch
        .iter()
        .enumerate()
        .map(|(i, j)| {
            let k = dup[i].unwrap_or(i);
            let (need, want) = (todo[k], needed(&j.file));
            if need & !ok_mask != 0 {
                return Err(error.clone().unwrap_or_default());
            }
            Ok(if want != 0 && need == 0 { FileWrite::AlreadyStored } else { FileWrite::Stored })
        })
        .collect();
    for (i, j) in batch.iter().enumerate() {
        if dup[i].is_none() {
            let f = &j.file;
            let done = todo[i] & ok_mask;
            if done != 0 {
                stored.add(f.node, &f.file_id, done);
            }
        }
    }
    for (j, o) in batch.into_iter().zip(outcome) {
        let _ = j.reply.send(o.map_err(|e| DomainError::Internal(format!("analytics: {e}"))));
    }
}

/// The inserts of a batch: per table, the rows of every job that still
/// needs it, with the token of those files.
fn build(batch: &[Job], todo: &[u8]) -> DomainResult<Vec<Insert>> {
    let mut out = Vec::new();
    for table in Table::ALL {
        let members: Vec<&SpoolRows> =
            batch.iter().zip(todo).filter(|(_, t)| *t & table.bit() != 0).map(|(j, _)| &j.file).collect();
        if members.is_empty() {
            continue;
        }
        let ids: Vec<(ServerId, &str)> = members.iter().map(|f| (f.node, f.file_id.as_str())).collect();
        let mut body = String::with_capacity(members.iter().map(|f| table.rows(f) * 200).sum());
        for f in &members {
            table.encode(f, &mut body)?;
        }
        out.push(Insert { table, token: token(table.name(), &ids), body });
    }
    Ok(out)
}

/// One insert, retried after each of `retries` with the same rows and token.
async fn attempt(sink: &dyn TableSink, ins: &Insert, retries: &[Duration]) -> DomainResult<()> {
    let mut last = sink.insert_once(ins.table, ins.body.clone(), &ins.token).await;
    for delay in retries {
        if last.is_ok() {
            break;
        }
        tokio::time::sleep(*delay).await;
        last = sink.insert_once(ins.table, ins.body.clone(), &ins.token).await;
    }
    last
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // reason: tests
mod tests {
    use super::*;
    use forge_core::Timestamp;
    use forge_core::analytics::NodeMinute;
    use forge_core::logs::{IngestContext, parse_batch};
    use std::sync::Mutex;

    /// Records inserts; fails a table while it is in `failing`, and drops an
    /// insert whose token it has seen (like the engine's window).
    #[derive(Default)]
    struct Fake {
        /// Inserts wait while the test holds this for writing.
        gate: tokio::sync::RwLock<()>,
        inserts: Mutex<Vec<(Table, usize, String)>>,
        tokens: Mutex<Vec<String>>,
        failing: Mutex<Vec<Table>>,
        stored: Mutex<HashMap<&'static str, usize>>,
    }

    #[async_trait]
    impl TableSink for Fake {
        async fn insert_once(&self, table: Table, body: String, token: &str) -> DomainResult<()> {
            self.inserts.lock().unwrap().push((table, body.lines().count(), token.to_owned()));
            let _open = self.gate.read().await;
            if self.failing.lock().unwrap().contains(&table) {
                return Err(DomainError::Internal("down".into()));
            }
            let mut tokens = self.tokens.lock().unwrap();
            if !tokens.iter().any(|t| t == token) {
                tokens.push(token.to_owned());
                *self.stored.lock().unwrap().entry(table.name()).or_default() += body.lines().count();
            }
            Ok(())
        }
    }

    impl Fake {
        fn stored(&self, t: Table) -> usize {
            self.stored.lock().unwrap().get(t.name()).copied().unwrap_or(0)
        }
    }

    fn file(node: ServerId, id: &str, logs: usize, minutes: usize) -> SpoolRows {
        let cx = IngestContext {
            service_id: forge_core::ServiceId::new(),
            server_id: Some(node),
            node_ts: None,
            now: Timestamp(1),
        };
        let body: Vec<String> = (0..logs).map(|i| format!("{{\"@mt\":\"line {i}\"}}")).collect();
        let logs = if logs == 0 { Vec::new() } else { parse_batch(&body.join("\n"), cx).unwrap() };
        let minutes = (0..minutes)
            .map(|i| NodeMinute {
                server_id: node,
                minute: Timestamp(60_000 * i64::try_from(i).unwrap()),
                cpu_avg: 1.0,
                cpu_max: 1.0,
                mem_used: 1,
                mem_total: 2,
                swap_used: 0,
                disk_used: 1,
                disk_total: 2,
                rx: 0,
                tx: 0,
                load1: 0.0,
                samples: 1,
            })
            .collect();
        SpoolRows { node, file_id: id.into(), logs, minutes, processes: vec![], containers: vec![] }
    }

    #[test]
    fn insert_settings_carry_the_token_and_deduplicate() {
        let node = ServerId::new();
        let id = "0190a0b0c0d0e0f00112233445566778";
        let t = token(APP_LOGS, &[(node, id)]);
        assert_eq!(t, format!("{id}:app_logs"), "one file: its id and the table");
        let s = insert_settings(&t);
        let get = |k: &str| s.iter().find(|(n, _)| *n == k).map(|(_, v)| v.as_str());
        assert_eq!(get("insert_deduplication_token"), Some(t.as_str()));
        assert_eq!(get("insert_deduplicate"), Some("1"));
        assert_eq!(get("async_insert_deduplicate"), Some("1"));
        assert_eq!(get("deduplicate_blocks_in_dependent_materialized_views"), Some("1"));
        assert_eq!(get("async_insert"), Some("0"));
        let other = ServerId::new();
        let g = token(APP_LOGS, &[(node, id), (other, id)]);
        assert!(g.starts_with('g') && g.ends_with(":app_logs") && g.len() < 80);
        assert_eq!(g, token(APP_LOGS, &[(node, id), (other, id)]), "stable");
        assert_ne!(g, token(NODE_MINUTES, &[(node, id), (other, id)]), "per table");
        assert_ne!(g, token(APP_LOGS, &[(other, id), (node, id)]));
    }

    /// Files queued while a batch is written form one insert per table, and
    /// each is answered after its tables confirmed.
    #[tokio::test]
    async fn queued_files_are_grouped_into_one_insert_per_table() {
        let fake = Arc::new(Fake::default());
        let w = ClickHouseSpoolWriter::with_sink(fake.clone(), 10_000, &[]);
        let send = |f: SpoolRows| {
            let w = w.clone();
            tokio::spawn(async move { w.write_spool_file(f).await })
        };
        let held = fake.gate.write().await;
        let first = send(file(ServerId::new(), &format!("{:032x}", 0), 2, 0));
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while fake.inserts.lock().unwrap().is_empty() {
            assert!(tokio::time::Instant::now() < deadline, "the first batch never started");
            tokio::task::yield_now().await;
        }
        // Five files queue up while the first batch is being written.
        let rest: Vec<_> = (1..=5).map(|i| send(file(ServerId::new(), &format!("{i:032x}"), 3, 1))).collect();
        while w.tx.capacity() > QUEUE - 5 {
            assert!(tokio::time::Instant::now() < deadline, "the files never queued");
            tokio::task::yield_now().await;
        }
        drop(held);
        assert_eq!(first.await.unwrap().unwrap(), FileWrite::Stored);
        for h in rest {
            assert_eq!(h.await.unwrap().unwrap(), FileWrite::Stored);
        }
        assert_eq!((fake.stored(Table::Logs), fake.stored(Table::NodeMinutes)), (17, 5));
        let inserts = fake.inserts.lock().unwrap().clone();
        assert_eq!(inserts.len(), 3, "the first file, then one insert per table for the other five");
        assert_eq!((inserts[1].1 + inserts[2].1), 15 + 5);
        assert!(inserts[1..].iter().all(|(_, _, t)| t.starts_with('g')), "a group token");
    }

    /// A table fails after another succeeded: the file sent again is written
    /// only to the table it is missing, and a retry that landed is not
    /// written twice.
    #[tokio::test]
    async fn a_file_sent_again_after_a_partial_failure_is_stored_once_per_table() {
        let fake = Arc::new(Fake::default());
        let w = ClickHouseSpoolWriter::with_sink(fake.clone(), 10_000, &[Duration::ZERO]);
        let f = file(ServerId::new(), &format!("{:032x}", 7), 4, 2);
        fake.failing.lock().unwrap().push(Table::NodeMinutes);
        assert!(w.write_spool_file(f.clone()).await.is_err());
        assert_eq!(fake.stored(Table::Logs), 4);
        let tries = fake.inserts.lock().unwrap().iter().filter(|(t, ..)| *t == Table::NodeMinutes).count();
        assert_eq!(tries, 2, "retried once with the same token");
        fake.failing.lock().unwrap().clear();
        assert_eq!(w.write_spool_file(f.clone()).await.unwrap(), FileWrite::Stored);
        assert_eq!((fake.stored(Table::Logs), fake.stored(Table::NodeMinutes)), (4, 2), "each table once");
        assert_eq!(w.write_spool_file(f.clone()).await.unwrap(), FileWrite::AlreadyStored);
        // A new writer (the worker restarted) forgets; the token still holds.
        let w = ClickHouseSpoolWriter::with_sink(fake.clone(), 10_000, &[]);
        assert_eq!(w.write_spool_file(f).await.unwrap(), FileWrite::Stored);
        assert_eq!((fake.stored(Table::Logs), fake.stored(Table::NodeMinutes)), (4, 2), "deduplicated by token");
    }
}
