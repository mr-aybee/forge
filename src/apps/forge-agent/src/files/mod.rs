//! File explorer operations (`files` command). On Linux they run in the root
//! helper; on Windows in the agent service (LocalSystem).
//!
//! Confinement, enforced here where the operation takes effect:
//! * Every operation is validated again (`FileOp::validate`) and every path
//!   it touches is checked against the protected-path policy (on the path as
//!   seen from the node, or from inside the container) before anything opens.
//! * The platform backend ([`Fs`]) resolves every path from the root's base
//!   handle one component at a time and never follows a link: Linux with
//!   `openat(O_NOFOLLOW)` and directory-relative calls, Windows by refusing
//!   reparse points. The written path is therefore the real path.
//! * Partial uploads are private (`0600`, root-owned on Linux) files next to
//!   their destination, hidden from listings, and moved into place only after
//!   their size and SHA-256 match.
//! * Trash and revisions live in Forge's agent data directory, a protected
//!   path, so no root can reach them directly.
//! * A native app's folder (`RootSpec::App`, Linux) is never worked on as
//!   root: [`app_worker`] runs each operation as the app's own account, with
//!   its trash and revisions in the app's store (D-083).

#[cfg(target_os = "linux")]
pub mod app_worker;
mod glob;
mod manage;
mod search;

#[cfg(unix)]
mod linux;
#[cfg(windows)]
mod windows;

#[cfg(test)]
mod tests;

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

use base64::Engine as _;
use forge_core::files::{
    EDITABLE_BYTES, EntryKind, FileEntry, FileOp, FilePath, MAX_REVISIONS, NodeOs, Overwrite, RootSpec, UPLOAD_PREFIX,
    is_forge_internal, protected_reason, protected_within,
};
#[cfg(unix)]
pub(crate) use linux::Root;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
#[cfg(windows)]
pub(crate) use windows::Root;

/// A directory with more entries than this is listed partially.
const MAX_DIR_ENTRIES: usize = 100_000;
/// Copies (and cross-filesystem moves) larger than this are refused.
const MAX_COPY_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_COPY_ENTRIES: usize = 10_000;
/// Recursive deletes stop after this many entries.
const MAX_DELETE_ENTRIES: usize = 200_000;
/// Trash entries older than this are purged whenever something is trashed.
const TRASH_DAYS: i64 = 30;
/// Total trash size kept; the oldest entries go first.
const TRASH_BYTES: u64 = 10 * 1024 * 1024 * 1024;
const REVISION_DAYS: i64 = 30;

static STORE_OVERRIDE: OnceLock<std::path::PathBuf> = OnceLock::new();

/// Moves Forge's trash and revision store for this process (integration
/// tests). The first call wins; production agents never call it.
#[doc(hidden)]
pub fn store_override(dir: &std::path::Path) {
    let _ = STORE_OVERRIDE.set(dir.to_path_buf());
}

/// Forge's private store for trash and revisions, in the agent's protected
/// data directory.
pub(crate) fn store_dir_or(default: &str) -> std::path::PathBuf {
    STORE_OVERRIDE.get().cloned().unwrap_or_else(|| std::path::PathBuf::from(default))
}

#[cfg(unix)]
const OS: NodeOs = NodeOs::Linux;
#[cfg(windows)]
const OS: NodeOs = NodeOs::Windows;

/// Metadata of one entry as the backends report it (links not followed).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Meta {
    pub kind: Kind,
    pub size: u64,
    pub modified_ms: i64,
    pub mode: Option<u32>,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    pub nlink: u64,
    /// Device and inode (Linux): mount detection and hard links.
    pub dev: u64,
    pub ino: u64,
    pub readonly: bool,
    pub hidden: bool,
    pub link_target: Option<String>,
    /// Opaque identity-and-content token.
    pub version: String,
}

/// [`EntryKind`] with a default, for [`Meta`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Kind {
    File,
    Dir,
    Symlink,
    #[default]
    Other,
}

impl From<Kind> for EntryKind {
    fn from(k: Kind) -> Self {
        match k {
            Kind::File => EntryKind::File,
            Kind::Dir => EntryKind::Dir,
            Kind::Symlink => EntryKind::Symlink,
            Kind::Other => EntryKind::Other,
        }
    }
}

/// Errors callers branch on.
#[derive(Debug)]
pub(crate) enum FsError {
    /// Rename across filesystems: copy instead.
    CrossDevice,
    /// The destination exists.
    Exists,
    Other(String),
}

impl From<String> for FsError {
    fn from(s: String) -> Self {
        Self::Other(s)
    }
}

impl FsError {
    fn msg(self) -> String {
        match self {
            Self::CrossDevice => "the source and destination are on different filesystems".into(),
            Self::Exists => "the destination already exists".into(),
            Self::Other(s) => s,
        }
    }
}

/// Positional reads and writes on an open file.
pub(crate) trait FileIo {
    fn read_at(&self, buf: &mut [u8], offset: u64) -> Result<usize, String>;
    fn write_all_at(&self, buf: &[u8], offset: u64) -> Result<(), String>;
    fn meta(&self) -> Result<Meta, String>;
    fn sync(&self) -> Result<(), String>;
    /// Owner and permission bits like `like` (Linux; no-op on Windows).
    fn chown_like(&self, like: &Meta) -> Result<(), String>;
}

