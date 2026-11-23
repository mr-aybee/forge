//! Runs catalog recipes.
//!
//! * Linux: inside `forge-agent-helper serve` (root). The script is the
//!   catalog's prelude + recipe, fed to `/bin/sh -s` on stdin, with a cleared
//!   environment holding only the validated variables.
//! * Windows: in the agent service itself (LocalSystem). The catalog's winget
//!   runner is written to a fresh file in the service's temp directory and
//!   run with PowerShell; the validated variables are added to the service's
//!   environment.
//!
//! One recipe runs at a time (package managers take global locks).

use std::collections::BTreeMap;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

use crate::helper::HelperResponse;

const TIMEOUT: Duration = Duration::from_secs(45 * 60);
const OUTPUT_TAIL: usize = 32 * 1024;

static RUNNING: Mutex<()> = Mutex::const_new(());

/// apt option: wait up to five minutes for the dpkg lock held by another apt
/// or dpkg run (unattended-upgrades, an install outside Forge) instead of
/// failing at once. The catalog prelude passes the same option.
/// Seconds apt waits for the dpkg lock (`APT_LOCK_TIMEOUT`).
pub const APT_LOCK_WAIT_SECS: u64 = 300;
pub const APT_LOCK_TIMEOUT: &str = "DPkg::Lock::Timeout=300";

/// Package manager runs outside recipes may wait for the dpkg lock, so they
/// get at least 15 minutes, well past the lock wait.
pub const PACKAGE_COMMAND_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// The timeout for running `cmd`: package managers get
/// [`PACKAGE_COMMAND_TIMEOUT`], anything else `default`.
#[must_use]
pub fn command_timeout(cmd: &str, default: Duration) -> Duration {
    if matches!(cmd, "apt-get" | "apt" | "dpkg" | "dnf" | "yum" | "zypper" | "apk") {
        default.max(PACKAGE_COMMAND_TIMEOUT)
    } else {
        default
    }
}

/// Runs `work` while holding the node-wide package lock: package recipes,
/// runtime installs and object store operations run one at a time.
pub async fn exclusive<F: std::future::Future>(work: F) -> F::Output {
    let _guard = RUNNING.lock().await;
    work.await
}

/// Keys whose values never leave the node.
fn is_secret(key: &str) -> bool {
    ["PASSWORD", "SECRET", "TOKEN", "KEY", "URL", "PASS"].iter().any(|s| key.contains(s))
}

/// Where a package's variables live on this OS.
#[must_use]
pub fn env_file(package: &str) -> std::path::PathBuf {
    #[cfg(windows)]
    {
        crate::config::windows_data_dir().join("pkgs").join(format!("{package}.env"))
    }
    #[cfg(not(windows))]
    {
        std::path::PathBuf::from(format!("/etc/forge/pkgs/{package}.env"))
    }
}

/// Reads the package's env file, redacting secret values.
#[must_use]
pub fn redacted_env(package: &str) -> BTreeMap<String, String> {
    let path = env_file(package);
    let Ok(text) = std::fs::read_to_string(&path) else { return BTreeMap::new() };
    text.lines()
        .map(|l| l.trim_start_matches('\u{feff}').trim_end_matches('\r'))
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| {
            let v = if is_secret(k) { format!("<kept on node in {}>", path.display()) } else { v.to_owned() };
            (k.to_owned(), v)
        })
        .collect()
}

fn tail(mut s: String) -> String {
    if s.len() > OUTPUT_TAIL {
        let mut cut = s.len() - OUTPUT_TAIL;
        while !s.is_char_boundary(cut) {
            cut += 1;
        }
        s = format!("…{}", &s[cut..]);
    }
    s
}

/// Runs a validated request. Each output line is also sent to `progress`
/// (live output for the console) when given.
pub async fn run(
    v: &forge_catalog::Validated,
    progress: Option<&tokio::sync::mpsc::UnboundedSender<String>>,
) -> HelperResponse {
    exclusive(async {
        match launch(v) {
            Ok((cmd, stdin, cleanup)) => {
                let r = execute(cmd, stdin, progress, v.package.id).await;
                if let Some(path) = cleanup {
                    let _ = std::fs::remove_file(path);
                }
                r
            }
            Err(e) => HelperResponse::failed(e),
        }
    })
    .await
}

