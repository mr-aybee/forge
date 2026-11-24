//! Agent self-update (GitHub issue #4), done by the root helper on Linux.
//!
//! 0. Take the update lock (`update.lock`, `flock`): a second request is
//!    refused as `busy`. Finish or undo a swap an earlier helper left
//!    half done ([`recover_in`]). Nothing to do when the installed files
//!    already have the requested hashes; refuse an older version as
//!    `downgrade` unless the command allows it.
//! 1. Download the agent and helper for this architecture from the control
//!    plane (`<base>/agent/download/linux/<arch>/<binary>`, fixed `curl`
//!    argv, size-capped) into a staging directory of its own.
//! 2. Verify each against the SHA-256 in the **signed** command; dry-run
//!    `forge-agent --version` and `forge-agent-helper --version` and require
//!    the expected version from both.
//! 3. Write the intent marker (old and new hashes), then per binary: copy to
//!    `<bin>.new` next to it, fsync, hash it again, keep the current one as
//!    `<bin>.prev`, rename `.new` over it and fsync the folder. Then the
//!    pending marker, and the intent marker goes.
//! 4. Restart helper and agent through `systemd-run` (after this reply).
//!
//! The staging folder and the authoritative pending marker live in
//! [`HELPER_STATE`], a root-only (0700) folder: the agent user cannot swap a
//! staged binary between the checksum check, the dry run and the install, nor
//! point the marker at another file. The agent sees a copy of the marker in
//! its own state folder ([`marker`]); root writes that copy without following
//! links and trusts its content only for an update an older helper started.
//!
//! Rollback: the helper's watchdog ([`watchdog`]) restores `*.prev` when the
//! new agent has not confirmed ([`confirm`], after its first good heartbeat)
//! within [`CONFIRM_WITHIN`], after more than [`MAX_STARTS`] helper starts
//! with the marker still present, or as soon as systemd has restarted the
//! agent [`MAX_STARTS`] times since the swap (an agent-only crash loop).

use std::path::{Path, PathBuf};
use std::time::Duration;

use forge_proto::messages::RefusalCode;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::executor::CommandError;

/// Installed binaries.
pub const AGENT_BIN: &str = "/usr/local/bin/forge-agent";
pub const HELPER_BIN: &str = "/usr/local/bin/forge-agent-helper";
/// The new agent must confirm within this time.
pub const CONFIRM_WITHIN: Duration = Duration::from_secs(300);
/// Helper starts allowed with the marker present before rolling back.
pub const MAX_STARTS: u32 = 3;
/// How often the watchdog looks at the agent's restart counter.
const WATCH_EVERY: Duration = Duration::from_secs(5);
/// Largest binary accepted (bytes).
const MAX_BYTES: u64 = 200 * 1024 * 1024;
/// How long a staged binary may take to print its version.
const DRY_RUN_LIMIT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpdateSpec {
    pub version: String,
    pub download_base: String,
    pub arch: String,
    pub agent_sha256: String,
    pub helper_sha256: String,
    /// Install even when `version` is older than this helper's.
    #[serde(default)]
    pub allow_downgrade: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pending {
    pub from: String,
    pub to: String,
    pub started_ms: i64,
    #[serde(default)]
    pub helper_starts: u32,
    /// systemd's restart counter of the agent unit when the update started
    /// (lowered when the counter restarts from zero). Absent until first read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_restarts_base: Option<u32>,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

/// Root-only state of the helper (staging, authoritative markers).
pub const HELPER_STATE: &str = "/var/lib/forge-agent-helper";
const PENDING: &str = "update-pending.json";
/// Largest marker read (bytes).
const MAX_MARKER: u64 = 64 * 1024;

/// The agent's copy of the pending marker, in its state directory. The agent
/// only checks that it exists; the helper keeps the real one in [`HELPER_STATE`].
#[must_use]
pub fn marker(agent_dir: &Path) -> PathBuf {
    agent_dir.join(PENDING)
}

/// Creates `dir` (0700) if needed and checks that it is a real directory
/// owned by this (root) process, not a link someone planted.
pub(crate) fn ensure_private_dir(dir: &Path) -> Result<(), String> {
    match std::fs::create_dir(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(format!("{}: {e}", dir.display())),
    }
    let meta = std::fs::symlink_metadata(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    if !meta.is_dir() {
        return Err(format!("{} is not a directory", dir.display()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if meta.uid() != rustix::process::geteuid().as_raw() {
            return Err(format!("{} is not owned by root", dir.display()));
        }
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    Ok(())
}

/// A temporary name next to `path` that no other writer uses: the file
/// name, the process id and 64 random bits.
fn unique_tmp(path: &Path) -> Result<PathBuf, String> {
    use ring::rand::SecureRandom as _;
    let mut r = [0u8; 8];
    ring::rand::SystemRandom::new().fill(&mut r).map_err(|_| "no randomness for a temporary name".to_owned())?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let suffix: String = r.iter().map(|b| format!("{b:02x}")).collect();
    Ok(path.with_file_name(format!(".{name}.{}.{suffix}.tmp", std::process::id())))
}

/// Writes `path` through a fresh temporary file and a rename: a link at
/// `path` or at the temporary name is replaced or refused, never followed.
/// The temporary name is unique, so concurrent writers never share it;
/// the last rename wins (callers that load, modify and save hold a lock).
pub(crate) fn write_nofollow(path: &Path, data: &[u8]) -> Result<(), String> {
    use std::io::Write as _;
    let tmp = unique_tmp(path)?;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o644).custom_flags(i32::try_from(rustix::fs::OFlags::NOFOLLOW.bits()).unwrap_or(0));
    }
    let mut f = opts.open(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
    let written = f.write_all(data).and_then(|()| f.sync_all()).map_err(|e| format!("{}: {e}", tmp.display()));
    drop(f);
    let renamed = written.and_then(|()| std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display())));
    if renamed.is_err() {
        // Best effort: a failed write leaves no temporary file behind.
        let _ = std::fs::remove_file(&tmp);
    }
    renamed
}

/// Reads up to `max` bytes of `path` without following a link at it.
pub(crate) fn read_nofollow(path: &Path, max: u64) -> Option<Vec<u8>> {
    use std::io::Read as _;
    let mut opts = std::fs::OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(i32::try_from(rustix::fs::OFlags::NOFOLLOW.bits()).unwrap_or(0));
    }
    let mut raw = Vec::new();
    opts.open(path).ok()?.take(max).read_to_end(&mut raw).ok()?;
    Some(raw)
}

/// SHA-256 (lower-case hex) and length of `path`, read in 64 KiB chunks
/// without following a link at it, so memory stays flat whatever the size.
/// Fails with `FileTooLarge` past `max` bytes. Blocking: call it from
/// `spawn_blocking` in async code.
pub(crate) fn sha256_nofollow(path: &Path, max: u64) -> std::io::Result<(String, u64)> {
    use std::io::Read as _;
    let mut opts = std::fs::OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(i32::try_from(rustix::fs::OFlags::NOFOLLOW.bits()).unwrap_or(0));
    }
    let mut f = opts.open(path)?;
    let mut ctx = ring::digest::Context::new(&ring::digest::SHA256);
    let mut buf = vec![0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > max {
            return Err(std::io::Error::new(std::io::ErrorKind::FileTooLarge, format!("larger than {max} bytes")));
        }
        ctx.update(&buf[..n]);
    }
    Ok((hex::encode(ctx.finish()), total))
}

/// Reads a marker without following a link at its path.
fn read_marker(path: &Path) -> Option<Pending> {
    serde_json::from_slice(&read_nofollow(path, MAX_MARKER)?).ok()
}

/// Records the pending update: the real marker in `state`, the agent's copy in `agent_dir`.
fn save_pending(p: &Pending, agent_dir: &Path, state: &Path) -> Result<(), String> {
    let raw = serde_json::to_vec(p).map_err(|e| e.to_string())?;
    write_nofollow(&state.join(PENDING), &raw)?;
    write_nofollow(&marker(agent_dir), &raw)
}

fn prev(p: &Path) -> PathBuf {
    sibling(p, ".prev")
}

/// `<path><suffix>` in the same folder.
fn sibling(p: &Path, suffix: &str) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// Where the binaries and the helper's state live: [`Layout::system`] on a
/// node, temporary folders in tests.
#[derive(Debug, Clone)]
pub struct Layout {
    pub agent_bin: PathBuf,
    pub helper_bin: PathBuf,
    /// Root-only state of the helper ([`HELPER_STATE`]).
    pub state: PathBuf,
    /// The agent's state folder (its copy of the pending marker).
    pub agent_dir: PathBuf,
}

impl Layout {
    #[must_use]
    pub fn system(agent_dir: &Path) -> Self {
        Self {
            agent_bin: PathBuf::from(AGENT_BIN),
            helper_bin: PathBuf::from(HELPER_BIN),
            state: PathBuf::from(HELPER_STATE),
            agent_dir: agent_dir.to_path_buf(),
        }
    }

    fn bins(&self) -> [&Path; 2] {
        [&self.agent_bin, &self.helper_bin]
    }
}

/// What an update needs from the node, replaced in tests.
pub trait UpdateHost {
    /// systemd supervises the agent and helper.
    fn supervised(&self) -> bool;
    /// Downloads `url` to `to`.
    ///
    /// # Errors
    /// When the download fails; `unsupported` without `curl`.
    fn download(&self, url: &str, to: &Path) -> Result<(), CommandError>;
    /// What `<bin> --version` prints.
    ///
    /// # Errors
    /// When it does not run or fails.
    fn report_version(&self, bin: &Path) -> Result<String, String>;
    /// Restarts helper and agent a few seconds after this reply.
    ///
    /// # Errors
    /// When the restart cannot be scheduled.
    fn restart(&self) -> Result<(), String>;
    /// systemd's restart counter of the agent unit.
    fn agent_restarts(&self) -> Option<u32>;
}

/// The node itself.
#[derive(Debug, Clone, Copy)]
pub struct SystemHost;

impl UpdateHost for SystemHost {
    fn supervised(&self) -> bool {
        Path::new("/run/systemd/system").exists()
    }

    fn download(&self, url: &str, to: &Path) -> Result<(), CommandError> {
        download(url, to)
    }

    fn report_version(&self, bin: &Path) -> Result<String, String> {
        use std::io::Read as _;
        // A clean environment, the staging folder as working folder, and a
        // time limit: the binary is verified, but runs as root while the
        // update lock is held.
        let mut child = std::process::Command::new(bin)
            .arg("--version")
            .env_clear()
            .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
            .current_dir(bin.parent().unwrap_or(Path::new("/")))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| format!("dry run: {e}"))?;
        let deadline = std::time::Instant::now() + DRY_RUN_LIMIT;
        let status = loop {
            match child.try_wait().map_err(|e| format!("dry run: {e}"))? {
                Some(st) => break st,
                None if std::time::Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("dry run: {} did not answer within {DRY_RUN_LIMIT:?}", bin.display()));
                }
                None => std::thread::sleep(Duration::from_millis(50)),
            }
        };
        let mut out = String::new();
        if let Some(mut o) = child.stdout.take() {
            let _ = o.by_ref().take(4096).read_to_string(&mut out);
        }
        if status.success() { Ok(out) } else { Err(format!("dry run: {} exited with {status}", bin.display())) }
    }

    fn restart(&self) -> Result<(), String> {
        restart_later()
    }

    fn agent_restarts(&self) -> Option<u32> {
        agent_restarts()
    }
}

