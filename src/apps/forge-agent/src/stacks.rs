//! Compose stacks on the node (root, inside `forge-agent-helper`; issue #7).
//! See `forge_catalog::stacks` for the model and the policy.
//!
//! * Every operation runs Docker's `docker compose` plugin with a fixed
//!   argument list (`-p <project> --project-directory <dir> -f compose.yaml`);
//!   nothing from the wire becomes a shell string or an option.
//! * The environment is cleared: only `PATH` and `HOME` are set, so Compose
//!   interpolates nothing from the helper's environment, and the written
//!   `compose.yaml` is already interpolated and checked (`$` escaped).
//! * The policy is checked again here, including bind sources after
//!   resolving symlinks, so a link inside an approved root cannot point at
//!   `/etc` or the Docker socket.
//! * `compose.yaml` and `.env` are written 0600 by root through a temporary
//!   file and a rename; the stack folder must not be (or pass through) a
//!   symlink.
//!
//! Linux only.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use forge_catalog::stacks::{
    STACKS_DIR, StackOp, StackPart, binds, check_path, host_paths, is_system_path, render_env, to_yaml,
};
use serde_json::{Value, json};
use tokio::process::Command;

const PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";
const COMPOSE_FILE: &str = "compose.yaml";
const ENV_FILE: &str = ".env";
/// Most log output returned at once.
const MAX_LOG_BYTES: usize = 512 * 1024;
const SHORT: Duration = Duration::from_secs(120);
const LONG: Duration = Duration::from_secs(30 * 60);

/// Runs one stack operation (validated first).
pub async fn run_op(op: &StackOp) -> Result<Value, String> {
    op.validate()?;
    match op {
        StackOp::Deploy { part, services, pull } => deploy(part, services, *pull).await,
        StackOp::Ps { project, dir } => ps(project, dir).await.map(|s| json!({ "services": s })),
        StackOp::Service { project, dir, action, services } => {
            let mut args = vec![action.as_str()];
            args.extend(services.iter().map(String::as_str));
            let t = if action == "pull" { LONG } else { SHORT };
            compose(project, dir, &args, t).await?;
            ps(project, dir).await.map(|s| json!({ "services": s }))
        }
        StackOp::Down { project, dir, volumes } => down(project, dir, *volumes).await,
        StackOp::Logs { project, dir, service, tail } => {
            let tail = (*tail).clamp(1, 10_000).to_string();
            let out =
                compose(project, dir, &["logs", "--no-color", "--timestamps", "--tail", &tail, service], SHORT).await?;
            let start = out.len().saturating_sub(MAX_LOG_BYTES);
            let start = (start..out.len()).find(|i| out.is_char_boundary(*i)).unwrap_or(out.len());
            Ok(json!({ "logs": &out[start..] }))
        }
    }
}

/// `docker compose -p <project> --project-directory <dir> -f <dir>/compose.yaml <args>`.
async fn compose(project: &str, dir: &str, args: &[&str], timeout: Duration) -> Result<String, String> {
    let file = format!("{dir}/{COMPOSE_FILE}");
    // Not `is_file`: that follows links, and Compose runs as root.
    let regular = |p: &str| std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_file());
    if !regular(&file) {
        return Err(format!("this stack is not deployed on the node ({file} is missing or not a regular file)"));
    }
    let mut argv: Vec<&str> =
        vec!["compose", "--ansi", "never", "-p", project, "--project-directory", dir, "-f", &file];
    let env_file = format!("{dir}/{ENV_FILE}");
    if std::fs::symlink_metadata(&env_file).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(format!("{env_file} is a symbolic link; deploy the stack again"));
    }
    if regular(&env_file) {
        argv.extend(["--env-file", &env_file]);
    }
    argv.extend_from_slice(args);
    run("docker", &argv, timeout).await
}

async fn run(cmd: &str, args: &[&str], timeout: Duration) -> Result<String, String> {
    let child = Command::new(cmd)
        .args(args)
        .env_clear()
        .env("PATH", PATH)
        .env("HOME", "/root")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .output();
    let out = tokio::time::timeout(timeout, child)
        .await
        .map_err(|_| format!("docker {} timed out", args.iter().find(|a| !a.starts_with('-')).unwrap_or(&"")))?
        .map_err(|e| format!("{cmd}: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    if out.status.success() {
        Ok(stdout)
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        Err(format!("docker compose failed: {}", compose_error(&err)))
    }
}

/// The reason from Compose's stderr. Progress and status lines (image
/// layers, `Container … Started`) are indented; messages and errors are not,
/// so those are kept. Without any, the last lines stand in.
fn compose_error(stderr: &str) -> String {
    let messages: Vec<&str> =
        stderr.lines().filter(|l| !l.is_empty() && !l.starts_with(char::is_whitespace)).map(str::trim_end).collect();
    if messages.is_empty() {
        last_lines(stderr.trim(), 20)
    } else {
        messages[messages.len().saturating_sub(10)..].join("\n")
    }
}

fn last_lines(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

async fn compose_available() -> Result<(), String> {
    run("docker", &["compose", "version", "--short"], SHORT).await.map(|_| ()).map_err(|_| {
        "the Docker Compose plugin is not installed on this node; install Docker Engine from the App Store".into()
    })
}

/// Creates the stack folder (0750, root) and refuses symlinks on the way.
fn ensure_dir(dir: &str) -> Result<(), String> {
    if dir.starts_with(STACKS_DIR) {
        for d in [STACKS_DIR, Path::new(dir).parent().and_then(Path::to_str).unwrap_or(STACKS_DIR)] {
            std::fs::create_dir_all(d).map_err(|e| format!("{d}: {e}"))?;
            std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o755)).map_err(|e| format!("{d}: {e}"))?;
        }
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("{dir}: {e}"))?;
    let real = std::fs::canonicalize(dir).map_err(|e| format!("{dir}: {e}"))?;
    if real != Path::new(dir) {
        return Err(format!("the stack folder {dir} passes through a symlink"));
    }
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o750)).map_err(|e| format!("{dir}: {e}"))
}