/// Runs an object store operation (`forge_catalog::storage`): the provider
/// script with the operation's variables. Shares the one-at-a-time lock with
/// recipes (both may use the package manager). The reply carries the parsed
/// report as `data`; the credential is only in the child's environment.
pub async fn run_store(
    op: &forge_catalog::storage::StoreOp,
    progress: Option<&tokio::sync::mpsc::UnboundedSender<String>>,
) -> HelperResponse {
    if let Err(e) = op.validate() {
        return HelperResponse::failed(format!("rejected: {e}"));
    }
    let _guard = RUNNING.lock().await;
    // `adopt` prints a credential: no live output for it, and the line is
    // taken out of the returned output below.
    let progress = if matches!(op, forge_catalog::storage::StoreOp::Adopt { .. }) { None } else { progress };
    match launch_store(op) {
        Ok((cmd, stdin, cleanup)) => {
            let mut r = execute(cmd, stdin, progress, "").await;
            if let Some(path) = cleanup {
                let _ = std::fs::remove_file(path);
            }
            let (cred, rest) = forge_catalog::storage::take_credential(r.output.as_deref().unwrap_or_default());
            r.output = Some(rest);
            r.details = cred.map(|c| {
                std::collections::BTreeMap::from([
                    ("access_key".to_owned(), c.access_key),
                    ("secret_key".to_owned(), c.secret_key),
                ])
            });
            r.data = r
                .output
                .as_deref()
                .and_then(forge_catalog::storage::StoreReport::parse)
                .and_then(|rep| serde_json::to_value(rep).ok());
            r
        }
        Err(e) => HelperResponse::failed(e),
    }
}

#[cfg(not(windows))]
fn launch_store(op: &forge_catalog::storage::StoreOp) -> Result<Launch, String> {
    let mut cmd = tokio::process::Command::new("/bin/sh");
    cmd.arg("-s");
    cmd.env_clear().env("PATH", "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin").env("LANG", "C.UTF-8");
    cmd.env("HOME", "/root");
    for (k, val) in op.env() {
        cmd.env(k, val);
    }
    Ok((cmd, Some(op.linux_script().into_bytes()), None))
}

#[cfg(windows)]
fn launch_store(op: &forge_catalog::storage::StoreOp) -> Result<Launch, String> {
    let (mut cmd, file) = powershell_file(forge_catalog::storage::RUSTFS_PS1)?;
    for (k, val) in op.env() {
        cmd.env(k, val);
    }
    Ok((cmd, None, Some(file)))
}

/// Writes a script to a new, uniquely named file in the service's temp
/// directory and prepares PowerShell to run it without inherited `FORGE_*`.
#[cfg(windows)]
fn powershell_file(script: &str) -> Result<(tokio::process::Command, std::path::PathBuf), String> {
    use std::io::Write;
    let mut name = [0u8; 12];
    getrandom(&mut name);
    let file = std::env::temp_dir().join(format!("forge-recipe-{}.ps1", hex(&name)));
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&file)
        .map_err(|e| format!("cannot write the script to {}: {e}", file.display()))?;
    f.write_all(b"\xEF\xBB\xBF").and_then(|()| f.write_all(script.as_bytes())).map_err(|e| e.to_string())?;
    drop(f);
    let shell = if which("pwsh.exe") { "pwsh.exe" } else { "powershell.exe" };
    let mut cmd = tokio::process::Command::new(shell);
    cmd.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File"]).arg(&file);
    // A PSModulePath inherited from PowerShell 7 makes Windows PowerShell 5.1
    // load the wrong module versions; each shell rebuilds its own default.
    cmd.env_remove("PSModulePath");
    for (k, _) in std::env::vars_os() {
        if k.to_string_lossy().to_ascii_uppercase().starts_with("FORGE_") {
            cmd.env_remove(&k);
        }
    }
    Ok((cmd, file))
}

