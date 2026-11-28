//! Windows backend. Paths are rebuilt from validated components under a
//! `\\?\` base; every component on the way is checked not to be a reparse
//! point (symbolic link, junction, mount point), and files are opened with
//! `FILE_FLAG_OPEN_REPARSE_POINT` and checked again on the handle.
//! Alternate data streams, reserved device names, names ending in a dot
//! or space and 8.3 short names (`PROGRA~3`, an alias that would bypass the
//! protected-path names) are rejected.
//!
//! Hard links to protected files are not detected on Windows: the file
//! index needed for that is not available through safe std APIs.

use std::fs::{Metadata, OpenOptions};
use std::os::windows::fs::{FileExt, MetadataExt, OpenOptionsExt};
use std::path::PathBuf;

use forge_core::files::RootSpec;

use super::{FileIo, Fs, FsError, Kind, MAX_DIR_ENTRIES, Meta};

const ATTR_READONLY: u32 = 0x1;
const ATTR_HIDDEN: u32 = 0x2;
const ATTR_SYSTEM: u32 = 0x4;
const ATTR_REPARSE: u32 = 0x400;
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
const ERROR_NOT_SAME_DEVICE: i32 = 17;

/// Forge's private store for trash and revisions (protected path), under
/// the agent's data folder (D-079).
fn store_dir() -> PathBuf {
    let default = crate::config::windows_data_dir().join("files");
    super::store_dir_or(&default.to_string_lossy())
}

#[derive(Debug)]
pub(crate) struct Root {
    /// `None`: the whole node, whose first component is a drive.
    base: Option<PathBuf>,
    store: PathBuf,
}

/// Characters, names and endings Windows treats specially.
pub(crate) fn valid_name(c: &str) -> Result<(), String> {
    if c.chars().any(|ch| (ch as u32) < 0x20 || matches!(ch, '<' | '>' | ':' | '"' | '|' | '?' | '*' | '\\')) {
        return Err(format!(
            "`{c}` contains a character Windows does not allow (alternate data streams are not supported)"
        ));
    }

    if c.ends_with('.') || c.ends_with(' ') {
        return Err(format!("`{c}` ends with a dot or space"));
    }

    // `PROGRA~3` reaches `ProgramData` under another spelling, past the
    // protected-path policy; only long names are resolved.
    if forge_core::files::is_short_name(c) {
        return Err(format!("protected path (`{c}` looks like an 8.3 short name; use the long name)"));
    }

    let stem = c.split('.').next().unwrap_or_default().trim_end().to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$")
        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && stem.as_bytes()[3].is_ascii_digit());

    if reserved {
        return Err(format!("`{c}` is a reserved device name"));
    }

    Ok(())
}

fn drive(c: &str) -> Result<PathBuf, String> {
    let d = c.as_bytes();
    if d.len() == 2 && d[0].is_ascii_alphabetic() && d[1] == b':' {
        Ok(PathBuf::from(format!(r"\\?\{}\", c.to_ascii_uppercase())))
    } else {
        Err(format!("`{c}` is not a drive; paths start with a drive such as /C:/"))
    }
}

fn io_err(what: &str, e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::NotFound => format!("`{what}` was not found"),
        std::io::ErrorKind::PermissionDenied => format!("permission denied on `{what}`"),
        std::io::ErrorKind::StorageFull => "the disk is full".to_owned(),
        std::io::ErrorKind::ReadOnlyFilesystem => "the filesystem is read-only".to_owned(),
        std::io::ErrorKind::DirectoryNotEmpty => format!("`{what}` is not empty"),
        _ => format!("`{what}`: {e}"),
    }
}

fn is_reparse(md: &Metadata) -> bool {
    md.file_attributes() & ATTR_REPARSE != 0
}

fn meta(md: &Metadata, name: &str) -> Meta {
    let attrs = md.file_attributes();

    let kind = if is_reparse(md) {
        if md.file_type().is_symlink() { Kind::Symlink } else { Kind::Other }
    } else if md.is_dir() {
        Kind::Dir
    } else if md.is_file() {
        Kind::File
    } else {
        Kind::Other
    };

    let modified_ms = md
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));

    Meta {
        kind,
        size: if md.is_file() { md.len() } else { 0 },
        modified_ms,
        readonly: attrs & ATTR_READONLY != 0,
        hidden: attrs & (ATTR_HIDDEN | ATTR_SYSTEM) != 0 || name.starts_with('.'),
        version: format!("{:x}-{:x}-{:x}-{:x}", md.len(), md.last_write_time(), md.creation_time(), attrs),
        nlink: 1,
        ..Meta::default()
    }
}

fn dir_meta() -> Meta {
    Meta { kind: Kind::Dir, ..Meta::default() }
}