/// A root opened on this node. Paths are component slices below the root's
/// base; implementations resolve them without following links.
pub(crate) trait Fs {
    type File: FileIo;
    fn stat(&self, path: &[String]) -> Result<Meta, String>;
    /// Names in a directory (without `.`/`..`), the count of names that are
    /// not UTF-8, and whether the listing was cut at [`MAX_DIR_ENTRIES`].
    fn read_dir(&self, path: &[String]) -> Result<(Vec<String>, usize, bool), String>;
    /// Opens an existing file for reading.
    fn open_read(&self, path: &[String]) -> Result<Self::File, String>;
    /// Opens a file for writing; `create` makes it (mode `0600`) if missing,
    /// `truncate` empties it. Fails on links and non-regular files.
    fn open_write(&self, path: &[String], create: bool, truncate: bool) -> Result<Self::File, String>;
    fn mkdir(&self, path: &[String]) -> Result<(), FsError>;
    /// Renames within this root, or into `to_root` (another root of this
    /// platform, such as the trash).
    fn rename(&self, from: &[String], to_root: &Self, to: &[String], replace: bool) -> Result<(), FsError>;
    /// Swaps two entries of this root in one atomic step (Linux
    /// `RENAME_EXCHANGE`). `Ok(false)` when the platform or filesystem cannot,
    /// so the caller falls back to two renames.
    fn exchange(&self, a: &[String], b: &[String]) -> Result<bool, String>;
    fn remove_file(&self, path: &[String]) -> Result<(), String>;
    fn remove_dir(&self, path: &[String]) -> Result<(), String>;
    fn symlink(&self, target: &str, path: &[String]) -> Result<(), String>;
    /// Gives a new entry the owner and group of its parent (Linux).
    fn inherit_owner(&self, path: &[String]) -> Result<(), String>;
    /// Free bytes on the filesystem holding `path`'s parent, if known.
    fn free_bytes(&self, path: &[String]) -> Option<u64>;
    /// Whether `(dev, ino)` is a file in a protected location (hard links).
    fn is_protected_inode(&self, dev: u64, ino: u64) -> bool;
    /// Sets permission bits (regular files and directories only).
    fn set_mode(&self, path: &[String], mode: u32) -> Result<(), String>;
    /// Sets owner and/or group (links are changed, never followed).
    fn set_owner(&self, path: &[String], uid: Option<u32>, gid: Option<u32>) -> Result<(), String>;
    /// The access control list as text (Windows `icacls`; Linux: mode only).
    fn acl(&self, path: &[String]) -> Result<String, String>;
    /// Forge's private store (`trash`, `revisions`) for this node.
    fn store(&self, sub: &str) -> Result<Self, String>
    where
        Self: Sized;
}

/// Searches running on this node at once.
const MAX_SEARCHES: usize = 2;

/// Holds one of the [`MAX_SEARCHES`] slots.
struct SearchSlot;

impl SearchSlot {
    fn count() -> &'static std::sync::atomic::AtomicUsize {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        &N
    }

    fn take() -> Result<Self, String> {
        use std::sync::atomic::Ordering;
        let n = Self::count();
        let mut cur = n.load(Ordering::Acquire);
        loop {
            if cur >= MAX_SEARCHES {
                return Err("the node is busy with other searches; try again shortly".into());
            }
            match n.compare_exchange(cur, cur + 1, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => return Ok(Self),
                Err(now) => cur = now,
            }
        }
    }
}

impl Drop for SearchSlot {
    fn drop(&mut self) {
        Self::count().fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }
}

/// Lowers the calling thread's CPU (and, by default, I/O) priority.
fn lower_priority() {
    #[cfg(target_os = "linux")]
    {
        // On Linux the nice value is per thread; the I/O priority of a
        // thread without an explicit class follows it.
        let _ = rustix::process::setpriority_process(None, 15);
    }
}

/// Runs a search on its own low-priority thread, so it neither slows
/// management commands nor leaves a pool thread at low priority.
async fn run_search(op: FileOp) -> Result<Value, String> {
    let slot = SearchSlot::take()?;
    let (tx, rx) = tokio::sync::oneshot::channel();
    std::thread::Builder::new()
        .name("forge-file-search".into())
        .spawn(move || {
            let _slot = slot;
            lower_priority();
            let r = Root::open(op.root(), None).and_then(|root| Ops::new(&root, op.root(), false).run(&op));
            let _ = tx.send(r);
        })
        .map_err(|e| format!("cannot start the search: {e}"))?;
    rx.await.map_err(|_| "the search stopped unexpectedly".to_owned())?
}

/// Runs one operation against this node's filesystem.
pub async fn run_async(op: FileOp) -> Result<Value, String> {
    op.validate()?;
    // D-083: an app's folder is worked on as the app's account. A plain
    // directory on an app's folder (from an older control plane) is treated
    // as that app's root; one around or inside app folders is refused.
    let op = match op.root().clone() {
        RootSpec::Dir { .. } if OS == NodeOs::Linux => {
            let spec = op.root().clone().for_linux_apps()?;
            op.with_root(spec)
        }
        _ => op,
    };
    if matches!(op.root(), RootSpec::App { .. }) {
        #[cfg(target_os = "linux")]
        return app_worker::run(op).await;
        #[cfg(not(target_os = "linux"))]
        return Err("app roots need a Linux node".into());
    }
    run_local(op).await
}

/// Runs one validated operation in this process, as whoever runs it: root
/// in the helper, or an app's account in its file worker.
pub(crate) async fn run_local(op: FileOp) -> Result<Value, String> {
    if matches!(op, FileOp::Search { .. }) && !matches!(op.root(), RootSpec::Container { .. }) {
        return run_search(op).await;
    }
    let pid = match op.root() {
        RootSpec::Container { container, .. } => Some(container_pid(container).await?),
        _ => None,
    };
    let spec = op.root().clone();
    let result = tokio::task::spawn_blocking(move || {
        let root = Root::open(op.root(), pid)?;
        Ops::new(&root, op.root(), pid.is_some()).run(&op)
    })
    .await
    .map_err(|_| "file operation panicked".to_owned())?;
    // A container replaced or restarted while the operation ran: say so
    // rather than report a result from another process's filesystem.
    if let (Some(before), RootSpec::Container { container, .. }) = (pid, &spec)
        && container_pid(container).await.ok() != Some(before)
    {
        return Err("the container was restarted or replaced during the operation; check the result and retry".into());
    }
    result
}

/// Container roots need Docker on a Linux node.
#[cfg(not(unix))]
async fn container_pid(_id: &str) -> Result<u32, String> {
    Err("container files are not supported on Windows nodes".into())
}

