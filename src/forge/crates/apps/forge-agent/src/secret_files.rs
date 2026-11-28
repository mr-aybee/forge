//! Secret and config files of native services (D-179), written by the
//! helper (root).
//!
//! Each service copy has one folder, `/var/lib/forge-agent/secrets/<env>/<name>[--<slot>]`,
//! under root-owned parents that others can traverse but not list. The
//! folder is owned by root and never writable by anyone else, so no other
//! account can plant a link in it. Each file is written to a fresh
//! temporary name (`O_EXCL`, no link followed), given its owner and mode,
//! then renamed into place, so a reader never sees half a value. Files the
//! spec no longer lists are removed. Values are never logged.

use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

use forge_catalog::secret_files::{HOST_ROOT, MountedFile};

/// Who reads the files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reader {
    /// A container: Docker (root) bind-mounts each file; inside the
    /// container each file has the owner and mode its reference sets.
    Container,
    /// A native service running as `uid`:`gid`: the folder is `root:gid`
    /// `0750` and each file is owned by the account unless its reference
    /// names another owner.
    Native { uid: u32, gid: u32 },
}

/// Ownership changes are skipped when the helper does not run as root
/// (unit tests).
fn chown(path: &Path, uid: u32, gid: u32) -> Result<(), String> {
    if !rustix::process::geteuid().is_root() {
        return Ok(());
    }

    std::os::unix::fs::lchown(path, Some(uid), Some(gid)).map_err(|e| format!("{}: {e}", path.display()))
}

fn set_mode(path: &Path, mode: u32) -> Result<(), String> {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// Creates `dir` (a real folder, not a link) with `mode`.
fn ensure_dir(dir: &Path, mode: u32) -> Result<(), String> {
    match std::fs::symlink_metadata(dir) {
        Ok(m) if m.is_dir() => {}
        Ok(_) => return Err(format!("{} is not a folder; remove it and deploy again", dir.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        Err(e) => return Err(format!("{}: {e}", dir.display())),
    }

    set_mode(dir, mode)
}

/// The folder of one service copy under `root` (the real root is
/// [`HOST_ROOT`]; tests use a temporary one).
#[must_use]
pub fn dir_under(root: &Path, environment: &str, name_and_slot: &str) -> std::path::PathBuf {
    root.join(environment).join(name_and_slot)
}

/// Writes `files` into `dir` for `reader` and removes every other file in
/// it. `dir`'s parents are created traversable (`0711`), `dir` itself
/// private.
pub fn sync(dir: &Path, files: &[MountedFile], reader: Reader) -> Result<(), String> {
    forge_catalog::secret_files::validate_files(files)?;

    if let Some(env_dir) = dir.parent() {
        if let Some(root) = env_dir.parent() {
            std::fs::create_dir_all(root).map_err(|e| format!("{}: {e}", root.display()))?;
            set_mode(root, 0o711)?;
        }

        ensure_dir(env_dir, 0o711)?;
    }

    ensure_dir(dir, 0o700)?;
    chown(dir, 0, 0)?;

    for f in files {
        write_one(dir, f, reader)?;
    }

    // Files the spec no longer lists, and temporary files of an
    // interrupted write.
    let keep: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();

    for entry in std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name();

        if keep.iter().any(|k| name.to_str() == Some(*k)) {
            continue;
        }

        let path = entry.path();

        let removed = if entry.file_type().map_err(|e| e.to_string())?.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };

        removed.map_err(|e| format!("{}: {e}", path.display()))?;
    }

    // Open the folder to the account only once every file is in place.
    if let Reader::Native { gid, .. } = reader {
        chown(dir, 0, gid)?;
        set_mode(dir, 0o750)?;
    }

    Ok(())
}

fn write_one(dir: &Path, f: &MountedFile, reader: Reader) -> Result<(), String> {
    let tmp = dir.join(format!(".{}.tmp", f.name));
    let path = dir.join(&f.name);

    // A leftover temporary file of an interrupted write.
    match std::fs::remove_file(&tmp) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("{}: {e}", tmp.display())),
    }

    let mut out = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(&tmp)
        .map_err(|e| format!("{}: {e}", tmp.display()))?;
    out.write_all(f.value.as_bytes()).map_err(|e| format!("{}: {e}", tmp.display()))?;
    out.sync_all().map_err(|e| format!("{}: {e}", tmp.display()))?;
    drop(out);

    let (uid, gid) = match reader {
        Reader::Container => (f.uid, f.gid),
        // A reference that names no owner (root) means the account itself.
        Reader::Native { uid, gid } => (if f.uid == 0 { uid } else { f.uid }, if f.gid == 0 { gid } else { f.gid }),
    };

    chown(&tmp, uid, gid)?;
    set_mode(&tmp, f.mode)?;

    std::fs::rename(&tmp, &path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Removes a service copy's folder and everything in it (the service left
/// the node). A missing folder is fine.
pub fn remove(dir: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(dir) {
        Ok(m) if m.is_dir() => std::fs::remove_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display())),
        Ok(_) => std::fs::remove_file(dir).map_err(|e| format!("{}: {e}", dir.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("{}: {e}", dir.display())),
    }
}