impl Root {
    pub(crate) fn open(spec: &RootSpec, _pid: Option<u32>) -> Result<Self, String> {
        let base = match spec {
            RootSpec::Node => None,
            RootSpec::App { .. } => return Err("app roots need a Linux node".into()),
            RootSpec::Dir { base } => {
                let (d, rest) = base.components().split_first().ok_or("an approved directory cannot be `/`")?;
                let root = Self { base: Some(drive(d)?), store: store_dir() };
                let p = root.resolve(rest, false)?;
                if !std::fs::symlink_metadata(&p).map_err(|e| io_err(d, &e))?.is_dir() {
                    return Err("the approved directory is missing".into());
                }

                Some(p)
            }
            RootSpec::Container { .. } => return Err("container files are not supported on Windows nodes".into()),
        };

        Ok(Self { base, store: store_dir() })
    }

    #[cfg(test)]
    pub(crate) fn at(dir: &std::path::Path, store: &std::path::Path) -> Result<Self, String> {
        Ok(Self { base: Some(dir.to_path_buf()), store: store.to_path_buf() })
    }

    /// Native path for `path`, refusing reparse points on the way; the
    /// final component may be one only with `last_may_be_link`.
    fn resolve(&self, path: &[String], last_may_be_link: bool) -> Result<PathBuf, String> {
        let (mut p, rest) = match &self.base {
            Some(b) => (b.clone(), path),
            None => {
                let (d, rest) = path.split_first().ok_or("the node root lists drives only")?;
                (drive(d)?, rest)
            }
        };

        for (i, c) in rest.iter().enumerate() {
            valid_name(c)?;
            p.push(c);
            let last = i + 1 == rest.len();

            match std::fs::symlink_metadata(&p) {
                Ok(md) if is_reparse(&md) && !(last && last_may_be_link) => {
                    return Err(format!("`{c}` is a link, junction or mount point; Forge does not follow them"));
                }
                Ok(md) if !last && !md.is_dir() => return Err(format!("`{c}` is not a directory")),
                Ok(_) => {}
                // A missing final component is fine (create, rename target).
                Err(e) if last && e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(io_err(c, &e)),
            }
        }

        Ok(p)
    }
}

pub(crate) struct File {
    f: std::fs::File,
}

impl FileIo for File {
    fn read_at(&self, buf: &mut [u8], offset: u64) -> Result<usize, String> {
        loop {
            match self.f.seek_read(buf, offset) {
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                r => return r.map_err(|e| format!("read failed: {e}")),
            }
        }
    }

    fn write_all_at(&self, buf: &[u8], offset: u64) -> Result<(), String> {
        let mut done = 0usize;

        while done < buf.len() {
            match self.f.seek_write(&buf[done..], offset + done as u64) {
                Ok(0) => return Err("write failed".into()),
                Ok(n) => done += n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(io_err("file", &e)),
            }
        }

        Ok(())
    }

    fn meta(&self) -> Result<Meta, String> {
        let md = self.f.metadata().map_err(|e| e.to_string())?;
        if is_reparse(&md) {
            return Err("the file is a link; Forge does not follow links".into());
        }

        Ok(meta(&md, ""))
    }

    fn sync(&self) -> Result<(), String> {
        self.f.sync_all().map_err(|e| io_err("file", &e))
    }

    fn chown_like(&self, _like: &Meta) -> Result<(), String> {
        Ok(())
    }
}

impl Fs for Root {
    type File = File;

