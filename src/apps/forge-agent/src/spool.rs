//! The node's analytics spool (D-142).
//!
//! Application log requests, container output and minute samples are
//! appended as NDJSON rows to one in-memory write buffer. The buffer is
//! sealed when it reaches its size or age limit: compressed with zstd
//! (level 1 to 3) into `<stem>.ndjson.zst.tmp`, fsynced, and renamed to
//! `<stem>.ndjson.zst.ready`. The uploader sends ready files oldest first,
//! one at a time; once the worker acknowledges a file it is renamed to
//! `.acked` and deleted. On start, [`Spool::open`] removes `.tmp` files (a
//! crash before the rename: they may be partial), deletes `.acked` files
//! and drops ready files past their maximum age.
//!
//! Hard limits: a byte cap over ready files plus the buffer, a floor of
//! free disk, and a maximum age. When sealing would pass the cap, the
//! oldest ready files are dropped and counted; the local listener answers
//! 429 instead of appending while the spool is full ([`Spool::accepting`]).
//! Memory is fixed: one write buffer of at most the seal size (plus one
//! row), and the uploader reads one file at a time. The agent never parses
//! the rows it writes.

use std::collections::VecDeque;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use forge_core::spool::{AppLog, BodyFormat, Counts, FileHeader, FileName, Row, SUFFIX, VERSION};
use forge_proto::messages::{SpoolStatus, SpoolTuning};
use tokio::sync::Notify;

const MIB: u64 = 1024 * 1024;
const TMP: &str = ".tmp";
const READY: &str = ".ready";
const ACKED: &str = ".acked";

/// Spool limits; [`Limits::from_tuning`] applies the node settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Most bytes of ready files plus the write buffer.
    pub max_bytes: u64,
    /// Nothing is written while the spool's disk has less free space.
    pub min_free_bytes: u64,
    /// Ready files older than this are dropped, milliseconds.
    pub max_age_ms: i64,
    /// The buffer is sealed at this many raw bytes.
    pub seal_bytes: u64,
    /// ... or this long after its first row, milliseconds.
    pub seal_ms: i64,
    /// zstd level, 1 to 3.
    pub level: i32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_bytes: 512 * MIB,
            min_free_bytes: 256 * MIB,
            max_age_ms: 3 * 24 * 3_600_000,
            seal_bytes: 4 * MIB,
            seal_ms: 10_000,
            level: 1,
        }
    }
}

impl Limits {
    /// The defaults with the node settings applied (`0` keeps a default),
    /// clamped to safe ranges.
    #[must_use]
    pub fn from_tuning(t: &SpoolTuning) -> Self {
        let d = Self::default();
        let pick = |v: u64, def: u64| if v == 0 { def } else { v };
        let secs_ms = |s: u64| i64::try_from(s.saturating_mul(1_000)).unwrap_or(i64::MAX);

        Self {
            max_bytes: pick(t.max_bytes, d.max_bytes).clamp(16 * MIB, 64 * 1024 * MIB),
            min_free_bytes: if t.min_free_bytes == 0 { d.min_free_bytes } else { t.min_free_bytes },
            max_age_ms: if t.max_age_secs == 0 {
                d.max_age_ms
            } else {
                secs_ms(t.max_age_secs.clamp(600, 30 * 86_400))
            },
            seal_bytes: pick(t.seal_bytes, d.seal_bytes).clamp(64 * 1024, 16 * MIB),
            seal_ms: if t.seal_secs == 0 { d.seal_ms } else { secs_ms(t.seal_secs.clamp(1, 300)) },
            level: if t.zstd_level == 0 { d.level } else { i32::try_from(t.zstd_level.clamp(1, 3)).unwrap_or(1) },
        }
    }
}

/// Free space on the disk holding the spool.
pub trait FreeDisk: Send + Sync + std::fmt::Debug {
    /// Bytes available to this process; `None` when unknown (not refused).
    fn free(&self, dir: &Path) -> Option<u64>;
}

/// The operating system's view (statvfs; on Windows the volume holding the folder).
#[derive(Debug, Default)]
pub struct SystemFreeDisk;

impl FreeDisk for SystemFreeDisk {
    #[cfg(unix)]
    fn free(&self, dir: &Path) -> Option<u64> {
        let s = rustix::fs::statvfs(dir).ok()?;
        Some(s.f_bavail.saturating_mul(s.f_frsize))
    }

