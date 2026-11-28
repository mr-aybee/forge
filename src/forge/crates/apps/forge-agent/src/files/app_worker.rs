//! File operations on a native app's folder run as the app's own account
//! (D-083), so a bug in Forge's path checks can reach no more than the app
//! itself could.
//!
//! The root helper resolves the app's account on this node (never from the
//! request), refuses what an app root never allows (changing owners, the
//! root-owned `.forge` launcher folder, setuid/setgid bits), and starts its
//! own binary as `file-worker` under the app's uid and gid with a cleared
//! environment. The worker checks its identity in `/proc/self/status`,
//! lowers its resource limits and runs the same file engine on the app's
//! folder as a plain directory, with trash and revisions in the app's own
//! store outside that folder.
//!
//! Before it starts any thread, the worker also confines itself with
//! Landlock: it may read and write only beneath the app's folder and its
//! store (plus read `/etc/passwd` and `/etc/group` for owner names), where
//! the kernel supports it (best effort; the helper logs when it does not).
//!
//! The worker is untrusted: the app's processes share its uid and may be
//! able to trace it. The helper only parses its answer (bounded, strict
//! schema), never acts on anything it returns, kills it on a timeout and
//! runs at most [`WORKERS`] at a time.

use std::sync::OnceLock;
use std::time::Duration;

use forge_core::files::{FileOp, FilePath, RootSpec};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Workers running at once on this node.
pub const WORKERS: usize = 8;
/// Largest request a worker reads (an upload chunk is the biggest).
const MAX_INPUT: usize = 16 * 1024 * 1024;
/// Largest answer the helper reads from a worker.
const MAX_OUTPUT: usize = 48 * 1024 * 1024;
/// How long one operation may run (a search walks the whole folder).
const OP_TIMEOUT: Duration = Duration::from_secs(120);
const SEARCH_TIMEOUT: Duration = Duration::from_secs(600);

static WORKER_EXE: OnceLock<std::path::PathBuf> = OnceLock::new();

/// Runs workers from this binary instead of the current one (integration
/// tests, whose current binary is the test runner). Production never calls it.
#[doc(hidden)]
pub fn worker_exe_override(path: &std::path::Path) {
    let _ = WORKER_EXE.set(path.to_path_buf());
}

fn slots() -> &'static tokio::sync::Semaphore {
    static S: OnceLock<tokio::sync::Semaphore> = OnceLock::new();
    S.get_or_init(|| tokio::sync::Semaphore::new(WORKERS))
}

/// The worker's one-line answer.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Answer {
    ok: bool,
    #[serde(default)]
    data: Option<Value>,
    #[serde(default)]
    error: Option<String>,
    /// Landlock: `full`, `partial`, `none` (kernel without it) or
    /// `error: <reason>` (for example a container's seccomp filter).
    #[serde(default)]
    confined: Option<String>,
}

/// Logs once per helper when workers run with less than full confinement,
/// with the reason, so a blocked syscall can be told from an old kernel.
fn note_confinement(level: Option<&str>) {
    static NOTED: OnceLock<()> = OnceLock::new();

    if level != Some("full") && NOTED.set(()).is_ok() {
        tracing::warn!(
            landlock = level.unwrap_or("unknown"),
            "file workers run without full Landlock confinement on this node; the app account's permissions still apply"
        );
    }
}

/// What an app root never allows, and the operation as the worker runs it:
/// on the app's folder as a plain directory.
pub(crate) fn prepare(op: FileOp, base: &FilePath) -> Result<FileOp, String> {
    if matches!(op, FileOp::SetOwner { .. }) {
        return Err("files in an app's folder belong to the app's account; changing the owner is not allowed".into());
    }

    let launcher = |p: &FilePath| p.components().first().is_some_and(|c| c == ".forge");
    if op.paths().into_iter().any(launcher) {
        return Err("`.forge` holds the app's launcher, which Forge manages; it cannot be changed here".into());
    }

    if op.subtrees().into_iter().any(FilePath::is_root) {
        return Err("the app's folder itself cannot be moved or removed here".into());
    }

    let op = match op {
        // No setuid or setgid programs from an app's folder.
        FileOp::SetPermissions { root, path, mode, recursive, dry_run } => {
            FileOp::SetPermissions { root, path, mode: mode & !0o6000, recursive, dry_run }
        }
        other => other,
    };

    Ok(op.with_root(RootSpec::Dir { base: base.clone() }))
}

