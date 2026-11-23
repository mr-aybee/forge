//! SFTP requests mapped to file-service operations.
//!
//! The client sees a virtual tree: `/` lists the grant's roots by name, and
//! `/<root>/…` is a path inside that root. Paths from the client are
//! normalised lexically (`.` and `..` cannot climb above `/`); the file
//! service and the node then validate and confine them like any other.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use forge_core::files::{
    EntryKind, FileEntry, FileOp, FilePath, MAX_CHUNK_BYTES, MAX_READ_BYTES, Overwrite, RootKind, RootSpec,
};
use forge_files::{Actor, Audit, FileError, FileService};
use russh_sftp::protocol::{Attrs, Data, File, FileAttributes, Handle, Name, OpenFlags, Status, StatusCode, Version};
use russh_sftp::server::StatusReply;
use serde_json::{Value, json};

/// Directory entries fetched per listing page.
const PAGE: u32 = 1000;
/// Directory entries shown at most.
const MAX_ENTRIES: usize = 20_000;
/// Usage is recorded at least every this many bytes.
const USAGE_EVERY: u64 = 4 * 1024 * 1024;
/// Node operations sent together in a transfer: upload chunks per batch and
/// reads ahead (issue #15). Each round trip over the agent link costs tens
/// of milliseconds, so batching sets the speed; 4 × 256 KiB keeps one
/// heartbeat answer and its results well inside the link's limits.
const BATCH: usize = 4;

/// An SFTP error reply.
#[derive(Debug)]
pub struct SftpError(StatusReply);

impl From<SftpError> for StatusReply {
    fn from(e: SftpError) -> Self {
        e.0
    }
}

fn err(code: StatusCode, msg: impl Into<String>) -> SftpError {
    SftpError(code.with_message(msg))
}

impl From<FileError> for SftpError {
    fn from(e: FileError) -> Self {
        let code = match e {
            FileError::NotFound(_) => StatusCode::NoSuchFile,
            FileError::Denied(_) | FileError::StepUp(_) => StatusCode::PermissionDenied,
            FileError::Unavailable(_) => StatusCode::NoConnection,
            _ => StatusCode::Failure,
        };
        err(code, e.to_string())
    }
}

type R<T> = Result<T, SftpError>;

struct RootView {
    name: String,
    id: String,
    kind: RootKind,
}

enum Open {
    Dir {
        listed: bool,
        path: String,
    },
    Read {
        root: String,
        path: FilePath,
        size: u64,
        version: String,
        sent: u64,
        started: Instant,
        /// Read-ahead: bytes fetched from the node at this offset. SFTP
        /// clients read 32 KiB at a time; each node read is a round trip
        /// over the agent link, so the gateway fetches up to
        /// `MAX_READ_BYTES` at once and serves the client from here.
        ahead: Option<(u64, Vec<u8>)>,
    },
    Write(Box<Upload>),
}

struct Upload {
    root: String,
    path: FilePath,
    id: String,
    buf: Vec<u8>,
    /// Bytes accepted from the client so far.
    next: u64,
    /// Bytes sent to the node so far.
    sent: u64,
    hash: ring::digest::Context,
}

pub(crate) struct SftpSession {
    files: Arc<FileService>,
    actor: Actor,
    server: forge_core::ServerId,
    roots: Vec<RootView>,
    write: bool,
    delete: bool,
    bandwidth: Option<u64>,
    handles: HashMap<String, Open>,
    next_handle: u64,
    unrecorded: u64,
}

/// A root name safe for a path component and unique within the grant.
fn root_name(label: &str, id: &str, kind: RootKind, taken: &[String]) -> String {
    let base: String = if kind == RootKind::Node {
        "node".into()
    } else {
        label
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '-' })
            .collect::<String>()
            .trim_matches(['-', '.'])
            .chars()
            .take(40)
            .collect()
    };
    let base = if base.is_empty() { id.to_owned() } else { base };
    if taken.contains(&base) { format!("{base}-{}", &id[id.len().saturating_sub(6)..]) } else { base }
}

/// Lexically normalises a client path to `/a/b` (no `.`, `..` or `//`).
fn normalize(p: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for c in p.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            c => parts.push(c),
        }
    }
    format!("/{}", parts.join("/"))
}