fn write_private(path: &str, content: &str) -> Result<(), String> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt;
    let tmp = format!("{path}.forge-tmp");
    let _ = std::fs::remove_file(&tmp);
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(i32::try_from(rustix::fs::OFlags::NOFOLLOW.bits()).unwrap_or(0))
        .open(&tmp)
        .map_err(|e| format!("{tmp}: {e}"))?;
    f.write_all(content.as_bytes()).and_then(|()| f.sync_all()).map_err(|e| format!("{tmp}: {e}"))?;
    drop(f);
    std::fs::rename(&tmp, path).map_err(|e| format!("{path}: {e}"))
}

/// Walks an absolute host path from `/` one component at a time without
/// following links (`O_NOFOLLOW`, `*at` calls). With `create`, missing
/// folders are made (0755, root) the same way, so Docker never creates a
/// bind source through a link. A link on the way is refused unless it is a
/// root-owned system link (`/var/run`, `/lib`, or a link directly under
/// `/`), which no container can plant. Returns whether the path exists.
///
/// Docker resolves the path again when it starts the container, so a link
/// swapped in after this check is still possible where an untrusted process
/// can write; the policy keeps those places (writable binds) off the path.
#[allow(clippy::cast_possible_truncation, clippy::unnecessary_cast)]
pub fn walk_host_path(path: &str, create: bool) -> Result<bool, String> {
    use rustix::fs::{self as rfs, AtFlags, FileType, Mode, OFlags};
    use rustix::io::Errno;
    let comps: Vec<&str> = path.split('/').filter(|c| !c.is_empty() && *c != ".").collect();
    if !path.starts_with('/') || comps.contains(&"..") {
        return Err(format!("{path}: the path must be absolute without `..`"));
    }
    let flags = OFlags::PATH | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let mut fd = rfs::open("/", flags, Mode::empty()).map_err(|e| format!("/: {e}"))?;
    let mut cur = String::new();
    for (i, c) in comps.iter().enumerate() {
        let parent_is_root = cur.is_empty();
        cur = format!("{cur}/{c}");
        let last = i + 1 == comps.len();
        let st = match rfs::statat(&fd, *c, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(st) => st,
            Err(Errno::NOENT) if create => {
                match rfs::mkdirat(&fd, *c, Mode::from_raw_mode(0o755)) {
                    Ok(()) | Err(Errno::EXIST) => {}
                    Err(e) => return Err(format!("{cur}: {e}")),
                }
                rfs::statat(&fd, *c, AtFlags::SYMLINK_NOFOLLOW).map_err(|e| format!("{cur}: {e}"))?
            }
            Err(Errno::NOENT) => return Ok(false),
            Err(e) => return Err(format!("{cur}: {e}")),
        };
        let link = FileType::from_raw_mode(st.st_mode as _) == FileType::Symlink;
        if link && !(st.st_uid as u32 == 0 && (parent_is_root || is_system_path(&cur))) {
            return Err(format!("{path}: {cur} is a symbolic link; bind sources must be real folders"));
        }
        if last {
            break;
        }
        // Trusted system links are followed; everything else is opened as is.
        let open_flags = if link { flags.difference(OFlags::NOFOLLOW) } else { flags };
        fd = rfs::openat(&fd, *c, open_flags, Mode::empty()).map_err(|e| match e {
            Errno::NOTDIR | Errno::LOOP => format!("{path}: {cur} is not a folder"),
            e => format!("{cur}: {e}"),
        })?;
    }
    Ok(true)
}

/// Re-checks host paths on the node: missing bind sources are created
/// without following links, no path may pass through a link, and the
/// resolved path must pass the policy too.
fn recheck_links(tree: &Value, part: &StackPart) -> Result<(), String> {
    let sources: Vec<String> = binds(tree, &part.dir).into_iter().map(|b| b.path).collect();
    for p in host_paths(tree, &part.dir) {
        let socket = p.ends_with(".sock");
        if !walk_host_path(&p, sources.contains(&p) && !socket)? {
            continue;
        }
        let real = std::fs::canonicalize(&p).map_err(|e| format!("{p}: {e}"))?;
        let real = real.to_string_lossy();
        if real != p {
            let r = check_path(&real, &part.policy);
            r.ensure(part.policy.allow_privileged).map_err(|e| format!("{p} resolves to {real}: {e}"))?;
        }
    }
    Ok(())
}

