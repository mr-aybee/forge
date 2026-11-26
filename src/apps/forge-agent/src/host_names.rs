//! The fleet's names in this node's hosts file (D-084).
//!
//! Forge sends the complete list and the suffix every name must end in;
//! this module checks the suffix and every entry again
//! (`forge_catalog::names`), replaces only the marked `names` block and
//! keeps every other byte. On Linux it runs in the root helper.
//!
//! Every hosts file is rewritten in place (same inode, under a lock),
//! without following links: `/etc/hosts` may be bind-mounted into
//! containers (`/etc/hosts:/etc/hosts:ro`, host-network pods), and each
//! Forge container's own hosts file (label `forge.app`) is a bind mount
//! Docker made. Keeping the inode also keeps owner, mode and SELinux
//! context. A file larger than the cap is refused, never cut.
//!
//! On Windows the agent (LocalSystem) rewrites the hosts file the same way
//! and retries when antivirus or another program holds it.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use forge_catalog::names::{BLOCK_TAG, NameEntry, check_suffix, render_block, validate_entries};
use serde_json::{Value, json};

/// Serializes every hosts-file write of this process (names and fabric).
static HOSTS_LOCK: Mutex<()> = Mutex::new(());

/// Largest node hosts file Forge rewrites.
const MAX_HOSTS: u64 = 16 * 1024 * 1024;
/// Largest container hosts file Forge rewrites.
#[cfg(unix)]
const MAX_CONTAINER_HOSTS: u64 = 4 * 1024 * 1024;

/// The node's hosts file.
#[must_use]
pub fn hosts_path() -> PathBuf {
    #[cfg(windows)]
    {
        let root = std::env::var_os("SystemRoot").map_or_else(|| r"C:\Windows".into(), PathBuf::from);
        root.join("System32").join("drivers").join("etc").join("hosts")
    }
    #[cfg(not(windows))]
    {
        PathBuf::from("/etc/hosts")
    }
}

/// Replaces the `tag` block of the node's hosts file with `entries`.
/// Returns whether the file changed.
pub fn write_block(tag: &str, entries: &[NameEntry]) -> Result<bool, String> {
    let _guard = HOSTS_LOCK.lock().map_err(|_| "hosts file lock poisoned".to_owned())?;
    rewrite_in_place(&hosts_path(), tag, entries, MAX_HOSTS)
}

/// Checks a `names` list: a valid suffix and every name inside it.
pub fn check_list(suffix: &str, entries: &[NameEntry]) -> Result<(), String> {
    check_suffix(suffix)?;
    validate_entries(entries)?;
    let tail = format!(".{suffix}");
    if let Some(e) = entries.iter().find(|e| !e.name.ends_with(&tail)) {
        return Err(format!("`{}` does not end in `{tail}`", e.name));
    }

    Ok(())
}

/// Checks a record set before it is written: every record and zone under
/// the suffix (or a reverse zone), and no more records than a names list.
pub fn check_records(suffix: &str, r: &forge_catalog::names::Records) -> Result<(), String> {
    check_suffix(suffix)?;

    let under = |n: &str| n == suffix || n.ends_with(&format!(".{suffix}")) || n.ends_with(".in-addr.arpa");

    if let Some(z) = r.zones.iter().find(|z| !under(z)) {
        return Err(format!("zone {z} is outside .{suffix}"));
    }

    if r.records.len() > forge_catalog::names::MAX_ENTRIES * 3 {
        return Err("too many records".into());
    }

    if let Some(rec) = r.records.iter().find(|x| !under(x.name())) {
        return Err(format!("record {} is outside .{suffix}", rec.name()));
    }

    Ok(())
}

/// Where forge-dns reads its records on a node: its own world-readable
/// folder (the agent's state folder is private to the agent).
#[must_use]
pub fn records_path() -> PathBuf {
    if cfg!(windows) {
        return crate::config::StatePaths::default_dir().join(forge_catalog::names::RECORDS_FILE);
    }

    PathBuf::from("/var/lib/forge-dns").join(forge_catalog::names::RECORDS_FILE)
}

