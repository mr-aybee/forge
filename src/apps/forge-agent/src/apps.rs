//! Runs services on the node (root, inside `forge-agent-helper`).
//!
//! Native apps (services and jobs) run as their own unprivileged system user
//! under systemd, or under `forge-agent-helper supervise` where there is no
//! systemd (containers, minimal VMs). Containers go through the Docker Engine
//! API. Paths and names come from [`forge_catalog::AppSpec`], which the helper
//! validates before anything is written.

use std::os::fd::OwnedFd;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use forge_catalog::{AppSpec, Runtime};
use rustix::fs as rfs;
use rustix::io::Errno;
use serde_json::{Value, json};
use tokio::process::Command;

use crate::docker;

const SUPERVISE_DIR: &str = "/run/forge/apps";
const LOG_DIR: &str = "/var/log/forge/apps";

pub(crate) fn has_systemd() -> bool {
    Path::new("/run/systemd/system").is_dir()
}

async fn sh(cmd: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(cmd).args(args).stdin(Stdio::null()).output().await.map_err(|e| format!("{cmd}: {e}"))?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    if out.status.success() { Ok(text) } else { Err(format!("{cmd} {}: {}", args.join(" "), text.trim())) }
}

/// Creates `dir` and sets `mode` (the helper may run with a restrictive umask).
fn mkdir(dir: &str, mode: u32) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{dir}: {e}"))?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(mode)).map_err(|e| format!("{dir}: {e}"))
}

fn write(path: &str, content: &str, mode: u32) -> Result<(), String> {
    if let Some(dir) = Path::new(path).parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }

    let tmp = format!("{path}.tmp");
    std::fs::write(&tmp, content).map_err(|e| format!("{tmp}: {e}"))?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode)).map_err(|e| e.to_string())?;

    std::fs::rename(&tmp, path).map_err(|e| format!("{path}: {e}"))
}

// ---- the app's own account (D-081) ------------------------------------------------
//
// Every native app runs as its own system account, named after it
// (`<ns>-<app>`, see `AppSpec::user_candidates`) and marked as Forge's in its
// GECOS comment. Forge adopts an existing account only with exactly that
// mark, so it never takes over a system or human account. Only portable
// tools are used (`getent`, `useradd`, `usermod`, `groupmod`): Debian,
// Ubuntu and RHEL-likes alike.

/// One account from the system's user database (NSS, so LDAP users count).
#[derive(Debug, PartialEq, Eq)]
struct Account {
    uid: u32,
    gid: u32,
    gecos: String,
    home: String,
    shell: String,
}

/// Parses one `getent passwd` line for `name`.
fn parse_passwd(name: &str, line: &str) -> Option<Account> {
    let f: Vec<&str> = line.trim_end().split(':').collect();
    if f.len() < 7 || f[0] != name {
        return None;
    }

    Some(Account {
        uid: f[2].parse().ok()?,
        gid: f[3].parse().ok()?,
        gecos: f[4].to_owned(),
        home: f[5].to_owned(),
        shell: f[6].to_owned(),
    })
}

/// Primary groups that grant root-equivalent rights.
const PRIVILEGED_GROUPS: [&str; 6] = ["root", "sudo", "wheel", "adm", "admin", "docker"];

/// Whether `a` looks like an account Forge made for this app, apart from
/// the mark: not root, a login-refusing shell, its home exactly the app's
/// folder (a user can change their own comment with `chfn` on some
/// systems, never their home), the home owned by it when it exists, and a
/// primary group without root-equivalent rights.
async fn shaped_like_app_account(home: &str, a: &Account) -> bool {
    if a.uid == 0 || a.gid == 0 || a.home != home {
        return false;
    }

    if !(a.shell.ends_with("/nologin") || a.shell == "/bin/false") {
        return false;
    }

    if let Ok(meta) = std::fs::symlink_metadata(&a.home) {
        use std::os::unix::fs::MetadataExt;

        if !meta.is_dir() || meta.uid() != a.uid {
            return false;
        }
    }

    let group = sh("getent", &["group", &a.gid.to_string()]).await.unwrap_or_default();
    let group = group.split(':').next().unwrap_or("").trim();

    !PRIVILEGED_GROUPS.contains(&group)
}

async fn account(name: &str) -> Option<Account> {
    let out = sh("getent", &["passwd", name]).await.ok()?;
    parse_passwd(name, out.lines().next()?)
}

/// The login shell that refuses logins: its path differs between distros.
fn nologin_shell() -> &'static str {
    ["/usr/sbin/nologin", "/sbin/nologin", "/usr/bin/nologin"]
        .into_iter()
        .find(|p| Path::new(p).exists())
        .unwrap_or("/bin/false")
}

/// An app's account if it exists: the candidate name that carries Forge's
/// mark for this app and has the shape Forge gives it. Two such accounts
/// mean someone imitated one: refused.
async fn find_account(environment: &str, app: &str) -> Result<Option<(String, Account)>, String> {
    let mark = forge_catalog::apps::app_user_mark(environment, app);
    let home = forge_catalog::apps::app_home(environment, app);
    let mut found = Vec::new();

    for name in forge_catalog::apps::app_user_candidates(environment, app) {
        if let Some(a) = account(&name).await
            && a.gecos == mark
            && shaped_like_app_account(&home, &a).await
        {
            found.push((name, a));
        }
    }

    match found.len() {
        0 => Ok(None),
        1 => Ok(found.pop()),
        _ => Err(format!(
            "more than one account claims to be {environment}-{app}'s ({}); remove the one Forge did not create",
            found.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(", ")
        )),
    }
}

async fn find_user(spec: &AppSpec) -> Result<Option<String>, String> {
    Ok(find_account(&spec.environment, &spec.name).await?.map(|(n, _)| n))
}

/// The account a native app runs as on this node, for running file
/// operations on its folder as it (D-083): name, uid and gid. Never taken
/// from a request; resolved here from the environment and app name.
pub async fn app_account(environment: &str, app: &str) -> Result<(String, u32, u32), String> {
    use std::os::unix::fs::MetadataExt;

    let Some((name, a)) = find_account(environment, app).await? else {
        return Err(format!("{environment}/{app} has no account on this node yet; deploy the app first"));
    };

    // Fail closed: the folder must exist as a real directory of that account.
    match std::fs::symlink_metadata(&a.home) {
        Ok(m) if m.is_dir() && m.uid() == a.uid => Ok((name, a.uid, a.gid)),
        _ => Err(format!("{} is missing or not {name}'s folder; deploy the app again", a.home)),
    }
}

/// Where an app's trash and revisions live (D-083): outside its folder,
/// which apps often serve as files, in root-owned parents it cannot rename.
/// `<ns>/<app>`, not `<ns>-<app>`: slugs contain `-`, so `a-b`/`c` and
/// `a`/`b-c` would share a folder.
#[must_use]
pub fn app_store(environment: &str, app: &str) -> String {
    format!("{APP_STORES}/{environment}/{app}")
}

const APP_STORES: &str = "/var/lib/forge-app-store";

/// Creates the app's store (0700, owned by the app) under root-owned,
/// traversable parents. An existing one must be a real folder owned by it.
pub fn ensure_app_store(environment: &str, app: &str, uid: u32, gid: u32) -> Result<String, String> {
    use std::os::unix::fs::MetadataExt;
    mkdir(APP_STORES, 0o711)?;
    mkdir(&format!("{APP_STORES}/{environment}"), 0o711)?;
    let dir = app_store(environment, app);
    match std::fs::symlink_metadata(&dir) {
        Ok(m) if m.is_dir() && m.uid() == uid => return Ok(dir),
        Ok(_) => return Err(format!("{dir} is not the app's own folder; remove it and try again")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("{dir}: {e}")),
    }

    std::fs::create_dir(&dir).map_err(|e| format!("{dir}: {e}"))?;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).map_err(|e| format!("{dir}: {e}"))?;

    // Just created by root in a root-owned parent: no link can stand in its place.
    std::os::unix::fs::lchown(&dir, Some(uid), Some(gid)).map_err(|e| format!("{dir}: {e}"))?;

    Ok(dir)
}