async fn deploy(part: &StackPart, services: &[String], pull: bool) -> Result<Value, String> {
    let (tree, ports) = part.prepare()?;
    recheck_links(&tree, part)?;
    compose_available().await?;
    ensure_dir(&part.dir)?;
    write_private(&format!("{}/{COMPOSE_FILE}", part.dir), &to_yaml(&tree))?;
    write_private(&format!("{}/{ENV_FILE}", part.dir), &render_env(&part.env))?;
    let project = part.project();
    let mut args =
        vec!["up", "--detach", "--no-build", "--remove-orphans", "--wait", "--wait-timeout", "600", "--pull"];
    args.push(if pull { "always" } else { "missing" });
    args.extend(services.iter().map(String::as_str));
    let up = compose(&project, &part.dir, &args, LONG).await;
    let status = ps(&project, &part.dir).await.unwrap_or_default();
    up.map_err(|e| format!("{e}\n{}", summary(&status)))?;
    Ok(json!({ "services": status, "ports": ports }))
}

fn summary(status: &[Value]) -> String {
    status
        .iter()
        .map(|s| {
            format!(
                "{}: {} {}",
                s["service"].as_str().unwrap_or("?"),
                s["state"].as_str().unwrap_or("?"),
                s["health"].as_str().unwrap_or("")
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `docker compose ps --all --format json`, normalized to
/// `[{service, name, state, health, status, ports, image, exit_code}]`.
async fn ps(project: &str, dir: &str) -> Result<Vec<Value>, String> {
    let out = compose(project, dir, &["ps", "--all", "--format", "json"], SHORT).await?;
    Ok(parse_ps(&out))
}

/// Compose prints a JSON array (older v2) or one object per line (newer).
#[must_use]
pub fn parse_ps(out: &str) -> Vec<Value> {
    let raw: Vec<Value> = match serde_json::from_str::<Value>(out.trim()) {
        Ok(Value::Array(a)) => a,
        Ok(v @ Value::Object(_)) => vec![v],
        _ => out.lines().filter_map(|l| serde_json::from_str(l.trim()).ok()).collect(),
    };
    raw.into_iter()
        .map(|c| {
            json!({
                "service": c["Service"],
                "name": c["Name"],
                "state": c["State"],
                "health": c["Health"],
                "status": c["Status"],
                "image": c["Image"],
                "exit_code": c["ExitCode"],
                "ports": c["Publishers"],
            })
        })
        .collect()
}

async fn down(project: &str, dir: &str, volumes: bool) -> Result<Value, String> {
    let file = format!("{dir}/{COMPOSE_FILE}");
    if Path::new(&file).is_file() {
        let mut args = vec!["down", "--remove-orphans"];
        if volumes {
            args.push("--volumes");
        }
        compose(project, dir, &args, LONG).await?;
    }
    for f in [COMPOSE_FILE, ENV_FILE] {
        let _ = std::fs::remove_file(format!("{dir}/{f}"));
    }
    // Forge's own folder goes when empty; data in a chosen folder stays.
    if dir.starts_with(STACKS_DIR) {
        let _ = std::fs::remove_dir(dir);
    }
    Ok(json!({ "down": true, "volumes_removed": volumes }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compose_error_keeps_the_reason_not_the_progress() {
        let mut err = String::new();
        for i in 0..60 {
            err.push_str(&format!(" {i:012x} Extracting 1B\n"));
        }
        err.push_str(" Container p-web-1 Started \n Container p-web-1 Waiting \ncontainer p-web-1 is unhealthy\n");
        assert_eq!(compose_error(&err), "container p-web-1 is unhealthy");
        let only_progress = " Network p_default Creating \n Network p_default Created \n";
        assert!(compose_error(only_progress).contains("Network p_default Created"));
    }

    #[test]
    fn ps_output_parses_both_formats() {
        let lines = "{\"Service\":\"web\",\"Name\":\"p-web-1\",\"State\":\"running\",\"Health\":\"healthy\"}\n{\"Service\":\"db\",\"State\":\"exited\",\"ExitCode\":1}\n";
        let v = parse_ps(lines);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0]["service"], "web");
        assert_eq!(v[0]["health"], "healthy");
        assert_eq!(v[1]["exit_code"], 1);
        let arr = "[{\"Service\":\"web\",\"State\":\"running\"}]";
        assert_eq!(parse_ps(arr)[0]["state"], "running");
        assert!(parse_ps("").is_empty());
    }

    #[test]
    fn errors_keep_the_last_lines() {
        let s = (0..30).map(|i| i.to_string()).collect::<Vec<_>>().join("\n");
        assert_eq!(last_lines(&s, 2), "28\n29");
    }
}
