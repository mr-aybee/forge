//! Linux backend: every call starts at the root's directory handle and
//! opens one component at a time with `O_NOFOLLOW`, then acts relative to
//! the parent's handle (`*at` calls). A link is never followed, so a link
//! swapped in between calls fails instead of escaping the root.

use std::collections::HashSet;
use std::os::fd::OwnedFd;
use std::path::PathBuf;

use forge_core::files::RootSpec;
use rustix::fs::{self, AtFlags, FileType, Mode, OFlags, RenameFlags, Stat};
use rustix::io::Errno;

use super::{FileIo, Fs, FsError, Kind, MAX_DIR_ENTRIES, Meta};

/// Forge's private store for trash and revisions (protected path).
const STORE: &str = "/var/lib/forge-agent/files";

fn store_dir() -> PathBuf {
    super::store_dir_or(STORE)
}

pub(crate) struct Root {
    base: OwnedFd,
    store: PathBuf,
    /// The base is the real node filesystem (hard-link protection applies).
    system: bool,
}

impl std::fmt::Debug for Root {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Root").field("store", &self.store).field("system", &self.system).finish_non_exhaustive()
    }
}

fn errno(what: &str, e: Errno) -> String {
    match e {
        Errno::NOENT => format!("`{what}` was not found"),
        Errno::LOOP | Errno::NOTDIR => format!("`{what}` is a link or not a directory; Forge does not follow links"),
        Errno::ACCESS | Errno::PERM => format!("permission denied on `{what}`"),
        Errno::NOSPC | Errno::DQUOT => "the disk is full".to_owned(),
        Errno::ROFS => "the filesystem is read-only".to_owned(),
        Errno::NOTEMPTY => format!("`{what}` is not empty"),
        Errno::ISDIR => format!("`{what}` is a directory"),
        Errno::TXTBSY => format!("`{what}` is busy"),
        other => format!("`{what}`: {other}"),
    }
}

fn last(path: &[String]) -> &str {
    path.last().map_or("/", String::as_str)
}

fn dir_flags() -> OFlags {
    OFlags::PATH | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC
}

impl Root {
    /// Opens a root. `pid` is the container's init process for container roots.
    pub(crate) fn open(spec: &RootSpec, pid: Option<u32>) -> Result<Self, String> {
        let slash = fs::open("/", OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC, Mode::empty())
            .map_err(|e| errno("/", e))?;

        let (start, base, system) = match spec {
            RootSpec::Node => (slash, Vec::new(), true),
            RootSpec::Dir { base } => (slash, base.components().to_vec(), true),
            // Opened by the app's own worker process as a plain directory (D-083).
            RootSpec::App { .. } => return Err("an app root runs as the app's account, not here".into()),
            RootSpec::Container { base, .. } => {
                let pid = pid.ok_or("container root without a process")?;

                // The container's root directory as its init process sees it
                // (works for shell-less images; no overlay internals).
                let fd = fs::open(
                    format!("/proc/{pid}/root").as_str(),
                    OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .map_err(|e| errno("container root", e))?;
                (fd, base.components().to_vec(), false)
            }
        };

        let base = walk_from(&start, &base)?;

        Ok(Self { base, store: store_dir(), system })
    }

    /// A root at an existing directory (tests).
    #[cfg(test)]
    pub(crate) fn at(dir: &std::path::Path, store: &std::path::Path) -> Result<Self, String> {
        let base = fs::open(dir, dir_flags(), Mode::empty()).map_err(|e| errno("base", e))?;
        Ok(Self { base, store: store.to_path_buf(), system: false })
    }

    fn walk(&self, comps: &[String]) -> Result<OwnedFd, String> {
        walk_from(&self.base, comps)
    }

    /// Parent directory handle and final name.
    fn split<'p>(&self, path: &'p [String]) -> Result<(OwnedFd, &'p str), String> {
        let (name, parents) = path.split_last().ok_or("cannot do that to the root itself")?;
        Ok((self.walk(parents)?, name.as_str()))
    }
}

fn walk_from(start: &OwnedFd, comps: &[String]) -> Result<OwnedFd, String> {
    let mut fd = fs::openat(start, ".", dir_flags(), Mode::empty()).map_err(|e| errno("/", e))?;

    for c in comps {
        fd = fs::openat(&fd, c.as_str(), dir_flags(), Mode::empty()).map_err(|e| errno(c, e))?;
    }

    Ok(fd)
}