/// Renames the account an older Forge created (`fa-<app>-<hash>`) to the
/// readable name. The uid stays, so every file the app owns stays its own.
async fn rename_legacy(spec: &AppSpec, legacy: &str, new: &str) -> Result<(), String> {
    // `usermod -l` refuses while the account runs processes.
    if has_systemd() {
        let unit = spec.unit();
        let _ = sh("systemctl", &["stop", &format!("{unit}.service")]).await;
    } else {
        stop_supervisor(spec).await;
    }

    sh("usermod", &["--login", new, "--comment", &spec.user_mark(), legacy]).await?;

    if sh("getent", &["group", legacy]).await.is_ok() {
        sh("groupmod", &["--new-name", new, legacy]).await?;
    }

    tracing::info!(app = %spec.key(), from = legacy, to = new, "app account renamed");

    Ok(())
}

/// Creates, adopts or renames the app's account and prepares its home.
/// Returns the account name.
async fn ensure_user(spec: &AppSpec) -> Result<String, String> {
    let user = match find_user(spec).await? {
        Some(u) => u,
        None => {
            let mut free = None;

            for name in spec.user_candidates() {
                if account(&name).await.is_none() {
                    free = Some(name);
                    break;
                }
            }

            let Some(name) = free else {
                return Err(format!(
                    "the account names for {} ({}) belong to accounts Orbyn Forge did not create; rename or remove them",
                    spec.key(),
                    spec.user_candidates().join(", ")
                ));
            };

            let legacy = spec.legacy_user();
            match account(&legacy).await {
                Some(a) if shaped_like_app_account(&spec.home(), &a).await => {
                    rename_legacy(spec, &legacy, &name).await?;
                }
                _ => {
                    sh(
                        "useradd",
                        &[
                            "--system",
                            "--user-group",
                            "--home-dir",
                            &spec.home(),
                            "--shell",
                            nologin_shell(),
                            "--comment",
                            &spec.user_mark(),
                            &name,
                        ],
                    )
                    .await?;
                }
            }

            name
        }
    };

    // World-traversable parents; the app directory itself is private to the app user.
    for dir in ["/srv/forge", "/srv/forge/apps", &format!("/srv/forge/apps/{}", spec.environment)] {
        mkdir(dir, 0o755)?;
    }

    mkdir(&spec.home(), 0o750)?;
    sh("chown", &[&format!("{user}:{user}"), &spec.home()]).await?;

    // Traversable, not listable.
    mkdir(&format!("/etc/forge/apps/{}", spec.environment), 0o711)?;

    // Trash and revisions of the file explorer on this app's folder (D-083).
    if let Some(a) = account(&user).await {
        ensure_app_store(&spec.environment, &spec.name, a.uid, a.gid)?;
    }

    Ok(user)
}