    #[cfg(not(unix))]
    fn free(&self, dir: &Path) -> Option<u64> {
        let disks = sysinfo::Disks::new_with_refreshed_list();
        disks
            .list()
            .iter()
            .filter(|d| dir.starts_with(d.mount_point()))
            .max_by_key(|d| d.mount_point().as_os_str().len())
            .map(sysinfo::Disk::available_space)
    }
}

/// Why the spool does not take a row.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SpoolError {
    #[error("the spool is full")]
    Full,
    #[error("the spool's disk is below its free-space floor")]
    DiskLow,
    #[error("spool i/o: {0}")]
    Io(String),
}

impl From<io::Error> for SpoolError {
    fn from(e: io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

/// A sealed file waiting for upload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadyFile {
    /// `<node>-<file id>`.
    pub stem: String,
    pub path: PathBuf,
    pub bytes: u64,
    /// When the file was started, epoch ms (from its UUIDv7 id).
    pub created_ms: i64,
}

#[derive(Debug, Default)]
struct Counters {
    dropped_files: AtomicU64,
    dropped_bytes: AtomicU64,
    uploaded_files: AtomicU64,
    refused_requests: AtomicU64,
}

#[derive(Debug)]
struct Inner {
    limits: Limits,
    buf: Vec<u8>,
    counts: Counts,
    /// When the buffer took its first row, epoch ms.
    opened_ms: Option<i64>,
    ready: VecDeque<ReadyFile>,
    ready_bytes: u64,
}

/// What [`Spool::open`] found from a previous run.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Recovery {
    /// Unfinished `.tmp` files removed.
    pub tmp_removed: u64,
    /// `.acked` files deleted.
    pub acked_removed: u64,
    /// Ready files kept for upload.
    pub ready: u64,
    /// Ready files dropped (age or cap).
    pub dropped: u64,
}

/// The spool of one node.
#[derive(Debug)]
pub struct Spool {
    dir: PathBuf,
    node: uuid::Uuid,
    inner: Mutex<Inner>,
    free: Arc<dyn FreeDisk>,
    counters: Counters,
    /// Signalled when a file is sealed.
    sealed: Notify,
}

fn file_name(stem: &str, stage: &str) -> String {
    format!("{stem}{SUFFIX}{stage}")
}

/// Creation time from a UUIDv7 file id (its first 48 bits, epoch ms).
fn created_ms(id: &str) -> Option<i64> {
    i64::from_str_radix(id.get(..12)?, 16).ok()
}

#[cfg(unix)]
fn sync_dir(dir: &Path) -> io::Result<()> {
    File::open(dir)?.sync_all()
}

#[cfg(not(unix))]
#[allow(clippy::unnecessary_wraps)] // reason: same signature as the Unix version
fn sync_dir(_dir: &Path) -> io::Result<()> {
    Ok(())
}

/// Splits `<stem>.ndjson.zst<stage>` into the stem and the stage.
fn parse_entry(name: &str) -> Option<(&str, &str)> {
    let at = name.find(SUFFIX)?;
    let (stem, rest) = name.split_at(at);
    let stage = &rest[SUFFIX.len()..];
    FileName::parse(stem)?;

    Some((stem, stage))
}

impl Spool {
    /// Opens (creating it, mode 0700) the spool in `dir` and recovers what
    /// a previous run left: see the module documentation.
    pub fn open(
        dir: &Path,
        node: uuid::Uuid,
        limits: Limits,
        free: Arc<dyn FreeDisk>,
        now_ms: i64,
    ) -> io::Result<(Self, Recovery)> {
        fs::create_dir_all(dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        }
        let mut rec = Recovery::default();
        let mut ready = Vec::new();

        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let name = entry.file_name();
            let Some((stem, stage)) = name.to_str().and_then(parse_entry) else { continue };
            match stage {
                TMP => {
                    fs::remove_file(entry.path())?;
                    rec.tmp_removed += 1;
                }
                ACKED => {
                    fs::remove_file(entry.path())?;
                    rec.acked_removed += 1;
                }
                READY => {
                    let id = FileName::parse(stem).map(|n| n.id).unwrap_or_default();
                    ready.push(ReadyFile {
                        stem: stem.to_owned(),
                        path: entry.path(),
                        bytes: entry.metadata()?.len(),
                        created_ms: created_ms(&id).unwrap_or(now_ms),
                    });
                }
                _ => {}
            }
        }

