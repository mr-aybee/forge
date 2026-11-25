//! Unix socket paths longer than the kernel allows (108 bytes with the
//! terminating NUL). A data directory deep in the file system makes
//! `<data_dir>/analytics-worker/control.sock` too long. Then both sides
//! open the socket's directory and use `/proc/self/fd/<fd>/<name>`, which
//! resolves to the same file: the directory's 0700 mode and the socket's
//! 0600 mode still apply, and nothing is placed in a shared folder.

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

/// Longest path used as is.
pub const MAX_DIRECT: usize = 100;

/// A path to bind or connect to; holds the directory open while needed.
#[derive(Debug)]
pub struct SocketPath {
    path: PathBuf,
    _dir: Option<File>,
}

impl SocketPath {
    /// The path to pass to `bind` or `connect`, valid while `self` lives.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// `path` itself when it is short enough; otherwise, on Linux, the path
/// through an open descriptor of its directory.
pub fn short(path: &Path) -> io::Result<SocketPath> {
    if path.as_os_str().len() <= MAX_DIRECT {
        return Ok(SocketPath { path: path.to_path_buf(), _dir: None });
    }

    via_fd(path)
}

#[cfg(target_os = "linux")]
fn via_fd(path: &Path) -> io::Result<SocketPath> {
    use std::os::fd::AsRawFd;

    let (dir, name) = match (path.parent(), path.file_name()) {
        (Some(d), Some(n)) => (d, n),
        _ => return Err(io::Error::new(io::ErrorKind::InvalidInput, "socket path has no directory")),
    };

    let dir = File::open(dir)?;
    let short = PathBuf::from(format!("/proc/self/fd/{}", dir.as_raw_fd())).join(name);
    if short.as_os_str().len() > MAX_DIRECT {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "socket file name too long"));
    }

    Ok(SocketPath { path: short, _dir: Some(dir) })
}

#[cfg(not(target_os = "linux"))]
fn via_fd(path: &Path) -> io::Result<SocketPath> {
    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("the socket path {} is longer than {MAX_DIRECT} bytes; use a shorter data_dir", path.display()),
    ))
}

#[cfg(all(test, target_os = "linux"))]
#[allow(clippy::unwrap_used)] // reason: tests
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_long_path_binds_and_connects_through_the_directory() {
        let base = tempfile::tempdir().unwrap();
        let deep = base.path().join("d".repeat(60)).join("e".repeat(60));
        std::fs::create_dir_all(&deep).unwrap();
        let path = deep.join("control.sock");
        assert!(path.as_os_str().len() > 108);
        let b = short(&path).unwrap();
        let listener = tokio::net::UnixListener::bind(b.path()).unwrap();
        assert!(path.exists(), "the socket is in the real directory");
        let c = short(&path).unwrap();
        let (client, server) = tokio::join!(tokio::net::UnixStream::connect(c.path()), listener.accept());
        client.unwrap();
        server.unwrap();
        assert_eq!(short(Path::new("/tmp/x.sock")).unwrap().path(), Path::new("/tmp/x.sock"));
    }
}