/// Removes the app's store if it is a real folder owned by the app's
/// account (`remove_dir_all` does not follow links).
async fn remove_app_store(spec: &AppSpec) {
    use std::os::unix::fs::MetadataExt;
    let dir = app_store(&spec.environment, &spec.name);
    let Ok(Some((_, a))) = find_account(&spec.environment, &spec.name).await else { return };
    if std::fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir() && m.uid() == a.uid) {
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// The env file of a native app: root and the app's own group may read it.
async fn write_app_env(spec: &AppSpec, user: &str) -> Result<(), String> {
    write(&spec.env_path(), &spec.env_file(), 0o600)?;
    sh("chown", &[&format!("root:{user}"), &spec.env_path()]).await?;

    std::fs::set_permissions(spec.env_path(), std::fs::Permissions::from_mode(0o640)).map_err(|e| e.to_string())
}

// ---- the launcher folder `<home>/.forge` -----------------------------------------
//
// The app home belongs to the app user, who can rename `.forge` or put a
// symlink in its place (`.forge` → `/etc/profile.d`). Root therefore never
// reaches it by path: it opens the folder with `O_NOFOLLOW`, checks that it is
// a real directory owned by root, and works relative to that handle.

const FORGE_DIR: &str = ".forge";

fn dir_open_flags() -> rfs::OFlags {
    rfs::OFlags::RDONLY | rfs::OFlags::DIRECTORY | rfs::OFlags::NOFOLLOW | rfs::OFlags::CLOEXEC
}

/// Opens (and with `create`, makes) `<home>/.forge`. Refuses a symlink, a
/// non-directory or a folder not owned by `owner` (root in production).
/// `Ok(None)`: it does not exist and `create` is false.
fn open_forge_dir(home: &str, create: bool, owner: u32) -> Result<Option<OwnedFd>, String> {
    let home_fd = rfs::open(home, dir_open_flags(), rfs::Mode::empty()).map_err(|e| format!("{home}: {e}"))?;
    let path = format!("{home}/{FORGE_DIR}");

    if create {
        match rfs::mkdirat(&home_fd, FORGE_DIR, rfs::Mode::from_raw_mode(0o755)) {
            Ok(()) | Err(Errno::EXIST) => {}
            Err(e) => return Err(format!("{path}: {e}")),
        }
    }

    let fd = match rfs::openat(&home_fd, FORGE_DIR, dir_open_flags(), rfs::Mode::empty()) {
        Ok(fd) => fd,
        Err(Errno::NOENT) if !create => return Ok(None),
        Err(Errno::LOOP | Errno::NOTDIR) => {
            return Err(format!("{path} is a symlink or not a directory; remove it and deploy again"));
        }
        Err(e) => return Err(format!("{path}: {e}")),
    };

    let st = rfs::fstat(&fd).map_err(|e| format!("{path}: {e}"))?;
    if st.st_uid != owner {
        return Err(format!("{path} is not owned by root; remove it and deploy again"));
    }

    rfs::fchmod(&fd, rfs::Mode::from_raw_mode(0o755)).map_err(|e| format!("{path}: {e}"))?;

    Ok(Some(fd))
}

/// Writes `name` inside `dir` atomically, never following a link.
fn write_at(dir: &OwnedFd, name: &str, content: &str, mode: u32) -> Result<(), String> {
    use std::io::Write as _;
    let tmp = format!("{name}.tmp");
    let _ = rfs::unlinkat(dir, tmp.as_str(), rfs::AtFlags::empty());
    let flags =
        rfs::OFlags::WRONLY | rfs::OFlags::CREATE | rfs::OFlags::EXCL | rfs::OFlags::NOFOLLOW | rfs::OFlags::CLOEXEC;
    let fd =
        rfs::openat(dir, tmp.as_str(), flags, rfs::Mode::from_raw_mode(mode)).map_err(|e| format!("{tmp}: {e}"))?;
    let mut f = std::fs::File::from(fd);
    f.write_all(content.as_bytes()).and_then(|()| f.sync_all()).map_err(|e| format!("{tmp}: {e}"))?;
    rfs::fchmod(&f, rfs::Mode::from_raw_mode(mode)).map_err(|e| format!("{tmp}: {e}"))?;
    drop(f);

    rfs::renameat(dir, tmp.as_str(), dir, name).map_err(|e| format!("{name}: {e}"))
}

/// Removes `<dir>/bin` and the links in it without following any of them.
fn remove_bin_at(dir: &OwnedFd) -> Result<(), String> {
    match rfs::statat(dir, "bin", rfs::AtFlags::SYMLINK_NOFOLLOW) {
        Err(Errno::NOENT) => return Ok(()),
        Err(e) => return Err(format!("bin: {e}")),
        Ok(st) if rfs::FileType::from_raw_mode(st.st_mode) != rfs::FileType::Directory => {
            return rfs::unlinkat(dir, "bin", rfs::AtFlags::empty()).map_err(|e| format!("bin: {e}"));
        }
        Ok(_) => {}
    }

    let bin = rfs::openat(dir, "bin", dir_open_flags(), rfs::Mode::empty()).map_err(|e| format!("bin: {e}"))?;
    let mut names = Vec::new();

    for entry in rfs::Dir::read_from(&bin).map_err(|e| format!("bin: {e}"))? {
        let entry = entry.map_err(|e| format!("bin: {e}"))?;
        let name = entry.file_name();
        if name.to_bytes() != b"." && name.to_bytes() != b".." {
            names.push(name.to_owned());
        }
    }

    // `bin` only ever holds links; a subdirectory makes this fail, never recurse.
    for n in &names {
        rfs::unlinkat(&bin, n.as_c_str(), rfs::AtFlags::empty()).map_err(|e| format!("bin/{n:?}: {e}"))?;
    }

    rfs::unlinkat(dir, "bin", rfs::AtFlags::REMOVEDIR).map_err(|e| format!("bin: {e}"))
}

/// Writes the launcher and the version-pinned runtime links (e.g. `php` →
/// `/usr/bin/php8.3`) into `<home>/.forge`.
fn write_launcher(spec: &AppSpec, owner: u32) -> Result<(), String> {
    let dir = open_forge_dir(&spec.home(), true, owner)?.ok_or("the launcher folder could not be created")?;
    write_at(&dir, &spec.launcher_file(), &spec.launcher_script(), 0o755)?;
    remove_bin_at(&dir)?;
    let links = spec.runtime_links();
    if !links.is_empty() {
        rfs::mkdirat(&dir, "bin", rfs::Mode::from_raw_mode(0o755)).map_err(|e| format!("bin: {e}"))?;
        let bin = rfs::openat(&dir, "bin", dir_open_flags(), rfs::Mode::empty()).map_err(|e| format!("bin: {e}"))?;
        rfs::fchmod(&bin, rfs::Mode::from_raw_mode(0o755)).map_err(|e| format!("bin: {e}"))?;

        for (name, target) in links {
            rfs::symlinkat(target.as_str(), &bin, name.as_str()).map_err(|e| format!("bin/{name}: {e}"))?;
        }
    }

    Ok(())
}

/// Removes the launcher, if `.forge` is still the folder Forge made.
fn remove_launcher(spec: &AppSpec, owner: u32) -> Result<(), String> {
    if let Some(dir) = open_forge_dir(&spec.home(), false, owner)? {
        let file = spec.launcher_file();
        match rfs::unlinkat(&dir, file.as_str(), rfs::AtFlags::empty()) {
            Ok(()) | Err(Errno::NOENT) => {}
            Err(e) => return Err(format!("{file}: {e}")),
        }
    }

    Ok(())
}

/// Reads `KEY=value` lines (package env files, then the app's own).
fn load_env(spec: &AppSpec) -> Vec<(String, String)> {
    // The node's log listener (D-142); a value the app sets comes later and wins.
    let mut env = vec![(forge_catalog::apps::LOG_URL_ENV.to_owned(), spec.log_url().to_owned())];

    for f in &spec.pkg_env_files() {
        if let Ok(text) = std::fs::read_to_string(format!("/etc/forge/pkgs/{f}.env")) {
            env.extend(text.lines().filter_map(|l| l.split_once('=')).map(|(k, v)| (k.to_owned(), v.to_owned())));
        }
    }

    env.extend(spec.env.iter().cloned());

    env
}

fn spec_path(spec: &AppSpec) -> String {
    format!("/etc/forge/apps/{}/{}{}.json", spec.environment, spec.name, spec.slot_suffix())
}

fn pid_path(spec: &AppSpec) -> String {
    format!("{SUPERVISE_DIR}/{}{}.pid", spec.key(), spec.slot_suffix())
}

fn log_path(spec: &AppSpec) -> String {
    format!("{LOG_DIR}/{}{}.log", spec.key(), spec.slot_suffix())
}

fn supervised_pid(spec: &AppSpec) -> Option<u32> {
    supervised_pid_of(&format!("{}{}", spec.key(), spec.slot_suffix()))
}

/// The live process `forge-agent-helper supervise` runs for the copy named
/// `<environment>-<name>[--<slot>]` on a node without systemd, if any. The PID
/// file is world-readable, so the unprivileged agent can use it too.
pub(crate) fn supervised_pid_of(copy: &str) -> Option<u32> {
    let pid: u32 = std::fs::read_to_string(format!("{SUPERVISE_DIR}/{copy}.pid")).ok()?.trim().parse().ok()?;
    Path::new(&format!("/proc/{pid}")).exists().then_some(pid)
}

async fn write_native(spec: &AppSpec) -> Result<String, String> {
    let user = ensure_user(spec).await?;
    write_app_env(spec, &user).await?;
    write_launcher(spec, 0)?;
    write(&spec_path(spec), &serde_json::to_string(spec).map_err(|e| e.to_string())?, 0o600)?;

    if has_systemd() {
        write(&format!("/etc/systemd/system/{}.service", spec.unit()), &spec.service_unit(&user), 0o644)?;
        sh("systemctl", &["daemon-reload"]).await?;
    }

    Ok(user)
}

async fn native_status(spec: &AppSpec) -> String {
    if has_systemd() {
        let unit = format!("{}.service", spec.unit());
        match sh("systemctl", &["is-active", &unit]).await {
            Ok(s) => s.trim().to_owned(),
            Err(e) => e.rsplit(':').next().unwrap_or("unknown").trim().to_owned(),
        }
    } else if supervised_pid(spec).is_some() {
        "active".into()
    } else {
        "inactive".into()
    }
}

/// Starts the supervisor (no systemd): `forge-agent-helper supervise <spec>` in its own process group.
fn spawn_supervisor(spec: &AppSpec) -> Result<(), String> {
    if supervised_pid(spec).is_some() {
        return Ok(());
    }

    use std::os::unix::process::CommandExt;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;

    // World-readable on purpose: the unprivileged agent's health check reads
    // the PID files. A plain create_dir_all would follow the helper's umask.
    if let Some(parent) = Path::new(SUPERVISE_DIR).parent() {
        mkdir(&parent.to_string_lossy(), 0o755)?;
    }

    mkdir(SUPERVISE_DIR, 0o755)?;
    let child = std::process::Command::new(exe)
        .arg("supervise")
        .arg(spec_path(spec))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map_err(|e| e.to_string())?;

    write(&pid_path(spec), &child.id().to_string(), 0o644)
}

async fn stop_supervisor(spec: &AppSpec) {
    if let Some(pid) = supervised_pid(spec) {
        // The supervisor leads its own process group: stop it and the app.
        let _ = sh("kill", &["-TERM", "--", &format!("-{pid}")]).await;
    }

    let _ = std::fs::remove_file(pid_path(spec));
}

async fn native(action: &str, spec: &AppSpec, tail: u32) -> Result<Value, String> {
    let unit = spec.unit();
    let systemd = has_systemd();
    let mut user = None;

    match action {
        // The account, its folder and its file-explorer store only (F-03):
        // nothing runs and no unit is written. Idempotent.
        "prepare" => user = Some(ensure_user(spec).await?),
        "deploy" => {
            let account = write_native(spec).await?;

            if let Some(a) = &spec.artifact {
                stage(spec, &account, a).await?;
            }

            user = Some(account);

            match (spec.runtime, systemd) {
                (Runtime::Systemd, true) => {
                    sh("systemctl", &["enable", &format!("{unit}.service")]).await?;
                    reset_start_limit(&unit).await;
                    sh("systemctl", &["restart", &format!("{unit}.service")]).await?;
                }
                (Runtime::Systemd, false) => {
                    stop_supervisor(spec).await;
                    spawn_supervisor(spec)?;
                }
                _ => {}
            }
        }
        "start" | "restart" | "stop" => match (spec.runtime, systemd) {
            (_, true) => {
                // An operator's start begins a fresh restart count.
                if action != "stop" {
                    reset_start_limit(&unit).await;
                }

                sh("systemctl", &[action, &format!("{unit}.service")]).await?;
            }
            (Runtime::Systemd, false) => {
                if action != "start" {
                    stop_supervisor(spec).await;
                }

                if action != "stop" {
                    spawn_supervisor(spec)?;
                }
            }
            _ => return Err(format!("`{action}` does not apply to this runtime")),
        },
        "remove" => {
            if systemd {
                let _ = sh("systemctl", &["disable", "--now", &format!("{unit}.service")]).await;
                let _ = std::fs::remove_file(format!("/etc/systemd/system/{unit}.service"));
                let _ = sh("systemctl", &["daemon-reload"]).await;
            } else {
                stop_supervisor(spec).await;
            }

            let _ = std::fs::remove_file(spec.env_path());

            if let Err(e) = remove_launcher(spec, 0) {
                tracing::warn!(app = %spec.key(), error = %e, "launcher left in place");
            }

            let _ = std::fs::remove_file(spec_path(spec));

            // The file explorer's trash and revisions go with the app (D-083),
            // only when the store is a real folder of this app's account.
            remove_app_store(spec).await;

            // Code and data under /srv/forge/apps are kept.
            return Ok(json!({ "state": "removed" }));
        }
        "retire" => {
            if systemd {
                let _ = sh("systemctl", &["disable", "--now", &format!("{unit}.service")]).await;
                let _ = std::fs::remove_file(format!("/etc/systemd/system/{unit}.service"));
                let _ = sh("systemctl", &["daemon-reload"]).await;
            } else {
                stop_supervisor(spec).await;
            }

            let _ = std::fs::remove_file(spec.env_path());

            if let Err(e) = remove_launcher(spec, 0) {
                tracing::warn!(app = %spec.key(), error = %e, "launcher left in place");
            }

            let _ = std::fs::remove_file(spec_path(spec));

            return Ok(json!({ "state": "retired" }));
        }
        "probe" => return Ok(probe(spec, native_ready(spec).await).await),
        "status" | "logs" => {}
        other => return Err(format!("unknown action `{other}`")),
    }

    let state = native_status(spec).await;
    let logs = if action == "logs" { Some(native_logs(spec, tail).await) } else { None };

    // `user`: the account the app runs as, for the deploy's audit record.
    Ok(json!({ "state": state, "logs": logs, "user": user }))
}

/// How long one `follow` segment runs; the console chains segments.
const FOLLOW_SEGMENT: std::time::Duration = std::time::Duration::from_secs(50);
const FOLLOW_POLL: std::time::Duration = std::time::Duration::from_secs(1);
/// Lines kept in a segment's final result (they were also streamed live).
const FOLLOW_KEEP: usize = 64 * 1024;

/// Streams new log lines for up to [`FOLLOW_SEGMENT`] through `emit`, polling
/// every second from `cursor`. Polling by position (journal cursor, file
/// offset, Docker timestamp) rather than holding a stream open is what keeps
/// it going across stop / start / restart: the next poll simply picks up the
/// lines of the new process, startup output included.
///
/// Returns `{cursor, lines, state}`: where to continue, and this segment's
/// lines (capped) so none are lost between segments.
pub async fn follow(spec: &AppSpec, cursor: &str, emit: &(dyn Fn(&str) + Sync)) -> Result<Value, String> {
    spec.validate()?;
    let deadline = tokio::time::Instant::now() + FOLLOW_SEGMENT;
    let mut cursor = cursor.to_owned();
    let mut kept = String::new();
    let out = |text: &str, kept: &mut String| {
        for l in text.lines().filter(|l| !l.is_empty()) {
            emit(l);

            if kept.len() < FOLLOW_KEEP {
                kept.push_str(l);
                kept.push('\n');
            }
        }
    };

    loop {
        let text = match spec.runtime {
            Runtime::Container => follow_container(spec, &mut cursor).await?,
            Runtime::Systemd if has_systemd() => follow_journal(spec, &mut cursor).await?,
            Runtime::Systemd => follow_file(spec, &mut cursor),
        };

        out(&text, &mut kept);

        if tokio::time::Instant::now() >= deadline {
            break;
        }

        tokio::time::sleep(FOLLOW_POLL).await;
    }

    let state = match spec.runtime {
        Runtime::Container => Value::Null,
        _ => json!(native_status(spec).await),
    };

    Ok(json!({ "cursor": cursor, "lines": kept, "state": state }))
}

/// systemd journal of the unit, after the cursor (first poll: last 200 lines).
async fn follow_journal(spec: &AppSpec, cursor: &mut String) -> Result<String, String> {
    let unit = format!("{}.service", spec.unit());
    let mut args = vec!["-u", unit.as_str(), "--no-pager", "-o", "short-iso", "--show-cursor"];
    let after;

    if cursor.is_empty() {
        args.extend(["-n", "200"]);
    } else {
        after = format!("--after-cursor={cursor}");
        args.push(&after);
    }

    let raw = sh("journalctl", &args).await?;
    let mut lines = Vec::new();

    for l in raw.lines() {
        if let Some(c) = l.strip_prefix("-- cursor: ") {
            c.trim().clone_into(cursor);
        } else if l != "-- No entries --" {
            lines.push(l);
        }
    }

    Ok(lines.join("\n"))
}

/// Supervisor log file (nodes without systemd), from a byte offset.
fn follow_file(spec: &AppSpec, cursor: &mut String) -> String {
    let text = std::fs::read(log_path(spec)).unwrap_or_default();
    let mut from = cursor.parse::<usize>().unwrap_or_else(|_| {
        // First poll: about the last 200 lines.
        let nl: Vec<usize> = text.iter().enumerate().filter(|(_, b)| **b == b'\n').map(|(i, _)| i + 1).collect();
        nl.len().checked_sub(201).map_or(0, |i| nl[i])
    });

    if from > text.len() {
        from = 0; // the file was truncated or replaced
    }

    *cursor = text.len().to_string();

    String::from_utf8_lossy(&text[from..]).into_owned()
}

/// Docker logs in the window since the last poll. The cursor is the end of
/// the previous window (Unix seconds); a missing container yields nothing
/// and the window waits for it to come back.
async fn follow_container(spec: &AppSpec, cursor: &mut String) -> Result<String, String> {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
    let since = cursor.parse::<f64>().unwrap_or(now - 3600.0);
    let first = cursor.is_empty();

    match crate::docker::logs_window(&spec.unit(), since, now).await? {
        Some(text) => {
            *cursor = format!("{now:.6}");

            if first {
                let lines: Vec<&str> = text.lines().collect();
                return Ok(lines[lines.len().saturating_sub(200)..].join("\n"));
            }

            Ok(text)
        }
        None => Ok(String::new()),
    }
}

async fn native_logs(spec: &AppSpec, tail: u32) -> String {
    let n = tail.clamp(1, 5000).to_string();

    if has_systemd() {
        sh("journalctl", &["-u", &format!("{}.service", spec.unit()), "-n", &n, "--no-pager", "-o", "short-iso"])
            .await
            .unwrap_or_else(|e| e)
    } else {
        let text = std::fs::read_to_string(log_path(spec)).unwrap_or_default();
        let lines: Vec<&str> = text.lines().collect();
        lines[lines.len().saturating_sub(tail as usize)..].join("\n")
    }
}

/// One run of the launcher as the app user, with the app's environment.
async fn run_once(spec: &AppSpec, limit: Duration) -> Result<i32, String> {
    mkdir("/var/log/forge", 0o755)?;
    mkdir(LOG_DIR, 0o750)?;
    let log = std::fs::OpenOptions::new().create(true).append(true).open(log_path(spec)).map_err(|e| e.to_string())?;
    let err = log.try_clone().map_err(|e| e.to_string())?;
    let wd = if spec.working_dir.is_empty() { spec.home() } else { spec.working_dir.clone() };

    // Resolved before env_clear(): the child's PATH would not find /usr/sbin.
    let runuser = ["/usr/sbin/runuser", "/sbin/runuser", "/usr/bin/runuser"]
        .into_iter()
        .find(|p| Path::new(p).exists())
        .ok_or("runuser (util-linux) is not installed")?;
    let user = find_user(spec).await?.ok_or("the app's account is missing; deploy the app again")?;
    let mut child = Command::new(runuser)
        .args(["-u", &user, "--", &spec.launcher()])
        .current_dir(&wd)
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("HOME", spec.home())
        .env("LANG", "C.UTF-8")
        .envs(load_env(spec))
        .stdin(Stdio::null())
        .stdout(log)
        .stderr(err)
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| e.to_string())?;
    let status = tokio::time::timeout(limit, child.wait())
        .await
        .map_err(|_| "timed out".to_owned())?
        .map_err(|e| e.to_string())?;

    Ok(status.code().unwrap_or(-1))
}