/// The init process of a running container, by exact id.
#[cfg(unix)]
async fn container_pid(id: &str) -> Result<u32, String> {
    let v = crate::docker::api("GET", &format!("/containers/{id}/json"), None, std::time::Duration::from_secs(10))
        .await?
        .ok_or("the container was not found")?;
    if v["Id"].as_str() != Some(id) {
        return Err("the container was replaced; reopen it".into());
    }
    if v.pointer("/State/Running").and_then(Value::as_bool) != Some(true) {
        return Err("the container is not running; start it to browse its files".into());
    }
    v.pointer("/State/Pid")
        .and_then(Value::as_u64)
        .and_then(|p| u32::try_from(p).ok())
        .filter(|p| *p > 0)
        .ok_or_else(|| "the container has no running process".to_owned())
}

/// Inside a container the container's own system paths are protected.
fn policy_os(root: &RootSpec) -> NodeOs {
    if matches!(root, RootSpec::Container { .. }) { NodeOs::Linux } else { OS }
}

/// The policy check for every path an operation touches.
pub(crate) fn protected(root: &RootSpec, path: &FilePath) -> Result<(), String> {
    match protected_reason(policy_os(root), &root.full_path(path)) {
        Some(why) => Err(format!("protected path ({why})")),
        None => Ok(()),
    }
}

/// The check for a path moved, replaced or removed as a whole: nothing
/// protected may lie below it either.
pub(crate) fn protected_tree(root: &RootSpec, path: &FilePath) -> Result<(), String> {
    protected(root, path)?;
    match protected_within(policy_os(root), &root.full_path(path)) {
        Some(why) => Err(format!("protected path (it contains protected files: {why})")),
        None => Ok(()),
    }
}

/// The error for a path on the way that exists but is no folder (a link
/// included: links are never followed).
fn not_a_folder(p: &FilePath) -> String {
    format!("`{p}` is a link or not a folder; Forge does not follow links")
}

/// A partial upload file not written for this long is abandoned: upload
/// sessions expire 24 hours after they start (`forge_files::UPLOAD_TTL_MS`),
/// so no resumable upload can still need it.
pub(crate) const STALE_PART_MS: i64 = 24 * 60 * 60 * 1000;

/// Removes abandoned partial uploads (`.forge-upload-*.part` regular files
/// older than [`STALE_PART_MS`]) in `dir`. Best effort; folders with that
/// prefix (extractions in progress) and anything else are left alone.
pub(crate) fn sweep_stale_parts<F: Fs>(fs: &F, dir: &[String]) {
    let Ok((names, _, _)) = fs.read_dir(dir) else { return };
    let cutoff = now_ms() - STALE_PART_MS;
    for n in names.into_iter().filter(|n| n.starts_with(UPLOAD_PREFIX) && n.ends_with(".part")) {
        let p = with(dir, n);
        if let Ok(m) = fs.stat(&p)
            && m.kind == Kind::File
            && m.modified_ms < cutoff
        {
            let _ = fs.remove_file(&p);
        }
    }
}

pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

pub(crate) fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub(crate) fn random_id() -> String {
    use ring::rand::SecureRandom as _;
    let mut b = [0u8; 12];
    let _ = ring::rand::SystemRandom::new().fill(&mut b);
    hex(&b)
}

/// One writer at a time per destination (commits, renames, copies).
struct PathLock(String);

impl PathLock {
    fn set() -> &'static Mutex<HashSet<String>> {
        static LOCKS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
        LOCKS.get_or_init(|| Mutex::new(HashSet::new()))
    }

    fn take(key: String) -> Result<Self, String> {
        let mut set = Self::set().lock().map_err(|_| "lock poisoned".to_owned())?;
        if !set.insert(key.clone()) {
            return Err("another change to this path is in progress; try again".into());
        }
        Ok(Self(key))
    }
}

impl Drop for PathLock {
    fn drop(&mut self) {
        if let Ok(mut set) = Self::set().lock() {
            set.remove(&self.0);
        }
    }
}

/// What a trash entry remembers.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TrashMeta {
    root: RootSpec,
    path: FilePath,
    deleted_at: i64,
    size: u64,
    kind: EntryKind,
}

/// uid/gid → name from `/etc/passwd` and `/etc/group` (empty on Windows).
pub(crate) struct Ids {
    pub(crate) users: std::collections::HashMap<u32, String>,
    pub(crate) groups: std::collections::HashMap<u32, String>,
}

impl Ids {
    /// Names, re-read at most once a minute.
    pub(crate) fn get() -> std::sync::Arc<Ids> {
        static IDS: Mutex<Option<(std::time::Instant, std::sync::Arc<Ids>)>> = Mutex::new(None);
        let mut cached = IDS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((at, ids)) = cached.as_ref()
            && at.elapsed() < std::time::Duration::from_secs(60)
        {
            return ids.clone();
        }
        let ids = std::sync::Arc::new(Self::load());
        *cached = Some((std::time::Instant::now(), ids.clone()));
        ids
    }

    fn load() -> Ids {
        {
            let parse = |file: &str| {
                std::fs::read_to_string(file)
                    .unwrap_or_default()
                    .lines()
                    .filter_map(|l| {
                        let mut f = l.split(':');
                        let name = f.next()?;
                        let id = f.nth(1)?.parse().ok()?;
                        Some((id, name.to_owned()))
                    })
                    .collect()
            };
            Ids { users: parse("/etc/passwd"), groups: parse("/etc/group") }
        }
    }
}

fn entry(name: &str, m: &Meta) -> FileEntry {
    let ids = Ids::get();
    FileEntry {
        name: name.to_owned(),
        kind: m.kind.into(),
        size: if m.kind == Kind::File { m.size } else { 0 },
        modified_ms: m.modified_ms,
        mode: m.mode.map(|x| x & 0o7777),
        owner: m.uid.map(|u| ids.users.get(&u).cloned().unwrap_or_else(|| u.to_string())),
        group: m.gid.map(|g| ids.groups.get(&g).cloned().unwrap_or_else(|| g.to_string())),
        link_target: m.link_target.clone(),
        mount: false,
        protected: false,
        readonly: m.readonly,
        hidden: m.hidden,
    }
}

pub(crate) fn c(p: &FilePath) -> &[String] {
    p.components()
}

pub(crate) fn with(p: &[String], name: impl Into<String>) -> Vec<String> {
    let mut v = p.to_vec();
    v.push(name.into());
    v
}

/// Copy limits.
struct Budget {
    bytes: u64,
    entries: usize,
}