    fn stat(&self, path: &[String]) -> Result<Meta, String> {
        if path.is_empty() && self.base.is_none() {
            return Ok(dir_meta());
        }

        // A drive on the node root: `\?\C:\` has no metadata of its own.
        if let ([d], None) = (path, &self.base) {
            drive(d)?;

            return match std::fs::metadata(format!(r"{d}\")) {
                Ok(m) if m.is_dir() => Ok(dir_meta()),
                _ => Err(format!("drive `{d}` was not found")),
            };
        }

        let p = self.resolve(path, true)?;
        let md = std::fs::symlink_metadata(&p).map_err(|e| io_err(path.last().map_or("/", String::as_str), &e))?;
        let mut m = meta(&md, path.last().map_or("", String::as_str));
        if m.kind == Kind::Symlink {
            m.link_target = std::fs::read_link(&p).ok().map(|t| t.display().to_string());
        }

        Ok(m)
    }

    fn read_dir(&self, path: &[String]) -> Result<(Vec<String>, usize, bool), String> {
        if path.is_empty() && self.base.is_none() {
            let drives = (b'A'..=b'Z')
                .map(|l| format!("{}:", l as char))
                .filter(|d| std::fs::metadata(format!(r"{d}\")).is_ok_and(|m| m.is_dir()))
                .collect();
            return Ok((drives, 0, false));
        }

        let p = self.resolve(path, false)?;
        let (mut names, mut skipped, mut truncated) = (Vec::new(), 0usize, false);

        for item in std::fs::read_dir(&p).map_err(|e| io_err(path.last().map_or("/", String::as_str), &e))? {
            let Ok(item) = item else { continue };

            if names.len() >= MAX_DIR_ENTRIES {
                truncated = true;
                break;
            }

            match item.file_name().into_string() {
                Ok(n) => names.push(n),
                Err(_) => skipped += 1,
            }
        }

        Ok((names, skipped, truncated))
    }

    fn open_read(&self, path: &[String]) -> Result<File, String> {
        let p = self.resolve(path, false)?;
        let f = OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&p)
            .map_err(|e| io_err(path.last().map_or("/", String::as_str), &e))?;
        let f = File { f };

        // Checked on the open handle: a link swapped in after the walk fails here.
        f.meta()?;

        Ok(f)
    }

    fn open_write(&self, path: &[String], create: bool, truncate: bool) -> Result<File, String> {
        let p = self.resolve(path, false)?;
        let f = OpenOptions::new()
            .read(true)
            .write(true)
            .create(create)
            .truncate(truncate)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&p)
            .map_err(|e| io_err(path.last().map_or("/", String::as_str), &e))?;
        let f = File { f };
        if f.meta()?.kind != Kind::File {
            return Err("not a regular file".into());
        }

        Ok(f)
    }

    fn mkdir(&self, path: &[String]) -> Result<(), FsError> {
        let p = self.resolve(path, false)?;
        std::fs::create_dir(&p).map_err(|e| match e.kind() {
            std::io::ErrorKind::AlreadyExists => FsError::Exists,
            _ => FsError::Other(io_err(path.last().map_or("/", String::as_str), &e)),
        })
    }

    fn rename(&self, from: &[String], to_root: &Self, to: &[String], replace: bool) -> Result<(), FsError> {
        let src = self.resolve(from, true)?;
        let dst = to_root.resolve(to, false)?;
        if !replace && std::fs::symlink_metadata(&dst).is_ok() {
            return Err(FsError::Exists);
        }

        // Replacing a directory is not atomic on Windows: refuse it.
        if replace && std::fs::symlink_metadata(&dst).is_ok_and(|m| m.is_dir()) {
            return Err(FsError::Exists);
        }

        std::fs::rename(&src, &dst).map_err(|e| {
            if e.raw_os_error() == Some(ERROR_NOT_SAME_DEVICE) {
                FsError::CrossDevice
            } else {
                FsError::Other(io_err(from.last().map_or("/", String::as_str), &e))
            }
        })
    }

    /// Windows has no atomic directory swap: the caller renames twice.
    fn exchange(&self, _a: &[String], _b: &[String]) -> Result<bool, String> {
        Ok(false)
    }

    fn remove_file(&self, path: &[String]) -> Result<(), String> {
        let p = self.resolve(path, true)?;
        let md = std::fs::symlink_metadata(&p).map_err(|e| io_err("file", &e))?;

        // Directory links and junctions are removed as directories.
        let r = if md.is_dir() { std::fs::remove_dir(&p) } else { std::fs::remove_file(&p) };

        r.map_err(|e| io_err(path.last().map_or("/", String::as_str), &e))
    }

    fn remove_dir(&self, path: &[String]) -> Result<(), String> {
        let p = self.resolve(path, false)?;
        std::fs::remove_dir(&p).map_err(|e| io_err(path.last().map_or("/", String::as_str), &e))
    }

    fn symlink(&self, _target: &str, path: &[String]) -> Result<(), String> {
        Err(format!(
            "`{}` is a link; copying links is not supported on Windows",
            path.last().map_or("/", String::as_str)
        ))
    }

    fn inherit_owner(&self, _path: &[String]) -> Result<(), String> {
        Ok(())
    }

    fn free_bytes(&self, _path: &[String]) -> Option<u64> {
        None
    }

    fn set_mode(&self, _path: &[String], _mode: u32) -> Result<(), String> {
        Err("changing permissions is not supported on Windows yet".into())
    }

    fn set_owner(&self, _path: &[String], _uid: Option<u32>, _gid: Option<u32>) -> Result<(), String> {
        Err("changing owners is not supported on Windows yet".into())
    }

    /// The ACL as `icacls` prints it (read-only; no shell involved).
    fn acl(&self, path: &[String]) -> Result<String, String> {
        let p = self.resolve(path, false)?;
        let out = std::process::Command::new(r"C:\Windows\System32\icacls.exe")
            .arg(&p)
            .output()
            .map_err(|e| format!("cannot run icacls: {e}"))?;
        let text = String::from_utf8_lossy(&out.stdout).chars().take(16 * 1024).collect::<String>();
        if out.status.success() { Ok(text) } else { Err("icacls could not read the ACL".into()) }
    }

    fn is_protected_inode(&self, _dev: u64, _ino: u64) -> bool {
        false
    }

    fn store(&self, sub: &str) -> Result<Self, String> {
        let dir = self.store.join(sub);
        std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create Forge's file store: {e}"))?;

        Ok(Self { base: Some(dir), store: self.store.clone() })
    }
}
