//! Self-update (DECISIONS.md D-037).
//!
//! A release publishes per-platform binaries, `SHA256SUMS` and
//! `SHA256SUMS.sig` (Ed25519 over the sums file, base64). The public key is
//! compiled in (`RELEASE_PUBLIC_KEY`, overridable in `[update]`). Applying an
//! update:
//!
//! 1. downloads the release's `SHA256SUMS` and checks its signature;
//! 2. downloads this platform's server and plugin runner and every agent
//!    binary handed to nodes, each checked against the signed sums;
//! 3. dry-runs the new server (`--version`, `config`, `check-migrations`)
//!    against this config: it must read it and know every migration the
//!    database has applied (D-098);
//! 4. backs up the database (SQLite `VACUUM INTO data/backups/…`);
//! 5. swaps the binaries (the old ones stay as `*.prev`), writes the agent
//!    folder's `MANIFEST.json` (the agents' version and signed hashes) and
//!    records the update in `data/updates/applied.json`;
//! 6. stops gracefully and starts the new binary in place (Unix `exec`,
//!    Windows: a new process).
//!
//! The new binary counts its start attempts in `applied.json`; after three
//! starts that never reached "serving", it restores `*.prev` (the agent
//! folder too) and starts that.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use forge_sdk::release::{
    DEFAULT_FEED, RELEASE_PUBLIC_KEY, compare_release, is_prerelease, parse_sums, verify_signature,
};

/// The Rust target this binary was built for (release asset names).
#[must_use]
pub fn target() -> &'static str {
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        "x86_64-unknown-linux-musl"
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        "aarch64-unknown-linux-musl"
    } else if cfg!(all(windows, target_arch = "x86_64")) {
        "x86_64-pc-windows-msvc"
    } else {
        "unsupported"
    }
}

const EXE: &str = if cfg!(windows) { ".exe" } else { "" };

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Release {
    pub version: String,
    pub notes: String,
    pub url: String,
    pub published_at: Option<String>,
    /// Asset name → download URL.
    pub assets: std::collections::BTreeMap<String, String>,
}

/// What the console shows.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Status {
    pub current: &'static str,
    pub latest: Option<Release>,
    pub available: bool,
    pub checked_at: Option<i64>,
    /// `idle`, `downloading`, `verifying`, `installing`, `restarting`, `failed`.
    pub state: String,
    pub message: Option<String>,
    pub enabled: bool,
}

fn status_cell() -> &'static Mutex<Status> {
    static S: OnceLock<Mutex<Status>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(Status { current: crate::VERSION, state: "idle".into(), ..Status::default() }))
}

fn with<R>(f: impl FnOnce(&mut Status) -> R) -> R {
    let mut g = status_cell().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    f(&mut g)
}

#[must_use]
pub fn status() -> Status {
    with(|s| s.clone())
}

fn set_state(state: &str, message: Option<String>) {
    with(|s| {
        s.state = state.to_owned();
        s.message = message;
    });
}

/// Fires when an installed update wants the process to restart.
#[must_use]
pub fn restart_signal() -> &'static tokio::sync::Notify {
    static N: OnceLock<tokio::sync::Notify> = OnceLock::new();
    N.get_or_init(tokio::sync::Notify::new)
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(600))
        .connect_timeout(Duration::from_secs(15))
        .user_agent(concat!("orbyn-forge/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())
}

/// Parses a GitHub "latest release" document.
#[must_use]
pub fn parse_release(doc: &Value) -> Option<Release> {
    let tag = doc.get("tag_name")?.as_str()?;
    let version = tag.trim_start_matches('v').to_owned();
    if !forge_catalog::versions::valid_build(&version) {
        return None;
    }
    let assets = doc
        .get("assets")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| {
                    Some((x.get("name")?.as_str()?.to_owned(), x.get("browser_download_url")?.as_str()?.to_owned()))
                })
                .collect()
        })
        .unwrap_or_default();
    Some(Release {
        version,
        notes: doc.get("body").and_then(Value::as_str).unwrap_or_default().chars().take(20_000).collect(),
        // The console links to it: only an https page, never `javascript:`
        // or another scheme from a feed (the feed's JSON is not signed).
        url: doc
            .get("html_url")
            .and_then(Value::as_str)
            .filter(|u| u.starts_with("https://") && u.len() <= 2048 && !u.chars().any(char::is_control))
            .unwrap_or_default()
            .to_owned(),
        published_at: doc.get("published_at").and_then(Value::as_str).map(str::to_owned),
        assets,
    })
}