type Launch = (tokio::process::Command, Option<Vec<u8>>, Option<std::path::PathBuf>);

#[cfg(not(windows))]
fn launch(v: &forge_catalog::Validated) -> Result<Launch, String> {
    let mut cmd = tokio::process::Command::new("/bin/sh");
    cmd.arg("-s").arg("--").arg(v.action);
    cmd.env_clear().env("PATH", "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin").env("LANG", "C.UTF-8");
    cmd.env("HOME", "/root");
    for (k, val) in &v.env {
        cmd.env(k, val);
    }
    Ok((cmd, Some(forge_catalog::script(v.package).into_bytes()), None))
}

#[cfg(windows)]
fn launch(v: &forge_catalog::Validated) -> Result<Launch, String> {
    use std::io::Write;
    let env = forge_catalog::winget_env(v).ok_or_else(|| format!("{} is not available on Windows", v.package.name))?;
    // A new, uniquely named file in the service's temp directory (LocalSystem:
    // %SystemRoot%\Temp); `create_new` refuses a file planted in advance.
    let mut name = [0u8; 12];
    getrandom(&mut name);
    let file = std::env::temp_dir().join(format!("forge-recipe-{}.ps1", hex(&name)));
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&file)
        .map_err(|e| format!("cannot write the recipe to {}: {e}", file.display()))?;
    // UTF-8 with BOM: Windows PowerShell 5.1 reads BOM-less files as ANSI.
    f.write_all(b"\xEF\xBB\xBF")
        .and_then(|()| f.write_all(forge_catalog::WINGET_RUNNER.as_bytes()))
        .map_err(|e| e.to_string())?;
    drop(f);
    let shell = if which("pwsh.exe") { "pwsh.exe" } else { "powershell.exe" };
    let mut cmd = tokio::process::Command::new(shell);
    cmd.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File"]).arg(&file);
    // A PSModulePath inherited from PowerShell 7 makes Windows PowerShell 5.1
    // load the wrong module versions; each shell rebuilds its own default.
    cmd.env_remove("PSModulePath");
    // Inherited FORGE_* variables must not leak into the recipe.
    for (k, _) in std::env::vars_os() {
        if k.to_string_lossy().to_ascii_uppercase().starts_with("FORGE_") {
            cmd.env_remove(&k);
        }
    }
    for (k, val) in &env {
        cmd.env(k, val);
    }
    Ok((cmd, None, Some(file)))
}

#[cfg(windows)]
fn which(exe: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(exe).is_file()))
}

#[cfg(windows)]
fn getrandom(buf: &mut [u8]) {
    // ring's SystemRandom is the workspace's one source of randomness.
    use ring::rand::SecureRandom;
    let _ = ring::rand::SystemRandom::new().fill(buf);
}

#[cfg(windows)]
fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Kills a recipe's whole process group when dropped: on cancel (the task is
/// aborted), on timeout, and harmlessly after a normal exit (the group is
/// gone by then). `kill` is run as a program: no unsafe code.
#[cfg_attr(not(unix), allow(dead_code))]
struct GroupKill(Option<u32>);