impl Budget {
    fn new() -> Self {
        Self { bytes: 0, entries: 0 }
    }

    fn take(&mut self, m: &Meta) -> Result<(), String> {
        self.entries += 1;
        if m.kind == Kind::File {
            self.bytes = self.bytes.saturating_add(m.size);
        }
        if self.entries > MAX_COPY_ENTRIES || self.bytes > MAX_COPY_BYTES {
            return Err("too large to copy at once (limit: 10,000 entries or 1 GiB)".into());
        }
        Ok(())
    }
}

fn write_small<F: Fs>(fs: &F, path: &[String], data: &[u8]) -> Result<(), String> {
    let f = fs.open_write(path, true, true)?;
    f.write_all_at(data, 0)?;
    f.sync()
}

fn read_all<F: Fs>(fs: &F, path: &[String], max: u64) -> Result<Vec<u8>, String> {
    let f = fs.open_read(path)?;
    let size = f.meta()?.size.min(max);
    read_range(&f, 0, size)
}

pub(crate) fn read_range<T: FileIo>(f: &T, offset: u64, want: u64) -> Result<Vec<u8>, String> {
    let mut buf = vec![0u8; usize::try_from(want).unwrap_or(0)];
    let mut got = 0usize;
    while got < buf.len() {
        match f.read_at(&mut buf[got..], offset + got as u64)? {
            0 => break,
            n => got += n,
        }
    }
    buf.truncate(got);
    Ok(buf)
}

fn sha256_of<T: FileIo>(f: &T, size: u64) -> Result<String, String> {
    let mut ctx = ring::digest::Context::new(&ring::digest::SHA256);
    let mut buf = vec![0u8; 1024 * 1024];
    let mut off = 0u64;
    while off < size {
        let n = f.read_at(&mut buf, off)?;
        if n == 0 {
            break;
        }
        ctx.update(&buf[..n]);
        off += n as u64;
    }
    Ok(hex(ctx.finish().as_ref()))
}

/// Removes a tree without following links.
pub(crate) fn remove_tree<F: Fs>(fs: &F, path: &[String], count: &mut usize) -> Result<(), String> {
    *count += 1;
    if *count > MAX_DELETE_ENTRIES {
        return Err("too many entries to delete at once".into());
    }
    let m = fs.stat(path)?;
    if m.kind == Kind::Dir {
        let (names, _, _) = fs.read_dir(path)?;
        for n in names {
            remove_tree(fs, &with(path, n), count)?;
        }
        fs.remove_dir(path)
    } else {
        fs.remove_file(path)
    }
}

/// Streams a regular file from `src` into `dst` through a private
/// temporary file next to the destination, then renames it into place.
fn copy_file<F: Fs>(src_fs: &F, from: &[String], dst_fs: &F, to: &[String], replace: bool) -> Result<(), String> {
    let src = src_fs.open_read(from)?;
    let m = src.meta()?;
    if m.kind != Kind::File {
        return Err("not a regular file".into());
    }
    if let Some(free) = dst_fs.free_bytes(to)
        && free < m.size
    {
        return Err("not enough free space at the destination".into());
    }
    let mut tmp = to.to_vec();
    if let Some(last) = tmp.last_mut() {
        *last = format!("{UPLOAD_PREFIX}{}.part", random_id());
    }
    let out = dst_fs.open_write(&tmp, true, true)?;
    let result = (|| {
        let mut buf = vec![0u8; 1024 * 1024];
        let mut off = 0u64;
        while off < m.size {
            let n = src.read_at(&mut buf, off)?;
            if n == 0 {
                break;
            }
            out.write_all_at(&buf[..n], off)?;
            off += n as u64;
        }
        out.sync()?;
        out.chown_like(&m)?;
        dst_fs.rename(&tmp, dst_fs, to, replace).map_err(FsError::msg)
    })();
    if result.is_err() {
        let _ = dst_fs.remove_file(&tmp);
    }
    result
}

/// Vets a source and destination path pair (protected paths).
type PairCheck<'a> = dyn Fn(&[String], &[String]) -> Result<(), String> + 'a;

/// Copies a file, link or directory tree between roots of this platform.
/// `check` vets every source and destination path (protected paths).
fn copy_tree<F: Fs>(
    src_fs: &F,
    from: &[String],
    dst_fs: &F,
    to: &[String],
    replace: bool,
    budget: &mut Budget,
    check: &PairCheck<'_>,
) -> Result<(), String> {
    check(from, to)?;
    let m = src_fs.stat(from)?;
    budget.take(&m)?;
    match m.kind {
        Kind::File => {
            copy_file(src_fs, from, dst_fs, to, replace)?;
            Ok(())
        }
        Kind::Symlink => {
            if replace {
                let _ = dst_fs.remove_file(to);
            }
            dst_fs.symlink(m.link_target.as_deref().unwrap_or_default(), to)
        }
        Kind::Dir => {
            match dst_fs.mkdir(to) {
                Ok(()) => dst_fs.inherit_owner(to)?,
                Err(FsError::Exists) if replace => {}
                Err(e) => return Err(e.msg()),
            }
            let (names, _, _) = src_fs.read_dir(from)?;
            for n in names.into_iter().filter(|n| !is_forge_internal(n)) {
                copy_tree(src_fs, &with(from, n.clone()), dst_fs, &with(to, n), replace, budget, check)?;
            }
            Ok(())
        }
        // Sockets, devices and FIFOs are not copied.
        Kind::Other => Ok(()),
    }
}

/// Size of a tree (bounded walk; `u64::MAX` when too large to count).
fn tree_size<F: Fs>(fs: &F, path: &[String]) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![path.to_vec()];
    let mut seen = 0usize;
    while let Some(p) = stack.pop() {
        seen += 1;
        if seen > MAX_COPY_ENTRIES * 10 {
            return u64::MAX;
        }
        let Ok(m) = fs.stat(&p) else { continue };
        if m.kind == Kind::Dir {
            if let Ok((names, _, _)) = fs.read_dir(&p) {
                stack.extend(names.into_iter().map(|n| with(&p, n)));
            }
        } else {
            total = total.saturating_add(m.size);
        }
    }
    total
}