/// Runs `op` (an app root) as the app's account. Called by the root helper.
pub async fn run(op: FileOp) -> Result<Value, String> {
    let RootSpec::App { environment, app } = op.root().clone() else {
        return Err("not an app root".into());
    };

    let (account, uid, gid) = crate::apps::app_account(&environment, &app).await?;
    let base = RootSpec::app_base(&environment, &app);
    let search = matches!(op, FileOp::Search { .. });
    let op = prepare(op, &base)?;
    let store = crate::apps::ensure_app_store(&environment, &app, uid, gid)?;
    let input = serde_json::to_vec(&op).map_err(|e| e.to_string())?;
    if input.len() > MAX_INPUT {
        return Err("the request is too large".into());
    }

    let _slot = slots().acquire().await.map_err(|_| "the file service is shutting down".to_owned())?;

    // This binary, by its absolute path: never a PATH lookup.
    let exe = match WORKER_EXE.get() {
        Some(p) => p.clone(),
        None => std::env::current_exe().map_err(|e| format!("cannot find the helper binary: {e}"))?,
    };

    let mut child = tokio::process::Command::new(exe)
        .args(["file-worker", &uid.to_string(), &gid.to_string(), &base.to_string(), &store])
        .uid(uid)
        .gid(gid)
        .env_clear()
        .current_dir("/")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("cannot start the file worker: {e}"))?;
    let mut stdin = child.stdin.take().ok_or("no worker input")?;
    let mut stdout = child.stdout.take().ok_or("no worker output")?;
    let exchange = async {
        stdin.write_all(&input).await.map_err(|e| e.to_string())?;
        drop(stdin);
        let mut out = Vec::new();
        (&mut stdout).take(MAX_OUTPUT as u64 + 1).read_to_end(&mut out).await.map_err(|e| e.to_string())?;

        if out.len() > MAX_OUTPUT {
            return Err("the file worker's answer is too large".to_owned());
        }

        let _ = child.wait().await;
        Ok(out)
    };
    let limit = if search { SEARCH_TIMEOUT } else { OP_TIMEOUT };
    let out = match tokio::time::timeout(limit, exchange).await {
        Ok(r) => r?,
        // Dropping the child (kill_on_drop) ends the worker.
        Err(_) => return Err(format!("the operation did not finish within {} s", limit.as_secs())),
    };

    let answer: Answer =
        serde_json::from_slice(&out).map_err(|_| "the file worker sent an invalid answer".to_owned())?;
    note_confinement(answer.confined.as_deref());

    match (answer.ok, answer.data, answer.error) {
        (true, Some(mut data), None) => {
            // The account the helper resolved, and the confinement the
            // worker reports; the control plane audits both as reported.
            if let Some(o) = data.as_object_mut() {
                o.insert("as_reported".into(), Value::from(account));
                let level = answer.confined.as_deref().unwrap_or("unknown");
                let level = level.split(':').next().unwrap_or("unknown").trim();
                o.insert("confined_reported".into(), Value::from(level.chars().take(16).collect::<String>()));
            }

            Ok(data)
        }
        (false, None, Some(e)) => Err(e.chars().take(2000).collect()),
        _ => Err("the file worker sent an invalid answer".into()),
    }
}

/// `forge-agent-helper file-worker <uid> <gid> <base> <store>`: one
/// operation from stdin, its answer as one JSON line on stdout.
#[must_use]
pub fn worker_main(args: &[String]) -> i32 {
    let mut confined = None;
    let answer = match worker(args, &mut confined) {
        Ok(data) => Answer { ok: true, data: Some(data), error: None, confined },
        Err(e) => Answer { ok: false, data: None, error: Some(e), confined },
    };

    match serde_json::to_string(&answer) {
        Ok(line) => {
            println!("{line}");
            0
        }
        Err(_) => 1,
    }
}