#[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss, clippy::unnecessary_cast)]
fn meta(st: &Stat) -> Meta {
    let kind = match FileType::from_raw_mode(st.st_mode as _) {
        FileType::RegularFile => Kind::File,
        FileType::Directory => Kind::Dir,
        FileType::Symlink => Kind::Symlink,
        _ => Kind::Other,
    };

    Meta {
        kind,
        size: st.st_size as u64,
        modified_ms: (st.st_mtime as i64).saturating_mul(1000) + (st.st_mtime_nsec as i64) / 1_000_000,
        mode: Some(st.st_mode as u32),
        uid: Some(st.st_uid as u32),
        gid: Some(st.st_gid as u32),
        nlink: st.st_nlink as u64,
        dev: st.st_dev as u64,
        ino: st.st_ino as u64,
        readonly: false,
        hidden: false,
        link_target: None,
        version: format!(
            "{:x}-{:x}-{:x}-{:x}.{:x}-{:x}.{:x}",
            st.st_dev as u64,
            st.st_ino as u64,
            st.st_size as u64,
            st.st_mtime as i64,
            st.st_mtime_nsec as u64,
            st.st_ctime as i64,
            st.st_ctime_nsec as u64
        ),
    }
}

pub(crate) struct File {
    fd: OwnedFd,
}

impl FileIo for File {
    fn read_at(&self, buf: &mut [u8], offset: u64) -> Result<usize, String> {
        loop {
            match rustix::io::pread(&self.fd, &mut *buf, offset) {
                Err(Errno::INTR) => {}
                r => return r.map_err(|e| format!("read failed: {e}")),
            }
        }
    }

    fn write_all_at(&self, buf: &[u8], offset: u64) -> Result<(), String> {
        let mut done = 0usize;

        while done < buf.len() {
            match rustix::io::pwrite(&self.fd, &buf[done..], offset + done as u64) {
                Ok(0) => return Err("write failed".into()),
                Ok(n) => done += n,
                Err(Errno::INTR) => {}
                Err(e) => return Err(errno("file", e)),
            }
        }

        Ok(())
    }

    fn meta(&self) -> Result<Meta, String> {
        fs::fstat(&self.fd).map(|s| meta(&s)).map_err(|e| e.to_string())
    }

    fn sync(&self) -> Result<(), String> {
        fs::fsync(&self.fd).map_err(|e| errno("file", e))
    }

    fn chown_like(&self, like: &Meta) -> Result<(), String> {
        if let (Some(uid), Some(gid)) = (like.uid, like.gid) {
            // Only root may give files away; unprivileged test runs skip it.
            let _ = fs::fchown(&self.fd, Some(rustix::fs::Uid::from_raw(uid)), Some(rustix::fs::Gid::from_raw(gid)));
        }

        if let Some(mode) = like.mode {
            fs::fchmod(&self.fd, Mode::from_raw_mode(mode & 0o7777)).map_err(|e| errno("file", e))?;
        }

        Ok(())
    }
}

impl Fs for Root {
    type File = File;

    fn stat(&self, path: &[String]) -> Result<Meta, String> {
        if path.is_empty() {
            return fs::fstat(&self.base).map(|s| meta(&s)).map_err(|e| errno("/", e));
        }

        let (parent, name) = self.split(path)?;
        let st = fs::statat(&parent, name, AtFlags::SYMLINK_NOFOLLOW).map_err(|e| errno(name, e))?;
        let mut m = meta(&st);
        m.hidden = name.starts_with('.');

        if m.kind == Kind::Symlink {
            m.link_target = fs::readlinkat(&parent, name, Vec::new())
                .ok()
                .map(|t| String::from_utf8_lossy(t.as_bytes()).chars().take(4096).collect());
        }

        Ok(m)
    }

    fn read_dir(&self, path: &[String]) -> Result<(Vec<String>, usize, bool), String> {
        let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;

        let dir = match path.split_last() {
            None => fs::openat(&self.base, ".", flags, Mode::empty()).map_err(|e| errno("/", e))?,
            Some((name, parents)) => {
                fs::openat(self.walk(parents)?, name.as_str(), flags, Mode::empty()).map_err(|e| errno(name, e))?
            }
        };

        let (mut names, mut skipped, mut truncated) = (Vec::new(), 0usize, false);

        for item in fs::Dir::read_from(&dir).map_err(|e| errno(last(path), e))? {
            let item = item.map_err(|e| e.to_string())?;
            let bytes = item.file_name().to_bytes();
            if bytes == b"." || bytes == b".." {
                continue;
            }

            if names.len() >= MAX_DIR_ENTRIES {
                truncated = true;
                break;
            }

            match std::str::from_utf8(bytes) {
                Ok(n) => names.push(n.to_owned()),
                // Not addressable through the UTF-8 API; reported as a count.
                Err(_) => skipped += 1,
            }
        }

        Ok((names, skipped, truncated))
    }