/// Operations over one opened root.
pub(crate) struct Ops<'a, F: Fs> {
    fs: &'a F,
    spec: &'a RootSpec,
    /// Container roots have no trash or revisions.
    container: bool,
}

impl<'a, F: Fs> Ops<'a, F> {
    pub(crate) fn new(fs: &'a F, spec: &'a RootSpec, container: bool) -> Self {
        Self { fs, spec, container }
    }

    pub(crate) fn run(&self, op: &FileOp) -> Result<Value, String> {
        for p in op.paths() {
            protected(self.spec, p)?;
        }
        for p in op.subtrees() {
            protected_tree(self.spec, p)?;
        }
        match op {
            FileOp::List { path, offset, limit, hidden, .. } => self.list(path, *offset, *limit, *hidden),
            FileOp::Stat { path, .. } => {
                let m = self.fs.stat(c(path))?;
                Ok(
                    json!({ "path": path.to_string(), "entry": entry(path.name().unwrap_or("/"), &m), "version": m.version }),
                )
            }
            FileOp::Read { path, offset, length, version, .. } => {
                let (f, m) = self.open_regular(c(path))?;
                if version.as_ref().is_some_and(|x| *x != m.version) {
                    return Err("source changed: the file was modified or replaced".into());
                }
                let data = read_range(&f, *offset, m.size.saturating_sub(*offset).min(u64::from(*length)))?;
                Ok(json!({
                    "path": path.to_string(),
                    "offset": offset,
                    "length": data.len(),
                    "size": m.size,
                    "eof": offset + data.len() as u64 >= m.size,
                    "version": m.version,
                    "modified_ms": m.modified_ms,
                    "content_b64": b64().encode(&data),
                }))
            }
            FileOp::Hash { path, .. } => {
                let (f, m) = self.open_regular(c(path))?;
                let hash = sha256_of(&f, m.size)?;
                Ok(json!({ "path": path.to_string(), "sha256": hash, "size": m.size, "version": m.version }))
            }
            FileOp::Mkdir { path, parents, .. } => self.mkdir(path, *parents),
            FileOp::UploadChunk { path, upload, offset, data_b64, .. } => self.chunk(path, upload, *offset, data_b64),
            FileOp::UploadStatus { path, upload, .. } => {
                let received = self.fs.stat(&Self::partial(path, upload)).map(|m| m.size).unwrap_or(0);
                Ok(json!({ "received": received }))
            }
            FileOp::UploadCommit { path, upload, size, sha256, overwrite, .. } => {
                self.commit(path, upload, *size, sha256, overwrite)
            }
            FileOp::UploadAbort { path, upload, .. } => {
                let _ = self.fs.remove_file(&Self::partial(path, upload));
                Ok(json!({ "aborted": true }))
            }
            FileOp::Rename { from, to, overwrite, .. } => self.rename(from, to, *overwrite),
            FileOp::Copy { from, to, overwrite, .. } => {
                let _lock = PathLock::take(self.lock_key(to))?;
                let mut budget = Budget::new();
                copy_tree(self.fs, c(from), self.fs, c(to), *overwrite, &mut budget, &|f, t| self.check_pair(f, t))?;
                Ok(
                    json!({ "from": from.to_string(), "to": to.to_string(), "bytes": budget.bytes, "entries": budget.entries }),
                )
            }
            FileOp::Delete { path, permanent, .. } => self.delete(path, *permanent),
            FileOp::TrashList { .. } => self.trash_list(),
            FileOp::TrashRestore { id, overwrite, .. } => self.trash_restore(id, *overwrite),
            FileOp::TrashPurge { id, .. } => self.trash_purge(id.as_deref()),
            FileOp::Revisions { path, .. } => self.revisions(path),
            FileOp::RevisionRead { path, revision, offset, length, .. } => {
                self.revision_read(path, revision, *offset, *length)
            }
            FileOp::Search { path, spec, .. } => search::run(self.fs, self.spec, path, spec),
            FileOp::SetPermissions { path, mode, recursive, dry_run, .. } => {
                if !manage::supports_permissions(self.spec) {
                    return Err("permissions can be changed on Linux node and directory roots only".into());
                }
                self.set_permissions(path, *mode, *recursive, *dry_run)
            }
            FileOp::SetOwner { path, owner, group, recursive, dry_run, .. } => {
                if !manage::supports_permissions(self.spec) {
                    return Err("owners can be changed on Linux node and directory roots only".into());
                }
                self.set_owner(path, owner.as_deref(), group.as_deref(), *recursive, *dry_run)
            }
            FileOp::Acl { path, .. } => self.acl(path),
            FileOp::Compress { sources, dest, format, .. } => self.compress(sources, dest.as_ref(), *format),
            FileOp::Extract { archive, dest, replace, .. } => self.extract(archive, dest, *replace),
            FileOp::TempRead { id, offset, length, .. } => self.temp_read(id, *offset, *length),
            FileOp::TempDelete { id, .. } => self.temp_delete(id),
        }
    }

    fn lock_key(&self, path: &FilePath) -> String {
        format!("{}\n{path}", serde_json::to_string(self.spec).unwrap_or_default())
    }

    fn check_pair(&self, from: &[String], to: &[String]) -> Result<(), String> {
        protected(self.spec, &FilePath::from_components(from))?;
        protected(self.spec, &FilePath::from_components(to))
    }

    fn list(&self, path: &FilePath, offset: u32, limit: u32, hidden: bool) -> Result<Value, String> {
        let dir = self.fs.stat(c(path))?;
        if dir.kind == Kind::Symlink {
            return Err(format!("`{path}` is a link; Forge does not follow links"));
        }
        if dir.kind != Kind::Dir {
            return Err(format!("`{path}` is not a directory"));
        }
        let (names, skipped, truncated) = self.fs.read_dir(c(path))?;
        let mut items: Vec<(String, Meta)> = names
            .into_iter()
            .filter(|n| !is_forge_internal(n) && (hidden || !n.starts_with('.')))
            // An entry removed since the listing is skipped.
            .filter_map(|n| self.fs.stat(&with(c(path), n.clone())).ok().map(|m| (n, m)))
            .filter(|(_, m)| hidden || !m.hidden)
            .collect();
        items.sort_by(|a, b| {
            if OS == NodeOs::Windows {
                a.0.to_lowercase().cmp(&b.0.to_lowercase()).then_with(|| a.0.cmp(&b.0))
            } else {
                a.0.cmp(&b.0)
            }
        });
        let total = items.len();
        let entries: Vec<FileEntry> = items
            .into_iter()
            .skip(offset as usize)
            .take(limit as usize)
            .map(|(n, m)| {
                let mut e = entry(&n, &m);
                e.mount = m.kind == Kind::Dir && dir.dev != 0 && m.dev != dir.dev;
                e.protected = path.join(&n).map_or(true, |p| protected(self.spec, &p).is_err());
                e
            })
            .collect();
        Ok(json!({
            "path": path.to_string(),
            "offset": offset,
            "total": total,
            "truncated": truncated,
            "skipped_names": skipped,
            "entries": entries,
        }))
    }