        ready.sort_by(|a, b| a.stem.cmp(&b.stem));
        let ready_bytes = ready.iter().map(|f| f.bytes).sum();
        let spool = Self {
            dir: dir.to_path_buf(),
            node,
            inner: Mutex::new(Inner {
                limits,
                buf: Vec::new(),
                counts: Counts::default(),
                opened_ms: None,
                ready: ready.into(),
                ready_bytes,
            }),
            free,
            counters: Counters::default(),
            sealed: Notify::new(),
        };

        {
            let mut inner = spool.lock();
            rec.dropped += spool.expire_locked(&mut inner, now_ms);
            rec.dropped += spool.enforce_cap_locked(&mut inner, 0);
            rec.ready = inner.ready.len() as u64;
        }

        if rec != Recovery::default() {
            tracing::info!(?rec, "spool recovered");
        }

        Ok((spool, rec))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    #[must_use]
    pub fn limits(&self) -> Limits {
        self.lock().limits
    }

    /// Applies new limits from the node settings.
    pub fn set_limits(&self, limits: Limits) {
        self.lock().limits = limits;
    }

    fn below_floor(&self, limits: &Limits) -> bool {
        self.free.free(&self.dir).is_some_and(|f| f < limits.min_free_bytes)
    }

    /// Whether the listener may append: the spool is under its cap and its
    /// disk above the floor.
    pub fn accepting(&self) -> Result<(), SpoolError> {
        let limits = {
            let inner = self.lock();
            if inner.ready_bytes + inner.buf.len() as u64 >= inner.limits.max_bytes {
                return Err(SpoolError::Full);
            }

            inner.limits
        };

        if self.below_floor(&limits) {
            return Err(SpoolError::DiskLow);
        }

        Ok(())
    }

    /// Counts a listener request refused with 429.
    pub fn count_refused(&self) {
        self.counters.refused_requests.fetch_add(1, Ordering::Relaxed);
    }

    /// Appends one row; seals first when the buffer is full.
    pub fn append(&self, row: &Row, now_ms: i64) -> Result<(), SpoolError> {
        let line = serde_json::to_vec(row).map_err(|e| SpoolError::Io(e.to_string()))?;
        self.append_line(&line, row.kind(), now_ms)
    }

    /// Appends an application's request body as one `log` row without
    /// parsing it. `key_hash` is the hex SHA-256 of its key.
    pub fn append_app_log(&self, key_hash: &str, body: &str, now_ms: i64) -> Result<(), SpoolError> {
        // Built without a copy of the body into an owned row.
        #[derive(serde::Serialize)]
        struct Ref<'a> {
            k: &'static str,
            key: &'a str,
            ts: i64,
            fmt: BodyFormat,
            body: &'a str,
        }
        let r = Ref { k: "log", key: key_hash, ts: now_ms, fmt: BodyFormat::Raw, body };
        let line = serde_json::to_vec(&r).map_err(|e| SpoolError::Io(e.to_string()))?;