    fn open_read(&self, path: &[String]) -> Result<File, String> {
        let (parent, name) = self.split(path)?;
        let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::NOCTTY | OFlags::CLOEXEC;
        let fd = fs::openat(&parent, name, flags, Mode::empty()).map_err(|e| match e {
            Errno::LOOP => format!("`{name}` is a link; Forge does not follow links"),
            e => errno(name, e),
        })?;

        Ok(File { fd })
    }

    fn open_write(&self, path: &[String], create: bool, truncate: bool) -> Result<File, String> {
        let (parent, name) = self.split(path)?;

        // Read access too: commits hash the partial file through this handle.
        let mut flags = OFlags::RDWR | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::NOCTTY | OFlags::CLOEXEC;

        if create {
            flags |= OFlags::CREATE;
        }

        if truncate {
            flags |= OFlags::TRUNC;
        }

        let fd = fs::openat(&parent, name, flags, Mode::from_raw_mode(0o600)).map_err(|e| match e {
            Errno::LOOP => format!("`{name}` is a link; Forge does not follow links"),
            e => errno(name, e),
        })?;
        let f = File { fd };
        if f.meta()?.kind != Kind::File {
            return Err(format!("`{name}` is not a regular file"));
        }

        Ok(f)
    }

    fn mkdir(&self, path: &[String]) -> Result<(), FsError> {
        let (parent, name) = self.split(path)?;
        fs::mkdirat(&parent, name, Mode::from_raw_mode(0o755)).map_err(|e| match e {
            Errno::EXIST => FsError::Exists,
            e => FsError::Other(errno(name, e)),
        })
    }

    fn rename(&self, from: &[String], to_root: &Self, to: &[String], replace: bool) -> Result<(), FsError> {
        let (fp, fname) = self.split(from)?;
        let (tp, tname) = to_root.split(to)?;
        let flags = if replace { RenameFlags::empty() } else { RenameFlags::NOREPLACE };

        let map = |e: Errno| match e {
            Errno::XDEV => FsError::CrossDevice,
            Errno::EXIST | Errno::NOTEMPTY => FsError::Exists,
            e => FsError::Other(errno(fname, e)),
        };

        match fs::renameat_with(&fp, fname, &tp, tname, flags) {
            // Filesystems without RENAME_NOREPLACE: check, then rename.
            Err(Errno::INVAL) if !replace => {
                if fs::statat(&tp, tname, AtFlags::SYMLINK_NOFOLLOW).is_ok() {
                    return Err(FsError::Exists);
                }

                fs::renameat(&fp, fname, &tp, tname).map_err(map)
            }
            r => r.map_err(map),
        }
    }

    fn exchange(&self, a: &[String], b: &[String]) -> Result<bool, String> {
        let (ap, aname) = self.split(a)?;
        let (bp, bname) = self.split(b)?;
        match fs::renameat_with(&ap, aname, &bp, bname, RenameFlags::EXCHANGE) {
            Ok(()) => Ok(true),
            // Filesystems or kernels without RENAME_EXCHANGE.
            Err(Errno::INVAL | Errno::NOSYS | Errno::OPNOTSUPP) => Ok(false),
            Err(e) => Err(errno(aname, e)),
        }
    }

    fn remove_file(&self, path: &[String]) -> Result<(), String> {
        let (parent, name) = self.split(path)?;
        fs::unlinkat(&parent, name, AtFlags::empty()).map_err(|e| errno(name, e))
    }

    fn remove_dir(&self, path: &[String]) -> Result<(), String> {
        let (parent, name) = self.split(path)?;
        fs::unlinkat(&parent, name, AtFlags::REMOVEDIR).map_err(|e| errno(name, e))
    }

    fn symlink(&self, target: &str, path: &[String]) -> Result<(), String> {
        let (parent, name) = self.split(path)?;
        fs::symlinkat(target, &parent, name).map_err(|e| errno(name, e))
    }

    fn inherit_owner(&self, path: &[String]) -> Result<(), String> {
        let (parent, name) = self.split(path)?;
        let p = fs::fstat(&parent).map_err(|e| e.to_string())?;

        // Only root may give files away; unprivileged test runs skip it.
        let _ = fs::chownat(
            &parent,
            name,
            Some(rustix::fs::Uid::from_raw(p.st_uid)),
            Some(rustix::fs::Gid::from_raw(p.st_gid)),
            AtFlags::SYMLINK_NOFOLLOW,
        );

        Ok(())
    }