/// Clears systemd's start-rate counter for `unit`, so a deploy or an
/// operator's start is not refused because of earlier starts: the unit's
/// `StartLimitBurst` should count automatic restarts after it, not every
/// start in the window. Best effort: a unit that never failed has nothing
/// to reset, and `systemctl` reports that as an error.
async fn reset_start_limit(unit: &str) {
    let _ = sh("systemctl", &["reset-failed", &format!("{unit}.service")]).await;
}

/// Whether the supervisor starts the app again after it ended with `exit`
/// (`None`: it could not be started), having restarted it `restarts` times.
fn restart_again(spec: &AppSpec, exit: Option<i32>, restarts: u32) -> bool {
    use forge_catalog::apps::RestartPolicy;

    let max = spec.restart_max_attempts;

    match spec.effective_restart() {
        RestartPolicy::No => false,
        RestartPolicy::Always | RestartPolicy::UnlessStopped => true,
        RestartPolicy::OnFailure => exit != Some(0) && (max == 0 || restarts < max),
    }
}

/// `forge-agent-helper supervise <spec.json>`: keeps a native service running
/// where there is no systemd. It starts the app again 3 s after it ends, as
/// its restart policy says: `on-failure` only after a non-zero exit and at
/// most `restart_max_attempts` times (`0` = no limit).
pub async fn supervise(spec_file: &str) -> Result<(), String> {
    let text = std::fs::read_to_string(spec_file).map_err(|e| e.to_string())?;
    let spec: AppSpec = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    spec.validate()?;
    let mut restarts = 0u32;

    loop {
        // Errors starting the app go to its log, where the console shows them.
        let (exit, note) = match run_once(&spec, Duration::from_secs(365 * 24 * 3600)).await {
            Ok(code) => (Some(code), format!("forge: process exited with status {code}")),
            Err(e) => (None, format!("forge: could not start the app: {e}")),
        };

        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(log_path(&spec)) {
            use std::io::Write;
            let _ = writeln!(f, "{note}");
        }

        if !restart_again(&spec, exit, restarts) {
            return Ok(());
        }

        restarts = restarts.saturating_add(1);
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

/// Resource limits, identity, health check and logging from the spec.
fn apply_container_options(body: &mut Value, spec: &AppSpec) {
    let host = &mut body["HostConfig"];

    if spec.cpu_millis > 0 {
        host["NanoCpus"] = json!(u64::from(spec.cpu_millis) * 1_000_000);
    }

    if spec.memory_mb > 0 {
        host["Memory"] = json!(u64::from(spec.memory_mb) * 1024 * 1024);
    }

    if spec.read_only {
        host["ReadonlyRootfs"] = json!(true);
    }

    // `forge-node` reaches the node's log listener (D-142), unless the app
    // names that host itself.
    let mut hosts = spec.extra_hosts.clone();
    if !hosts.iter().any(|h| h.split(':').next() == Some("forge-node")) {
        hosts.push(forge_catalog::apps::LOG_HOST_ENTRY.to_owned());
    }

    host["ExtraHosts"] = json!(hosts);

    if spec.log_max_mb > 0 {
        host["LogConfig"] =
            json!({ "Type": "json-file", "Config": { "max-size": format!("{}m", spec.log_max_mb), "max-file": "3" } });
    }

    if !spec.entrypoint.trim().is_empty() {
        body["Entrypoint"] = json!(spec.entrypoint.split_whitespace().collect::<Vec<_>>());
    }

    if !spec.user.is_empty() {
        body["User"] = json!(spec.user);
    }

    if !spec.hostname.is_empty() {
        body["Hostname"] = json!(spec.hostname);
    }

    if spec.stop_timeout_secs > 0 {
        body["StopTimeout"] = json!(spec.stop_timeout_secs);
    }

    for (k, v) in &spec.labels {
        body["Labels"][k] = json!(v);
    }

    apply_runtime_options(body, spec);

    if let Some(hc) = &spec.healthcheck {
        let ns = |s: u32| u64::from(s) * 1_000_000_000;
        body["Healthcheck"] = json!({
            "Test": ["CMD-SHELL", hc.command],
            "Interval": ns(hc.interval_secs),
            "Timeout": ns(hc.timeout_secs),
            "Retries": hc.retries,
            "StartPeriod": ns(hc.start_period_secs),
        });
    }
}

/// Capabilities, ulimits, tmpfs, DNS, sysctls, devices and init
/// (`plans/service-orchestration.md` step 1). The server approved the
/// privileged ones; the spec was validated on both sides.
fn apply_runtime_options(body: &mut Value, spec: &AppSpec) {
    let host = &mut body["HostConfig"];
    let cap = |c: &String| c.strip_prefix("CAP_").unwrap_or(c).to_owned();

    if !spec.cap_add.is_empty() {
        host["CapAdd"] = json!(spec.cap_add.iter().map(cap).collect::<Vec<_>>());
    }

    if !spec.cap_drop.is_empty() {
        host["CapDrop"] = json!(spec.cap_drop.iter().map(cap).collect::<Vec<_>>());
    }

    if !spec.ulimits.is_empty() {
        let u: Vec<Value> =
            spec.ulimits.iter().map(|u| json!({ "Name": u.name, "Soft": u.soft, "Hard": u.hard })).collect();
        host["Ulimits"] = json!(u);
    }

    for t in &spec.tmpfs {
        let opts = if t.size_mb > 0 { format!("size={}m", t.size_mb) } else { String::new() };
        host["Tmpfs"][&t.target] = json!(opts);
    }

    if !spec.dns.is_empty() {
        host["Dns"] = json!(spec.dns.iter().map(ToString::to_string).collect::<Vec<_>>());
    }

    for (k, v) in &spec.sysctls {
        host["Sysctls"][k] = json!(v);
    }

    if !spec.devices.is_empty() {
        let d: Vec<Value> = spec
            .devices
            .iter()
            .map(|d| {
                let inside = if d.container.is_empty() { &d.host } else { &d.container };
                let perms = if d.permissions.is_empty() { "rwm" } else { &d.permissions };

                json!({ "PathOnHost": d.host, "PathInContainer": inside, "CgroupPermissions": perms })
            })
            .collect();
        host["Devices"] = json!(d);
    }

    if spec.init {
        host["Init"] = json!(true);
    }
}

async fn container(action: &str, spec: &AppSpec, tail: u32) -> Result<Value, String> {
    let name = spec.unit();
    let t = Duration::from_secs(90);

    match action {
        // Containers keep their files in volumes: nothing to prepare.
        "prepare" => return Ok(json!({ "state": "prepared" })),
        "deploy" => {
            write(&spec.env_path(), &spec.env_file(), 0o600)?;
            let (from, tag, image_ref) = pull_ref(&spec.image);
            pull(&from, &tag).await?;
            let _ = docker::api("DELETE", &format!("/containers/{name}?force=1"), None, t).await;
            let mut bindings = serde_json::Map::new();
            let mut exposed = serde_json::Map::new();

            for p in &spec.ports {
                let (body, proto) = p.split_once('/').unwrap_or((p, "tcp"));
                let parts: Vec<&str> = body.split(':').collect();
                let (ip, host, ctr) = match parts.as_slice() {
                    [h, c] => ("", *h, *c),
                    [ip, h, c] => (*ip, *h, *c),
                    _ => continue,
                };

                let key = format!("{ctr}/{proto}");
                exposed.insert(key.clone(), json!({}));
                bindings.insert(key, json!([{ "HostIp": ip, "HostPort": host }]));
            }

            // Host folders: the policy again (Docker socket, system and
            // Forge paths), made without following links, then the
            // resolved path checked too (see `stacks::walk_host_path`).
            for v in &spec.volumes {
                // Named volumes live in Docker's own storage: no host path.
                let Some(host) = forge_catalog::apps::volume_host_path(v) else { continue };
                if let Some(why) = forge_catalog::stacks::protected_host_path(host) {
                    return Err(format!("volume `{v}` is not allowed: {why}"));
                }

                crate::stacks::walk_host_path(host, true)?;
                let real = std::fs::canonicalize(host).map_err(|e| format!("{host}: {e}"))?;
                if let Some(why) = forge_catalog::stacks::protected_host_path(&real.to_string_lossy()) {
                    return Err(format!("volume `{v}` resolves to {}: {why}", real.display()));
                }
            }

            let env: Vec<String> = load_env(spec).into_iter().map(|(k, v)| format!("{k}={v}")).collect();
            let mut body = json!({
                "Image": image_ref,
                "Env": env,
                "Labels": {
                    "forge.app": format!("{}/{}", spec.environment, spec.name),
                    "forge.slot": spec.slot,
                    "forge.release": spec.release,
                },
                "ExposedPorts": exposed,
                "HostConfig": {
                    "PortBindings": bindings,
                    "Binds": spec.volumes,
                    "RestartPolicy": {
                        "Name": spec.effective_restart().docker_name(),
                        "MaximumRetryCount": spec.restart_max_attempts,
                    },
                },
            });

            if !spec.command.trim().is_empty() {
                body["Cmd"] = json!(["/bin/sh", "-c", spec.command.trim()]);
            }

            apply_container_options(&mut body, spec);
            docker::api("POST", &format!("/containers/create?name={name}"), Some(&body), t).await?;

            // The cluster container network as a second network, so the
            // default bridge (and its published ports) stays as it was.
            if let Some(net) = &spec.mesh_network {
                let connect = json!({ "Container": name });
                docker::api("POST", &format!("/networks/{net}/connect"), Some(&connect), t).await?;
            }

            // Extra networks the operator named; they must already exist.
            for net in &spec.networks {
                let connect = json!({ "Container": name });
                docker::api("POST", &format!("/networks/{net}/connect"), Some(&connect), t)
                    .await
                    .map_err(|e| format!("network `{net}`: {e}"))?;
            }

            docker::api("POST", &format!("/containers/{name}/start"), None, t).await?;
        }
        "start" | "stop" | "restart" => {
            docker::api("POST", &format!("/containers/{name}/{action}"), None, t)
                .await?
                .ok_or("container not found")?;
        }
        "remove" => {
            let _ = docker::api("DELETE", &format!("/containers/{name}?force=1"), None, t).await?;
            let _ = std::fs::remove_file(spec.env_path());

            return Ok(json!({ "state": "removed" }));
        }
        "retire" => {
            let _ = docker::api("DELETE", &format!("/containers/{name}?force=1"), None, t).await?;
            let _ = std::fs::remove_file(spec.env_path());

            return Ok(json!({ "state": "retired" }));
        }
        "probe" => return Ok(probe(spec, container_ready(&name).await).await),
        "status" | "logs" => {}
        other => return Err(format!("unknown action `{other}`")),
    }

    let state = match docker::api("GET", &format!("/containers/{name}/json"), None, t).await? {
        Some(v) => v.pointer("/State/Status").and_then(Value::as_str).unwrap_or("unknown").to_owned(),
        None => "not-deployed".to_owned(),
    };

    let logs = if action == "logs" { Some(docker::logs(&name, tail.clamp(1, 5000)).await?) } else { None };

    // The digest the image resolved to: a release pinned by tag records it,
    // and every other node then pulls exactly that (#73).
    let digest = if action == "deploy" { image_digest(&spec.image).await } else { None };

    Ok(json!({
        "state": if state == "running" { "active".to_owned() } else { state },
        "logs": logs,
        "image_digest": digest,
    }))
}

/// Pulls an image; a registry that times out or drops the connection is
/// tried again (three attempts in about 30 s), so one network blip does not
/// fail a deploy. Other errors (unknown image, denied) fail at once.
async fn pull(from: &str, tag: &str) -> Result<(), String> {
    let path = format!("/images/create?fromImage={from}&tag={tag}");
    let mut last = String::new();

    for attempt in 0..3u64 {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_secs(5 * attempt * attempt)).await;
        }

        match docker::api("POST", &path, None, Duration::from_secs(20 * 60)).await {
            Ok(_) => return Ok(()),
            Err(e) if transient_pull_error(&e) => last = e,
            Err(e) => return Err(e),
        }
    }

    Err(format!("pulling {from}:{tag} failed three times: {last}"))
}