fn dir_attrs() -> FileAttributes {
    FileAttributes { permissions: Some(0o040_755), size: Some(0), ..FileAttributes::empty() }
}

fn attrs(e: &FileEntry) -> FileAttributes {
    let kind = match e.kind {
        EntryKind::Dir => 0o040_000,
        EntryKind::Symlink => 0o120_000,
        EntryKind::File => 0o100_000,
        EntryKind::Other => 0o100_000,
    };
    let perm = e.mode.unwrap_or(if e.kind == EntryKind::Dir { 0o755 } else { 0o644 }) & 0o7777;
    let mtime = u32::try_from(e.modified_ms / 1000).ok();
    FileAttributes {
        size: Some(e.size),
        permissions: Some(kind | perm),
        mtime,
        atime: mtime,
        ..FileAttributes::empty()
    }
}

impl SftpSession {
    /// A session for a grant actor (`None` for anything else).
    pub(crate) async fn new(files: Arc<FileService>, actor: Actor) -> Option<Self> {
        let Actor::Grant { grant, .. } = &actor else { return None };
        let (server, write, delete) = (grant.server_id, grant.write, grant.delete);
        let bandwidth = grant.bandwidth_kbps.map(|k| u64::from(k) * 1024);
        let ids = grant.roots.clone();
        // Labels from the roots the grant covers and its creator still sees.
        let known = files.roots(&actor, server).await.map(|(_, r)| r).unwrap_or_default();
        let mut roots: Vec<RootView> = Vec::new();
        for id in ids {
            let Some(r) = known.iter().find(|r| r.id == id) else { continue };
            let taken: Vec<String> = roots.iter().map(|x| x.name.clone()).collect();
            roots.push(RootView { name: root_name(&r.label, &r.id, r.kind, &taken), id: r.id.clone(), kind: r.kind });
        }
        Some(Self {
            files,
            actor,
            server,
            roots,
            write,
            delete,
            bandwidth,
            handles: HashMap::new(),
            next_handle: 0,
            unrecorded: 0,
        })
    }

    /// `(root id, path inside it)` for a client path; `None` for `/`.
    fn locate(&self, client: &str) -> R<Option<(String, FilePath, RootKind)>> {
        let norm = normalize(client);
        let mut it = norm.splitn(3, '/').skip(1);
        let Some(first) = it.next().filter(|f| !f.is_empty()) else { return Ok(None) };
        let root =
            self.roots.iter().find(|r| r.name == first).ok_or_else(|| err(StatusCode::NoSuchFile, "no such folder"))?;
        let rest = it.next().unwrap_or("");
        let path = FilePath::parse(&format!("/{rest}")).map_err(|e| err(StatusCode::Failure, e))?;
        Ok(Some((root.id.clone(), path, root.kind)))
    }

    async fn run(&self, root: &str, op: FileOp) -> R<Value> {
        Ok(self.files.run(&self.actor, self.server, root, op, Audit::Skip).await?)
    }

    async fn audit(&self, action: &str, details: Value) {
        self.files.audit(&self.actor, self.server, action, details).await;
    }

    async fn stat(&self, client: &str) -> R<FileAttributes> {
        let Some((root, path, _)) = self.locate(client)? else { return Ok(dir_attrs()) };
        if path.is_root() {
            return Ok(dir_attrs());
        }
        let out = self.run(&root, FileOp::Stat { root: RootSpec::Node, path }).await?;
        let e: FileEntry =
            serde_json::from_value(out["entry"].clone()).map_err(|_| err(StatusCode::Failure, "bad node answer"))?;
        Ok(attrs(&e))
    }

    fn handle(&mut self, open: Open) -> String {
        self.next_handle += 1;
        let h = format!("h{}", self.next_handle);
        self.handles.insert(h.clone(), open);
        h
    }

    /// Keeps a session under its bandwidth cap.
    async fn pace(&self, started: Instant, total: u64) {
        if let Some(bps) = self.bandwidth {
            let due = Duration::from_secs_f64(total as f64 / bps as f64);
            let spent = started.elapsed();
            if due > spent {
                tokio::time::sleep(due - spent).await;
            }
        }
    }