/// Checks the spec before anything touches the disk.
pub fn validate(u: &UpdateSpec) -> Result<(), String> {
    let hex64 = |s: &str| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit());
    if !hex64(&u.agent_sha256) || !hex64(&u.helper_sha256) {
        return Err("invalid checksums".into());
    }
    if !matches!(u.arch.as_str(), "x86_64" | "aarch64") {
        return Err("unsupported architecture".into());
    }
    if u.version.is_empty()
        || u.version.len() > 64
        || !u.version.bytes().all(|b| b.is_ascii_alphanumeric() || b"._+-~".contains(&b))
    {
        return Err("invalid version".into());
    }
    let base = url::Url::parse(&u.download_base).map_err(|_| "invalid download base".to_owned())?;
    if !matches!(base.scheme(), "http" | "https") || base.host_str().is_none() || base.query().is_some() {
        return Err("download base must be an http(s) URL without a query".into());
    }
    Ok(())
}

fn sha256_file(p: &Path) -> Result<String, String> {
    sha256_nofollow(p, MAX_BYTES).map(|(hex, _)| hex).map_err(|e| format!("{}: {e}", p.display()))
}

fn download(url: &str, to: &Path) -> Result<(), CommandError> {
    let out = std::process::Command::new("curl")
        .args([
            "-fsSL",
            "--proto",
            "=http,https",
            "--max-redirs",
            "0",
            "--max-time",
            "600",
            "--max-filesize",
            &MAX_BYTES.to_string(),
            "-o",
        ])
        .arg(to)
        .arg(url)
        .output()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => {
                CommandError::refused(RefusalCode::Unsupported, "self-update needs curl on this node; install curl")
            }
            _ => CommandError::from(format!("curl: {e}")),
        })?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!("download failed: {}", String::from_utf8_lossy(&out.stderr).trim()).into())
    }
}

fn restart_later() -> Result<(), String> {
    // After this reply: restart helper and agent outside this process.
    let out = std::process::Command::new("systemd-run")
        .args([
            "--quiet",
            "--collect",
            "--on-active=3",
            // A timer fires within its accuracy window, one minute by
            // default: without this the restart came 3 to 63 seconds later.
            "--timer-property=AccuracySec=1s",
            &format!("--unit=forge-agent-update-{}", now_ms()),
            "/bin/systemctl",
            "restart",
            "forge-agent-helper.service",
            "forge-agent.service",
        ])
        .output()
        .map_err(|e| format!("systemd-run: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!("systemd-run failed: {}", String::from_utf8_lossy(&out.stderr).trim()))
    }
}

/// Runs in the root helper: download, verify, stage, swap, restart.
///
/// # Errors
/// A [`CommandError`]; refusals the control plane acts on carry a code
/// (`unsupported`: no systemd or curl; `busy`: another update runs;
/// `downgrade`: an older version without `allow_downgrade`).
pub fn apply(u: &UpdateSpec, agent_dir: &Path) -> Result<Value, CommandError> {
    apply_in(u, &Layout::system(agent_dir), &SystemHost)
}

/// The intent marker: written before the first binary is replaced, removed
/// after the pending marker is written. A helper that finds it finishes or
/// undoes the swap by the files' hashes ([`recover_in`]).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct Intent {
    from: String,
    to: String,
    files: Vec<IntentFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct IntentFile {
    path: PathBuf,
    /// The hash before the swap; `None` when the file did not exist.
    old: Option<String>,
    new: String,
}

const INTENT: &str = "update-intent.json";
const LOCK: &str = "update.lock";

/// Held while an update runs: one at a time per node, across helper
/// processes (`flock` on `update.lock`, released when dropped).
struct UpdateLock {
    _file: std::fs::File,
}

impl UpdateLock {
    fn acquire(state: &Path) -> Result<Self, CommandError> {
        let path = state.join(LOCK);
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        #[cfg(unix)]
        rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive)
            .map_err(|_| CommandError::refused(RefusalCode::Busy, "another agent update is running on this node"))?;
        Ok(Self { _file: file })
    }
}