/// A registry error worth another try: timeouts and dropped connections.
fn transient_pull_error(e: &str) -> bool {
    let e = e.to_ascii_lowercase();
    [
        "timeout",
        "timed out",
        "connection reset",
        "connection refused",
        "eof",
        "tls handshake",
        "temporary failure",
        "i/o timeout",
        "502",
        "503",
        "504",
    ]
    .iter()
    .any(|m| e.contains(m))
}

/// `(fromImage, tag, reference)` for pulling and running `image`:
/// `repo:tag`, `repo@sha256:…` or `repo:tag@sha256:…` (pulled by digest).
fn pull_ref(image: &str) -> (String, String, String) {
    if let Some((name, digest)) = image.split_once('@') {
        // The tag is for people; the digest decides what runs.
        let repo = match name.rsplit_once(':') {
            Some((n, t)) if !t.contains('/') => n,
            _ => name,
        };

        return (repo.to_owned(), digest.to_owned(), format!("{repo}@{digest}"));
    }

    match image.rsplit_once(':') {
        Some((n, t)) if !t.contains('/') => (n.to_owned(), t.to_owned(), image.to_owned()),
        _ => (image.to_owned(), "latest".to_owned(), format!("{image}:latest")),
    }
}

/// The registry digest (`sha256:…`) of a pulled image, from its RepoDigests.
async fn image_digest(image: &str) -> Option<String> {
    let (_, _, reference) = pull_ref(image);
    let v = docker::api("GET", &format!("/images/{reference}/json"), None, Duration::from_secs(15)).await.ok()??;

    v["RepoDigests"]
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .find_map(|d| d.split_once('@').map(|(_, h)| h.to_owned()))
}