/// Whether `latest` is newer than this build ([`compare_release`]: a
/// release candidate or dev build of `X.Y.Z` is older than `X.Y.Z`).
#[must_use]
pub fn is_newer(latest: &str) -> bool {
    compare_release(latest, crate::VERSION) == std::cmp::Ordering::Greater
}

/// The GitHub-compatible "list releases" URL: `[update] releases_url`, or
/// `feed_url` without its trailing `/latest`.
#[must_use]
pub fn releases_url(feed: &str, configured: Option<&str>) -> String {
    if let Some(u) = configured.map(str::trim).filter(|u| !u.is_empty()) {
        return u.to_owned();
    }
    let f = feed.trim_end_matches('/');
    f.strip_suffix("/latest").unwrap_or(f).to_owned()
}

/// The releases of a GitHub "list releases" document that this build may
/// upgrade to, or runs: newer than or equal to `current`, newest first.
/// Drafts and prereleases are dropped unless `current` is a prerelease
/// itself (local dev builds are `-dev.…`). Older versions are never offered.
#[must_use]
pub fn select_releases(docs: &[Value], current: &str) -> Vec<Release> {
    let pre_ok = is_prerelease(current);
    let mut out: Vec<Release> = docs
        .iter()
        .filter(|d| !d.get("draft").and_then(Value::as_bool).unwrap_or(false))
        .filter(|d| pre_ok || !d.get("prerelease").and_then(Value::as_bool).unwrap_or(false))
        .filter_map(parse_release)
        .filter(|r| pre_ok || !is_prerelease(&r.version))
        .filter(|r| compare_release(&r.version, current) != std::cmp::Ordering::Less)
        .collect();
    out.sort_by(|a, b| compare_release(&b.version, &a.version));
    out.dedup_by(|a, b| a.version == b.version);
    out
}

async fn get_json(url: &str) -> Result<Value, String> {
    client()?
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())
}

/// Releases this build may upgrade to (or runs), newest first.
pub async fn list_releases(list_url: &str) -> Result<Vec<Release>, String> {
    let sep = if list_url.contains('?') { '&' } else { '?' };
    let doc = get_json(&format!("{list_url}{sep}per_page=50")).await?;
    let docs = doc.as_array().ok_or("the release feed did not return a list of releases")?;
    Ok(select_releases(docs, crate::VERSION))
}

/// One release by version (`<list>/tags/v<version>`).
pub async fn release_by_tag(list_url: &str, version: &str) -> Result<Release, String> {
    if !forge_catalog::versions::valid_build(version) {
        return Err(format!("`{version}` is not a version"));
    }
    let doc = get_json(&format!("{}/tags/v{version}", list_url.trim_end_matches('/'))).await?;
    let rel = parse_release(&doc).ok_or("the release feed returned no usable release")?;
    if rel.version != version {
        return Err(format!("the release feed returned {} for {version}", rel.version));
    }
    Ok(rel)
}

/// Asks the release feed for the newest version.
pub async fn check(feed: &str, now_ms: i64) -> Result<Status, String> {
    let doc: Value = client()?
        .get(feed)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    let rel = parse_release(&doc).ok_or("the release feed returned no usable release")?;
    Ok(with(|s| {
        s.available = is_newer(&rel.version);
        s.latest = Some(rel);
        s.checked_at = Some(now_ms);
        s.clone()
    }))
}

async fn download(c: &reqwest::Client, url: &str) -> Result<Vec<u8>, String> {
    let r =
        c.get(url).send().await.map_err(|e| format!("{url}: {e}"))?.error_for_status().map_err(|e| e.to_string())?;
    Ok(r.bytes().await.map_err(|e| e.to_string())?.to_vec())
}

fn sha256_hex(b: &[u8]) -> String {
    forge_sdk::release::sha256_hex(b)
}