    /// Opens a regular file, refusing hard links to protected files.
    pub(crate) fn open_regular(&self, path: &[String]) -> Result<(F::File, Meta), String> {
        let f = self.fs.open_read(path)?;
        let m = f.meta()?;
        if m.kind != Kind::File {
            return Err(format!("`{}` is not a regular file", path.last().map_or("/", String::as_str)));
        }
        if m.nlink > 1 && self.fs.is_protected_inode(m.dev, m.ino) {
            return Err("protected path (hard link to a protected file)".into());
        }
        Ok((f, m))
    }

    /// The first folder on the way to `dir` (itself included) that is
    /// missing or not a folder, for errors that name the whole path.
    pub(crate) fn first_missing(&self, dir: &FilePath) -> Option<FilePath> {
        let comps = c(dir);
        (1..=comps.len()).map(|i| &comps[..i]).find_map(|sub| match self.fs.stat(sub) {
            Ok(m) if m.kind == Kind::Dir => None,
            _ => Some(FilePath::from_components(sub)),
        })
    }

    /// The error for a folder that is missing below `dir`.
    pub(crate) fn missing_folder(&self, what: &str, dir: &FilePath) -> String {
        match self.first_missing(dir) {
            Some(m) if self.fs.stat(c(&m)).is_ok() => not_a_folder(&m),
            Some(m) if &m == dir => format!("{what} {dir} does not exist"),
            Some(m) => format!("{what} {dir} does not exist (its parent folder {m} is missing)"),
            None => format!("{what} {dir} is not available"),
        }
    }

    /// Creates `path`; with `parents`, every missing folder on the way.
    fn mkdir(&self, path: &FilePath, parents: bool) -> Result<Value, String> {
        if !parents {
            if let Some(m) = self.first_missing(&path.parent()) {
                return Err(if self.fs.stat(c(&m)).is_ok() {
                    not_a_folder(&m)
                } else {
                    format!("parent folder {m} does not exist; create it first or use `parents`")
                });
            }
            match self.fs.mkdir(c(path)) {
                Ok(()) => {}
                Err(FsError::Exists) => return Err(format!("`{path}` already exists")),
                Err(e) => return Err(e.msg()),
            }
            self.fs.inherit_owner(c(path))?;
            return Ok(json!({ "path": path.to_string(), "created": 1 }));
        }
        let comps = c(path);
        let mut created = 0u32;
        for i in 1..=comps.len() {
            let sub = &comps[..i];
            let shown = FilePath::from_components(sub);
            protected(self.spec, &shown)?;
            match self.fs.mkdir(sub) {
                Ok(()) => {
                    self.fs.inherit_owner(sub)?;
                    created += 1;
                }
                Err(FsError::Exists) => {
                    if self.fs.stat(sub)?.kind != Kind::Dir {
                        return Err(not_a_folder(&shown));
                    }
                }
                Err(e) => return Err(e.msg()),
            }
        }
        Ok(json!({ "path": path.to_string(), "created": created }))
    }

    /// `.forge-upload-<id>.part` next to the destination.
    fn partial(path: &FilePath, upload: &str) -> Vec<String> {
        with(c(&path.parent()), format!("{UPLOAD_PREFIX}{upload}.part"))
    }

    fn chunk(&self, path: &FilePath, upload: &str, offset: u64, data_b64: &str) -> Result<Value, String> {
        let data = b64().decode(data_b64).map_err(|_| "chunk is not valid base64".to_owned())?;
        if !matches!(self.fs.stat(c(&path.parent())), Ok(m) if m.kind == Kind::Dir) {
            return Err(self.missing_folder("the destination folder", &path.parent()));
        }
        let tmp = Self::partial(path, upload);
        let f = if offset == 0 {
            sweep_stale_parts(self.fs, c(&path.parent()));
            self.fs.open_write(&tmp, true, true)?
        } else {
            let f = self
                .fs
                .open_write(&tmp, false, false)
                .map_err(|_| "the upload was not started or has expired".to_owned())?;
            let have = f.meta()?.size;
            if have == offset + data.len() as u64 {
                // A retried chunk that was already written.
                return Ok(json!({ "received": have }));
            }
            if have != offset {
                return Err(format!("upload is at byte {have}, not {offset}; resume from there"));
            }
            f
        };
        if let Err(e) = f.write_all_at(&data, offset) {
            // A half-written chunk (disk full, I/O error) cannot be resumed
            // reliably: drop the partial file so nothing large stays behind.
            drop(f);
            let _ = self.fs.remove_file(&tmp);
            return Err(e);
        }
        Ok(json!({ "received": offset + data.len() as u64 }))
    }

    /// Commits an upload. On a refusal that cannot be retried (the target
    /// exists or changed, a failed rename) the partial file is removed, so a
    /// refused upload never leaves a full-size `.part` file behind (F-08).
    /// An incomplete upload or a checksum mismatch keeps it: the client can
    /// resume, or commit again with the right checksum. Abandoned partial
    /// files are swept ([`sweep_stale_parts`]) and removed when the session
    /// expires.
    fn commit(
        &self,
        path: &FilePath,
        upload: &str,
        size: u64,
        sha256: &str,
        overwrite: &Overwrite,
    ) -> Result<Value, String> {
        let _lock = PathLock::take(self.lock_key(path))?;
        let r = self.commit_locked(path, upload, size, sha256, overwrite);
        if let Err(e) = &r
            && !e.starts_with("upload incomplete")
            && !e.starts_with("the upload was not started")
            && !e.starts_with("checksum mismatch")
        {
            let _ = self.fs.remove_file(&Self::partial(path, upload));
        }
        r
    }