impl Drop for GroupKill {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = self.0.filter(|p| *p > 1) {
            let _ = std::process::Command::new("kill")
                .args(["-KILL", "--", &format!("-{pid}")])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
}

async fn execute(
    mut cmd: tokio::process::Command,
    stdin: Option<Vec<u8>>,
    progress: Option<&tokio::sync::mpsc::UnboundedSender<String>>,
    package: &str,
) -> HelperResponse {
    cmd.stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    // Its own process group, so a cancelled or timed-out recipe takes every
    // process it started (apt-get, curl, docker…) with it.
    #[cfg(unix)]
    cmd.process_group(0);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return HelperResponse::failed(format!("cannot start the recipe shell: {e}")),
    };
    let _group = GroupKill(child.id());
    if let (Some(bytes), Some(mut pipe)) = (stdin, child.stdin.take()) {
        if let Err(e) = pipe.write_all(&bytes).await {
            return HelperResponse::failed(format!("cannot pass recipe: {e}"));
        }
        drop(pipe);
    }
    let (mut out, mut err) = (child.stdout.take(), child.stderr.take());
    let collect = async {
        use tokio::io::AsyncBufReadExt;
        let (mut o, mut e) = (String::new(), String::new());
        let read_o = async {
            if let Some(s) = out.take() {
                let mut lines = tokio::io::BufReader::new(s).lines();
                while let Ok(Some(l)) = lines.next_line().await {
                    let l = l.trim_end_matches('\r').to_owned();
                    if let Some(p) = progress {
                        let _ = p.send(l.clone());
                    }
                    o.push_str(&l);
                    o.push('\n');
                }
            }
        };
        let read_e = async {
            if let Some(s) = err.take() {
                let mut lines = tokio::io::BufReader::new(s).lines();
                while let Ok(Some(l)) = lines.next_line().await {
                    let l = l.trim_end_matches('\r').to_owned();
                    if let Some(p) = progress {
                        let _ = p.send(l.clone());
                    }
                    e.push_str(&l);
                    e.push('\n');
                }
            }
        };
        tokio::join!(read_o, read_e);
        let status = child.wait().await;
        (o, e, status)
    };
    let Ok((stdout, stderr, status)) = tokio::time::timeout(TIMEOUT, collect).await else {
        return HelperResponse::failed("recipe timed out after 45 minutes");
    };
    let state = stdout.lines().rev().find(|l| !l.trim().is_empty()).map(|l| l.trim().to_owned());
    let ok = status.as_ref().is_ok_and(std::process::ExitStatus::success);
    let combined = if stderr.is_empty() { stdout } else { format!("{stdout}{stderr}") };
    let error = (!ok).then(|| {
        stderr.lines().rev().find(|l| l.starts_with("forge: error:")).map_or_else(
            || format!("recipe failed ({})", status.map(|s| s.to_string()).unwrap_or_default()),
            |l| l.trim_start_matches("forge: error:").trim().to_owned(),
        )
    });
    HelperResponse {
        ok,
        state: if ok { state } else { None },
        error,
        output: Some(tail(combined)),
        details: Some(redacted_env(package)),
        data: None,
    }
}

#[cfg(test)]
mod tests {
    use super::{APT_LOCK_WAIT_SECS, PACKAGE_COMMAND_TIMEOUT, TIMEOUT, command_timeout, exclusive, is_secret};
    use std::time::Duration;

    /// A dpkg lock wait must never time out the install first (#17).
    #[test]
    fn install_timeouts_outlast_the_dpkg_lock_wait() {
        let floor = Duration::from_secs(2 * APT_LOCK_WAIT_SECS);
        assert!(TIMEOUT >= floor.max(Duration::from_secs(600)));
        assert!(PACKAGE_COMMAND_TIMEOUT >= floor.max(Duration::from_secs(600)));
        assert!(crate::executor::PACKAGE_TIMEOUT > TIMEOUT);
        assert_eq!(command_timeout("apt-get", Duration::from_secs(120)), PACKAGE_COMMAND_TIMEOUT);
        assert_eq!(command_timeout("wg", Duration::from_secs(120)), Duration::from_secs(120));
    }
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn concurrent_installs_on_a_node_run_one_at_a_time() {
        let running = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let tasks: Vec<_> = (0..4)
            .map(|_| {
                let (running, peak) = (running.clone(), peak.clone());
                tokio::spawn(exclusive(async move {
                    let now = running.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    for _ in 0..10 {
                        tokio::task::yield_now().await;
                    }
                    running.fetch_sub(1, Ordering::SeqCst);
                }))
            })
            .collect();
        for t in tasks {
            assert!(t.await.is_ok());
        }
        assert_eq!(peak.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn secret_like_keys_are_redacted() {
        assert!(is_secret("FORGE_PKG_POSTGRESQL_PASSWORD"));
        assert!(is_secret("FORGE_PKG_REDIS_URL"));
        assert!(is_secret("FORGE_PKG_RUSTFS_SECRET_KEY"));
        assert!(!is_secret("FORGE_PKG_POSTGRESQL_PORT"));
    }
}