fn worker(args: &[String], confined: &mut Option<String>) -> Result<Value, String> {
    let [uid, gid, base, store] = args else { return Err("usage: file-worker <uid> <gid> <base> <store>".into()) };
    let uid: u32 = uid.parse().map_err(|_| "bad uid")?;
    let gid: u32 = gid.parse().map_err(|_| "bad gid")?;
    check_identity(&std::fs::read_to_string("/proc/self/status").map_err(|e| e.to_string())?, uid, gid)?;
    limit_resources();

    // Before any thread exists: Landlock binds this thread and its children.
    *confined = Some(confine(base, store));
    let base = FilePath::parse(base)?;
    super::store_override(std::path::Path::new(store));
    let mut input = Vec::new();
    std::io::Read::read_to_end(&mut std::io::Read::take(std::io::stdin(), MAX_INPUT as u64 + 1), &mut input)
        .map_err(|e| e.to_string())?;

    if input.len() > MAX_INPUT {
        return Err("the request is too large".into());
    }

    let op: FileOp = serde_json::from_slice(&input).map_err(|e| format!("invalid request: {e}"))?;

    // Only the operation the helper prepared: on this folder, never an owner change.
    if op.root() != &(RootSpec::Dir { base }) || matches!(op, FileOp::SetOwner { .. }) {
        return Err("refused: not the app's folder".into());
    }

    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;

    rt.block_on(super::run_local(op))
}

/// Refuses to run unless every user and group id of this process is the
/// app's: real, effective, saved and filesystem ids, and no supplementary
/// group but its own.
pub(crate) fn check_identity(status: &str, uid: u32, gid: u32) -> Result<(), String> {
    if uid == 0 || gid == 0 {
        return Err("refused: a worker never runs as root".into());
    }

    let ids = |key: &str| -> Option<Vec<u32>> {
        let line = status.lines().find(|l| l.starts_with(key))?;
        line[key.len()..].split_whitespace().map(|v| v.parse().ok()).collect()
    };
    let all =
        |v: Option<Vec<u32>>, want: u32, n: usize| v.is_some_and(|v| v.len() == n && v.iter().all(|x| *x == want));

    if !all(ids("Uid:"), uid, 4) || !all(ids("Gid:"), gid, 4) {
        return Err("refused: the worker does not run as the app's account".into());
    }

    if !ids("Groups:").is_some_and(|g| g.iter().all(|x| *x == gid)) {
        return Err("refused: the worker holds other groups".into());
    }

    Ok(())
}

/// Landlock (D-083 phase 2): read and write only beneath the app's folder
/// and its store, read `/etc/passwd` and `/etc/group`; no TCP at all; no
/// abstract UNIX sockets or signals to processes outside (the app's own).
/// The newest rights Forge knows (truncate, device ioctls) where the kernel
/// has them: best effort, so a kernel without Landlock (RHEL 8) or with an
/// older version runs with what it has, moves between folders (`REFER`,
/// into the trash) keep working from version 2 on. Returns the level
/// reached, or `error: <reason>`.
fn confine(base: &str, store: &str) -> String {
    use landlock::{
        ABI, Access, AccessFs, AccessNet, Ruleset, RulesetAttr, RulesetCreatedAttr, RulesetError, RulesetStatus, Scope,
        path_beneath_rules,
    };
    let fs = AccessFs::from_all(ABI::V5);
    let status = (|| -> Result<RulesetStatus, RulesetError> {
        Ok(Ruleset::default()
            .handle_access(fs)?
            .handle_access(AccessNet::from_all(ABI::V4))?
            .scope(Scope::AbstractUnixSocket | Scope::Signal)?
            .create()?
            .add_rules(path_beneath_rules([base, store], fs))?
            .add_rules(path_beneath_rules(["/etc/passwd", "/etc/group"], AccessFs::ReadFile))?
            .restrict_self()?
            .ruleset)
    })();

    match status {
        Ok(RulesetStatus::FullyEnforced) => "full".into(),
        Ok(RulesetStatus::PartiallyEnforced) => "partial".into(),
        Ok(RulesetStatus::NotEnforced) => "none".into(),
        Err(e) => format!("error: {}", e.to_string().chars().take(200).collect::<String>()),
    }
}