    fn commit_locked(
        &self,
        path: &FilePath,
        upload: &str,
        size: u64,
        sha256: &str,
        overwrite: &Overwrite,
    ) -> Result<Value, String> {
        let tmp = Self::partial(path, upload);
        let f = self
            .fs
            .open_write(&tmp, false, false)
            .map_err(|_| "the upload was not started or has expired".to_owned())?;
        let have = f.meta()?.size;
        if have != size {
            return Err(format!("upload incomplete: {have} of {size} bytes received"));
        }
        if !sha256_of(&f, size)?.eq_ignore_ascii_case(sha256) {
            return Err("checksum mismatch: the uploaded bytes differ from the source".into());
        }
        f.sync()?;
        let existing = self.fs.stat(c(path)).ok();
        match (&existing, overwrite) {
            (Some(m), _) if m.kind != Kind::File => return Err(format!("`{path}` exists and is not a regular file")),
            (Some(_), Overwrite::Never) => return Err("the destination already exists".into()),
            (Some(m), Overwrite::IfVersion { version }) if &m.version != version => {
                return Err("source changed: the file was modified since you opened it".into());
            }
            (None, Overwrite::IfVersion { .. }) => {
                return Err("source changed: the file was deleted since you opened it".into());
            }
            _ => {}
        }
        match &existing {
            Some(m) => {
                // New content keeps the owner and permissions but never
                // setuid/setgid: replacing a setuid binary must not hand
                // the new bytes its privileges (re-add with `file.admin`).
                f.chown_like(&Meta { mode: m.mode.map(|x| x & !0o6000), ..m.clone() })?;
                if m.size <= EDITABLE_BYTES && !self.container {
                    // Best effort: a failed revision never blocks a save.
                    let _ = self.save_revision(path);
                }
            }
            None => {
                // The partial file is private (0600) while it uploads; a new
                // file gets the usual 0644 and its folder's owner.
                f.chown_like(&Meta { mode: Some(0o644), ..Meta::default() })?;
                drop(f);
                self.fs.inherit_owner(&tmp)?;
            }
        }
        self.fs.rename(&tmp, self.fs, c(path), existing.is_some()).map_err(|e| match e {
            FsError::Exists => "the destination was created by someone else meanwhile".to_owned(),
            e => e.msg(),
        })?;
        let m = self.fs.stat(c(path))?;
        Ok(json!({ "path": path.to_string(), "size": m.size, "version": m.version, "replaced": existing.is_some() }))
    }

    fn rename(&self, from: &FilePath, to: &FilePath, overwrite: bool) -> Result<Value, String> {
        let _lock = PathLock::take(self.lock_key(to))?;
        let src = self.fs.stat(c(from))?;
        match self.fs.rename(c(from), self.fs, c(to), overwrite) {
            Ok(()) => {}
            Err(FsError::CrossDevice) => {
                let mut budget = Budget::new();
                copy_tree(self.fs, c(from), self.fs, c(to), overwrite, &mut budget, &|f, t| self.check_pair(f, t))?;
                remove_tree(self.fs, c(from), &mut 0)?;
            }
            Err(e) => return Err(e.msg()),
        }
        Ok(json!({ "from": from.to_string(), "to": to.to_string(), "kind": EntryKind::from(src.kind) }))
    }

    fn delete(&self, path: &FilePath, permanent: bool) -> Result<Value, String> {
        let m = self.fs.stat(c(path))?;
        if permanent {
            let mut n = 0;
            remove_tree(self.fs, c(path), &mut n)?;
            return Ok(json!({ "path": path.to_string(), "permanent": true, "entries": n }));
        }
        if self.container {
            return Err("containers have no trash; delete permanently instead".into());
        }
        let trash = self.fs.store("trash")?;
        let id = format!("{}-{}", now_ms(), random_id());
        trash.mkdir(std::slice::from_ref(&id)).map_err(FsError::msg)?;
        let size = tree_size(self.fs, c(path));
        let meta =
            TrashMeta { root: self.spec.clone(), path: path.clone(), deleted_at: now_ms(), size, kind: m.kind.into() };
        let undo = |e: String| {
            let _ = remove_tree(&trash, std::slice::from_ref(&id), &mut 0);
            e
        };
        write_small(
            &trash,
            &with(std::slice::from_ref(&id), "meta.json"),
            &serde_json::to_vec(&meta).map_err(|e| e.to_string())?,
        )
        .map_err(undo)?;
        let dest = with(std::slice::from_ref(&id), "item");
        match self.fs.rename(c(path), &trash, &dest, false) {
            Ok(()) => {}
            Err(FsError::CrossDevice) => {
                if size > MAX_COPY_BYTES {
                    return Err(undo(
                        "too large to move to the trash from this filesystem; delete permanently instead".into(),
                    ));
                }
                let mut budget = Budget::new();
                copy_tree(self.fs, c(path), &trash, &dest, false, &mut budget, &|_, _| Ok(())).map_err(undo)?;
                remove_tree(self.fs, c(path), &mut 0)?;
            }
            Err(e) => return Err(undo(e.msg())),
        }
        prune_trash(&trash);
        Ok(json!({ "path": path.to_string(), "trash_id": id, "size": size }))
    }

    fn trash_entries(&self, trash: &F) -> Vec<(String, TrashMeta)> {
        let Ok((ids, _, _)) = trash.read_dir(&[]) else { return vec![] };
        ids.into_iter()
            .filter_map(|id| {
                let raw = read_all(trash, &with(std::slice::from_ref(&id), "meta.json"), 64 * 1024).ok()?;
                Some((id, serde_json::from_slice::<TrashMeta>(&raw).ok()?))
            })
            .filter(|(_, m)| &m.root == self.spec)
            .collect()
    }