        self.append_line(&line, forge_core::spool::Kind::Log, now_ms)
    }

    fn append_line(&self, line: &[u8], kind: forge_core::spool::Kind, now_ms: i64) -> Result<(), SpoolError> {
        let mut inner = self.lock();
        if !inner.buf.is_empty() && (inner.buf.len() + line.len()) as u64 > inner.limits.seal_bytes {
            self.seal_locked(&mut inner, now_ms)?;
        }

        inner.buf.extend_from_slice(line);
        inner.buf.push(b'\n');
        inner.counts.add(kind);
        inner.opened_ms.get_or_insert(now_ms);

        if inner.buf.len() as u64 >= inner.limits.seal_bytes {
            self.seal_locked(&mut inner, now_ms)?;
        }

        Ok(())
    }

    /// Seals the buffer when it is older than the seal age, and drops ready
    /// files past the maximum age. Called once a second.
    pub fn tick(&self, now_ms: i64) -> Result<(), SpoolError> {
        let mut inner = self.lock();
        self.expire_locked(&mut inner, now_ms);

        if inner.opened_ms.is_some_and(|t| now_ms - t >= inner.limits.seal_ms) {
            self.seal_locked(&mut inner, now_ms)?;
        }

        Ok(())
    }

    /// Seals the buffer now (container lines are acknowledged to the
    /// helper only once they are on disk).
    pub fn seal(&self, now_ms: i64) -> Result<(), SpoolError> {
        self.seal_named(now_ms).map(drop)
    }

    /// [`seal`](Self::seal), returning the new file's stem (`None` when the
    /// buffer was empty).
    pub fn seal_named(&self, now_ms: i64) -> Result<Option<String>, SpoolError> {
        let mut inner = self.lock();
        let before = inner.ready.back().map(|f| f.stem.clone());
        self.seal_locked(&mut inner, now_ms)?;
        let after = inner.ready.back().map(|f| f.stem.clone());

        Ok(after.filter(|a| Some(a) != before.as_ref()))
    }

    /// Whether `stem` is still waiting for upload.
    #[must_use]
    pub fn contains(&self, stem: &str) -> bool {
        self.lock().ready.iter().any(|f| f.stem == stem)
    }

    fn seal_locked(&self, inner: &mut Inner, now_ms: i64) -> Result<(), SpoolError> {
        if inner.buf.is_empty() {
            return Ok(());
        }

        let counts = std::mem::take(&mut inner.counts);
        let created = inner.opened_ms.take().unwrap_or(now_ms);
        let raw = inner.buf.len() as u64;

        // Make room first: the cap counts what is about to be written.
        let limits = inner.limits;
        self.enforce_cap_locked(inner, raw.min(limits.max_bytes));

        while self.below_floor(&limits) {
            if !self.drop_oldest_locked(inner) {
                break;
            }
        }

        if self.below_floor(&limits) {
            self.count_drop(raw);
            inner.buf.clear();

            return Err(SpoolError::DiskLow);
        }

        let name = FileName::new(self.node);
        let stem = name.stem();
        let header = FileHeader { v: VERSION, node: self.node.to_string(), created, counts };
        let result = self.write_file(&stem, &header, &inner.buf, limits.level);
        inner.buf.clear();

        if inner.buf.capacity() as u64 > limits.seal_bytes.saturating_mul(2) {
            inner.buf.shrink_to(usize::try_from(limits.seal_bytes).unwrap_or(usize::MAX));
        }

        match result {
            Ok(f) => {
                inner.ready_bytes += f.bytes;
                inner.ready.push_back(f);
                self.enforce_cap_locked(inner, 0);
                self.sealed.notify_one();
                Ok(())
            }
            Err(e) => {
                self.count_drop(raw);
                Err(e.into())
            }
        }
    }

    /// Writes `<stem>.tmp`, fsyncs it, renames it ready and syncs the folder.
    fn write_file(&self, stem: &str, header: &FileHeader, rows: &[u8], level: i32) -> io::Result<ReadyFile> {
        let tmp = self.dir.join(file_name(stem, TMP));
        let ready = self.dir.join(file_name(stem, READY));
        let write = || -> io::Result<u64> {
            let file = File::create(&tmp)?;
            let mut enc = zstd::stream::write::Encoder::new(io::BufWriter::new(file), level)?;
            enc.include_checksum(true)?;
            serde_json::to_writer(&mut enc, header).map_err(io::Error::other)?;
            enc.write_all(b"\n")?;
            enc.write_all(rows)?;
            let file = enc.finish()?.into_inner().map_err(io::IntoInnerError::into_error)?;
            file.sync_all()?;
            Ok(file.metadata()?.len())
        };

        let bytes = match write() {
            Ok(b) => b,
            Err(e) => {
                let _ = fs::remove_file(&tmp);
                return Err(e);
            }
        };

        fs::rename(&tmp, &ready)?;
        sync_dir(&self.dir)?;
        let id = FileName::parse(stem).map(|n| n.id).unwrap_or_default();

        Ok(ReadyFile {
            stem: stem.to_owned(),
            path: ready,
            bytes,
            created_ms: created_ms(&id).unwrap_or(header.created),
        })
    }

    fn count_drop(&self, bytes: u64) {
        self.counters.dropped_files.fetch_add(1, Ordering::Relaxed);
        self.counters.dropped_bytes.fetch_add(bytes, Ordering::Relaxed);
    }

    /// Drops the oldest ready file; false when there is none.
    fn drop_oldest_locked(&self, inner: &mut Inner) -> bool {
        let Some(f) = inner.ready.pop_front() else { return false };
        inner.ready_bytes = inner.ready_bytes.saturating_sub(f.bytes);

        if let Err(e) = fs::remove_file(&f.path)
            && e.kind() != io::ErrorKind::NotFound
        {
            tracing::warn!(file = %f.stem, error = %e, "removing a dropped spool file failed");
        }

        self.count_drop(f.bytes);
        tracing::warn!(file = %f.stem, bytes = f.bytes, "spool full: dropped the oldest file");

        true
    }

    /// Drops the oldest files until `incoming` more bytes fit under the cap.
    fn enforce_cap_locked(&self, inner: &mut Inner, incoming: u64) -> u64 {
        let mut dropped = 0;

        while inner.ready_bytes + incoming > inner.limits.max_bytes && self.drop_oldest_locked(inner) {
            dropped += 1;
        }

        dropped
    }

    fn expire_locked(&self, inner: &mut Inner, now_ms: i64) -> u64 {
        let mut dropped = 0;

        while inner.ready.front().is_some_and(|f| now_ms - f.created_ms > inner.limits.max_age_ms) {
            self.drop_oldest_locked(inner);
            dropped += 1;
        }

        dropped
    }

    /// The oldest ready file.
    #[must_use]
    pub fn oldest(&self) -> Option<ReadyFile> {
        self.lock().ready.front().cloned()
    }

    /// The worker stored `stem`: rename it acked, then delete it.
    pub fn ack(&self, stem: &str) -> io::Result<()> {
        self.remove(stem, true)
    }

    /// The worker refused `stem` for good (too large, unreadable): drop it.
    pub fn reject(&self, stem: &str) -> io::Result<()> {
        self.remove(stem, false)
    }

    fn remove(&self, stem: &str, acked: bool) -> io::Result<()> {
        let mut inner = self.lock();
        let Some(at) = inner.ready.iter().position(|f| f.stem == stem) else { return Ok(()) };
        let f = inner.ready.remove(at).ok_or_else(|| io::Error::other("spool index"))?;
        inner.ready_bytes = inner.ready_bytes.saturating_sub(f.bytes);

        // Counted while the lock is held, so a status read never sees the
        // file gone from the queue without it counted as uploaded: the
        // worker has stored it, deleting the local copy below is cleanup.
        if acked {
            self.counters.uploaded_files.fetch_add(1, Ordering::Relaxed);
        }

        drop(inner);

        if acked {
            let done = self.dir.join(file_name(stem, ACKED));
            fs::rename(&f.path, &done)?;
            fs::remove_file(&done)?;
        } else {
            fs::remove_file(&f.path)?;
            self.count_drop(f.bytes);
        }

        Ok(())
    }

    /// Waits until a file is sealed.
    pub async fn sealed(&self) {
        self.sealed.notified().await;
    }

    /// The heartbeat's report.
    #[must_use]
    pub fn status(&self, now_ms: i64) -> SpoolStatus {
        let inner = self.lock();
        let c = &self.counters;

        SpoolStatus {
            files: inner.ready.len() as u64,
            bytes: inner.ready_bytes,
            dropped_files: c.dropped_files.load(Ordering::Relaxed),
            dropped_bytes: c.dropped_bytes.load(Ordering::Relaxed),
            oldest_age_secs: inner
                .ready
                .front()
                .map_or(0, |f| u64::try_from((now_ms - f.created_ms).max(0) / 1_000).unwrap_or(0)),
            uploaded_files: c.uploaded_files.load(Ordering::Relaxed),
            refused_requests: c.refused_requests.load(Ordering::Relaxed),
        }
    }

    /// Raw bytes waiting in the write buffer.
    #[must_use]
    pub fn buffered(&self) -> usize {
        self.lock().buf.len()
    }
}