    async fn record_usage(&mut self, bytes: u64, force: bool) {
        self.unrecorded += bytes;
        if self.unrecorded > 0 && (force || self.unrecorded >= USAGE_EVERY) {
            self.files.grant_usage(&self.actor, self.unrecorded).await;
            self.unrecorded = 0;
        }
    }

    /// Sends buffered upload bytes to the node in `MAX_CHUNK_BYTES` chunks,
    /// up to [`BATCH`] per round trip (written in order on the node).
    async fn flush(&mut self, up: &mut Upload, all: bool) -> R<()> {
        while up.buf.len() >= MAX_CHUNK_BYTES * BATCH || (all && !up.buf.is_empty()) {
            let take = up.buf.len().min(MAX_CHUNK_BYTES * BATCH);
            let bytes: Vec<u8> = up.buf.drain(..take).collect();
            let chunks: Vec<&[u8]> = bytes.chunks(MAX_CHUNK_BYTES).collect();
            self.files.upload_chunks(&self.actor, &up.id, up.sent, &chunks).await?;
            up.sent += take as u64;
        }
        Ok(())
    }

    /// Up to [`BATCH`] consecutive reads from `offset` in one round trip,
    /// joined; stops at the first short read (end of file).
    async fn read_ahead(&self, root: &str, path: &FilePath, version: &str, offset: u64) -> R<Vec<u8>> {
        let ops: Vec<FileOp> = (0..BATCH as u64)
            .map(|i| FileOp::Read {
                root: RootSpec::Node,
                path: path.clone(),
                offset: offset + i * u64::from(MAX_READ_BYTES),
                length: MAX_READ_BYTES,
                version: Some(version.to_owned()),
            })
            .collect();
        let mut out = Vec::new();
        for (i, r) in self.files.run_many(&self.actor, self.server, root, ops).await.into_iter().enumerate() {
            let v = match r {
                Ok(v) => v,
                // Past the end of the file: what came before is the answer.
                Err(_) if i > 0 => break,
                Err(e) => return Err(e.into()),
            };
            let data = base64::engine::general_purpose::STANDARD
                .decode(v["content_b64"].as_str().unwrap_or_default())
                .map_err(|_| err(StatusCode::Failure, "bad node answer"))?;
            let short = data.len() < MAX_READ_BYTES as usize;
            out.extend_from_slice(&data);
            if short {
                break;
            }
        }
        Ok(out)
    }
}

/// The client's `[offset, offset + want)` from read-ahead bytes fetched at
/// `at`, if they cover the start of it (a short tail is fine: the client
/// asks again for the rest).
fn slice_ahead(at: u64, buf: &[u8], offset: u64, want: usize) -> Option<Vec<u8>> {
    let start = usize::try_from(offset.checked_sub(at)?).ok()?;
    if start >= buf.len() {
        return None;
    }
    Some(buf[start..buf.len().min(start + want)].to_vec())
}

impl russh_sftp::server::Handler for SftpSession {
    type Error = SftpError;

    fn unimplemented(&self) -> Self::Error {
        err(StatusCode::OpUnsupported, "not supported")
    }

    async fn init(&mut self, _version: u32, _ext: HashMap<String, String>) -> Result<Version, Self::Error> {
        Ok(Version::new())
    }