/// The real folder of a spec's copy.
#[must_use]
pub fn dir_of(spec: &forge_catalog::AppSpec) -> std::path::PathBuf {
    let dir = spec.secrets_dir();
    debug_assert!(dir.starts_with(HOST_ROOT));

    std::path::PathBuf::from(dir)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // tests fail loudly on purpose
mod tests {
    use std::os::unix::fs::MetadataExt;

    use super::*;

    fn file(name: &str, mode: u32, value: &str) -> MountedFile {
        MountedFile {
            name: name.into(),
            target: format!("/run/secrets/{name}"),
            mode,
            uid: 0,
            gid: 0,
            value: value.into(),
        }
    }

    fn mode_of(p: &Path) -> u32 {
        std::fs::metadata(p).unwrap().mode() & 0o777
    }

    #[test]
    fn files_are_written_with_their_modes_and_stale_ones_removed() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = dir_under(tmp.path(), "prod", "api");

        sync(&dir, &[file("db", 0o400, "one"), file("app.conf", 0o444, "x=1")], Reader::Container).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("db")).unwrap(), "one");
        assert_eq!(mode_of(&dir.join("db")), 0o400);
        assert_eq!(mode_of(&dir.join("app.conf")), 0o444);
        assert_eq!(mode_of(&dir), 0o700);
        assert_eq!(mode_of(dir.parent().unwrap()), 0o711);

        // A rotation replaces the value; a dropped file and a leftover
        // temporary file go.
        std::fs::write(dir.join(".db.tmp"), "half").unwrap();
        sync(&dir, &[file("db", 0o400, "two")], Reader::Container).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("db")).unwrap(), "two");
        let names: Vec<String> =
            std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        assert_eq!(names, ["db"]);

        remove(&dir).unwrap();
        assert!(!dir.exists());
        remove(&dir).unwrap();
    }

    #[test]
    fn a_native_folder_opens_to_the_account_group_only() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = dir_under(tmp.path(), "prod", "worker--a");

        sync(&dir, &[file("token", 0o400, "t")], Reader::Native { uid: 1000, gid: 1000 }).unwrap();
        assert_eq!(mode_of(&dir), 0o750);
        assert_eq!(mode_of(&dir.join("token")), 0o400);
    }

    #[test]
    fn a_link_in_place_of_the_folder_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let elsewhere = tmp.path().join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        std::fs::create_dir_all(tmp.path().join("prod")).unwrap();
        std::os::unix::fs::symlink(&elsewhere, tmp.path().join("prod/api")).unwrap();

        let err =
            sync(&dir_under(tmp.path(), "prod", "api"), &[file("db", 0o400, "v")], Reader::Container).unwrap_err();
        assert!(err.contains("not a folder"), "{err}");
        assert!(std::fs::read_dir(&elsewhere).unwrap().next().is_none());
    }
}