/// An [`AppLog`] row as the listener writes it (for tests and fleet-sim).
#[must_use]
pub fn app_log_row(key_hash: &str, body: &str, ts: i64) -> Row {
    Row::Log(AppLog { key: key_hash.into(), ts, fmt: BodyFormat::Raw, body: body.into() })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // reason: tests
mod tests {
    use super::*;
    use std::io::Read;
    use std::sync::atomic::AtomicBool;

    #[derive(Debug, Default)]
    struct FakeDisk {
        full: AtomicBool,
    }
    impl FreeDisk for FakeDisk {
        fn free(&self, _dir: &Path) -> Option<u64> {
            Some(if self.full.load(Ordering::Relaxed) { 0 } else { u64::MAX })
        }
    }

    const NOW: i64 = 1_700_000_000_000;

    fn limits() -> Limits {
        Limits { max_bytes: 64 * 1024, seal_bytes: 4 * 1024, min_free_bytes: 1, ..Limits::default() }
    }

    fn open(dir: &Path, l: Limits, disk: Arc<FakeDisk>) -> (Spool, Recovery) {
        Spool::open(dir, uuid::Uuid::nil(), l, disk, NOW).unwrap()
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> =
            fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        v.sort();

        v
    }

    /// Reads a sealed file the way the worker does.
    pub(crate) fn read(path: &Path) -> (FileHeader, Vec<Row>) {
        let mut text = String::new();
        zstd::stream::read::Decoder::new(File::open(path).unwrap()).unwrap().read_to_string(&mut text).unwrap();
        let mut lines = text.lines();
        let header = serde_json::from_str(lines.next().unwrap()).unwrap();

        (header, lines.map(|l| serde_json::from_str(l).unwrap()).collect())
    }

    #[test]
    fn rows_are_sealed_by_age_into_a_readable_file_and_deleted_on_ack() {
        let dir = tempfile::tempdir().unwrap();
        let (s, _) = open(dir.path(), limits(), Arc::default());
        s.append_app_log("ab", "{\"@mt\":\"a \\\"b\\\"\"}\n{\"@mt\":\"c\"}", NOW).unwrap();
        s.tick(NOW + 1_000).unwrap();
        assert!(s.oldest().is_none(), "not sealed before its age");
        s.tick(NOW + 10_000).unwrap();
        let f = s.oldest().unwrap();
        assert!(f.path.to_str().unwrap().ends_with(".ndjson.zst.ready"));
        let (h, rows) = read(&f.path);
        assert_eq!((h.v, h.counts.log, h.created), (1, 1, NOW));
        assert_eq!(rows, vec![app_log_row("ab", "{\"@mt\":\"a \\\"b\\\"\"}\n{\"@mt\":\"c\"}", NOW)]);
        s.ack(&f.stem).unwrap();
        assert!(names(dir.path()).is_empty(), "nothing left after the ack");
        let st = s.status(NOW);
        assert_eq!((st.files, st.bytes, st.uploaded_files), (0, 0, 1));
    }

    #[test]
    fn size_seals_and_the_cap_drops_the_oldest_file() {
        let dir = tempfile::tempdir().unwrap();
        let l = Limits { level: 1, ..limits() };
        let (s, _) = open(dir.path(), l, Arc::default());

        // Incompressible bodies so the files are near their raw size.
        let mut seed = 1u64;
        let mut body = || {
            (0..3_000)
                .map(|_| {
                    seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                    char::from(b'a' + u8::try_from(seed >> 59).unwrap())
                })
                .collect::<String>()
        };

        for i in 0..200 {
            s.append_app_log("k", &body(), NOW + i).unwrap();
        }

        let st = s.status(NOW);
        assert!(st.files > 1, "sealed by size");
        assert!(st.bytes <= l.max_bytes, "{} over the cap", st.bytes);
        assert!(st.dropped_files > 0, "the oldest files were dropped and counted");
        assert_eq!(st.files as usize, names(dir.path()).len());
        assert!(s.buffered() as u64 <= l.seal_bytes);
    }

    #[test]
    fn listener_is_refused_at_the_cap_and_below_the_disk_floor() {
        let dir = tempfile::tempdir().unwrap();
        let disk = Arc::new(FakeDisk::default());
        let (s, _) = open(dir.path(), Limits { max_bytes: 16 * 1024 * 1024, ..limits() }, disk.clone());
        assert_eq!(s.accepting(), Ok(()));
        disk.full.store(true, Ordering::Relaxed);
        assert_eq!(s.accepting(), Err(SpoolError::DiskLow));

        // A full disk drops what was buffered instead of writing it.
        s.append_app_log("k", "x", NOW).unwrap();
        assert_eq!(s.seal(NOW), Err(SpoolError::DiskLow));
        assert!(names(dir.path()).is_empty(), "nothing written to a full disk");
        assert_eq!(s.status(NOW).dropped_files, 1);
        disk.full.store(false, Ordering::Relaxed);
        s.append_app_log("k", "y", NOW).unwrap();
        s.set_limits(Limits { max_bytes: 1, ..limits() });
        assert_eq!(s.accepting(), Err(SpoolError::Full));
    }

    #[test]
    fn restart_recovers_every_stage() {
        let dir = tempfile::tempdir().unwrap();
        let node = uuid::Uuid::nil();
        let (s, _) = open(dir.path(), limits(), Arc::default());
        s.append(&app_log_row("k", "a", NOW), NOW).unwrap();
        s.seal(NOW).unwrap();
        s.append(&app_log_row("k", "b", NOW), NOW).unwrap();
        s.seal(NOW).unwrap();

        let (a, b) = {
            let i = s.lock();
            (i.ready[0].clone(), i.ready[1].clone())
        };

        // Lines still in the buffer at a crash are lost: they were never on disk.
        s.append(&app_log_row("k", "lost", NOW), NOW).unwrap();
        drop(s);

        // A crash while writing (tmp), between ack-rename and delete (acked),
        // and an old file past its age.
        let tmp = FileName::new(node).stem();
        fs::write(dir.path().join(file_name(&tmp, TMP)), b"partial").unwrap();
        fs::rename(&a.path, dir.path().join(file_name(&a.stem, ACKED))).unwrap();
        let old = format!("{node}-{:012x}{}", NOW - 4 * 86_400_000, "0".repeat(20));
        fs::write(dir.path().join(file_name(&old, READY)), b"old").unwrap();
        fs::write(dir.path().join("unrelated.txt"), b"kept").unwrap();
        let (s, rec) = open(dir.path(), limits(), Arc::default());
        assert_eq!(rec, Recovery { tmp_removed: 1, acked_removed: 1, ready: 1, dropped: 1 });
        assert_eq!(s.oldest().map(|f| f.stem), Some(b.stem.clone()), "the sent-but-unacked file is sent again");
        assert_eq!(read(&b.path).1, vec![app_log_row("k", "b", NOW)]);
        s.ack(&b.stem).unwrap();
        assert_eq!(names(dir.path()), vec!["unrelated.txt".to_owned()]);
        let st = s.status(NOW);
        assert_eq!((st.files, st.bytes), (0, 0));
    }

    #[test]
    fn max_age_drops_waiting_files_while_running() {
        let dir = tempfile::tempdir().unwrap();
        let (s, _) = open(dir.path(), Limits { max_age_ms: 60_000, ..limits() }, Arc::default());
        s.append(&app_log_row("k", "a", NOW), NOW).unwrap();
        s.seal(NOW).unwrap();
        let created = s.oldest().unwrap().created_ms;
        s.tick(created + 30_000).unwrap();
        assert_eq!(s.status(created).files, 1);
        s.tick(created + 61_000).unwrap();
        let st = s.status(created);
        assert_eq!((st.files, st.dropped_files), (0, 1));
        assert!(names(dir.path()).is_empty());
    }

    #[test]
    fn rows_of_every_kind_round_trip_and_tuning_is_clamped() {
        let dir = tempfile::tempdir().unwrap();
        let (s, _) = open(dir.path(), limits(), Arc::default());
        let mut b = forge_core::analytics::MinuteBucket::new(0);
        b.add(&forge_core::analytics::NodeSample { cpu: 10.0, ..Default::default() });
        let row = Row::NodeMinute(b.close());
        s.append(&row, NOW).unwrap();
        s.seal(NOW).unwrap();
        let (h, rows) = read(&s.oldest().unwrap().path);
        assert_eq!((h.counts.node_minute, rows), (1, vec![row]));
        let l = Limits::from_tuning(&SpoolTuning { zstd_level: 19, max_bytes: 1, seal_secs: 0, ..Default::default() });
        assert_eq!((l.level, l.max_bytes, l.seal_ms), (3, 16 * MIB, 10_000));
    }
}