    fn trash_list(&self) -> Result<Value, String> {
        if self.container {
            return Ok(json!({ "entries": [] }));
        }
        let trash = self.fs.store("trash")?;
        let mut entries = self.trash_entries(&trash);
        entries.sort_by_key(|(_, m)| std::cmp::Reverse(m.deleted_at));
        let entries: Vec<Value> = entries
            .into_iter()
            .map(|(id, m)| json!({ "id": id, "path": m.path.to_string(), "deleted_at": m.deleted_at, "size": m.size, "kind": m.kind }))
            .collect();
        Ok(json!({ "entries": entries }))
    }

    fn trash_restore(&self, id: &str, overwrite: bool) -> Result<Value, String> {
        let trash = self.fs.store("trash")?;
        let (_, meta) = self
            .trash_entries(&trash)
            .into_iter()
            .find(|(i, _)| i == id)
            .ok_or("that trash entry does not exist for this root")?;
        protected_tree(self.spec, &meta.path)?;
        let parent = self.fs.stat(c(&meta.path.parent()));
        if parent.map(|m| m.kind) != Ok(Kind::Dir) {
            return Err("the original folder no longer exists; recreate it first".into());
        }
        let item = with(&[id.to_owned()], "item");
        let _lock = PathLock::take(self.lock_key(&meta.path))?;
        match trash.rename(&item, self.fs, c(&meta.path), overwrite) {
            Ok(()) => {}
            Err(FsError::CrossDevice) => {
                let mut budget = Budget::new();
                copy_tree(&trash, &item, self.fs, c(&meta.path), overwrite, &mut budget, &|_, t| {
                    protected(self.spec, &FilePath::from_components(t))
                })?;
            }
            Err(FsError::Exists) => return Err("something now exists at the original path".into()),
            Err(e) => return Err(e.msg()),
        }
        remove_tree(&trash, &[id.to_owned()], &mut 0)?;
        Ok(json!({ "path": meta.path.to_string(), "restored": true }))
    }

    fn trash_purge(&self, id: Option<&str>) -> Result<Value, String> {
        let trash = self.fs.store("trash")?;
        let mut purged = 0usize;
        for (i, _) in self.trash_entries(&trash) {
            if id.is_none_or(|x| x == i) {
                remove_tree(&trash, &[i], &mut 0)?;
                purged += 1;
            }
        }
        if id.is_some() && purged == 0 {
            return Err("that trash entry does not exist for this root".into());
        }
        Ok(json!({ "purged": purged }))
    }

    /// Directory in the revision store for one file of one root.
    fn revision_key(&self, path: &FilePath) -> String {
        hex(ring::digest::digest(&ring::digest::SHA256, self.lock_key(path).as_bytes()).as_ref())
    }

    fn save_revision(&self, path: &FilePath) -> Result<(), String> {
        let store = self.fs.store("revisions")?;
        let key = self.revision_key(path);
        match store.mkdir(std::slice::from_ref(&key)) {
            Ok(()) | Err(FsError::Exists) => {}
            Err(e) => return Err(e.msg()),
        }
        copy_file(
            self.fs,
            c(path),
            &store,
            &with(std::slice::from_ref(&key), format!("{}-{}", now_ms(), random_id())),
            false,
        )?;
        let (mut names, _, _) = store.read_dir(std::slice::from_ref(&key))?;
        names.sort_unstable_by(|a, b| b.cmp(a));
        let cutoff = now_ms() - REVISION_DAYS * 86_400_000;
        for (i, n) in names.iter().enumerate() {
            let ts: i64 = n.split('-').next().and_then(|t| t.parse().ok()).unwrap_or(0);
            if i >= MAX_REVISIONS || ts < cutoff {
                let _ = store.remove_file(&with(std::slice::from_ref(&key), n.clone()));
            }
        }
        Ok(())
    }

    fn revisions(&self, path: &FilePath) -> Result<Value, String> {
        if self.container {
            return Ok(json!({ "path": path.to_string(), "revisions": [] }));
        }
        let store = self.fs.store("revisions")?;
        let key = self.revision_key(path);
        let (mut names, _, _) = store.read_dir(std::slice::from_ref(&key)).unwrap_or_default();
        names.sort_unstable_by(|a, b| b.cmp(a));
        let revs: Vec<Value> = names
            .into_iter()
            .filter_map(|n| {
                let m = store.stat(&with(std::slice::from_ref(&key), n.clone())).ok()?;
                let ts: i64 = n.split('-').next()?.parse().ok()?;
                Some(json!({ "id": n, "saved_at": ts, "size": m.size }))
            })
            .collect();
        Ok(json!({ "path": path.to_string(), "revisions": revs }))
    }

    fn revision_read(&self, path: &FilePath, rev: &str, offset: u64, length: u32) -> Result<Value, String> {
        let store = self.fs.store("revisions")?;
        let f = store
            .open_read(&with(&[self.revision_key(path)], rev))
            .map_err(|_| "that revision does not exist".to_owned())?;
        let m = f.meta()?;
        let data = read_range(&f, offset, m.size.saturating_sub(offset).min(u64::from(length)))?;
        Ok(json!({
            "path": path.to_string(),
            "revision": rev,
            "offset": offset,
            "length": data.len(),
            "size": m.size,
            "eof": offset + data.len() as u64 >= m.size,
            "content_b64": b64().encode(&data),
        }))
    }
}

/// Drops trash entries past the age limit, then the oldest past the size cap.
fn prune_trash<F: Fs>(trash: &F) {
    let Ok((mut ids, _, _)) = trash.read_dir(&[]) else { return };
    // Ids start with the deletion time: newest first.
    ids.sort_unstable_by(|a, b| b.cmp(a));
    let cutoff = now_ms() - TRASH_DAYS * 86_400_000;
    let mut total = 0u64;
    for id in ids {
        let meta = read_all(trash, &with(std::slice::from_ref(&id), "meta.json"), 64 * 1024)
            .ok()
            .and_then(|r| serde_json::from_slice::<TrashMeta>(&r).ok());
        let (ts, size) = meta.map_or((0, 0), |m| (m.deleted_at, m.size));
        total = total.saturating_add(size);
        if ts < cutoff || total > TRASH_BYTES {
            let _ = remove_tree(trash, &[id], &mut 0);
        }
    }
}