/// No core dumps, bounded open files, CPU time and file size. Best effort:
/// the helper's timeout and answer cap hold either way.
fn limit_resources() {
    use rustix::process::{Resource, Rlimit, setrlimit};
    let set = |r: Resource, v: u64| {
        let _ = setrlimit(r, Rlimit { current: Some(v), maximum: Some(v) });
    };
    set(Resource::Core, 0);
    set(Resource::Nofile, 512);
    set(Resource::Cpu, SEARCH_TIMEOUT.as_secs() + 60);

    // Copies and moves are capped at 1 GiB; leave room for one such file.
    set(Resource::Fsize, 2 * 1024 * 1024 * 1024);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> FilePath {
        FilePath::parse(s).unwrap_or_else(|e| panic!("{e}"))
    }

    fn app() -> RootSpec {
        RootSpec::App { environment: "staging".into(), app: "api".into() }
    }

    #[test]
    fn app_roots_refuse_owner_changes_the_launcher_and_the_folder_itself() {
        let base = RootSpec::app_base("staging", "api");
        let own = FileOp::SetOwner {
            root: app(),
            path: p("/x"),
            owner: Some("root".into()),
            group: None,
            recursive: false,
            dry_run: false,
        };
        assert!(prepare(own, &base).is_err());
        let launcher = FileOp::Delete { root: app(), path: p("/.forge"), permanent: false };
        assert!(prepare(launcher, &base).is_err_and(|e| e.contains(".forge")));
        let inside = FileOp::Read { root: app(), path: p("/.forge/run.sh"), offset: 0, length: 10, version: None };
        assert!(prepare(inside, &base).is_err());
        let onto = FileOp::Rename { root: app(), from: p("/x"), to: p("/.forge"), overwrite: true };
        assert!(prepare(onto, &base).is_err_and(|e| e.contains(".forge")), "a rename onto the launcher folder");
        let whole = FileOp::Delete { root: app(), path: p("/"), permanent: false };
        assert!(prepare(whole, &base).is_err());
        let ok = FileOp::List { root: app(), path: p("/public"), offset: 0, limit: 100, hidden: false };
        assert_eq!(prepare(ok, &base).map(|o| o.root().clone()), Ok(RootSpec::Dir { base: base.clone() }));
    }

    #[test]
    fn setuid_and_setgid_bits_are_stripped() {
        let base = RootSpec::app_base("staging", "api");
        let op = FileOp::SetPermissions {
            root: app(),
            path: p("/bin/tool"),
            mode: 0o6755,
            recursive: false,
            dry_run: false,
        };

        match prepare(op, &base) {
            Ok(FileOp::SetPermissions { mode, .. }) => assert_eq!(mode, 0o755),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_worker_checks_every_id() {
        let good = "Name:\tforge-agent-hel\nUid:\t998\t998\t998\t998\nGid:\t997\t997\t997\t997\nGroups:\t997 \n";
        assert!(check_identity(good, 998, 997).is_ok());
        let no_groups = "Uid:\t998\t998\t998\t998\nGid:\t997\t997\t997\t997\nGroups:\t\n";
        assert!(check_identity(no_groups, 998, 997).is_ok());
        let saved_root = "Uid:\t998\t998\t0\t998\nGid:\t997\t997\t997\t997\nGroups:\t\n";
        assert!(check_identity(saved_root, 998, 997).is_err());
        let extra_group = "Uid:\t998\t998\t998\t998\nGid:\t997\t997\t997\t997\nGroups:\t997 27\n";
        assert!(check_identity(extra_group, 998, 997).is_err_and(|e| e.contains("other groups")));
        assert!(check_identity(good, 0, 0).is_err());
        assert!(check_identity("garbage", 998, 997).is_err());
    }
}
