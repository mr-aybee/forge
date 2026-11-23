//! The one writer (D-143). Every write goes through one thread: spool
//! files, the worker's other writes, and the hourly jobs. So each file has
//! one writer by construction, and jobs never race a batch.
//!
//! The thread takes the first command plus every command already queued
//! (up to `batch_rows`) and writes them as one batch (group commit): per
//! family one transaction that appends the rows (time-ordered) and the ids
//! of the spool files they came from. A file whose id a family already
//! holds is skipped for that family, so a file sent again is stored once
//! per table even after a crash between families.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, Mutex, PoisonError, RwLock};

use duckdb::{Appender, Connection, params};
use forge_core::analytics::{ContainerMinute, FileWrite, NodeMinute, NodeProcessMinute, SpoolRows};
use forge_core::logs::{LogEvent, LogStream};
use forge_core::{Clock, ServerId};
use tokio::sync::oneshot;

use crate::Error;
use crate::catalog::{Catalog, Lease};
use crate::parts::{Family, Part};

/// Spool file ids remembered per node and family. An agent sends again only
/// its oldest unacknowledged file, so the file sent again is always one of
/// the last few stored for that node.
pub const IDS_PER_NODE: usize = 16;
/// Most nodes remembered per family.
const MAX_NODES: usize = 100_000;
/// Days of `spool_files` read at start-up (the agent's 72 h spool age plus one).
pub const ID_DAYS: i64 = 4;
/// Most commands in one batch.
const MAX_ITEMS: usize = 256;

/// Rows without a file id (the worker's own buffers and the log batcher).
#[derive(Debug, Default)]
pub struct Rows {
    pub logs: Vec<LogEvent>,
    pub minutes: Vec<NodeMinute>,
    pub processes: Vec<NodeProcessMinute>,
    pub containers: Vec<ContainerMinute>,
}

/// A job run on the writer thread.
pub type Job = Box<dyn FnOnce(&mut Writer) -> Result<(), Error> + Send>;

pub enum Cmd {
    // A job is a closure; see the manual `Debug` below.
    Spool(SpoolRows, oneshot::Sender<Result<FileWrite, Error>>),
    Rows(Rows, oneshot::Sender<Result<(), Error>>),
    Job(Job, oneshot::Sender<Result<(), Error>>),
}

impl std::fmt::Debug for Cmd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Cmd::Spool(..) => "Cmd::Spool",
            Cmd::Rows(..) => "Cmd::Rows",
            Cmd::Job(..) => "Cmd::Job",
        })
    }
}

impl Cmd {
    fn rows(&self) -> usize {
        match self {
            Cmd::Spool(f, _) => usize::try_from(f.rows()).unwrap_or(usize::MAX),
            Cmd::Rows(r, _) => r.logs.len() + r.minutes.len() + r.processes.len() + r.containers.len(),
            Cmd::Job(..) => 0,
        }
    }
}

/// The ids a family holds, per node.
#[derive(Debug, Default)]
pub struct Seen(HashMap<(Family, ServerId), VecDeque<String>>);

impl Seen {
    #[must_use]
    pub fn contains(&self, f: Family, node: ServerId, id: &str) -> bool {
        self.0.get(&(f, node)).is_some_and(|ids| ids.iter().any(|i| i == id))
    }

    pub fn insert(&mut self, f: Family, node: ServerId, id: &str) {
        if !self.0.contains_key(&(f, node)) && self.0.len() >= MAX_NODES * Family::SPOOL.len() {
            return;
        }
        let ids = self.0.entry((f, node)).or_default();
        if ids.iter().any(|i| i == id) {
            return;
        }
        if ids.len() >= IDS_PER_NODE {
            ids.pop_front();
        }
        ids.push_back(id.to_owned());
    }
}

/// Rows of metrics files already folded into the rollups:
/// `(day, table) → row id bound`. Readers hold the read lock while they
/// combine rollups with the newer minute rows.
pub type Rolled = Arc<RwLock<HashMap<(i64, &'static str), i64>>>;

/// The writer thread's state.
#[allow(missing_debug_implementations)] // reason: holds a DuckDB connection, which has no `Debug`
pub struct Writer {
    pub(crate) catalog: Arc<Catalog>,
    pub(crate) conn: Connection,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) seen: Seen,
    pub(crate) rolled: Rolled,
    /// Today's files of the spool families, held attached.
    current: Option<(i64, Lease)>,
}