/// Writes `names.json` for forge-dns: a temporary file renamed over the
/// old one, readable by everyone (it holds names and addresses only).
pub async fn write_records(r: &forge_catalog::names::Records) -> Result<(), String> {
    let text = serde_json::to_vec_pretty(r).map_err(|e| e.to_string())?;
    let path = records_path();
    let tmp = path.with_extension("json.tmp");

    if let Some(dir) = path.parent() {
        tokio::fs::create_dir_all(dir).await.map_err(|e| format!("{}: {e}", dir.display()))?;
    }

    tokio::fs::write(&tmp, text).await.map_err(|e| format!("{}: {e}", tmp.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mode = std::fs::Permissions::from_mode(0o644);
        tokio::fs::set_permissions(&tmp, mode).await.map_err(|e| format!("{}: {e}", tmp.display()))?;
    }

    tokio::fs::rename(&tmp, &path).await.map_err(|e| format!("{}: {e}", path.display()))
}

/// Applies a `names` list: the node's hosts file and, with `containers`,
/// those of its Forge containers.
pub async fn apply(suffix: String, entries: Vec<NameEntry>, containers: bool) -> Result<Value, String> {
    check_list(&suffix, &entries)?;
    let list = entries.clone();

    let changed =
        tokio::task::spawn_blocking(move || write_block(BLOCK_TAG, &list)).await.map_err(|e| e.to_string())??;

    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut report = json!({ "names": entries.len(), "changed": changed });
    #[cfg(unix)]
    if containers {
        let (updated, errors) = containers::apply(&entries).await;
        report["containers"] = json!(updated);

        if !errors.is_empty() {
            report["container_errors"] = json!(errors);
        }
    }
    #[cfg(not(unix))]
    let _ = containers;
    Ok(report)
}

/// Why a file could not be opened.
enum OpenError {
    /// Another program holds it; worth trying again in a moment.
    Busy(String),
    /// Anything else (missing, not a file, access denied): retrying cannot help.
    Final(String),
}

impl OpenError {
    fn from_io(path: &Path, e: &std::io::Error) -> Self {
        // ERROR_SHARING_VIOLATION (32) and ERROR_LOCK_VIOLATION (33).
        let busy = cfg!(windows) && matches!(e.raw_os_error(), Some(32 | 33));
        let msg = format!("{}: {e}", path.display());
        if busy { Self::Busy(msg) } else { Self::Final(msg) }
    }
}

/// Opens a regular file for reading and writing without following a link.
fn open_nofollow(path: &Path) -> Result<std::fs::File, OpenError> {
    let meta = std::fs::symlink_metadata(path).map_err(|e| OpenError::from_io(path, &e))?;
    if !meta.file_type().is_file() {
        return Err(OpenError::Final(format!("{} is not a regular file (a link?); not touching it", path.display())));
    }

    let mut opts = std::fs::OpenOptions::new();
    opts.read(true).write(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(i32::try_from(rustix::fs::OFlags::NOFOLLOW.bits()).unwrap_or(0));
    }
    opts.open(path).map_err(|e| OpenError::from_io(path, &e))
}

/// Replaces the `tag` block of the file at `path` in place: read, render,
/// truncate and write on the same open file, then fsync. Refuses files
/// over `max` bytes. Returns whether the file changed.
fn rewrite_in_place(path: &Path, tag: &str, entries: &[NameEntry], max: u64) -> Result<bool, String> {
    use std::io::{Read as _, Seek as _, Write as _};
    let shown = |e: std::io::Error| format!("{}: {e}", path.display());
    let mut f = open_with_retry(path)?;
    let len = f.metadata().map_err(shown)?.len();
    if len > max {
        return Err(format!("{} is {len} bytes, over the {max}-byte limit; not touching it", path.display()));
    }

    let mut raw = Vec::new();

    // One byte past the cap tells a file that grew since the check.
    std::io::Read::by_ref(&mut f).take(max + 1).read_to_end(&mut raw).map_err(shown)?;

    if raw.len() as u64 > max {
        return Err(format!("{} is over the {max}-byte limit; not touching it", path.display()));
    }

    let current =
        String::from_utf8(raw).map_err(|_| format!("{} is not UTF-8 text; not touching it", path.display()))?;
    let next = render_block(&current, tag, entries)?;
    if next == current {
        return Ok(false);
    }

    f.rewind().map_err(shown)?;
    f.set_len(0).map_err(shown)?;
    f.write_all(next.as_bytes()).map_err(shown)?;
    f.sync_all().map_err(shown)?;

    Ok(true)
}

/// Windows: antivirus or another program may hold the file for a moment, so
/// a sharing violation is retried. Other errors (access denied, missing
/// file) fail at once: waiting would only hold `HOSTS_LOCK` longer.
fn open_with_retry(path: &Path) -> Result<std::fs::File, String> {
    let tries = if cfg!(windows) { 5 } else { 1 };
    let mut last = String::new();

    for attempt in 0..tries {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_millis(400));
        }

        match open_nofollow(path) {
            Ok(f) => return Ok(f),
            Err(OpenError::Final(e)) => return Err(e),
            Err(OpenError::Busy(e)) => last = e,
        }
    }

    last.push_str(" (another program, such as antivirus, may hold the file)");

    Err(last)
}

#[cfg(unix)]
mod containers {
    use std::path::Path;
    use std::time::Duration;

    use forge_catalog::names::{BLOCK_TAG, NameEntry};

    const T: Duration = Duration::from_secs(20);

    /// A container's hosts file as Docker reports it, accepted only at
    /// `<docker root>/containers/<its 64-hex id>/hosts`.
    pub(super) fn checked_hosts_path(id: &str, path: &str) -> Option<String> {
        let ok_id = id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit());
        let p = Path::new(path);
        let parent = p.parent()?;