/// The hash of `p`, `None` when it does not exist.
fn hash_of(p: &Path) -> Result<Option<String>, String> {
    match sha256_nofollow(p, MAX_BYTES) {
        Ok((h, _)) => Ok(Some(h)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", p.display())),
    }
}

/// Flushes a folder's entries (renames) to disk.
fn sync_dir(dir: &Path) -> Result<(), String> {
    #[cfg(unix)]
    std::fs::File::open(dir).and_then(|d| d.sync_all()).map_err(|e| format!("{}: {e}", dir.display()))?;
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

/// Installs `src` at `dst` (expected hash `want`), keeping the current file
/// as `dst.prev`: a copy to `dst.new` in the same folder, fsync, a second
/// hash, then one rename and an fsync of the folder.
fn install_file(src: &Path, dst: &Path, want: &str) -> Result<(), String> {
    use std::io::Write as _;
    let new = sibling(dst, ".new");
    let _ = std::fs::remove_file(&new);
    {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o755).custom_flags(i32::try_from(rustix::fs::OFlags::NOFOLLOW.bits()).unwrap_or(0));
        }
        let mut out = opts.open(&new).map_err(|e| format!("{}: {e}", new.display()))?;
        let data = std::fs::read(src).map_err(|e| format!("{}: {e}", src.display()))?;
        out.write_all(&data).and_then(|()| out.sync_all()).map_err(|e| format!("{}: {e}", new.display()))?;
    }
    if hash_of(&new)?.as_deref() != Some(want) {
        let _ = std::fs::remove_file(&new);
        return Err(format!("{}: the copy does not match the checksum", new.display()));
    }
    let old = prev(dst);
    let _ = std::fs::remove_file(&old);
    if dst.exists() && std::fs::hard_link(dst, &old).is_err() {
        std::fs::copy(dst, &old).map_err(|e| format!("backup {}: {e}", dst.display()))?;
    }
    std::fs::rename(&new, dst).map_err(|e| format!("{}: {e}", dst.display()))?;
    sync_dir(dst.parent().unwrap_or(Path::new("/")))
}

/// Finishes or undoes a swap an earlier helper left half done (its intent
/// marker is still there): when every file has its new hash, the update is
/// complete and only the pending marker is written; otherwise every file
/// that has its new hash gets its `.prev` back. Returns what it did, and
/// restarts the services when it changed binaries.
fn recover_in(l: &Layout, host: &dyn UpdateHost) -> Option<&'static str> {
    let path = l.state.join(INTENT);
    let intent: Intent = serde_json::from_slice(&read_nofollow(&path, MAX_MARKER)?).ok()?;
    let current: Vec<Option<String>> = intent.files.iter().map(|f| hash_of(&f.path).ok().flatten()).collect();
    let done = intent.files.iter().zip(&current).all(|(f, c)| c.as_deref() == Some(f.new.as_str()));
    let what = if done {
        if read_marker(&l.state.join(PENDING)).is_none() {
            let p = Pending {
                from: intent.from.clone(),
                to: intent.to.clone(),
                started_ms: now_ms(),
                helper_starts: 0,
                agent_restarts_base: host.agent_restarts(),
            };
            if let Err(e) = save_pending(&p, &l.agent_dir, &l.state) {
                eprintln!("forge-agent-helper: self-update marker: {e}");
            }
        }
        "finished"
    } else {
        let mut restored = true;
        for (f, c) in intent.files.iter().zip(&current) {
            let _ = std::fs::remove_file(sibling(&f.path, ".new"));
            if c.as_deref() != Some(f.new.as_str()) {
                continue;
            }
            let back = prev(&f.path);
            let r = match &f.old {
                // The file did not exist before the update.
                None => std::fs::remove_file(&f.path),
                Some(_) if hash_of(&back).ok().flatten() == f.old => std::fs::rename(&back, &f.path),
                Some(_) => Err(std::io::Error::other("its .prev is not the file it replaced")),
            };
            if let Err(e) = r {
                eprintln!("forge-agent-helper: self-update: restoring {}: {e}", f.path.display());
                restored = false;
            }
        }
        if !restored {
            // Keep the record: the next start or update tries again.
            return Some("partly reverted");
        }
        "reverted"
    };
    let _ = std::fs::remove_file(&path);
    let _ = sync_dir(&l.state);
    eprintln!("forge-agent-helper: an interrupted self-update to {} was {what}", intent.to);
    let _ = host.restart();
    Some(what)
}

/// At helper start: finishes or undoes a swap a crash interrupted.
pub fn recover(agent_dir: &Path) -> Option<&'static str> {
    let l = Layout::system(agent_dir);
    let _lock = UpdateLock::acquire(&l.state).ok()?;
    recover_in(&l, &SystemHost)
}

/// The version `<bin> --version` prints (`forge-agent 1.2.3`).
fn reported_version(host: &dyn UpdateHost, bin: &Path, want: &str) -> Result<(), CommandError> {
    let out = host.report_version(bin).map_err(CommandError::from)?;
    if out.split_whitespace().nth(1) == Some(want) {
        Ok(())
    } else {
        let name = bin.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        Err(format!("the new {name} reports `{}`, expected {want}: nothing was installed", out.trim()).into())
    }
}