/// One family's share of a batch.
#[derive(Default)]
struct Share<'a> {
    logs: Vec<&'a LogEvent>,
    minutes: Vec<&'a NodeMinute>,
    processes: Vec<&'a NodeProcessMinute>,
    containers: Vec<&'a ContainerMinute>,
    ids: Vec<(ServerId, &'a str)>,
    /// Indexes of the batch items with rows here.
    items: Vec<usize>,
}

impl Share<'_> {
    fn is_empty(&self) -> bool {
        self.logs.is_empty() && self.minutes.is_empty() && self.processes.is_empty() && self.containers.is_empty()
    }

    fn range(&self) -> Option<(i64, i64)> {
        let ts = self
            .logs
            .iter()
            .map(|e| e.ts.0)
            .chain(self.minutes.iter().map(|m| m.minute.0))
            .chain(self.processes.iter().map(|p| p.minute.0))
            .chain(self.containers.iter().map(|c| c.minute.0));
        ts.fold(None, |acc, t| Some(acc.map_or((t, t), |(lo, hi): (i64, i64)| (lo.min(t), hi.max(t)))))
    }
}

fn some(s: &str) -> Option<&str> {
    (!s.is_empty()).then_some(s)
}

fn appender<'c>(c: &'c Connection, part: &Part, table: &str) -> Result<Appender<'c>, Error> {
    Ok(c.appender_to_catalog_and_db(table, &part.alias(), "main")?)
}

impl Writer {
    #[must_use]
    pub fn new(catalog: Arc<Catalog>, conn: Connection, clock: Arc<dyn Clock>, seen: Seen, rolled: Rolled) -> Self {
        Self { catalog, conn, clock, seen, rolled, current: None }
    }

    /// Today's day number.
    pub(crate) fn today(&self) -> i64 {
        self.clock.now().0.div_euclid(crate::parts::DAY_MS)
    }

    /// The files written now; the lease moves at midnight.
    pub(crate) fn current_parts(&mut self) -> Result<Vec<Part>, Error> {
        let today = self.today();
        if self.current.as_ref().is_none_or(|(d, _)| *d != today) {
            let parts: Vec<Part> = Family::SPOOL.iter().map(|f| Part { family: *f, index: today }).collect();
            let lease = self.catalog.lease(&parts, true)?;
            self.current = Some((today, lease));
        }
        Ok(self.current.as_ref().map(|(_, l)| l.parts().to_vec()).unwrap_or_default())
    }

    /// Serves commands until every sender is gone.
    /// Group commit: the batch is the first command plus whatever queued
    /// up meanwhile (up to `batch_rows`). Nothing waits on a timer, so a lone
    /// upload is written at once, and under load the files that arrive
    /// while a batch commits form the next one.
    pub fn run(mut self, rx: &Receiver<Cmd>, batch_rows: usize) {
        while let Ok(first) = rx.recv() {
            let mut batch = Vec::new();
            let mut job = None;
            match first {
                Cmd::Job(j, reply) => job = Some((j, reply)),
                c => {
                    let mut rows = c.rows();
                    batch.push(c);
                    while batch.len() < MAX_ITEMS && rows < batch_rows {
                        match rx.try_recv() {
                            Ok(Cmd::Job(j, reply)) => {
                                job = Some((j, reply));
                                break;
                            }
                            Ok(c) => {
                                rows += c.rows();
                                batch.push(c);
                            }
                            Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
                        }
                    }
                }
            }
            if !batch.is_empty() {
                self.write(batch);
            }
            if let Some((j, reply)) = job {
                let _ = reply.send(j(&mut self));
            }
        }
    }

    fn write(&mut self, batch: Vec<Cmd>) {
        let (outcomes, error) = match self.write_batch(&batch) {
            Ok(o) => (o, None),
            Err(e) => (Vec::new(), Some(e.to_string())),
        };
        for (i, cmd) in batch.into_iter().enumerate() {
            let err = || Error::Write(error.clone().unwrap_or_default());
            match cmd {
                Cmd::Spool(_, reply) => {
                    let _ = reply.send(outcomes.get(i).copied().ok_or_else(err));
                }
                Cmd::Rows(_, reply) => {
                    let _ = reply.send(if error.is_some() { Err(err()) } else { Ok(()) });
                }
                Cmd::Job(..) => {}
            }
        }
    }