/// The default check of a container copy: running, and not unhealthy by
/// its image HEALTHCHECK.
async fn container_ready(name: &str) -> (bool, String) {
    match docker::api("GET", &format!("/containers/{name}/json"), None, Duration::from_secs(10)).await {
        Ok(Some(v)) => {
            let running = v.pointer("/State/Running").and_then(Value::as_bool).unwrap_or(false);
            let health = v.pointer("/State/Health/Status").and_then(Value::as_str).unwrap_or("");

            if !running {
                let s = v.pointer("/State/Status").and_then(Value::as_str).unwrap_or("not running");
                (false, format!("the container is {s}"))
            } else if health == "unhealthy" || health == "starting" {
                (false, format!("the image HEALTHCHECK reports {health}"))
            } else {
                (true, String::new())
            }
        }
        Ok(None) => (false, "the container does not exist".into()),
        Err(e) => (false, e),
    }
}

/// The default check of a native copy: the unit (or supervised process) is
/// active, and its port accepts connections when it declares one.
async fn native_ready(spec: &AppSpec) -> (bool, String) {
    let state = native_status(spec).await;
    if state != "active" {
        return (false, format!("{} is {state}", spec.unit()));
    }

    if let Some(port) = spec.listen_port {
        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
        match tokio::time::timeout(Duration::from_secs(2), tokio::net::TcpStream::connect(addr)).await {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => return (false, format!("port {port}: {e}")),
            Err(_) => return (false, format!("port {port}: no answer within 2 s")),
        }
    }

    (true, String::new())
}

/// Most bytes a release archive may have (2 GiB).
const MAX_ARCHIVE: u64 = 2 * 1024 * 1024 * 1024;
/// Release folders kept per app (older ones are removed when a new one is staged).
const KEEP_RELEASES: u64 = 5;

/// Runs `args` as the app's own account, with its home as `HOME`.
async fn as_app(user: &str, home: &str, args: &[&str]) -> Result<String, String> {
    let mut full = vec!["-u", user, "--", "env", "GIT_TERMINAL_PROMPT=0"];
    let home_env = format!("HOME={home}");
    full.push(&home_env);
    full.extend_from_slice(args);

    sh("runuser", &full).await
}