/// Files of a release this node needs: (asset name, where it goes, executable).
fn plan(rel: &Release, exe: &Path, dist: &Path) -> Result<Vec<(String, PathBuf)>, String> {
    let t = target();
    if t == "unsupported" {
        return Err("self-update is not available for this platform".into());
    }
    let dir = exe.parent().ok_or("cannot locate the server binary")?;
    let mut files = vec![
        (format!("forge-server-{t}{EXE}"), exe.to_path_buf()),
        (format!("forge-plugin-runner-{t}{EXE}"), dir.join(format!("forge-plugin-runner{EXE}"))),
        (format!("forge-analytics-worker-{t}{EXE}"), dir.join(format!("forge-analytics-worker{EXE}"))),
    ];
    // Agent binaries handed to new and updated nodes.
    for name in rel.assets.keys().filter(|n| n.starts_with("forge-agent-")) {
        files.push((name.clone(), dist.join(name)));
    }
    for (asset, _) in &files[..1] {
        if !rel.assets.contains_key(asset) {
            return Err(format!("release {} has no {asset}", rel.version));
        }
    }
    files.retain(|(a, _)| rel.assets.contains_key(a));
    Ok(files)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Applied {
    pub from: String,
    pub to: String,
    pub at: i64,
    /// Start attempts of the new version that did not reach "serving".
    #[serde(default)]
    pub attempts: u32,
    #[serde(default)]
    pub backup: Option<String>,
    /// What to do with the agents once the new server serves.
    #[serde(default)]
    pub agents: Option<AgentFollowUp>,
    /// Names of the files replaced in the agent folder (binaries,
    /// `MANIFEST.json`, `SHA256SUMS`). A rollback restores each `*.prev`, or
    /// removes the file when it had none, so the folder matches the restored
    /// server again. Only names of [`dist_file_allowed`] in the configured
    /// folder are touched.
    #[serde(default)]
    pub dist_files: Vec<String>,
}

/// The agent follow-up requested with an update (D-098): after the new
/// server serves, a canary rollout of outdated Linux agents to its version,
/// as the person who applied the update (re-authorized per node).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentFollowUp {
    /// `rollout` or `none`.
    pub mode: String,
    /// Nodes per batch (the first is the canary).
    pub batch: usize,
    pub principal: forge_core::Principal,
    pub context: forge_core::AuthzContext,
}

fn applied_path(data_dir: &Path) -> PathBuf {
    data_dir.join("updates").join("applied.json")
}

/// The installed binary path for a running executable path. After an update
/// swaps the binaries, the OS reports the running file under its new name
/// (`forge-server.prev`, or `… (deleted)` on Linux); the installed path is
/// the one without that suffix. Restarts and later updates use this path,
/// never the `.prev` copy.
#[must_use]
pub fn installed_path(running: &Path) -> PathBuf {
    let mut s = running.as_os_str().to_string_lossy().into_owned();
    if let Some(t) = s.strip_suffix(" (deleted)") {
        s = t.to_owned();
    }
    while let Some(t) = s.strip_suffix(".prev") {
        s = t.to_owned();
    }
    PathBuf::from(s)
}

/// The installed server binary, resolved once (call [`remember_install_path`]
/// at startup, before any update can rename the running file).
fn install_exe() -> Result<PathBuf, String> {
    static P: OnceLock<Option<PathBuf>> = OnceLock::new();
    restart_path(P.get_or_init(|| std::env::current_exe().ok().map(|p| installed_path(&p))).clone())
}

/// The path to restart from: only an absolute, resolved path. A bare name
/// would be looked up through `PATH` and could start another binary, so an
/// unknown path is an error, never a fallback.
fn restart_path(resolved: Option<PathBuf>) -> Result<PathBuf, String> {
    match resolved {
        Some(p) if p.is_absolute() => Ok(p),
        _ => Err("cannot determine where forge-server is installed, so it was not restarted; \n                  restart the forge-server service to start the new version"
            .to_owned()),
    }
}

/// The binary to restart into after an update. When it cannot be
/// determined, marks the update failed (the caller audits it and keeps
/// serving) instead of guessing.
pub fn restart_target() -> Result<PathBuf, String> {
    install_exe().inspect_err(|e| fail(e.clone()))
}

/// Resolves and remembers the installed binary path. Call first in `main`.
pub fn remember_install_path() {
    let _ = install_exe();
}

fn prev(p: &Path) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(".prev");
    PathBuf::from(s)
}