    /// Writes a batch; on success the outcome of each item.
    fn write_batch(&mut self, batch: &[Cmd]) -> Result<Vec<FileWrite>, Error> {
        let parts = self.current_parts()?;
        let part = |f: Family| parts.iter().find(|p| p.family == f).copied();
        let mut shares: HashMap<Family, Share<'_>> = Family::SPOOL.iter().map(|f| (*f, Share::default())).collect();
        // Per item: (families with rows, families skipped as already stored).
        let mut counts = vec![(0u8, 0u8); batch.len()];
        let mut batch_ids: HashSet<(Family, ServerId, &str)> = HashSet::new();
        for (i, cmd) in batch.iter().enumerate() {
            let (logs, minutes, processes, containers, id) = match cmd {
                Cmd::Spool(f, _) => {
                    (&f.logs, &f.minutes, &f.processes, &f.containers, Some((f.node, f.file_id.as_str())))
                }
                Cmd::Rows(r, _) => (&r.logs, &r.minutes, &r.processes, &r.containers, None),
                Cmd::Job(..) => continue,
            };
            for fam in Family::SPOOL {
                let has = match fam {
                    Family::Logs => !logs.is_empty(),
                    Family::Metrics => !minutes.is_empty() || !containers.is_empty(),
                    Family::Processes => !processes.is_empty(),
                    Family::Rollups => false,
                };
                if !has {
                    continue;
                }
                counts[i].0 += 1;
                if let Some((node, fid)) = id
                    && (self.seen.contains(fam, node, fid) || !batch_ids.insert((fam, node, fid)))
                {
                    counts[i].1 += 1;
                    continue;
                }
                let Some(s) = shares.get_mut(&fam) else { continue };
                match fam {
                    Family::Logs => s.logs.extend(logs.iter()),
                    Family::Metrics => {
                        s.minutes.extend(minutes.iter());
                        s.containers.extend(containers.iter());
                    }
                    Family::Processes => s.processes.extend(processes.iter()),
                    Family::Rollups => {}
                }
                if let Some((node, fid)) = id {
                    s.ids.push((node, fid));
                }
                s.items.push(i);
            }
        }
        // Ranges first: a crash after this leaves them too wide, never too narrow.
        let widen: Vec<(Part, i64, i64)> =
            shares.iter().filter_map(|(f, s)| Some((part(*f)?, s.range()?))).map(|(p, (lo, hi))| (p, lo, hi)).collect();
        self.catalog.widen(&widen)?;
        let now = self.clock.now().0;
        for fam in Family::SPOOL {
            let (Some(s), Some(p)) = (shares.get_mut(&fam), part(fam)) else { continue };
            if s.is_empty() {
                continue;
            }
            self.commit(&p, s, now)?;
            for (node, fid) in &s.ids {
                self.seen.insert(fam, *node, fid);
            }
        }
        Ok(counts
            .iter()
            .map(|(has, skipped)| if *has > 0 && has == skipped { FileWrite::AlreadyStored } else { FileWrite::Stored })
            .collect())
    }