    async fn realpath(&mut self, id: u32, path: String) -> Result<Name, Self::Error> {
        let p = normalize(&path);
        Ok(Name { id, files: vec![File::new(p, dir_attrs())] })
    }

    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        Ok(Attrs { id, attrs: SftpSession::stat(self, &path).await? })
    }

    async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        Ok(Attrs { id, attrs: SftpSession::stat(self, &path).await? })
    }

    async fn fstat(&mut self, id: u32, handle: String) -> Result<Attrs, Self::Error> {
        let attrs = match self.handles.get(&handle) {
            Some(Open::Read { size, .. }) => {
                FileAttributes { size: Some(*size), permissions: Some(0o100_644), ..FileAttributes::empty() }
            }
            Some(Open::Write(up)) => {
                FileAttributes { size: Some(up.next), permissions: Some(0o100_644), ..FileAttributes::empty() }
            }
            Some(Open::Dir { .. }) => dir_attrs(),
            None => return Err(err(StatusCode::Failure, "invalid handle")),
        };
        Ok(Attrs { id, attrs })
    }

    async fn setstat(&mut self, id: u32, _path: String, _attrs: FileAttributes) -> Result<Status, Self::Error> {
        // Times and permissions are not changed through SFTP; accept so
        // clients that set them after an upload do not fail the transfer.
        Ok(ok(id))
    }

    async fn fsetstat(&mut self, id: u32, _handle: String, _attrs: FileAttributes) -> Result<Status, Self::Error> {
        Ok(ok(id))
    }

    async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, Self::Error> {
        let norm = normalize(&path);
        if let Some((_, p, _)) = self.locate(&norm)?
            && !p.is_root()
        {
            let a = SftpSession::stat(self, &norm).await?;
            if !a.is_dir() {
                return Err(err(StatusCode::Failure, "not a directory"));
            }
        }
        let handle = self.handle(Open::Dir { listed: false, path: norm });
        Ok(Handle { id, handle })
    }

    async fn readdir(&mut self, id: u32, handle: String) -> Result<Name, Self::Error> {
        let path = match self.handles.get_mut(&handle) {
            Some(Open::Dir { listed, path }) => {
                if *listed {
                    return Err(err(StatusCode::Eof, ""));
                }
                *listed = true;
                path.clone()
            }
            _ => return Err(err(StatusCode::Failure, "invalid handle")),
        };
        let Some((root, p, _)) = self.locate(&path)? else {
            let files = self.roots.iter().map(|r| File::new(r.name.clone(), dir_attrs())).collect();
            return Ok(Name { id, files });
        };
        let mut files = Vec::new();
        let mut offset = 0u32;
        loop {
            let op = FileOp::List { root: RootSpec::Node, path: p.clone(), offset, limit: PAGE, hidden: true };
            let out = self.run(&root, op).await?;
            let entries: Vec<FileEntry> = serde_json::from_value(out["entries"].clone()).unwrap_or_default();
            let n = entries.len();
            files.extend(entries.iter().filter(|e| !e.protected).map(|e| File::new(e.name.clone(), attrs(e))));
            offset += u32::try_from(n).unwrap_or(PAGE);
            if n == 0 || u64::from(offset) >= out["total"].as_u64().unwrap_or(0) || files.len() >= MAX_ENTRIES {
                break;
            }
        }
        self.audit("file.list", json!({ "root": root, "path": p.to_string(), "entries": files.len(), "via": "sftp" }))
            .await;
        Ok(Name { id, files })
    }

    async fn open(
        &mut self,
        id: u32,
        filename: String,
        pflags: OpenFlags,
        _attrs: FileAttributes,
    ) -> Result<Handle, Self::Error> {
        let (root, path, _) = self.locate(&filename)?.ok_or_else(|| err(StatusCode::Failure, "not a file"))?;
        if path.is_root() {
            return Err(err(StatusCode::Failure, "not a file"));
        }
        let writing = pflags.intersects(OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::APPEND);
        if !writing {
            let out = self.run(&root, FileOp::Stat { root: RootSpec::Node, path: path.clone() }).await?;
            if out.pointer("/entry/kind").and_then(Value::as_str) != Some("file") {
                return Err(err(StatusCode::Failure, "not a regular file"));
            }
            let size = out.pointer("/entry/size").and_then(Value::as_u64).unwrap_or(0);
            let version = out["version"].as_str().unwrap_or_default().to_owned();
            let handle =
                self.handle(Open::Read { root, path, size, version, sent: 0, started: Instant::now(), ahead: None });
            return Ok(Handle { id, handle });
        }
        if !self.write {
            return Err(err(StatusCode::PermissionDenied, "this access is read-only"));
        }
        if pflags.contains(OpenFlags::APPEND) {
            return Err(err(StatusCode::OpUnsupported, "appending is not supported; upload the whole file"));
        }
        // Replacing an existing file needs the delete permission too.
        let overwrite =
            if pflags.contains(OpenFlags::EXCLUDE) || !self.delete { Overwrite::Never } else { Overwrite::Always };
        let up = self
            .files
            .begin_upload(&self.actor, self.server, &root, path.clone(), forge_files::MAX_UPLOAD_BYTES, overwrite)
            .await?;
        let handle = self.handle(Open::Write(Box::new(Upload {
            root,
            path,
            id: up.id,
            buf: Vec::new(),
            next: 0,
            sent: 0,
            hash: ring::digest::Context::new(&ring::digest::SHA256),
        })));
        Ok(Handle { id, handle })
    }

    async fn read(&mut self, id: u32, handle: String, offset: u64, len: u32) -> Result<Data, Self::Error> {
        let Some(Open::Read { root, path, size, version, ahead, .. }) = self.handles.get(&handle) else {
            return Err(err(StatusCode::Failure, "invalid handle"));
        };
        if offset >= *size {
            return Err(err(StatusCode::Eof, ""));
        }
        let want = usize::try_from(len.clamp(1, MAX_READ_BYTES)).unwrap_or(32 * 1024);
        let data = match ahead.as_ref().and_then(|(at, buf)| slice_ahead(*at, buf, offset, want)) {
            Some(d) => d,
            None => {
                let (root, path, version) = (root.clone(), path.clone(), version.clone());
                let fetched = self.read_ahead(&root, &path, &version, offset).await?;
                let served = fetched[..fetched.len().min(want)].to_vec();
                if let Some(Open::Read { ahead, .. }) = self.handles.get_mut(&handle) {
                    *ahead = Some((offset, fetched));
                }
                served
            }
        };
        let Some(Open::Read { started, sent, .. }) = self.handles.get(&handle) else {
            return Err(err(StatusCode::Failure, "invalid handle"));
        };
        let (started, total) = (*started, *sent);
        if data.is_empty() {
            return Err(err(StatusCode::Eof, ""));
        }
        let n = data.len() as u64;
        if let Some(Open::Read { sent, .. }) = self.handles.get_mut(&handle) {
            *sent += n;
        }
        self.pace(started, total + n).await;
        self.record_usage(n, false).await;
        Ok(Data { id, data })
    }

    async fn write(&mut self, id: u32, handle: String, offset: u64, data: Vec<u8>) -> Result<Status, Self::Error> {
        let Some(Open::Write(mut up)) = self.handles.remove(&handle) else {
            return Err(err(StatusCode::Failure, "invalid handle"));
        };
        if offset != up.next {
            self.handles.insert(handle, Open::Write(up));
            return Err(err(StatusCode::Failure, "only sequential writes are supported"));
        }
        up.hash.update(&data);
        up.next += data.len() as u64;
        up.buf.extend_from_slice(&data);
        let r = self.flush(&mut up, false).await;
        let n = data.len() as u64;
        self.handles.insert(handle, Open::Write(up));
        r?;
        self.record_usage(n, false).await;
        Ok(ok(id))
    }

    async fn close(&mut self, id: u32, handle: String) -> Result<Status, Self::Error> {
        match self.handles.remove(&handle) {
            Some(Open::Write(mut up)) => {
                let result = async {
                    self.flush(&mut up, true).await?;
                    let sha: String = up.hash.clone().finish().as_ref().iter().map(|b| format!("{b:02x}")).collect();
                    self.files.commit_upload_sized(&self.actor, &up.id, Some(up.next), &sha).await?;
                    Ok::<(), SftpError>(())
                }
                .await;
                if result.is_err() {
                    let _ = self.files.abort_upload(&self.actor, &up.id).await;
                }
                self.record_usage(0, true).await;
                result?;
                self.audit(
                    "file.sftp.upload",
                    json!({ "root": up.root, "path": up.path.to_string(), "size": up.next }),
                )
                .await;
            }
            Some(Open::Read { root, path, sent, .. }) => {
                self.record_usage(0, true).await;
                if sent > 0 {
                    self.audit("file.sftp.download", json!({ "root": root, "path": path.to_string(), "bytes": sent }))
                        .await;
                }
            }
            Some(Open::Dir { .. }) => {}
            None => return Err(err(StatusCode::Failure, "invalid handle")),
        }
        Ok(ok(id))
    }

    async fn remove(&mut self, id: u32, filename: String) -> Result<Status, Self::Error> {
        let (root, path, kind) =
            self.locate(&filename)?.ok_or_else(|| err(StatusCode::PermissionDenied, "cannot remove a root"))?;
        // Removed files go to the trash (containers have none).
        let op = FileOp::Delete { root: RootSpec::Node, path, permanent: kind == RootKind::Container };
        self.files.run(&self.actor, self.server, &root, op, Audit::Record).await?;
        Ok(ok(id))
    }

    async fn mkdir(&mut self, id: u32, path: String, _attrs: FileAttributes) -> Result<Status, Self::Error> {
        let (root, path, _) = self.locate(&path)?.ok_or_else(|| err(StatusCode::Failure, "already exists"))?;
        self.files
            .run(
                &self.actor,
                self.server,
                &root,
                FileOp::Mkdir { root: RootSpec::Node, path, parents: false },
                Audit::Record,
            )
            .await?;
        Ok(ok(id))
    }

    async fn rmdir(&mut self, id: u32, path: String) -> Result<Status, Self::Error> {
        let (root, path, kind) =
            self.locate(&path)?.ok_or_else(|| err(StatusCode::PermissionDenied, "cannot remove a root"))?;
        let list = FileOp::List { root: RootSpec::Node, path: path.clone(), offset: 0, limit: 1, hidden: true };
        if self.run(&root, list).await?["total"].as_u64().unwrap_or(1) > 0 {
            return Err(err(StatusCode::Failure, "the directory is not empty"));
        }
        let op = FileOp::Delete { root: RootSpec::Node, path, permanent: kind == RootKind::Container };
        self.files.run(&self.actor, self.server, &root, op, Audit::Record).await?;
        Ok(ok(id))
    }

    async fn rename(&mut self, id: u32, oldpath: String, newpath: String) -> Result<Status, Self::Error> {
        let (r1, from, _) =
            self.locate(&oldpath)?.ok_or_else(|| err(StatusCode::PermissionDenied, "cannot rename a root"))?;
        let (r2, to, _) =
            self.locate(&newpath)?.ok_or_else(|| err(StatusCode::PermissionDenied, "cannot rename a root"))?;
        if r1 != r2 {
            return Err(err(StatusCode::OpUnsupported, "move files within one folder tree; copy across them"));
        }
        let op = FileOp::Rename { root: RootSpec::Node, from, to, overwrite: false };
        self.files.run(&self.actor, self.server, &r1, op, Audit::Record).await?;
        Ok(ok(id))
    }
}