/// Moves `src` to `dest`, keeping the current `dest` as `dest.prev`. A
/// running binary can be renamed (Linux and Windows), not overwritten.
fn swap_in(src: &Path, dest: &Path) -> Result<(), String> {
    if dest.exists() {
        let _ = std::fs::remove_file(prev(dest));
        std::fs::rename(dest, prev(dest)).map_err(|e| format!("{}: {e}", dest.display()))?;
    } else {
        // No earlier copy: a stale `.prev` must not be restored on rollback.
        let _ = std::fs::remove_file(prev(dest));
    }
    if std::fs::rename(src, dest).is_err() {
        std::fs::copy(src, dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    }
    Ok(())
}

/// Whether `name` is a file the self-updater may replace in the agent
/// folder: an agent binary, `MANIFEST.json` or `SHA256SUMS`. A plain name,
/// never a path.
fn dist_file_allowed(name: &str) -> bool {
    !name.contains(['/', '\\'])
        && !name.contains("..")
        && (name.starts_with("forge-agent-") || name == crate::install::MANIFEST || name == "SHA256SUMS")
}

/// Puts back what an update replaced: the server binaries' `*.prev`, and
/// every agent-folder file it recorded (its `*.prev`, or removed when the
/// update added it). Returns the files it could not restore.
fn restore_previous(a: &Applied, exe: &Path, dist: &Path) -> Vec<String> {
    let dir = exe.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut failed = Vec::new();
    for f in [
        exe.to_path_buf(),
        dir.join(format!("forge-plugin-runner{EXE}")),
        dir.join(format!("forge-analytics-worker{EXE}")),
    ] {
        if prev(&f).exists() && std::fs::rename(prev(&f), &f).is_err() {
            failed.push(f.display().to_string());
        }
    }
    for name in &a.dist_files {
        if !dist_file_allowed(name) {
            failed.push(format!("{name} (not an agent-folder file)"));
            continue;
        }
        let f = dist.join(name);
        let r = if prev(&f).exists() {
            std::fs::rename(prev(&f), &f)
        } else {
            match std::fs::remove_file(&f) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            }
        };
        if r.is_err() {
            failed.push(f.display().to_string());
        }
    }
    failed
}