/// Fetches a release's code into `<home>/releases/<n>` (#73), once: a
/// pinned Git commit, or an archive checked against its SHA-256. Download
/// and unpacking run as the app's own account, so a hostile archive can
/// only touch what that account can write anyway; root only checks the
/// result (without following links) and renames it into place.
async fn stage(spec: &AppSpec, user: &str, a: &forge_catalog::deploy::ArtifactRef) -> Result<(), String> {
    use forge_catalog::deploy::{ArchiveKind, ArtifactRef};
    a.validate()?;
    let home = spec.home();
    let releases = format!("{home}/releases");
    let dir = spec.release_dir();
    let marker = format!("{dir}/.forge-release");

    let want = match a {
        ArtifactRef::Git { commit, .. } => format!("git {commit}"),
        ArtifactRef::Archive { sha256, .. } => format!("sha256 {sha256}"),
        ArtifactRef::Image { .. } => return Err("an image is not fetched into a folder".into()),
    };

    if crate::self_update::read_nofollow(Path::new(&marker), 4096)
        .is_some_and(|m| String::from_utf8_lossy(&m).trim() == want)
    {
        return Ok(());
    }

    as_app(user, &home, &["mkdir", "-p", &releases]).await?;
    let tmp = format!("{releases}/.staging-{}", spec.release);
    let _ = as_app(user, &home, &["rm", "-rf", &tmp]).await;

    match a {
        ArtifactRef::Git { repo, commit, .. } => {
            as_app(user, &home, &["git", "init", "-q", &tmp]).await?;
            as_app(user, &home, &["git", "-C", &tmp, "fetch", "-q", "--depth", "1", repo, commit])
                .await
                .map_err(|e| format!("git fetch {commit} failed (is git installed and the commit reachable?): {e}"))?;
            as_app(
                user,
                &home,
                &["git", "-C", &tmp, "-c", "advice.detachedHead=false", "checkout", "-q", "FETCH_HEAD"],
            )
            .await?;
            let head = as_app(user, &home, &["git", "-C", &tmp, "rev-parse", "HEAD"]).await?;
            if head.trim() != commit {
                return Err(format!("checked out {} instead of {commit}", head.trim()));
            }
        }
        ArtifactRef::Archive { url, sha256, strip_components } => {
            let file = format!("{releases}/.download-{}", spec.release);

            // A retried step starts from a fresh download.
            let _ = as_app(user, &home, &["rm", "-f", &file]).await;

            // curl streams to disk as the app's account. `prlimit --fsize`
            // stops it at the cap even when the server sends no
            // Content-Length (`--max-filesize` alone cannot).
            let max = MAX_ARCHIVE.to_string();
            let fsize = format!("--fsize={max}:{max}");
            as_app(
                user,
                &home,
                &[
                    "prlimit",
                    &fsize,
                    "curl",
                    "-fsSL",
                    "--proto",
                    "=https",
                    "--max-redirs",
                    "5",
                    "--max-time",
                    "1800",
                    "--max-filesize",
                    &max,
                    "-o",
                    &file,
                    url,
                ],
            )
            .await
            .map_err(|e| format!("download failed (archives are limited to 2 GiB): {e}"))?;

            // Hashed in 64 KiB chunks off the async runtime: memory stays
            // flat whatever the archive's size.
            let path = std::path::PathBuf::from(&file);
            let hashed = tokio::task::spawn_blocking(move || crate::self_update::sha256_nofollow(&path, MAX_ARCHIVE))
                .await
                .map_err(|e| format!("hashing the archive stopped: {e}"))?;

            let got = match hashed {
                Ok((got, _)) => got,
                Err(e) => {
                    let _ = tokio::fs::remove_file(&file).await;
                    return Err(format!("cannot read the downloaded archive: {e}"));
                }
            };

            if got != *sha256 {
                let _ = tokio::fs::remove_file(&file).await;
                return Err(format!("the archive's SHA-256 is {got}, not {sha256}: refused"));
            }

            as_app(user, &home, &["mkdir", "-p", &tmp]).await?;
            let strip = format!("--strip-components={strip_components}");

            let r = match ArchiveKind::of(url) {
                Some(ArchiveKind::TarGz) => {
                    as_app(
                        user,
                        &home,
                        &["tar", "-xzf", &file, "-C", &tmp, "--no-same-owner", "--no-same-permissions", &strip],
                    )
                    .await
                }
                Some(ArchiveKind::Tar) => {
                    as_app(
                        user,
                        &home,
                        &["tar", "-xf", &file, "-C", &tmp, "--no-same-owner", "--no-same-permissions", &strip],
                    )
                    .await
                }
                Some(ArchiveKind::Zip) => {
                    let r = as_app(user, &home, &["unzip", "-q", &file, "-d", &tmp])
                        .await
                        .map_err(|e| format!("unzip failed (is unzip installed?): {e}"));

                    if r.is_ok() && *strip_components > 0 {
                        strip_zip(user, &home, &tmp, *strip_components).await?;
                    }

                    r
                }
                None => Err("unknown archive type".into()),
            };

            let _ = tokio::fs::remove_file(&file).await;
            r?;
        }
        ArtifactRef::Image { .. } => {}
    }

    as_app(user, &home, &["sh", "-c", "printf '%s\\n' \"$1\" > \"$2\"", "sh", &want, &format!("{tmp}/.forge-release")])
        .await?;
    let _ = as_app(user, &home, &["rm", "-rf", &dir]).await;
    as_app(user, &home, &["mv", &tmp, &dir]).await?;
    prune_releases(user, &home, &releases, spec.release.parse().unwrap_or(0)).await;

    Ok(())
}

/// Drops `n` leading folders of an unzipped tree (each level must be one folder).
async fn strip_zip(user: &str, home: &str, tmp: &str, n: u32) -> Result<(), String> {
    for _ in 0..n {
        let entries: Vec<_> = std::fs::read_dir(tmp).map_err(|e| e.to_string())?.filter_map(Result::ok).collect();
        let [only] = entries.as_slice() else { return Err("stripComponents needs one top folder in the zip".into()) };
        if !only.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            return Err("stripComponents needs one top folder in the zip".into());
        }

        let inner = only.path().to_string_lossy().into_owned();
        let up = format!("{tmp}.up");
        as_app(user, home, &["mv", &inner, &up]).await?;
        as_app(user, home, &["rmdir", tmp]).await?;
        as_app(user, home, &["mv", &up, tmp]).await?;
    }

    Ok(())
}

/// Removes release folders older than the newest [`KEEP_RELEASES`] (as the app).
async fn prune_releases(user: &str, home: &str, releases: &str, current: u64) {
    let Ok(rd) = std::fs::read_dir(releases) else { return };

    for e in rd.filter_map(Result::ok) {
        let name = e.file_name().to_string_lossy().into_owned();
        if let Ok(n) = name.parse::<u64>()
            && n + KEEP_RELEASES <= current
        {
            let _ = as_app(user, home, &["rm", "-rf", &format!("{releases}/{name}")]).await;
        }
    }
}

/// One readiness probe of a copy (#73): the default check, then each
/// required HTTP and TCP readiness check of the workload. Checks that run
/// code, reach another host or send vault-backed headers are left to the
/// node's regular health runner and listed as skipped.
async fn probe(spec: &AppSpec, default: (bool, String)) -> Value {
    use forge_catalog::health::Probe;
    let mut checks = vec![json!({ "name": "default", "ok": default.0, "detail": default.1 })];
    let mut ready = default.0;

    if let Some(policy) = &spec.health {
        for c in policy.readiness.iter().filter(|c| c.enabled) {
            let skip = match &c.probe {
                Probe::Http(h) => {
                    !forge_catalog::health::workload_host(&h.host) || h.headers.iter().any(|x| x.secret.is_some())
                }
                Probe::Tcp(t) => !forge_catalog::health::workload_host(&t.host),
                Probe::Exec(_) | Probe::Shell(_) => true,
            };

            if skip {
                checks.push(json!({ "name": c.name, "ok": null, "detail": "checked by the node's health runner" }));
                continue;
            }

            let timeout = Duration::from_secs(u64::from(c.timeout_secs.max(1)));
            let ctx = crate::health_probe::Context { fleet: &[], secrets: &[] };
            let o = crate::health_probe::run(&c.probe, timeout, ctx).await;
            let ok = o.verdict == crate::health_probe::Verdict::Pass;
            if c.required && !ok {
                ready = false;
            }

            checks.push(json!({ "name": c.name, "ok": ok, "required": c.required, "detail": o.detail }));
        }
    }

    json!({ "ready": ready, "checks": checks })
}