    /// One family's rows and ids in one transaction.
    fn commit(&self, p: &Part, s: &mut Share<'_>, now: i64) -> Result<(), Error> {
        s.logs.sort_by_key(|e| e.ts);
        s.minutes.sort_by_key(|m| m.minute);
        s.containers.sort_by_key(|c| c.minute);
        s.processes.sort_by_key(|r| r.minute);
        self.conn.execute_batch("BEGIN")?;
        let r = self.append(p, s, now);
        match r {
            Ok(()) => Ok(self.conn.execute_batch("COMMIT")?),
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(e)
            }
        }
    }

    fn append(&self, p: &Part, s: &Share<'_>, now: i64) -> Result<(), Error> {
        let c = &self.conn;
        if !s.logs.is_empty() {
            let mut a = appender(c, p, "app_logs")?;
            for e in &s.logs {
                let props = serde_json::to_string(&e.properties).map_err(|e| Error::Write(e.to_string()))?;
                a.append_row(params![
                    e.service_id.map(|v| v.to_string()),
                    e.ts.0,
                    e.id.to_string(),
                    e.node_ts.map(|t| t.0),
                    e.ingest_ts.0,
                    e.level.as_str(),
                    e.message,
                    e.message_template,
                    e.exception.as_deref().unwrap_or_default(),
                    e.trace_id.as_deref().unwrap_or_default(),
                    e.span_id.as_deref().unwrap_or_default(),
                    e.event_type,
                    e.server_id.map(|v| v.to_string()),
                    props,
                    e.source.as_str(),
                    e.container_id.as_deref().unwrap_or_default(),
                    e.container_name.as_deref().unwrap_or_default(),
                    e.stack.as_deref().unwrap_or_default(),
                    e.stream.map_or("", LogStream::as_str),
                ])?;
            }
            a.flush()?;
        }
        if !s.minutes.is_empty() {
            let mut a = appender(c, p, "node_minute")?;
            for m in &s.minutes {
                a.append_row(params![
                    m.server_id.to_string(),
                    m.minute.0,
                    m.cpu_avg,
                    m.cpu_max,
                    m.mem_used,
                    m.mem_total,
                    m.swap_used,
                    m.disk_used,
                    m.disk_total,
                    m.rx,
                    m.tx,
                    m.load1,
                    m.samples,
                ])?;
            }
            a.flush()?;
        }
        if !s.containers.is_empty() {
            let mut a = appender(c, p, "container_minute")?;
            for m in &s.containers {
                a.append_row(params![
                    m.server_id.to_string(),
                    m.container_name,
                    m.minute.0,
                    m.container_id,
                    m.image,
                    m.service_id.map(|v| v.to_string()),
                    m.stack.as_deref().unwrap_or_default(),
                    m.cpu,
                    m.mem_used,
                    m.mem_limit,
                    m.rx,
                    m.tx,
                    m.block_read,
                    m.block_write,
                    m.restarts,
                ])?;
            }
            a.flush()?;
        }
        if !s.processes.is_empty() {
            let mut a = appender(c, p, "top_process")?;
            for r in &s.processes {
                a.append_row(params![r.server_id.to_string(), r.minute.0, r.pid, r.name, r.user, r.cpu, r.mem, r.cmd])?;
            }
            a.flush()?;
        }
        if !s.ids.is_empty() {
            let mut a = appender(c, p, "spool_files")?;
            for (node, fid) in &s.ids {
                a.append_row(params![node.to_string(), some(fid), now])?;
            }
            a.flush()?;
        }
        Ok(())
    }
}

/// Reads the last ids per node of the spool families' recent files.
pub fn load_seen(catalog: &Arc<Catalog>, conn: &Connection, today: i64) -> Result<Seen, Error> {
    let mut seen = Seen::default();
    for fam in Family::SPOOL {
        let parts: Vec<Part> = catalog.files(fam).into_iter().filter(|p| p.index > today - ID_DAYS).collect();
        let lease = catalog.lease(&parts, false)?;
        if lease.parts().is_empty() {
            continue;
        }
        let union = lease
            .parts()
            .iter()
            .map(|p| format!("SELECT node, file_id, stored_at FROM {}.spool_files", p.alias()))
            .collect::<Vec<_>>()
            .join(" UNION ALL ");
        let sql = format!(
            "SELECT node, file_id FROM ({union}) QUALIFY row_number() OVER (PARTITION BY node ORDER BY stored_at DESC) <= {IDS_PER_NODE} ORDER BY stored_at"
        );
        let mut st = conn.prepare(&sql)?;
        let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)))?;
        for row in rows {
            let (node, fid) = row?;
            if let (Ok(node), Some(fid)) = (node.parse::<ServerId>(), fid) {
                seen.insert(fam, node, &fid);
            }
        }
    }
    Ok(seen)
}

/// Reads the rollup bounds from `meta.rolled`.
pub fn load_rolled(conn: &Connection) -> Result<HashMap<(i64, &'static str), i64>, Error> {
    let mut out = HashMap::new();
    let mut st = conn.prepare("SELECT part, tbl, max(upto) FROM meta.rolled GROUP BY part, tbl")?;
    let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?)))?;
    for row in rows {
        let (part, tbl, upto) = row?;
        if let Some(t) = crate::jobs::MINUTE_TABLES.iter().find(|t| **t == tbl) {
            out.insert((part, *t), upto);
        }
    }
    Ok(out)
}

/// Poison-tolerant lock helper for the shared maps.
pub fn guard<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}