        (ok_id
            && p.is_absolute()
            && !path.contains("/../")
            && p.file_name()? == "hosts"
            && parent.file_name()? == id
            && parent.parent()?.file_name()? == "containers")
            .then(|| path.to_owned())
    }

    /// Rewrites the names block in every running Forge container's hosts
    /// file. Returns how many changed and the errors.
    pub(super) async fn apply(entries: &[NameEntry]) -> (usize, Vec<String>) {
        let filter = "%7B%22label%22%3A%5B%22forge.app%22%5D%7D";
        let list = match crate::docker::api("GET", &format!("/containers/json?filters={filter}"), None, T).await {
            Ok(Some(v)) => v,
            // No Docker on this node: nothing to do.
            Ok(None) | Err(_) => return (0, Vec::new()),
        };

        let mut updated = 0;
        let mut errors = Vec::new();

        for c in list.as_array().into_iter().flatten() {
            let Some(id) = c.get("Id").and_then(serde_json::Value::as_str) else { continue };
            let short = &id[..12.min(id.len())];

            let info = match crate::docker::api("GET", &format!("/containers/{id}/json"), None, T).await {
                Ok(Some(v)) => v,
                _ => continue,
            };

            let reported = info.get("HostsPath").and_then(serde_json::Value::as_str).unwrap_or_default();

            let Some(path) = checked_hosts_path(id, reported) else {
                errors.push(format!("{short}: unexpected hosts path"));
                continue;
            };

            let list = entries.to_vec();
            let r = tokio::task::spawn_blocking(move || {
                super::rewrite_in_place(Path::new(&path), BLOCK_TAG, &list, super::MAX_CONTAINER_HOSTS)
            })
            .await;

            match r {
                Ok(Ok(true)) => updated += 1,
                Ok(Ok(false)) => {}
                Ok(Err(e)) => errors.push(format!("{short}: {e}")),
                Err(e) => errors.push(e.to_string()),
            }
        }

        (updated, errors)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn only_docker_container_hosts_files_are_touched() {
            let id = "a".repeat(64);
            let good = format!("/var/lib/docker/containers/{id}/hosts");
            assert_eq!(checked_hosts_path(&id, &good), Some(good.clone()));
            assert!(checked_hosts_path(&id, &format!("/srv/docker/containers/{id}/hosts")).is_some(), "other root");

            for bad in [
                "/etc/hosts".to_owned(),
                format!("/var/lib/docker/containers/{id}/../../../../etc/hosts"),
                format!("/var/lib/docker/containers/{}/hosts", "b".repeat(64)),
                format!("/var/lib/docker/other/{id}/hosts"),
                format!("var/lib/docker/containers/{id}/hosts"),
                format!("/var/lib/docker/containers/{id}/resolv.conf"),
            ] {
                assert_eq!(checked_hosts_path(&id, &bad), None, "{bad}");
            }

            assert_eq!(checked_hosts_path("abc", "/var/lib/docker/containers/abc/hosts"), None);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn e(name: &str) -> NameEntry {
        NameEntry { name: name.into(), ip: "10.0.0.1".parse().unwrap() }
    }

    #[test]
    fn files_are_rewritten_in_place_and_keep_everything_else() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("hosts");
        std::fs::write(&p, "127.0.0.1 localhost\n172.17.0.2 abc").unwrap();
        #[cfg(unix)]
        let ino = {
            use std::os::unix::fs::MetadataExt;
            std::fs::metadata(&p).unwrap().ino()
        };
        assert!(rewrite_in_place(&p, BLOCK_TAG, &[e("a.local")], MAX_HOSTS).unwrap());
        assert!(!rewrite_in_place(&p, BLOCK_TAG, &[e("a.local")], MAX_HOSTS).unwrap(), "unchanged");
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.starts_with("127.0.0.1 localhost\n172.17.0.2 abc\n# BEGIN orbyn-forge names\n"), "{text}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(std::fs::metadata(&p).unwrap().ino(), ino, "same file: bind mounts still see it");
            assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 1, "no temp file");
            let link = d.path().join("link");
            std::os::unix::fs::symlink(&p, &link).unwrap();
            assert!(rewrite_in_place(&link, BLOCK_TAG, &[e("b.local")], MAX_HOSTS).is_err(), "links are refused");
        }
    }

    /// Review of D-084: a file over the cap is refused, never cut.
    #[test]
    fn oversized_files_are_refused_not_truncated() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("hosts");
        let big = "10.9.9.9 filler\n".repeat(100);
        std::fs::write(&p, &big).unwrap();
        let err = rewrite_in_place(&p, BLOCK_TAG, &[e("a.local")], 1000).unwrap_err();
        assert!(err.contains("limit"), "{err}");
        assert_eq!(std::fs::read_to_string(&p).unwrap(), big, "untouched");
    }

    /// Review of D-084: the node only writes names under the suffix.
    #[test]
    fn lists_must_stay_under_a_safe_suffix() {
        assert!(check_list("local", &[e("a.local"), e("b.lab.local")]).is_ok());
        assert!(check_list("local", &[e("github.com")]).is_err());
        assert!(check_list("local", &[e("evil-local.com")]).is_err());
        assert!(check_list("com", &[e("github.com")]).is_err(), "public suffixes are refused");
        assert!(check_list("corp.example.com", &[e("a.corp.example.com")]).is_ok());
    }
}