fn apply_in(u: &UpdateSpec, l: &Layout, host: &dyn UpdateHost) -> Result<Value, CommandError> {
    validate(u)?;
    if !host.supervised() {
        return Err(CommandError::refused(
            RefusalCode::Unsupported,
            "self-update needs systemd on this node; reinstall the agent instead",
        ));
    }
    // Root-only staging: what is checked below is exactly what gets installed.
    ensure_private_dir(&l.state)?;
    let _lock = UpdateLock::acquire(&l.state)?;
    recover_in(l, host);
    let (want_agent, want_helper) = (u.agent_sha256.to_lowercase(), u.helper_sha256.to_lowercase());
    // Idempotent by what is installed, not by this helper's version: a
    // retry after the swap restarts (if needed) instead of swapping again.
    if hash_of(&l.agent_bin)?.as_deref() == Some(want_agent.as_str())
        && hash_of(&l.helper_bin)?.as_deref() == Some(want_helper.as_str())
    {
        let pending = read_marker(&l.state.join(PENDING)).is_some_and(|p| p.to == u.version);
        if pending {
            host.restart()?;
        }
        return Ok(
            json!({ "updated": false, "version": u.version, "reason": "already installed", "restarting": pending }),
        );
    }
    if forge_sdk::release::compare_release(&u.version, crate::VERSION) == std::cmp::Ordering::Less && !u.allow_downgrade
    {
        return Err(CommandError::refused(
            RefusalCode::Downgrade,
            format!("{} is older than {}: not installed without allow_downgrade", u.version, crate::VERSION),
        ));
    }
    // A staging folder per request; earlier ones are left-overs.
    if let Ok(dir) = std::fs::read_dir(&l.state) {
        for e in dir.flatten().filter(|e| e.file_name().to_string_lossy().starts_with("update-staging")) {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
    let stage = l.state.join(format!("update-staging-{}", now_ms()));
    ensure_private_dir(&stage)?;
    let r = stage_and_swap(u, l, host, &stage, (&want_agent, &want_helper));
    let _ = std::fs::remove_dir_all(&stage);
    let pending = r?;
    host.restart()?;
    Ok(json!({ "updated": true, "from": pending.from, "to": pending.to, "restarting_in_secs": 3 }))
}

/// Downloads, checks and dry-runs both binaries in `stage`, then swaps them
/// in under the intent marker and writes the pending marker.
fn stage_and_swap(
    u: &UpdateSpec,
    l: &Layout,
    host: &dyn UpdateHost,
    stage: &Path,
    (want_agent, want_helper): (&str, &str),
) -> Result<Pending, CommandError> {
    let base = u.download_base.trim_end_matches('/');
    let (a, h) = (stage.join("forge-agent"), stage.join("forge-agent-helper"));
    host.download(&format!("{base}/agent/download/linux/{}/forge-agent", u.arch), &a)?;
    host.download(&format!("{base}/agent/download/linux/{}/forge-agent-helper", u.arch), &h)?;
    if sha256_file(&a)? != want_agent || sha256_file(&h)? != want_helper {
        return Err("checksum mismatch: nothing was installed".into());
    }
    #[cfg(unix)]
    for p in [&a, &h] {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    }
    // Dry run: both new binaries must start and report the expected version;
    // a helper that cannot start would leave the node without its watchdog.
    reported_version(host, &a, &u.version)?;
    reported_version(host, &h, &u.version)?;
    let intent = Intent {
        from: crate::VERSION.to_owned(),
        to: u.version.clone(),
        files: vec![
            IntentFile { path: l.helper_bin.clone(), old: hash_of(&l.helper_bin)?, new: want_helper.to_owned() },
            IntentFile { path: l.agent_bin.clone(), old: hash_of(&l.agent_bin)?, new: want_agent.to_owned() },
        ],
    };
    write_nofollow(&l.state.join(INTENT), &serde_json::to_vec(&intent).map_err(|e| e.to_string())?)?;
    sync_dir(&l.state)?;
    for (src, f) in [&h, &a].into_iter().zip(&intent.files) {
        if let Err(e) = install_file(src, &f.path, &f.new) {
            // Undo what was swapped; the intent marker goes with it.
            recover_in(l, host);
            return Err(e.into());
        }
    }
    let pending = Pending {
        from: crate::VERSION.to_owned(),
        to: u.version.clone(),
        started_ms: now_ms(),
        helper_starts: 0,
        agent_restarts_base: host.agent_restarts(),
    };
    save_pending(&pending, &l.agent_dir, &l.state)?;
    let _ = std::fs::remove_file(l.state.join(INTENT));
    sync_dir(&l.state)?;
    Ok(pending)
}

/// Restores the previous binaries and restarts (watchdog). False while an
/// update holds the lock.
pub fn rollback(agent_dir: &Path, reason: &str) -> bool {
    rollback_in(&Layout::system(agent_dir), &SystemHost, reason)
}

/// Returns false, doing nothing, while an update holds the lock (the
/// watchdog tries again on its next tick).
fn rollback_in(l: &Layout, host: &dyn UpdateHost, reason: &str) -> bool {
    let Ok(_lock) = UpdateLock::acquire(&l.state) else { return false };
    eprintln!("forge-agent-helper: self-update rollback ({reason})");
    for b in l.bins() {
        if prev(b).exists() {
            let _ = std::fs::rename(prev(b), b);
        }
    }
    if let Some(dir) = l.agent_bin.parent() {
        let _ = sync_dir(dir);
    }
    let _ = std::fs::rename(l.state.join(PENDING), l.state.join("update-rolled-back.json"));
    let _ = std::fs::remove_file(marker(&l.agent_dir));
    let _ = host.restart();
    true
}

/// Clears the marker (called by the new agent after a good heartbeat).
pub fn confirm(agent_dir: &Path) -> Result<Value, String> {
    confirm_in(&Layout::system(agent_dir))
}

fn confirm_in(l: &Layout) -> Result<Value, String> {
    let (agent_dir, state) = (l.agent_dir.as_path(), l.state.as_path());
    // Never while an update runs or a swap is half done: confirming removes
    // the `.prev` copies a rollback or recovery needs.
    let _lock = match state.exists().then(|| UpdateLock::acquire(state)) {
        Some(Err(_)) => return Ok(json!({ "confirmed": false, "reason": "an update is running" })),
        lock => lock,
    };
    if state.join(INTENT).exists() {
        return Ok(json!({ "confirmed": false, "reason": "an update is not finished" }));
    }
    let real = state.join(PENDING);
    // An update started by an older helper only left the agent's copy.
    let Some(p) = read_marker(&real).or_else(|| read_marker(&marker(agent_dir))) else {
        let _ = std::fs::remove_file(marker(agent_dir));
        return Ok(json!({ "confirmed": false }));
    };
    if p.to != crate::VERSION {
        // The helper must run the new version too; otherwise wait.
        return Ok(json!({ "confirmed": false, "reason": "helper is not the new version yet" }));
    }
    if real.exists() {
        let _ = std::fs::rename(&real, state.join("update-last.json"));
    }
    let _ = std::fs::remove_file(marker(agent_dir));
    for b in l.bins() {
        let _ = std::fs::remove_file(prev(b));
    }
    Ok(json!({ "confirmed": true, "version": p.to }))
}

/// Helper start: counts starts with a marker present and arms the timer.
/// Returns the deadline to watch, if any.
pub fn watchdog_start(agent_dir: &Path) -> Option<Duration> {
    watchdog_start_in(&Layout::system(agent_dir), &SystemHost)
}

fn watchdog_start_in(l: &Layout, host: &dyn UpdateHost) -> Option<Duration> {
    let (agent_dir, state) = (l.agent_dir.as_path(), l.state.as_path());
    // An update started by an older helper only left the agent's copy: adopt it.
    let mut p = read_marker(&state.join(PENDING)).or_else(|| read_marker(&marker(agent_dir)))?;
    p.helper_starts += 1;
    if let Err(e) = ensure_private_dir(state).and_then(|()| save_pending(&p, agent_dir, state)) {
        eprintln!("forge-agent-helper: self-update marker: {e}");
    }
    // The update restarts the helper once itself.
    if p.helper_starts > MAX_STARTS + 1 {
        // At start nothing else holds the lock; if something does, the
        // next start tries again.
        let _ = rollback_in(l, host, "too many restarts");
        return None;
    }
    let elapsed = u64::try_from(now_ms() - p.started_ms).unwrap_or(0);
    Some(CONFIRM_WITHIN.saturating_sub(Duration::from_millis(elapsed)))
}

/// How often systemd restarted the agent unit by itself (`NRestarts`). A
/// manual restart, like the one the update does, does not count.
fn agent_restarts() -> Option<u32> {
    let out = std::process::Command::new("/bin/systemctl")
        .args(["show", "forge-agent.service", "--property=NRestarts", "--value"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

/// Records what the agent's restart counter shows now. True when systemd has
/// restarted the agent [`MAX_STARTS`] times since the update: a crash loop.
fn agent_crash_loop_in(agent_dir: &Path, state: &Path, restarts: u32) -> bool {
    let Some(mut p) = read_marker(&state.join(PENDING)) else { return false };
    // First reading, or systemd reset its counter (the update's own restart).
    let base = match p.agent_restarts_base {
        Some(b) if b <= restarts => b,
        _ => {
            p.agent_restarts_base = Some(restarts);
            if let Err(e) = save_pending(&p, agent_dir, state) {
                eprintln!("forge-agent-helper: self-update marker: {e}");
            }
            restarts
        }
    };
    restarts - base >= MAX_STARTS
}

/// Helper task: while the marker is there, rolls back when the new agent
/// keeps crashing or has not confirmed at the deadline.
pub async fn watchdog(agent_dir: PathBuf) {
    let Some(wait) = watchdog_start(&agent_dir) else { return };
    let state = Path::new(HELPER_STATE);
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        tokio::time::sleep(left.min(WATCH_EVERY)).await;
        if !state.join(PENDING).exists() {
            return;
        }
        let reason = if tokio::time::Instant::now() >= deadline {
            Some("the new agent did not reconnect in time")
        } else {
            let restarts = tokio::task::spawn_blocking(agent_restarts).await.ok().flatten();
            restarts
                .is_some_and(|n| agent_crash_loop_in(&agent_dir, state, n))
                .then_some("the new agent keeps crashing")
        };
        if let Some(reason) = reason {
            let dir = agent_dir.clone();
            // A rollback waits for an update that holds the lock: next tick.
            if tokio::task::spawn_blocking(move || rollback(&dir, reason)).await.unwrap_or(false) {
                return;
            }
        }
    }
}

// ------------------------------------------------------------ capability

/// Folder of the installed binaries.
const BIN_DIR: &str = "/usr/local/bin";

/// What the helper sees on this host, for [`assess`]. Plain values, so the
/// assessment is tested without a host.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostProbe {
    /// systemd runs as init (`/run/systemd/system`).
    pub systemd: bool,
    pub curl: bool,
    /// `/usr/local/bin` is writable and not on a read-only mount.
    pub bin_dir_writable: bool,
    /// Free bytes on the file system of `/usr/local/bin`.
    pub free_bytes: Option<u64>,
    /// Size of the installed agent and helper.
    pub binaries_bytes: u64,
    /// The binaries sit in a container's image layer (overlay root).
    pub image_layer: bool,
}

/// Whether this node can update its own agent, and why not.
#[must_use]
pub fn assess(p: &HostProbe) -> forge_proto::facts::SelfUpdateCapability {
    use forge_proto::facts::{SelfUpdateCapability, Supervisor, UpdateIssue};
    let mut reasons = Vec::new();
    if !p.systemd {
        reasons.push(UpdateIssue::NoSystemd);
    }
    if !p.curl {
        reasons.push(UpdateIssue::NoCurl);
    }
    if !p.bin_dir_writable {
        reasons.push(UpdateIssue::BinDirNotWritable);
    }
    // The swap writes `.new` copies next to the binaries and keeps `.prev`.
    if p.free_bytes.is_some_and(|free| free < p.binaries_bytes.saturating_mul(2)) {
        reasons.push(UpdateIssue::LowDisk);
    }
    let warnings = if p.image_layer { vec![UpdateIssue::EphemeralContainer] } else { Vec::new() };
    SelfUpdateCapability {
        supervisor: if p.systemd { Supervisor::Systemd } else { Supervisor::None },
        ready: reasons.is_empty(),
        reasons,
        warnings,
        helper_version: crate::VERSION.to_owned(),
    }
}

/// Whether `curl` is on `PATH` (or in the usual places).
fn curl_installed() -> bool {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .chain(["/usr/bin", "/bin", "/usr/local/bin"].map(PathBuf::from))
        .any(|d| d.join("curl").is_file())
}

/// Looks at this host. Blocking (file system calls): run it in
/// `spawn_blocking`.
#[cfg(target_os = "linux")]
#[must_use]
pub fn probe_host() -> HostProbe {
    use rustix::fs::{Access, StatVfsMountFlags};
    let dir = Path::new(BIN_DIR);
    let vfs = rustix::fs::statvfs(dir).ok();
    let read_only = vfs.as_ref().is_some_and(|v| v.f_flag.contains(StatVfsMountFlags::RDONLY));
    let size = |p: &str| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
    let container = Path::new("/.dockerenv").exists() || Path::new("/run/.containerenv").exists();
    // OVERLAYFS_SUPER_MAGIC: the image layer of a container.
    // `f_type` has a different integer type per target.
    let overlay = rustix::fs::statfs(dir).is_ok_and(|f| u64::try_from(f.f_type).ok() == Some(0x794c_7630));
    HostProbe {
        systemd: Path::new("/run/systemd/system").exists(),
        curl: curl_installed(),
        bin_dir_writable: !read_only && rustix::fs::access(dir, Access::WRITE_OK).is_ok(),
        free_bytes: vfs.map(|v| v.f_bavail.saturating_mul(v.f_frsize)),
        binaries_bytes: size(AGENT_BIN) + size(HELPER_BIN),
        image_layer: container && overlay,
    }
}

/// Elsewhere self-update is not available.
#[cfg(not(target_os = "linux"))]
#[must_use]
pub fn probe_host() -> HostProbe {
    HostProbe { curl: curl_installed(), ..HostProbe::default() }
}

/// This host's self-update capability (for the helper's `Capabilities`).
pub async fn capability() -> Option<forge_proto::facts::SelfUpdateCapability> {
    match tokio::task::spawn_blocking(probe_host).await {
        Ok(p) => Some(assess(&p)),
        Err(e) => {
            // Unknown is the honest answer: the server then treats the node
            // like an older agent and lets the node's own refusal decide.
            tracing::warn!(error = %e, "self-update capability probe failed");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn streamed_sha256_matches_the_one_shot_digest() -> TestResult {
        let dir = tempfile::tempdir()?;
        for len in [0usize, 1, 64 * 1024 - 1, 64 * 1024, 64 * 1024 + 1, 3 * 64 * 1024 + 17] {
            let data: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
            let p = dir.path().join(format!("f{len}"));
            std::fs::write(&p, &data)?;
            let want = hex::encode(ring::digest::digest(&ring::digest::SHA256, &data));
            assert_eq!(sha256_nofollow(&p, u64::MAX)?, (want, len as u64), "length {len}");
        }
        Ok(())
    }

    #[test]
    fn streamed_sha256_handles_a_large_sparse_file_and_enforces_the_cap() -> TestResult {
        let dir = tempfile::tempdir()?;
        let p = dir.path().join("sparse");
        let len = 300 * 1024 * 1024;
        std::fs::File::create(&p)?.set_len(len)?;
        let mut ctx = ring::digest::Context::new(&ring::digest::SHA256);
        let zeros = vec![0u8; 1024 * 1024];
        for _ in 0..300 {
            ctx.update(&zeros);
        }
        assert_eq!(sha256_nofollow(&p, len)?, (hex::encode(ctx.finish()), len));
        let err = sha256_nofollow(&p, len - 1).err().map(|e| e.kind());
        assert_eq!(err, Some(std::io::ErrorKind::FileTooLarge));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn streamed_sha256_does_not_follow_a_link() -> TestResult {
        let dir = tempfile::tempdir()?;
        let target = dir.path().join("t");
        std::fs::write(&target, b"x")?;
        let link = dir.path().join("l");
        std::os::unix::fs::symlink(&target, &link)?;
        assert!(sha256_nofollow(&link, u64::MAX).is_err());
        Ok(())
    }

    fn spec() -> UpdateSpec {
        UpdateSpec {
            version: "0.2.0".into(),
            download_base: "https://forge.example.com".into(),
            arch: "x86_64".into(),
            agent_sha256: "a".repeat(64),
            helper_sha256: "b".repeat(64),
            allow_downgrade: false,
        }
    }

    #[test]
    fn the_capability_names_every_blocker_and_warns_about_image_layers() {
        use forge_proto::facts::{Supervisor, UpdateIssue};
        let good = HostProbe {
            systemd: true,
            curl: true,
            bin_dir_writable: true,
            free_bytes: Some(1 << 30),
            binaries_bytes: 60 << 20,
            image_layer: false,
        };
        let c = assess(&good);
        assert!(c.ready && c.reasons.is_empty() && c.warnings.is_empty(), "{c:?}");
        assert_eq!(c.supervisor, Supervisor::Systemd);
        let bad = HostProbe {
            systemd: false,
            curl: false,
            bin_dir_writable: false,
            free_bytes: Some(100 << 20),
            binaries_bytes: 60 << 20,
            image_layer: true,
        };
        let c = assess(&bad);
        assert!(!c.ready);
        assert_eq!(c.supervisor, Supervisor::None);
        assert_eq!(
            c.reasons,
            [UpdateIssue::NoSystemd, UpdateIssue::NoCurl, UpdateIssue::BinDirNotWritable, UpdateIssue::LowDisk]
        );
        assert_eq!(c.warnings, [UpdateIssue::EphemeralContainer]);
        // Unknown free space is not a blocker.
        assert!(assess(&HostProbe { free_bytes: None, ..good }).ready);
    }

    #[test]
    fn specs_are_validated() {
        assert!(validate(&spec()).is_ok());
        assert!(validate(&UpdateSpec { agent_sha256: "zz".into(), ..spec() }).is_err());
        assert!(validate(&UpdateSpec { arch: "mips".into(), ..spec() }).is_err());
        assert!(validate(&UpdateSpec { version: "1.0; rm -rf /".into(), ..spec() }).is_err());
        assert!(validate(&UpdateSpec { download_base: "file:///etc".into(), ..spec() }).is_err());
        assert!(validate(&UpdateSpec { download_base: "https://x/?a=b".into(), ..spec() }).is_err());
    }

    fn dirs() -> (tempfile::TempDir, tempfile::TempDir, PathBuf) {
        let agent = tempfile::tempdir().unwrap_or_else(|_| std::process::abort());
        let root = tempfile::tempdir().unwrap_or_else(|_| std::process::abort());
        let state = root.path().join("forge-agent-helper");
        (agent, root, state)
    }

    /// A layout in temporary folders: `bin/` for the binaries.
    fn layout(agent: &Path, root: &Path, state: &Path) -> Layout {
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).ok();
        Layout {
            agent_bin: bin.join("forge-agent"),
            helper_bin: bin.join("forge-agent-helper"),
            state: state.to_path_buf(),
            agent_dir: agent.to_path_buf(),
        }
    }

    fn pending() -> Pending {
        Pending {
            from: "0.0.1".into(),
            to: crate::VERSION.into(),
            started_ms: now_ms(),
            helper_starts: 0,
            agent_restarts_base: None,
        }
    }

    #[test]
    fn an_agent_crash_loop_is_counted_from_the_update() {
        let (agent, _root, state) = dirs();
        // No marker: never a crash loop.
        assert!(!agent_crash_loop_in(agent.path(), &state, 9));
        ensure_private_dir(&state).ok();
        // The unit had 7 restarts before the update; its restart resets the counter.
        save_pending(&Pending { agent_restarts_base: Some(7), ..pending() }, agent.path(), &state).ok();
        assert!(!agent_crash_loop_in(agent.path(), &state, 0));
        assert_eq!(read_marker(&state.join(PENDING)).and_then(|p| p.agent_restarts_base), Some(0));
        assert!(!agent_crash_loop_in(agent.path(), &state, MAX_STARTS - 1));
        assert!(agent_crash_loop_in(agent.path(), &state, MAX_STARTS));
        // A marker without a base (older helper) starts counting at the first reading.
        save_pending(&pending(), agent.path(), &state).ok();
        assert!(!agent_crash_loop_in(agent.path(), &state, 5));
        assert!(!agent_crash_loop_in(agent.path(), &state, 5 + MAX_STARTS - 1));
        assert!(agent_crash_loop_in(agent.path(), &state, 5 + MAX_STARTS));
    }

    #[test]
    fn confirm_and_watchdog_use_the_marker() {
        let (agent, _root, state) = dirs();
        // No marker: nothing to do.
        let (l, host) = (layout(agent.path(), _root.path(), &state), FakeHost::default());
        assert_eq!(confirm_in(&l).unwrap_or_default()["confirmed"], false);
        assert!(watchdog_start_in(&l, &host).is_none());
        // A marker for this very version confirms; the agent sees its copy.
        ensure_private_dir(&state).ok();
        save_pending(&pending(), agent.path(), &state).ok();
        assert!(marker(agent.path()).exists());
        let wait = watchdog_start_in(&l, &host);
        assert!(wait.is_some_and(|w| w <= CONFIRM_WITHIN));
        assert_eq!(read_marker(&state.join(PENDING)).map(|p| p.helper_starts), Some(1));
        assert_eq!(confirm_in(&l).unwrap_or_default()["confirmed"], true);
        assert!(!marker(agent.path()).exists());
        assert!(!state.join(PENDING).exists());
    }

    #[test]
    fn a_marker_left_by_an_older_helper_is_adopted() {
        let (agent, root, state) = dirs();
        let l = layout(agent.path(), root.path(), &state);
        std::fs::write(marker(agent.path()), serde_json::to_vec(&pending()).unwrap_or_default()).ok();
        assert!(watchdog_start_in(&l, &FakeHost::default()).is_some());
        assert!(state.join(PENDING).exists());
        assert_eq!(confirm_in(&l).unwrap_or_default()["confirmed"], true);
    }

    #[cfg(unix)]
    #[test]
    fn markers_never_follow_a_link_planted_by_the_agent_user() {
        let (agent, root, state) = dirs();
        // The agent user points its marker (and the temp name) at a root file.
        let victim = root.path().join("victim");
        std::fs::write(&victim, "keep").ok();
        std::os::unix::fs::symlink(&victim, marker(agent.path())).ok();
        std::os::unix::fs::symlink(&victim, marker(agent.path()).with_extension("json.tmp")).ok();
        ensure_private_dir(&state).ok();
        save_pending(&pending(), agent.path(), &state).ok();
        assert_eq!(std::fs::read_to_string(&victim).unwrap_or_default(), "keep");
        assert!(std::fs::symlink_metadata(marker(agent.path())).is_ok_and(|m| m.is_file()));
        // A link as the agent's marker is not read through.
        let _ = std::fs::remove_file(marker(agent.path()));
        let _ = std::fs::remove_file(state.join(PENDING));
        std::fs::write(&victim, serde_json::to_vec(&pending()).unwrap_or_default()).ok();
        std::os::unix::fs::symlink(&victim, marker(agent.path())).ok();
        assert!(read_marker(&marker(agent.path())).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn the_helper_state_folder_must_be_a_real_private_directory() {
        use std::os::unix::fs::PermissionsExt;
        let (_agent, root, state) = dirs();
        let elsewhere = root.path().join("elsewhere");
        std::fs::create_dir(&elsewhere).ok();
        std::os::unix::fs::symlink(&elsewhere, &state).ok();
        assert!(ensure_private_dir(&state).is_err());
        let _ = std::fs::remove_file(&state);
        ensure_private_dir(&state).unwrap_or_else(|e| panic!("{e}"));
        let mode = std::fs::metadata(&state).map(|m| m.permissions().mode() & 0o777).unwrap_or_default();
        assert_eq!(mode, 0o700);
    }

    // ------------------------------------------------------------ swap

    /// The node, faked: downloads come from `files` by binary name, `--version`
    /// prints from `versions` by binary name, restarts are counted.
    #[derive(Debug, Default)]
    struct FakeHost {
        files: std::collections::HashMap<&'static str, Vec<u8>>,
        versions: std::collections::HashMap<&'static str, String>,
        unsupervised: bool,
        downloads: std::sync::atomic::AtomicUsize,
        restarts: std::sync::atomic::AtomicUsize,
    }

    impl UpdateHost for FakeHost {
        fn supervised(&self) -> bool {
            !self.unsupervised
        }
        fn download(&self, url: &str, to: &Path) -> Result<(), CommandError> {
            self.downloads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let name = url.rsplit('/').next().unwrap_or_default();
            let body = self.files.get(name).ok_or_else(|| CommandError::from(format!("404 {url}")))?;
            std::fs::write(to, body).map_err(|e| CommandError::from(e.to_string()))
        }
        fn report_version(&self, bin: &Path) -> Result<String, String> {
            let name = bin.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            self.versions.get(name.as_str()).cloned().ok_or_else(|| format!("{name} crashed"))
        }
        fn restart(&self) -> Result<(), String> {
            self.restarts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
        fn agent_restarts(&self) -> Option<u32> {
            Some(0)
        }
    }

    const NEW: &str = "99.0.0";

    fn sha(b: &[u8]) -> String {
        hex::encode(ring::digest::digest(&ring::digest::SHA256, b))
    }

    /// A node with the old binaries installed, and a host offering new ones.
    fn node() -> (tempfile::TempDir, tempfile::TempDir, Layout, FakeHost, UpdateSpec) {
        let (agent, root, state) = dirs();
        let l = layout(agent.path(), root.path(), &state);
        std::fs::write(&l.agent_bin, b"old agent").ok();
        std::fs::write(&l.helper_bin, b"old helper").ok();
        let host = FakeHost {
            files: [("forge-agent", b"new agent".to_vec()), ("forge-agent-helper", b"new helper".to_vec())].into(),
            versions: [
                ("forge-agent", format!("forge-agent {NEW}\n")),
                ("forge-agent-helper", format!("forge-agent-helper {NEW}\n")),
            ]
            .into(),
            ..FakeHost::default()
        };
        let u = UpdateSpec {
            version: NEW.into(),
            agent_sha256: sha(b"new agent"),
            helper_sha256: sha(b"new helper"),
            ..spec()
        };
        (agent, root, l, host, u)
    }

    fn installed(l: &Layout) -> (String, String) {
        let r = |p: &Path| std::fs::read_to_string(p).unwrap_or_default();
        (r(&l.agent_bin), r(&l.helper_bin))
    }

    fn leftovers(l: &Layout) -> Vec<String> {
        let mut out = Vec::new();
        for dir in [l.agent_bin.parent().unwrap_or(Path::new("/")), l.state.as_path()] {
            for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                let n = e.file_name().to_string_lossy().into_owned();
                if n.ends_with(".new") || n.starts_with("update-staging") || n == INTENT {
                    out.push(n);
                }
            }
        }
        out
    }

    #[test]
    fn an_update_swaps_both_binaries_keeps_prev_and_restarts() -> TestResult {
        let (_a, _r, l, host, u) = node();
        let out = apply_in(&u, &l, &host).map_err(|e| e.message)?;
        assert_eq!(out["updated"], true);
        assert_eq!(installed(&l), ("new agent".into(), "new helper".into()));
        assert_eq!(std::fs::read_to_string(prev(&l.agent_bin))?, "old agent");
        assert_eq!(std::fs::read_to_string(prev(&l.helper_bin))?, "old helper");
        assert_eq!(read_marker(&l.state.join(PENDING)).map(|p| p.to), Some(NEW.to_owned()));
        assert!(marker(&l.agent_dir).exists());
        assert!(leftovers(&l).is_empty(), "{:?}", leftovers(&l));
        assert_eq!(host.restarts.load(std::sync::atomic::Ordering::SeqCst), 1);
        Ok(())
    }

    #[test]
    fn a_checksum_mismatch_installs_nothing() {
        let (_a, _r, l, host, u) = node();
        let e = apply_in(&UpdateSpec { helper_sha256: "c".repeat(64), ..u }, &l, &host).err();
        assert!(e.is_some_and(|e| e.message.contains("checksum mismatch")));
        assert_eq!(installed(&l), ("old agent".into(), "old helper".into()));
        assert!(leftovers(&l).is_empty() && !l.state.join(PENDING).exists());
        assert_eq!(host.restarts.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[test]
    fn a_helper_that_fails_its_dry_run_installs_nothing() {
        let (_a, _r, l, mut host, u) = node();
        host.versions.remove("forge-agent-helper");
        let e = apply_in(&u, &l, &host).err();
        assert!(e.is_some_and(|e| e.message.contains("crashed")));
        // A helper reporting another version is refused too.
        host.versions.insert("forge-agent-helper", "forge-agent-helper 1.0.0".into());
        let e = apply_in(&u, &l, &host).err();
        assert!(e.is_some_and(|e| e.message.contains("forge-agent-helper")));
        assert_eq!(installed(&l), ("old agent".into(), "old helper".into()));
        assert!(leftovers(&l).is_empty());
    }

    #[test]
    fn a_second_update_while_one_runs_is_busy() -> TestResult {
        let (_a, _r, l, host, u) = node();
        ensure_private_dir(&l.state)?;
        let held = UpdateLock::acquire(&l.state).map_err(|e| e.message)?;
        let e = apply_in(&u, &l, &host).err();
        assert_eq!(e.and_then(|e| e.code), Some(RefusalCode::Busy));
        drop(held);
        assert!(apply_in(&u, &l, &host).is_ok());
        Ok(())
    }

    #[test]
    fn installed_hashes_make_a_retry_a_no_op() -> TestResult {
        let (_a, _r, l, host, u) = node();
        apply_in(&u, &l, &host).map_err(|e| e.message)?;
        let downloads = host.downloads.load(std::sync::atomic::Ordering::SeqCst);
        // Same command again (a retry): nothing downloaded or swapped; the
        // pending restart is asked for again.
        let again = apply_in(&u, &l, &host).map_err(|e| e.message)?;
        assert_eq!((again["updated"].as_bool(), again["restarting"].as_bool()), (Some(false), Some(true)));
        assert_eq!(host.downloads.load(std::sync::atomic::Ordering::SeqCst), downloads);
        assert_eq!(std::fs::read_to_string(prev(&l.agent_bin))?, "old agent", "the backup is kept");
        Ok(())
    }

    #[test]
    fn an_older_version_needs_allow_downgrade() {
        let (_a, _r, l, mut host, u) = node();
        let old = UpdateSpec { version: "0.0.0".into(), ..u };
        host.versions = [
            ("forge-agent", "forge-agent 0.0.0".to_owned()),
            ("forge-agent-helper", "forge-agent-helper 0.0.0".to_owned()),
        ]
        .into();
        let e = apply_in(&old, &l, &host).err();
        assert_eq!(e.and_then(|e| e.code), Some(RefusalCode::Downgrade));
        assert_eq!(installed(&l), ("old agent".into(), "old helper".into()));
        assert!(apply_in(&UpdateSpec { allow_downgrade: true, ..old }, &l, &host).is_ok());
        assert_eq!(installed(&l), ("new agent".into(), "new helper".into()));
    }

    #[test]
    fn without_systemd_the_update_is_unsupported() {
        let (_a, _r, l, mut host, u) = node();
        host.unsupervised = true;
        assert_eq!(apply_in(&u, &l, &host).err().and_then(|e| e.code), Some(RefusalCode::Unsupported));
    }

    /// The state a crash leaves after the helper was swapped and before the
    /// agent was: the intent marker, the new helper (old one as `.prev`), the
    /// old agent.
    fn crash_after_helper_swap(l: &Layout, u: &UpdateSpec) -> TestResult {
        ensure_private_dir(&l.state)?;
        let intent = Intent {
            from: "1.0.0".into(),
            to: u.version.clone(),
            files: vec![
                IntentFile { path: l.helper_bin.clone(), old: Some(sha(b"old helper")), new: u.helper_sha256.clone() },
                IntentFile { path: l.agent_bin.clone(), old: Some(sha(b"old agent")), new: u.agent_sha256.clone() },
            ],
        };
        write_nofollow(&l.state.join(INTENT), &serde_json::to_vec(&intent)?)?;
        let staged = l.state.join("staged-helper");
        std::fs::write(&staged, b"new helper")?;
        install_file(&staged, &l.helper_bin, &u.helper_sha256)?;
        Ok(())
    }

    #[test]
    fn a_crash_after_the_helper_swap_is_undone_at_start() -> TestResult {
        let (_a, _r, l, host, u) = node();
        crash_after_helper_swap(&l, &u)?;
        assert_eq!(installed(&l), ("old agent".into(), "new helper".into()));
        assert_eq!(recover_in(&l, &host), Some("reverted"));
        assert_eq!(installed(&l), ("old agent".into(), "old helper".into()));
        assert!(!l.state.join(INTENT).exists() && !l.state.join(PENDING).exists());
        assert_eq!(host.restarts.load(std::sync::atomic::Ordering::SeqCst), 1, "the reverted helper restarts");
        // Nothing left to recover; the retried update then goes through.
        assert_eq!(recover_in(&l, &host), None);
        apply_in(&u, &l, &host).map_err(|e| e.message)?;
        assert_eq!(installed(&l), ("new agent".into(), "new helper".into()));
        Ok(())
    }

    #[test]
    fn a_crash_after_both_swaps_is_finished_at_start() -> TestResult {
        let (_a, _r, l, host, u) = node();
        crash_after_helper_swap(&l, &u)?;
        let staged = l.state.join("staged-agent");
        std::fs::write(&staged, b"new agent")?;
        install_file(&staged, &l.agent_bin, &u.agent_sha256)?;
        // Before the pending marker: the update completes.
        assert_eq!(recover_in(&l, &host), Some("finished"));
        assert_eq!(installed(&l), ("new agent".into(), "new helper".into()));
        assert_eq!(read_marker(&l.state.join(PENDING)).map(|p| p.to), Some(NEW.to_owned()));
        assert!(!l.state.join(INTENT).exists());
        Ok(())
    }

    #[test]
    fn confirm_waits_while_a_swap_is_half_done_or_an_update_runs() -> TestResult {
        let (_a, _r, l, host, u) = node();
        crash_after_helper_swap(&l, &u)?;
        // The agent user plants its own marker copy and asks to confirm:
        // the `.prev` copies recovery needs must stay.
        std::fs::write(marker(&l.agent_dir), serde_json::to_vec(&pending())?)?;
        assert_eq!(confirm_in(&l)?["confirmed"], false);
        assert!(prev(&l.helper_bin).exists());
        assert_eq!(recover_in(&l, &host), Some("reverted"));
        let held = UpdateLock::acquire(&l.state).map_err(|e| e.message)?;
        assert_eq!(confirm_in(&l)?["confirmed"], false);
        assert!(!rollback_in(&l, &host, "test"), "a rollback waits for the lock");
        drop(held);
        Ok(())
    }

    #[test]
    fn undoing_a_swap_removes_a_file_the_update_added() -> TestResult {
        let (_a, _r, l, host, u) = node();
        ensure_private_dir(&l.state)?;
        std::fs::remove_file(&l.helper_bin)?;
        let intent = Intent {
            from: "1.0.0".into(),
            to: u.version.clone(),
            files: vec![
                IntentFile { path: l.helper_bin.clone(), old: None, new: u.helper_sha256.clone() },
                IntentFile { path: l.agent_bin.clone(), old: Some(sha(b"old agent")), new: u.agent_sha256.clone() },
            ],
        };
        write_nofollow(&l.state.join(INTENT), &serde_json::to_vec(&intent)?)?;
        let staged = l.state.join("staged-helper");
        std::fs::write(&staged, b"new helper")?;
        install_file(&staged, &l.helper_bin, &u.helper_sha256)?;
        assert_eq!(recover_in(&l, &host), Some("reverted"));
        assert!(!l.helper_bin.exists());
        assert_eq!(std::fs::read_to_string(&l.agent_bin)?, "old agent");
        Ok(())
    }

    #[test]
    fn a_retried_command_resumes_an_interrupted_swap() -> TestResult {
        // The helper crashed mid-swap and the server sends the update again:
        // the half swap is undone first, then the update runs normally.
        let (_a, _r, l, host, u) = node();
        crash_after_helper_swap(&l, &u)?;
        let out = apply_in(&u, &l, &host).map_err(|e| e.message)?;
        assert_eq!(out["updated"], true);
        assert_eq!(installed(&l), ("new agent".into(), "new helper".into()));
        assert_eq!(std::fs::read_to_string(prev(&l.helper_bin))?, "old helper");
        assert!(leftovers(&l).is_empty());
        Ok(())
    }
}