/// Runs one action for a validated spec.
pub async fn run(action: &str, spec: &AppSpec, tail: u32) -> Result<Value, String> {
    spec.validate()?;

    match spec.runtime {
        Runtime::Container => container(action, spec, tail).await,
        Runtime::Systemd => native(action, spec, tail).await,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn me() -> u32 {
        rustix::process::getuid().as_raw()
    }

    #[test]
    fn container_options_map_onto_the_docker_create_body() {
        let spec: AppSpec = serde_json::from_value(json!({
            "environment": "uat", "name": "cache", "runtime": "container", "image": "redis:7",
            "cap_add": ["CAP_NET_ADMIN"], "cap_drop": ["ALL"],
            "ulimits": [{"name": "nofile", "soft": 1024, "hard": 4096}],
            "tmpfs": [{"target": "/tmp", "size_mb": 64}, {"target": "/run"}],
            "dns": ["192.0.2.53"], "sysctls": [["net.core.somaxconn", "1024"]],
            "devices": [{"host": "/dev/fuse"}], "init": true,
        }))
        .unwrap();
        let mut body = json!({ "HostConfig": {} });
        apply_runtime_options(&mut body, &spec);
        let h = &body["HostConfig"];

        assert_eq!(h["CapAdd"], json!(["NET_ADMIN"]));
        assert_eq!(h["CapDrop"], json!(["ALL"]));
        assert_eq!(h["Ulimits"], json!([{ "Name": "nofile", "Soft": 1024, "Hard": 4096 }]));
        assert_eq!(h["Tmpfs"], json!({ "/tmp": "size=64m", "/run": "" }));
        assert_eq!(h["Dns"], json!(["192.0.2.53"]));
        assert_eq!(h["Sysctls"], json!({ "net.core.somaxconn": "1024" }));
        assert_eq!(
            h["Devices"],
            json!([{ "PathOnHost": "/dev/fuse", "PathInContainer": "/dev/fuse", "CgroupPermissions": "rwm" }])
        );
        assert_eq!(h["Init"], json!(true));
    }

    #[test]
    fn the_supervisor_follows_the_restart_policy() {
        let spec = |v: Value| -> AppSpec {
            let mut b = json!({ "environment": "uat", "name": "w", "runtime": "systemd", "command": "/usr/bin/true" });

            for (k, x) in v.as_object().unwrap() {
                b[k] = x.clone();
            }

            serde_json::from_value(b).unwrap()
        };

        let legacy = spec(json!({}));
        assert!(restart_again(&legacy, Some(0), 99), "the old flag restarts always");

        let on_failure = spec(json!({ "restart_policy": "on-failure", "restart_max_attempts": 2 }));
        assert!(!restart_again(&on_failure, Some(0), 0), "a clean exit is not restarted");
        assert!(restart_again(&on_failure, Some(1), 0));
        assert!(restart_again(&on_failure, None, 1), "a failed start counts as a failure");
        assert!(!restart_again(&on_failure, Some(1), 2), "the limit is reached");

        let unlimited = spec(json!({ "restart_policy": "on-failure" }));
        assert!(restart_again(&unlimited, Some(1), 10_000));
        assert!(!restart_again(&spec(json!({ "restart_policy": "no" })), Some(1), 0));
    }

    #[test]
    fn a_spec_without_options_adds_nothing() {
        let spec: AppSpec = serde_json::from_value(json!({
            "environment": "uat", "name": "web", "runtime": "container", "image": "nginx:1.27",
        }))
        .unwrap();
        let mut body = json!({ "HostConfig": {} });
        apply_runtime_options(&mut body, &spec);

        assert_eq!(body, json!({ "HostConfig": {} }));
        assert_eq!(spec.effective_restart().docker_name(), "unless-stopped");
    }

    #[test]
    fn only_network_blips_are_pulled_again() {
        assert!(transient_pull_error(
            "Docker: Get \"https://registry-1.docker.io/v2/\": net/http: request canceled (Client.Timeout exceeded while awaiting headers) (HTTP 500)"
        ));
        assert!(transient_pull_error("read tcp 10.0.0.2:5000: connection reset by peer"));
        assert!(!transient_pull_error("pull access denied for nosuch/image, repository does not exist"));
        assert!(!transient_pull_error("manifest unknown"));
    }

    #[test]
    fn passwd_lines_are_matched_by_name() {
        let line = "staging-api:x:998:997:Orbyn Forge app staging/api:/srv/forge/apps/staging/api:/usr/sbin/nologin\n";
        assert_eq!(
            parse_passwd("staging-api", line),
            Some(Account {
                uid: 998,
                gid: 997,
                gecos: "Orbyn Forge app staging/api".into(),
                home: "/srv/forge/apps/staging/api".into(),
                shell: "/usr/sbin/nologin".into(),
            })
        );
        assert_eq!(parse_passwd("staging", line), None, "a prefix is another account");
        assert_eq!(parse_passwd("x", "x:x:1"), None, "short line");
    }

    #[tokio::test]
    async fn only_accounts_shaped_like_forge_ones_are_adopted() {
        let spec: AppSpec = serde_json::from_value(serde_json::json!({
            "environment": "staging", "name": "api", "runtime": "systemd", "command": "true"
        }))
        .unwrap();
        let good = Account {
            uid: 998,
            gid: 997,
            gecos: spec.user_mark(),
            home: spec.home(),
            shell: "/usr/sbin/nologin".into(),
        };

        // The test home does not exist here, so ownership is not checked.
        assert!(shaped_like_app_account(&spec.home(), &good).await);
        let human = Account { home: "/home/alice".into(), shell: "/bin/bash".into(), uid: 1000, gid: 1000, ..good };
        assert!(!shaped_like_app_account(&spec.home(), &human).await, "a user who set the mark with chfn");
        let mut root = Account { uid: 0, gid: 0, ..human };
        root.home = spec.home();
        root.shell = "/usr/sbin/nologin".into();
        assert!(!shaped_like_app_account(&spec.home(), &root).await, "uid 0");
        let shell =
            Account { uid: 998, gid: 997, gecos: spec.user_mark(), home: spec.home(), shell: "/bin/bash".into() };
        assert!(!shaped_like_app_account(&spec.home(), &shell).await, "login shell");
    }

    #[test]
    fn app_stores_of_different_apps_never_share_a_folder() {
        // Slugs contain `-`: `a-b`/`c` and `a`/`b-c` must not collide.
        assert_ne!(app_store("a-b", "c"), app_store("a", "b-c"));
        assert_eq!(app_store("staging", "api"), "/var/lib/forge-app-store/staging/api");
    }

    #[test]
    fn a_nologin_shell_is_found() {
        assert!(Path::new(nologin_shell()).exists() || nologin_shell() == "/bin/false");
    }

    #[test]
    fn launcher_folder_refuses_a_symlink_planted_by_the_app_user() {
        let home = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(target.path(), home.path().join(".forge")).unwrap();
        let h = home.path().to_str().unwrap();
        let err = open_forge_dir(h, true, me()).unwrap_err();
        assert!(err.contains("symlink"), "{err}");
        assert!(open_forge_dir(h, false, me()).is_err());

        // Nothing was written through the link.
        assert_eq!(std::fs::read_dir(target.path()).unwrap().count(), 0);
    }

    #[test]
    fn launcher_folder_refuses_a_folder_owned_by_someone_else() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir(home.path().join(".forge")).unwrap();
        let err = open_forge_dir(home.path().to_str().unwrap(), true, me().wrapping_add(1)).unwrap_err();
        assert!(err.contains("not owned by root"), "{err}");
    }

    #[test]
    fn launcher_is_written_and_removed_without_following_links() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path().to_str().unwrap();
        assert!(open_forge_dir(h, false, me()).unwrap().is_none());
        let dir = open_forge_dir(h, true, me()).unwrap().unwrap();

        // A planted `run.sh.tmp` link and a `bin` with a link to a victim file.
        let victim = home.path().join("victim");
        std::fs::write(&victim, "keep").unwrap();
        std::os::unix::fs::symlink(&victim, home.path().join(".forge/run.sh.tmp")).unwrap();
        std::fs::create_dir(home.path().join(".forge/bin")).unwrap();
        std::os::unix::fs::symlink(&victim, home.path().join(".forge/bin/php")).unwrap();
        write_at(&dir, "run.sh", "#!/bin/sh\nexec true\n", 0o755).unwrap();
        remove_bin_at(&dir).unwrap();
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep");
        let run = home.path().join(".forge/run.sh");
        assert_eq!(std::fs::read_to_string(&run).unwrap(), "#!/bin/sh\nexec true\n");
        assert_eq!(std::fs::metadata(&run).unwrap().permissions().mode() & 0o777, 0o755);
        assert!(!home.path().join(".forge/bin").exists());

        // A `bin` made the way write_launcher makes it is removed again.
        rfs::mkdirat(&dir, "bin", rfs::Mode::from_raw_mode(0o755)).unwrap();
        let bin = rfs::openat(&dir, "bin", dir_open_flags(), rfs::Mode::empty()).unwrap();
        rfs::symlinkat("/usr/bin/php8.3", &bin, "php").unwrap();
        remove_bin_at(&dir).unwrap();
        assert!(!home.path().join(".forge/bin").exists());
        drop(dir);

        // Removing the launcher through a planted link does nothing to the target.
        let other = tempfile::tempdir().unwrap();
        std::fs::write(other.path().join("run.sh"), "x").unwrap();
        std::fs::remove_file(&run).unwrap();
        std::fs::remove_dir(home.path().join(".forge")).unwrap();
        std::os::unix::fs::symlink(other.path(), home.path().join(".forge")).unwrap();
        assert!(open_forge_dir(h, false, me()).is_err());
        assert!(other.path().join("run.sh").exists());
    }
}