fn ok(id: u32) -> Status {
    Status { id, status_code: StatusCode::Ok, error_message: "Ok".into(), language_tag: "en-US".into() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_ahead_serves_what_it_covers() {
        let buf: Vec<u8> = (0..=255u8).cycle().take(1000).collect();
        assert_eq!(slice_ahead(100, &buf, 100, 10).as_deref(), Some(&buf[0..10]));
        assert_eq!(slice_ahead(100, &buf, 600, 32).as_deref(), Some(&buf[500..532]));
        // A short tail: the client asks again for the rest.
        assert_eq!(slice_ahead(100, &buf, 1090, 32).as_deref(), Some(&buf[990..1000]));
        assert_eq!(slice_ahead(100, &buf, 1100, 32), None, "past the buffer");
        assert_eq!(slice_ahead(100, &buf, 99, 32), None, "before the buffer");
    }

    #[test]
    fn client_paths_cannot_climb_out() {
        assert_eq!(normalize("/a/../../etc/passwd"), "/etc/passwd");
        assert_eq!(normalize("."), "/");
        assert_eq!(normalize("web//conf/./x"), "/web/conf/x");
        assert_eq!(normalize("/.."), "/");
    }

    #[test]
    fn root_names_are_safe_and_unique() {
        assert_eq!(root_name("Web App!", "fr_aaaa11", RootKind::Service, &[]), "Web-App");
        assert_eq!(root_name("x", "node", RootKind::Node, &[]), "node");
        assert_eq!(root_name("Web App", "fr_bbbb22", RootKind::Service, &["Web-App".into()]), "Web-App-bbbb22");
        assert_eq!(root_name("../..", "fr_cccc33", RootKind::Service, &[]), "fr_cccc33");
    }
}