/// Downloads, verifies and installs `rel`. On success the caller restarts.
#[allow(clippy::too_many_arguments)]
pub async fn apply(
    rel: &Release,
    public_key: &str,
    data_dir: &Path,
    dist: &Path,
    config_path: Option<&Path>,
    backup: impl std::future::Future<Output = Result<Option<PathBuf>, String>>,
    agents: Option<AgentFollowUp>,
    now_ms: i64,
) -> Result<Applied, String> {
    if public_key.contains("PLACEHOLDER") {
        return Err("this build has no release signing key; install updates by hand".into());
    }
    let exe = install_exe()?;
    let files = plan(rel, &exe, dist)?;
    let c = client()?;
    set_state("downloading", Some(format!("Downloading {}", rel.version)));
    let sums_url = rel.assets.get("SHA256SUMS").ok_or("the release has no SHA256SUMS")?;
    let sig_url = rel.assets.get("SHA256SUMS.sig").ok_or("the release has no SHA256SUMS.sig")?;
    let sums_raw = download(&c, sums_url).await?;
    let sig = String::from_utf8_lossy(&download(&c, sig_url).await?).into_owned();
    set_state("verifying", Some("Checking the release signature".into()));
    verify_signature(public_key, &sums_raw, &sig)?;
    let sums = parse_sums(&String::from_utf8_lossy(&sums_raw));

    let stage = data_dir.join("updates").join(&rel.version);
    std::fs::create_dir_all(&stage).map_err(|e| e.to_string())?;
    let mut staged = Vec::new();
    for (asset, dest) in &files {
        set_state("downloading", Some(format!("Downloading {asset}")));
        let bytes = download(&c, &rel.assets[asset]).await?;
        let want = sums.get(asset).ok_or_else(|| format!("{asset} is not listed in the signed SHA256SUMS"))?;
        if &sha256_hex(&bytes) != want {
            return Err(format!("{asset}: checksum mismatch"));
        }
        let p = stage.join(asset);
        std::fs::write(&p, &bytes).map_err(|e| format!("{}: {e}", p.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755));
        }
        staged.push((p, dest.clone()));
    }

    // Dry run: the new server must start far enough to read this config.
    set_state("verifying", Some("Trying the new version".into()));
    let new_server = &staged[0].0;
    let out = std::process::Command::new(new_server)
        .arg("--version")
        .output()
        .map_err(|e| format!("cannot run the new server: {e}"))?;
    let printed = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() || !printed.contains(&rel.version) {
        return Err(format!("the new server reports `{}`, not {}", printed.trim(), rel.version));
    }
    let mut cfg = std::process::Command::new(new_server);
    if let Some(p) = config_path {
        cfg.arg("--config").arg(p);
    }
    let out = cfg.arg("config").output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!(
            "the new server cannot read this configuration: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    // It must know every migration the database has applied, or it would
    // refuse to start after the swap (D-098).
    let mut mig = std::process::Command::new(new_server);
    if let Some(p) = config_path {
        mig.arg("--config").arg(p);
    }
    let out = mig.arg("check-migrations").output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!(
            "the new server does not know this database's schema: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }

    set_state("installing", Some("Backing up the database".into()));
    let backup = backup.await?;

    // The agent folder says which version its binaries are (MANIFEST.json,
    // from the signed sums) and keeps a plain checksum list. Both are built
    // before anything is swapped, and swapped (and restored on a rollback)
    // like the binaries.
    let agent_sums: std::collections::BTreeMap<String, String> = staged
        .iter()
        .filter(|(_, dest)| dest.starts_with(dist))
        .filter_map(|(_, dest)| {
            let name = dest.file_name()?.to_string_lossy().into_owned();
            Some((name.clone(), sums.get(&name)?.clone()))
        })
        .collect();
    let dist_meta = if agent_sums.is_empty() {
        None
    } else {
        let manifest = crate::install::DistManifest { version: rel.version.clone(), files: agent_sums };
        let meta = stage.join("dist-meta");
        std::fs::create_dir_all(&meta).map_err(|e| e.to_string())?;
        manifest.write(&meta)?;
        let list: String = manifest.files.iter().map(|(n, h)| format!("{h}  {n}\n")).collect();
        std::fs::write(meta.join("SHA256SUMS"), list).map_err(|e| e.to_string())?;
        Some(meta)
    };

    set_state("installing", Some("Installing".into()));
    let mut dist_files = Vec::new();
    for (src, dest) in &staged {
        if let Some(d) = dest.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        swap_in(src, dest)?;
        if dest.starts_with(dist)
            && let Some(name) = dest.file_name()
        {
            dist_files.push(name.to_string_lossy().into_owned());
        }
    }
    if let Some(meta) = &dist_meta {
        for name in [crate::install::MANIFEST, "SHA256SUMS"] {
            swap_in(&meta.join(name), &dist.join(name))?;
            dist_files.push(name.to_owned());
        }
    }
    let applied = Applied {
        from: crate::VERSION.to_owned(),
        to: rel.version.clone(),
        at: now_ms,
        attempts: 0,
        backup: backup.map(|p| p.display().to_string()),
        agents,
        dist_files,
    };
    let ap = applied_path(data_dir);
    std::fs::write(&ap, serde_json::to_vec_pretty(&applied).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_dir_all(&stage);
    set_state("restarting", Some(format!("Restarting into {}", rel.version)));
    Ok(applied)
}

/// Starts the (new) binary in place of this process. Unix: `exec` keeps the
/// PID, so systemd, Docker or a shell keep supervising it. Windows: a new
/// process with the same arguments, then this one exits.
pub fn restart_into_self(exe: &Path) -> ! {
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = std::process::Command::new(exe).args(&args).exec();
        eprintln!("forge-server: restart failed: {err}");
        std::process::exit(75);
    }
    #[cfg(not(unix))]
    {
        match std::process::Command::new(exe).args(&args).spawn() {
            Ok(_) => std::process::exit(0),
            Err(e) => {
                eprintln!("forge-server: restart failed: {e}");
                std::process::exit(75);
            }
        }
    }
}

/// Run before anything else at startup. Counts attempts of a freshly
/// installed version; after three failed starts it restores the previous
/// binaries and starts them.
pub fn startup_guard(data_dir: &Path, dist: &Path) {
    let ap = applied_path(data_dir);
    let Ok(raw) = std::fs::read(&ap) else { return };
    let Ok(mut a) = serde_json::from_slice::<Applied>(&raw) else { return };
    if a.to != crate::VERSION {
        return; // already rolled back, or replaced by hand
    }
    a.attempts += 1;
    let _ = std::fs::write(&ap, serde_json::to_vec_pretty(&a).unwrap_or_default());
    if a.attempts <= 3 {
        return;
    }
    eprintln!("forge-server: {} failed to start {} times; restoring {}", a.to, a.attempts - 1, a.from);
    let Ok(exe) = install_exe() else { return };
    for f in restore_previous(&a, &exe, dist) {
        eprintln!("forge-server: could not restore {f}");
    }
    let _ = std::fs::rename(&ap, ap.with_file_name("rolled-back.json"));
    restart_into_self(&exe);
}

/// Called once the server is serving: the update succeeded.
pub fn mark_serving(data_dir: &Path) -> Option<Applied> {
    let ap = applied_path(data_dir);
    let a: Applied = serde_json::from_slice(&std::fs::read(&ap).ok()?).ok()?;
    if a.to != crate::VERSION {
        return None;
    }
    let _ = std::fs::rename(&ap, ap.with_file_name("last-applied.json"));
    Some(a)
}

/// Called once the server is serving, after [`mark_serving`] found nothing:
/// if an update to another version was installed but this process still
/// runs the version it replaced, the restart did not start the new binary.
/// Records the failure in the update status (and `failed.json`) and returns
/// the update so the caller can log and audit it.
pub fn detect_stale_restart(data_dir: &Path) -> Option<Applied> {
    let ap = applied_path(data_dir);
    let a: Applied = serde_json::from_slice(&std::fs::read(&ap).ok()?).ok()?;
    if a.to == crate::VERSION || a.from != crate::VERSION {
        return None;
    }
    let _ = std::fs::rename(&ap, ap.with_file_name("failed.json"));
    fail(format!(
        "{} was installed, but the server restarted into {}. Restart the forge-server service to start {}.",
        a.to, a.from, a.to
    ));
    Some(a)
}

/// Marks a failed apply in the status (binaries were not touched).
pub fn fail(message: String) {
    set_state("failed", Some(message));
}

/// Release key tools for CI: `forge-server release keygen | sign <file>`.
pub mod keys {
    use base64::Engine;

    /// A new Ed25519 key: (private PKCS#8 base64, public base64url).
    pub fn generate() -> Result<(String, String), String> {
        use ring::signature::KeyPair;
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 = ring::signature::Ed25519KeyPair::generate_pkcs8(&rng).map_err(|_| "key generation failed")?;
        let kp = ring::signature::Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).map_err(|_| "key generation failed")?;
        Ok((
            base64::engine::general_purpose::STANDARD.encode(pkcs8.as_ref()),
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(kp.public_key().as_ref()),
        ))
    }

    /// The base64url public key of a base64 PKCS#8 private key.
    pub fn public(private_b64: &str) -> Result<String, String> {
        use ring::signature::KeyPair;
        let raw = base64::engine::general_purpose::STANDARD
            .decode(private_b64.trim())
            .map_err(|_| "the key is not base64")?;
        let kp =
            ring::signature::Ed25519KeyPair::from_pkcs8(&raw).map_err(|_| "the key is not an Ed25519 PKCS#8 key")?;
        Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(kp.public_key().as_ref()))
    }

    /// Signs `data` with a base64 PKCS#8 private key; returns the base64 signature.
    pub fn sign(private_b64: &str, data: &[u8]) -> Result<String, String> {
        let raw = base64::engine::general_purpose::STANDARD
            .decode(private_b64.trim())
            .map_err(|_| "the key is not base64")?;
        let kp =
            ring::signature::Ed25519KeyPair::from_pkcs8(&raw).map_err(|_| "the key is not an Ed25519 PKCS#8 key")?;
        Ok(base64::engine::general_purpose::STANDARD.encode(kp.sign(data).as_ref()))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn signatures_round_trip_and_reject_tampering() {
        let (private, public) = keys::generate().unwrap();
        let sums = b"abc  forge-server-x86_64-unknown-linux-musl\n";
        let sig = keys::sign(&private, sums).unwrap();
        assert!(verify_signature(&public, sums, &sig).is_ok());
        assert!(verify_signature(&public, b"tampered", &sig).is_err());
        let (_, other) = keys::generate().unwrap();
        assert!(verify_signature(&other, sums, &sig).is_err());
    }

    #[test]
    fn local_dev_builds_order_below_their_version_and_after_each_other() {
        use std::cmp::Ordering::{Greater, Less};
        // Dev builds are stamped <next version>-dev.<yyyyMMdd.HHmmss>: below
        // that release, above the one before it.
        assert_eq!(compare_release("0.1.1-dev.20261112.153000", "0.1.1"), Less);
        assert_eq!(compare_release("0.1.1-dev.20261112.153000", "0.1.0"), Greater);
        assert_eq!(compare_release("0.1.1-dev.20261112.153001", "0.1.1-dev.20261112.153000"), Greater);
        assert_eq!(compare_release("0.1.1-dev.20261113.090000", "0.1.1-dev.20261112.235959"), Greater);
        assert_eq!(compare_release("1.0.0-rc-01", "1.0.0"), Less);
    }

    #[test]
    fn parses_sums_and_releases() {
        let s = parse_sums(&format!(
            "{}  forge-server-x\n{} *forge-agent-linux-x86_64\nbad line\n",
            "a".repeat(64),
            "B".repeat(64)
        ));
        assert_eq!(s.len(), 2);
        assert_eq!(s["forge-agent-linux-x86_64"], "b".repeat(64));
        let doc = serde_json::json!({
            "tag_name": "v0.2.0", "body": "notes", "html_url": "https://x",
            "assets": [{ "name": "SHA256SUMS", "browser_download_url": "https://x/SHA256SUMS" }]
        });
        let r = parse_release(&doc).unwrap();
        // Security review: a feed cannot hand the console a script link.
        let mut evil = doc.clone();
        evil["html_url"] = serde_json::json!("javascript:alert(document.cookie)");
        assert_eq!(parse_release(&evil).unwrap().url, "");
        assert_eq!(r.version, "0.2.0");
        assert_eq!(r.assets["SHA256SUMS"], "https://x/SHA256SUMS");
        assert!(is_newer("99.0.0"));
        assert!(!is_newer("0.0.1"));
    }

    #[test]
    fn the_release_list_url_comes_from_the_feed() {
        let gh = "https://api.github.com/repos/o/r/releases/latest";
        assert_eq!(releases_url(gh, None), "https://api.github.com/repos/o/r/releases");
        assert_eq!(releases_url("http://feed:8080/releases/latest/", None), "http://feed:8080/releases");
        assert_eq!(releases_url(gh, Some("https://mirror/releases")), "https://mirror/releases");
        assert_eq!(releases_url(gh, Some("  ")), "https://api.github.com/repos/o/r/releases");
    }

    #[test]
    fn releases_are_newer_or_equal_and_newest_first() {
        let rel = |tag: &str| serde_json::json!({ "tag_name": tag, "assets": [] });
        let mut draft = rel("v9.0.0");
        draft["draft"] = serde_json::json!(true);
        let mut pre = rel("v8.0.0-rc.1");
        pre["prerelease"] = serde_json::json!(true);
        let docs = vec![rel("v1.2.0"), rel("v1.0.0"), rel("v2.0.0"), rel("v0.9.0"), draft, pre, rel("v1.10.0")];
        let v: Vec<String> = select_releases(&docs, "1.2.0").into_iter().map(|r| r.version).collect();
        assert_eq!(v, ["2.0.0", "1.10.0", "1.2.0"], "older never offered; drafts and prereleases dropped");
        // A release candidate sees the final release of its version.
        let rc = vec![rel("v1.0.0"), rel("v1.0.0-rc-02"), rel("v1.0.0-rc-01"), rel("v0.9.0")];
        let v: Vec<String> = select_releases(&rc, "1.0.0-rc-01").into_iter().map(|r| r.version).collect();
        assert_eq!(v, ["1.0.0", "1.0.0-rc-02", "1.0.0-rc-01"]);
        // A dev build sees dev builds.
        let dev = vec![
            rel("v0.1.0-dev.20261112.153000"),
            rel("v0.1.0-dev.20261113.090000"),
            rel("v0.1.0-dev.20261101.000000"),
        ];
        let v: Vec<String> =
            select_releases(&dev, "0.1.0-dev.20261112.153000").into_iter().map(|r| r.version).collect();
        assert_eq!(v, ["0.1.0-dev.20261113.090000", "0.1.0-dev.20261112.153000"]);
    }

    #[test]
    fn startup_guard_counts_attempts_then_gives_up() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("updates")).unwrap();
        let a = Applied {
            from: "0.0.9".into(),
            to: "0.0.1".into(),
            at: 0,
            attempts: 0,
            backup: None,
            agents: None,
            dist_files: Vec::new(),
        };
        std::fs::write(applied_path(dir.path()), serde_json::to_vec(&a).unwrap()).unwrap();
        // Another version's record is left alone.
        startup_guard(dir.path(), &dir.path().join("dist"));
        let back: Applied = serde_json::from_slice(&std::fs::read(applied_path(dir.path())).unwrap()).unwrap();
        assert_eq!(back.attempts, 0);
    }

    #[test]
    fn a_rollback_restores_the_agent_folder_with_the_server() {
        let dir = tempfile::tempdir().unwrap();
        let (bin, dist) = (dir.path().join("bin"), dir.path().join("dist"));
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&dist).unwrap();
        let exe = bin.join(format!("forge-server{EXE}"));
        let w = |p: &Path, s: &str| std::fs::write(p, s).unwrap();
        w(&exe, "old server");
        let agent = dist.join("forge-agent-linux-x86_64");
        w(&agent, "old agent");
        // An update over a folder without a manifest: the binaries and the
        // server are swapped, the manifest is new.
        let staged = dir.path().join("staged");
        std::fs::create_dir_all(&staged).unwrap();
        for (name, body) in [("server", "new server"), ("agent", "new agent"), ("manifest", "{}")] {
            w(&staged.join(name), body);
        }
        swap_in(&staged.join("server"), &exe).unwrap();
        swap_in(&staged.join("agent"), &agent).unwrap();
        let manifest = dist.join(crate::install::MANIFEST);
        swap_in(&staged.join("manifest"), &manifest).unwrap();
        assert_eq!(std::fs::read_to_string(&agent).unwrap(), "new agent");
        let a = Applied {
            from: "1.0.0".into(),
            to: "1.1.0".into(),
            at: 0,
            attempts: 4,
            backup: None,
            agents: None,
            dist_files: vec!["forge-agent-linux-x86_64".into(), crate::install::MANIFEST.into()],
        };
        assert!(restore_previous(&a, &exe, &dist).is_empty());
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "old server");
        assert_eq!(std::fs::read_to_string(&agent).unwrap(), "old agent");
        assert!(!manifest.exists(), "a manifest the update added is removed: the old agents are the old server's");
        assert!(!prev(&agent).exists());
        // A tampered record cannot reach outside the agent folder.
        let victim = dir.path().join("victim");
        w(&victim, "keep");
        let evil =
            Applied { dist_files: vec![victim.display().to_string(), "../victim".into(), "forge-server".into()], ..a };
        assert_eq!(restore_previous(&evil, &exe, &dist).len(), 3);
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep");
    }

    #[test]
    fn restarts_use_the_installed_path_not_the_prev_copy() {
        let p = |s: &str| installed_path(Path::new(s));
        assert_eq!(p("/opt/forge/forge-server"), PathBuf::from("/opt/forge/forge-server"));
        assert_eq!(p("/opt/forge/forge-server.prev"), PathBuf::from("/opt/forge/forge-server"));
        assert_eq!(p("/opt/forge/forge-server (deleted)"), PathBuf::from("/opt/forge/forge-server"));
        assert_eq!(p("/opt/forge/forge-server.prev (deleted)"), PathBuf::from("/opt/forge/forge-server"));
        assert_eq!(
            p(r"C:\Program Files\Forge\forge-server.exe.prev"),
            PathBuf::from(r"C:\Program Files\Forge\forge-server.exe")
        );
        assert_eq!(p(r"C:\Forge\forge-server.exe"), PathBuf::from(r"C:\Forge\forge-server.exe"));
    }

    #[test]
    fn an_unknown_install_path_is_refused_not_looked_up_in_path() {
        for bad in [None, Some(PathBuf::from("forge-server")), Some(PathBuf::from("forge-server.exe"))] {
            let e = restart_path(bad).unwrap_err();
            assert!(e.contains("restart the forge-server service"), "{e}");
        }
        let abs = std::env::temp_dir().join("forge-server");
        assert_eq!(restart_path(Some(abs.clone())).unwrap(), abs);
    }

    #[test]
    fn a_restart_into_the_old_version_is_reported_as_failed() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("updates")).unwrap();
        let a = Applied {
            from: crate::VERSION.into(),
            to: "99.0.0".into(),
            at: 0,
            attempts: 0,
            backup: None,
            agents: None,
            dist_files: Vec::new(),
        };
        std::fs::write(applied_path(dir.path()), serde_json::to_vec(&a).unwrap()).unwrap();
        assert!(mark_serving(dir.path()).is_none());
        let stale = detect_stale_restart(dir.path()).unwrap();
        assert_eq!(stale.to, "99.0.0");
        let s = status();
        assert_eq!(s.state, "failed");
        assert!(s.message.unwrap_or_default().contains("99.0.0"));
        assert!(dir.path().join("updates").join("failed.json").exists());
        assert!(detect_stale_restart(dir.path()).is_none());
    }
}