    #[allow(clippy::unnecessary_cast)]
    fn free_bytes(&self, path: &[String]) -> Option<u64> {
        let parent = self.walk(path.split_last()?.1).ok()?;
        let s = fs::fstatvfs(&parent).ok()?;

        Some((s.f_bavail as u64).saturating_mul(s.f_frsize as u64))
    }

    fn set_mode(&self, path: &[String], mode: u32) -> Result<(), String> {
        let (parent, name) = self.split(path)?;
        let st = fs::statat(&parent, name, AtFlags::SYMLINK_NOFOLLOW).map_err(|e| errno(name, e))?;
        let mut flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::NOCTTY | OFlags::CLOEXEC;

        match FileType::from_raw_mode(st.st_mode as _) {
            FileType::RegularFile => {}
            FileType::Directory => flags |= OFlags::DIRECTORY,
            _ => return Err(format!("`{name}` is not a regular file or folder")),
        }

        let fd = fs::openat(&parent, name, flags, Mode::empty()).map_err(|e| errno(name, e))?;

        fs::fchmod(&fd, Mode::from_raw_mode(mode & 0o7777)).map_err(|e| errno(name, e))
    }

    fn set_owner(&self, path: &[String], uid: Option<u32>, gid: Option<u32>) -> Result<(), String> {
        let (parent, name) = self.split(path)?;
        fs::chownat(
            &parent,
            name,
            uid.map(rustix::fs::Uid::from_raw),
            gid.map(rustix::fs::Gid::from_raw),
            AtFlags::SYMLINK_NOFOLLOW,
        )
        .map_err(|e| errno(name, e))
    }

    fn acl(&self, path: &[String]) -> Result<String, String> {
        let m = self.stat(path)?;
        Ok(format!("POSIX permissions {:04o} (extended ACLs are not shown)", m.mode.unwrap_or(0) & 0o7777))
    }

    fn is_protected_inode(&self, dev: u64, ino: u64) -> bool {
        self.system && protected_inodes(PROTECTED_FILES, true).contains(&(dev, ino))
    }

    fn store(&self, sub: &str) -> Result<Self, String> {
        let dir = self.store.join(sub);
        std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create Forge's file store: {e}"))?;
        let _ = std::fs::set_permissions(&self.store, std::os::unix::fs::PermissionsExt::from_mode(0o700));
        let base = fs::open(&dir, dir_flags(), Mode::empty()).map_err(|e| errno(sub, e))?;

        Ok(Self { base, store: self.store.clone(), system: false })
    }
}

/// Protected locations whose files a hard link must not expose.
const PROTECTED_FILES: &[&str] =
    &["/etc/forge", "/var/lib/forge", "/var/lib/forge-agent", "/root/.ssh", "/etc/ssh", "/etc/shadow", "/etc/gshadow"];

/// `(dev, inode)` of the files below `roots` (and `/home/*/.ssh` when
/// `homes`). Computed when a file with several links is opened, so files
/// created or replaced since (a new `/etc/shadow`, new keys) are covered.
/// Bounded walk that never follows links.
pub(crate) fn protected_inodes(roots: &[&str], homes: bool) -> HashSet<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let mut out = HashSet::new();
    let mut stack: Vec<(PathBuf, u8)> = roots.iter().map(|p| (PathBuf::from(p), 0)).collect();

    if homes && let Ok(h) = std::fs::read_dir("/home") {
        stack.extend(h.flatten().map(|h| (h.path().join(".ssh"), 0)));
    }

    let mut budget = 20_000usize;

    while let Some((p, depth)) = stack.pop() {
        let Ok(md) = std::fs::symlink_metadata(&p) else { continue };
        budget = budget.saturating_sub(1);

        if budget == 0 {
            break;
        }

        if md.is_file() {
            out.insert((md.dev(), md.ino()));
        } else if md.is_dir()
            && depth < 8
            && let Ok(rd) = std::fs::read_dir(&p)
        {
            stack.extend(rd.flatten().map(|e| (e.path(), depth + 1)));
        }
    }

    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn protected_inodes_see_files_created_later() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let root = dir.path().to_str().unwrap_or_default().to_owned();
        std::fs::write(dir.path().join("a"), b"1").unwrap_or_else(|e| panic!("{e}"));
        let before = super::protected_inodes(&[&root], false);
        assert_eq!(before.len(), 1);

        // A key written (or a file replaced) after the first check is covered.
        std::fs::write(dir.path().join("new-key"), b"2").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(super::protected_inodes(&[&root], false).len(), 2);
    }
}
