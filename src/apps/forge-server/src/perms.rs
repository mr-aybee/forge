//! Owner-only access to the server's data (Linux and other Unix systems).
//!
//! The data directory holds the SQLite database and its `-wal`/`-shm` files
//! (password and token hashes, sealed vault items), the keys, the internal
//! CA (`pki/`), database backups (`backups/`) and staged updates
//! (`updates/`). At start the server makes those directories `0700` and
//! takes group and other permissions off every file in them, so nothing
//! depends on the parent directory alone. New files follow the process
//! umask `077` (and the unit's `UMask=0077`); SQLite gives `-wal` and `-shm`
//! the mode of the database file.
//!
//! Only the data directory itself is fatal: if it cannot be made `0700` the
//! server does not start. A file or subdirectory that cannot be changed
//! (an unusual mount) is logged with its path and the command that fixes
//! it, so an upgrade never fails on it. On Windows the installer's ACL
//! (SYSTEM and Administrators only) covers the whole data directory.

use std::path::{Path, PathBuf};

/// Subdirectories that hold secrets or copies of them.
pub const PRIVATE_DIRS: &[&str] = &["keys", "pki", "backups", "updates"];

/// How deep the walk goes below each private directory (updates are
/// `updates/<version>/<file>`).
#[cfg(unix)]
const MAX_DEPTH: usize = 4;

/// What [`harden`] could not change: `(path, error)`.
pub type Unfixed = Vec<(PathBuf, String)>;

/// Sets the process umask so new files are owner-only. Call first in
/// `main`, before anything creates files.
pub fn owner_only_umask() {
    #[cfg(unix)]
    {
        let _ = rustix::process::umask(rustix::fs::Mode::from_raw_mode(0o077));
    }
}

/// Makes `data` owner-only. `Err` when the data directory itself cannot be
/// restricted; otherwise the paths that could not be changed.
pub fn harden(data: &Path) -> Result<Unfixed, String> {
    #[cfg(unix)]
    {
        unix::harden(data)
    }
    #[cfg(not(unix))]
    {
        let _ = data;
        Ok(Vec::new())
    }
}

#[cfg(unix)]
mod unix {
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    use super::{MAX_DEPTH, PRIVATE_DIRS, Unfixed};

    /// `mode` without group and other bits; the owner's bits stay (staged
    /// update binaries keep their execute bit).
    pub(super) const fn owner_only(mode: u32) -> u32 {
        mode & 0o700
    }

    fn set(path: &Path, mode: u32, unfixed: &mut Unfixed) {
        let res = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
        if let Err(e) = res {
            unfixed.push((path.to_path_buf(), e.to_string()));
        }
    }

    /// Files and directories below `dir`, never following symlinks.
    fn walk(dir: &Path, depth: usize, unfixed: &mut Unfixed) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };

        for e in entries.flatten() {
            let path = e.path();
            let Ok(meta) = std::fs::symlink_metadata(&path) else { continue };
            let ft = meta.file_type();
            if ft.is_symlink() {
                continue;
            }

            let mode = meta.permissions().mode() & 0o7777;

            if ft.is_dir() {
                if mode & 0o077 != 0 || mode & 0o700 != 0o700 {
                    set(&path, 0o700, unfixed);
                }

                if depth < MAX_DEPTH {
                    walk(&path, depth + 1, unfixed);
                }
            } else if ft.is_file() && mode & 0o077 != 0 {
                set(&path, owner_only(mode), unfixed);
            }
        }
    }

    pub(super) fn harden(data: &Path) -> Result<Unfixed, String> {
        std::fs::set_permissions(data, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("cannot make {} owner-only (0700): {e}", data.display()))?;
        let mut unfixed = Vec::new();

        // Files directly in the data directory (forge.db and its -wal/-shm,
        // marker files) and the private subdirectories with their contents.
        let Ok(entries) = std::fs::read_dir(data) else { return Ok(unfixed) };

        for e in entries.flatten() {
            let path = e.path();
            let Ok(meta) = std::fs::symlink_metadata(&path) else { continue };
            let ft = meta.file_type();
            let mode = meta.permissions().mode() & 0o7777;
            if ft.is_file() && mode & 0o077 != 0 {
                set(&path, owner_only(mode), &mut unfixed);
            } else if ft.is_dir()
                && path.file_name().and_then(|n| n.to_str()).is_some_and(|n| PRIVATE_DIRS.contains(&n))
            {
                if mode != 0o700 {
                    set(&path, 0o700, &mut unfixed);
                }

                walk(&path, 1, &mut unfixed);
            }
        }

        Ok(unfixed)
    }
}

#[cfg(all(test, unix))]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn mode(p: &Path) -> u32 {
        std::fs::symlink_metadata(p).unwrap().permissions().mode() & 0o7777
    }

    fn put(p: &Path, m: u32) {
        std::fs::write(p, b"x").unwrap();
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(m)).unwrap();
    }

    #[test]
    fn data_files_and_private_dirs_become_owner_only() {
        let d = tempfile::tempdir().unwrap();
        let data = d.path().join("forge");
        std::fs::create_dir(&data).unwrap();
        std::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o755)).unwrap();

        for f in ["forge.db", "forge.db-wal", "forge.db-shm"] {
            put(&data.join(f), 0o644);
        }

        for sub in PRIVATE_DIRS {
            let s = data.join(sub);
            std::fs::create_dir(&s).unwrap();
            std::fs::set_permissions(&s, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        put(&data.join("pki/ca.key"), 0o644);
        put(&data.join("backups/forge-1.db"), 0o664);
        std::fs::create_dir(data.join("updates/1.2.3")).unwrap();
        put(&data.join("updates/1.2.3/forge-server"), 0o755);

        // A symlink to something outside is never followed or changed.
        let outside = d.path().join("outside");
        put(&outside, 0o644);
        std::os::unix::fs::symlink(&outside, data.join("keys/link")).unwrap();

        let unfixed = harden(&data).unwrap();
        assert!(unfixed.is_empty(), "{unfixed:?}");
        assert_eq!(mode(&data), 0o700);

        for f in ["forge.db", "forge.db-wal", "forge.db-shm", "pki/ca.key"] {
            assert_eq!(mode(&data.join(f)), 0o600, "{f}");
        }

        assert_eq!(mode(&data.join("backups/forge-1.db")), 0o600);

        for sub in PRIVATE_DIRS {
            assert_eq!(mode(&data.join(sub)), 0o700, "{sub}");
        }

        assert_eq!(mode(&data.join("updates/1.2.3")), 0o700);
        assert_eq!(mode(&data.join("updates/1.2.3/forge-server")), 0o700, "owner keeps execute");
        assert_eq!(mode(&outside), 0o644, "symlink target untouched");

        // Idempotent.
        assert!(harden(&data).unwrap().is_empty());
    }

    #[test]
    fn owner_bits_stay_and_special_bits_go() {
        assert_eq!(unix::owner_only(0o644), 0o600);
        assert_eq!(unix::owner_only(0o755), 0o700);
        assert_eq!(unix::owner_only(0o4755), 0o700, "no setuid");
        assert_eq!(unix::owner_only(0o600), 0o600);
    }

    #[test]
    fn a_missing_data_directory_is_fatal() {
        let d = tempfile::tempdir().unwrap();
        assert!(harden(&d.path().join("nope")).is_err());
    }
}
