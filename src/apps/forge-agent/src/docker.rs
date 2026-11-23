//! Docker Engine API over `/var/run/docker.sock` (root helper only).
//!
//! Access to the Docker socket is root-equivalent, so the unprivileged agent
//! never gets it; the helper exposes a closed set of operations instead and
//! returns container environment values (they often hold secrets) only when
//! the signed `inspect` command asks for them (`reveal_env`, MFA on the server).

use std::time::Duration;

use http_body_util::{BodyExt, Full};
use hyper::Request;
use hyper::body::Bytes;
use hyper_util::rt::TokioIo;
use serde_json::{Value, json};

use crate::helper::ContainerOp;

/// The Docker Engine socket.
pub const SOCKET: &str = "/var/run/docker.sock";
const MAX_BODY: usize = 8 * 1024 * 1024;
const LOG_LIMIT: usize = 256 * 1024;

async fn call(method: &str, path: &str, timeout: Duration) -> Result<(u16, Bytes), String> {
    call_with(method, path, None, timeout).await
}

async fn call_with(method: &str, path: &str, body: Option<&Value>, timeout: Duration) -> Result<(u16, Bytes), String> {
    call_at(std::path::Path::new(SOCKET), method, path, body, timeout).await
}

/// One Engine API call on the Docker socket at `socket` (tests use a fake one).
pub(crate) async fn call_at(
    socket: &std::path::Path,
    method: &str,
    path: &str,
    body: Option<&Value>,
    timeout: Duration,
) -> Result<(u16, Bytes), String> {
    let run = async {
        let stream = tokio::net::UnixStream::connect(socket)
            .await
            .map_err(|e| format!("{} ({}: {e})", forge_catalog::apps::DOCKER_UNAVAILABLE, socket.display()))?;
        let (mut sender, conn) =
            hyper::client::conn::http1::handshake(TokioIo::new(stream)).await.map_err(|e| e.to_string())?;
        tokio::spawn(conn);
        let req = Request::builder()
            .method(method)
            .uri(format!("http://docker/v1.43{path}"))
            .header("Host", "docker")
            .header("Content-Type", "application/json")
            .body(Full::new(body.map_or_else(Bytes::new, |b| Bytes::from(b.to_string()))))
            .map_err(|e| e.to_string())?;
        let resp = sender.send_request(req).await.map_err(|e| e.to_string())?;
        let status = resp.status().as_u16();
        let body = http_body_util::Limited::new(resp.into_body(), MAX_BODY)
            .collect()
            .await
            .map_err(|e| format!("reading Docker response: {e}"))?
            .to_bytes();
        Ok((status, body))
    };
    tokio::time::timeout(timeout, run).await.map_err(|_| "Docker did not respond in time".to_owned())?
}

/// Opens a streaming Engine API call (`GET`, e.g. logs with `follow=1` or
/// `/events`) on the socket at `socket`: the answer's body arrives as Docker
/// writes it. `Ok(None)` on 404. Waits at most `timeout` for the headers.
pub(crate) async fn stream_at(
    socket: &std::path::Path,
    path: &str,
    timeout: Duration,
) -> Result<Option<hyper::body::Incoming>, String> {
    let open = async {
        let stream = tokio::net::UnixStream::connect(socket)
            .await
            .map_err(|e| format!("{} ({}: {e})", forge_catalog::apps::DOCKER_UNAVAILABLE, socket.display()))?;
        let (mut sender, conn) =
            hyper::client::conn::http1::handshake(TokioIo::new(stream)).await.map_err(|e| e.to_string())?;
        tokio::spawn(conn);
        let req = Request::builder()
            .method("GET")
            .uri(format!("http://docker/v1.43{path}"))
            .header("Host", "docker")
            .body(Full::new(Bytes::new()))
            .map_err(|e| e.to_string())?;
        sender.send_request(req).await.map_err(|e| e.to_string())
    };
    let resp = tokio::time::timeout(timeout, open).await.map_err(|_| "Docker did not respond in time".to_owned())??;
    let status = resp.status().as_u16();
    if status == 404 {
        return Ok(None);
    }
    if status != 200 {
        let body = http_body_util::Limited::new(resp.into_body(), 64 * 1024)
            .collect()
            .await
            .map(http_body_util::Collected::to_bytes)
            .unwrap_or_default();
        return Err(docker_error(status, &body));
    }
    Ok(Some(resp.into_body()))
}

/// CPU percent of one core from a `stats?stream=false` answer.
#[must_use]
pub(crate) fn stats_cpu_percent(s: &Value) -> f64 {
    cpu_percent(s)
}

pub(crate) fn docker_error(status: u16, body: &[u8]) -> String {
    let msg = serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|v| v.get("message").and_then(Value::as_str).map(str::to_owned))
        .unwrap_or_else(|| String::from_utf8_lossy(body).trim().to_owned());
    format!("Docker: {msg} (HTTP {status})")
}

async fn json_call(method: &str, path: &str, timeout: Duration) -> Result<Value, String> {
    let (status, body) = call(method, path, timeout).await?;
    if !(200..300).contains(&status) && status != 304 {
        return Err(docker_error(status, &body));
    }
    if body.is_empty() { Ok(Value::Null) } else { serde_json::from_slice(&body).map_err(|e| e.to_string()) }
}

/// A Forge app container's run state for its health check (D-085): only the
/// short id, whether it runs, Docker's status word and the image
/// `HEALTHCHECK` status (`healthy`, `unhealthy`, `starting` or null). Nothing
/// else (no environment, labels or mounts) leaves the helper.
pub async fn container_state(name: &str) -> Result<Value, String> {
    if !crate::helper::valid_app_container(name) {
        return Err(format!("`{name}` is not a Forge app container"));
    }
    let v = json_call("GET", &format!("/containers/{name}/json"), Duration::from_secs(5)).await?;
    Ok(state_view(&v))
}

/// The four fields [`container_state`] returns, from Docker's inspect JSON.
#[must_use]
pub fn state_view(v: &Value) -> Value {
    serde_json::json!({
        "id": short(v["Id"].as_str().unwrap_or_default()),
        "running": v.pointer("/State/Running").and_then(Value::as_bool).unwrap_or(false),
        "status": v.pointer("/State/Status").and_then(Value::as_str).unwrap_or_default(),
        "health": v.pointer("/State/Health/Status").and_then(Value::as_str),
    })
}

fn short(id: &str) -> String {
    id.chars().take(12).collect()
}

/// Demultiplexes Docker's stdout/stderr log stream (non-TTY containers).
pub(crate) fn demux(raw: &[u8]) -> String {
    let mut out = Vec::new();
    let mut i = 0;
    let framed = raw.len() >= 8 && matches!(raw[0], 0..=2) && raw[1..4] == [0, 0, 0];
    if !framed {
        return String::from_utf8_lossy(raw).into_owned();
    }
    while i + 8 <= raw.len() {
        let len = u32::from_be_bytes([raw[i + 4], raw[i + 5], raw[i + 6], raw[i + 7]]) as usize;
        let end = (i + 8 + len).min(raw.len());
        out.extend_from_slice(&raw[i + 8..end]);
        i = end;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn summarize(c: &Value) -> Value {
    json!({
        "id": short(c["Id"].as_str().unwrap_or_default()),
        "name": c["Names"][0].as_str().unwrap_or_default().trim_start_matches('/'),
        "image": c["Image"],
        "state": c["State"],
        "status": c["Status"],
        "created": c["Created"],
        "ports": c["Ports"].as_array().map(|ps| ps.iter().filter_map(|p| {
            let private = p["PrivatePort"].as_u64()?;
            Some(match p["PublicPort"].as_u64() {
                Some(public) => format!("{}:{public}->{private}/{}", p["IP"].as_str().unwrap_or("0.0.0.0"), p["Type"].as_str().unwrap_or("tcp")),
                None => format!("{private}/{}", p["Type"].as_str().unwrap_or("tcp")),
            })
        }).collect::<Vec<_>>()).unwrap_or_default(),
        "labels": c["Labels"],
    })
}

fn cpu_percent(s: &Value) -> f64 {
    let f = |p: &str| s.pointer(p).and_then(Value::as_f64).unwrap_or(0.0);
    let cpu = f("/cpu_stats/cpu_usage/total_usage") - f("/precpu_stats/cpu_usage/total_usage");
    let sys = f("/cpu_stats/system_cpu_usage") - f("/precpu_stats/system_cpu_usage");
    let n = s.pointer("/cpu_stats/online_cpus").and_then(Value::as_f64).unwrap_or(1.0);
    if sys > 0.0 && cpu > 0.0 { (cpu / sys) * n * 100.0 } else { 0.0 }
}

/// Calls the Engine API with an optional JSON body; 404 is reported as `Ok(None)`.
pub async fn api(method: &str, path: &str, body: Option<&Value>, timeout: Duration) -> Result<Option<Value>, String> {
    let (status, bytes) = call_with(method, path, body, timeout).await?;
    if status == 404 {
        return Ok(None);
    }
    if !(200..300).contains(&status) && status != 304 {
        return Err(docker_error(status, &bytes));
    }
    // Pulls stream JSON lines; surface an error that arrives mid-stream.
    let text = String::from_utf8_lossy(&bytes);
    if let Some(err) = text
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find_map(|v| v["error"].as_str().map(str::to_owned))
    {
        return Err(format!("Docker: {err}"));
    }
    Ok(Some(serde_json::from_slice(&bytes).unwrap_or(Value::Null)))
}

/// Last `tail` log lines of a container, demultiplexed.
pub async fn logs(name: &str, tail: u32) -> Result<String, String> {
    let (status, body) = call(
        "GET",
        &format!("/containers/{name}/logs?stdout=1&stderr=1&timestamps=1&tail={tail}"),
        Duration::from_secs(30),
    )
    .await?;
    if status != 200 {
        return Err(docker_error(status, &body));
    }
    Ok(demux(&body))
}

/// Logs written in `[since, until)` (Unix seconds, fractional), timestamped.
/// `Ok(None)` when the container does not exist (stopped and removed).
pub async fn logs_window(name: &str, since: f64, until: f64) -> Result<Option<String>, String> {
    let (status, body) = call(
        "GET",
        &format!("/containers/{name}/logs?stdout=1&stderr=1&timestamps=1&since={since:.6}&until={until:.6}"),
        Duration::from_secs(15),
    )
    .await?;
    match status {
        200 => Ok(Some(demux(&body))),
        404 => Ok(None),
        _ => Err(docker_error(status, &body)),
    }
}

/// The `inspect` answer without environment values: identity, state, ports,
/// mounts, network names, environment *names*, labels.
fn inspect_view(v: &Value) -> Value {
    let env_keys: Vec<&str> = v
        .pointer("/Config/Env")
        .and_then(Value::as_array)
        .map(|e| e.iter().filter_map(|x| x.as_str()?.split('=').next()).collect())
        .unwrap_or_default();
    json!({
        "id": short(v["Id"].as_str().unwrap_or_default()),
        "name": v["Name"].as_str().unwrap_or_default().trim_start_matches('/'),
        "image": v.pointer("/Config/Image"),
        "state": v["State"],
        "restart_policy": v.pointer("/HostConfig/RestartPolicy/Name"),
        "ports": v.pointer("/NetworkSettings/Ports"),
        "mounts": v["Mounts"].as_array().map(|m| m.iter().map(|x| json!({"source": x["Source"], "destination": x["Destination"], "mode": x["Mode"]})).collect::<Vec<_>>()),
        "networks": v.pointer("/NetworkSettings/Networks").and_then(Value::as_object).map(|n| n.keys().cloned().collect::<Vec<_>>()),
        "env_keys": env_keys,
        "labels": v.pointer("/Config/Labels"),
        "created": v["Created"],
    })
}

/// `[{key, value}]` from Docker's `KEY=VALUE` list (a variable without `=`
/// has an empty value).
fn env_values(v: &Value) -> Value {
    Value::Array(
        v.pointer("/Config/Env")
            .and_then(Value::as_array)
            .map(|e| {
                e.iter()
                    .filter_map(Value::as_str)
                    .map(|x| {
                        let (k, val) = x.split_once('=').unwrap_or((x, ""));
                        json!({ "key": k, "value": val })
                    })
                    .collect()
            })
            .unwrap_or_default(),
    )
}

/// Runs one validated operation.
pub async fn run(op: &ContainerOp) -> Result<Value, String> {
    const QUICK: Duration = Duration::from_secs(30);
    match op {
        ContainerOp::Info => {
            let v = json_call("GET", "/info", QUICK).await?;
            Ok(json!({
                "version": v["ServerVersion"], "containers": v["Containers"], "running": v["ContainersRunning"],
                "images": v["Images"], "swarm": v.pointer("/Swarm/LocalNodeState"), "cpus": v["NCPU"], "memory": v["MemTotal"],
            }))
        }
        ContainerOp::List => {
            let v = json_call("GET", "/containers/json?all=1", QUICK).await?;
            Ok(Value::Array(v.as_array().map(|a| a.iter().map(summarize).collect()).unwrap_or_default()))
        }
        ContainerOp::Images => {
            let v = json_call("GET", "/images/json", QUICK).await?;
            Ok(Value::Array(
                v.as_array()
                    .map(|a| {
                        a.iter()
                            .map(|i| json!({ "id": short(i["Id"].as_str().unwrap_or_default().trim_start_matches("sha256:")), "tags": i["RepoTags"], "size": i["Size"], "created": i["Created"] }))
                            .collect()
                    })
                    .unwrap_or_default(),
            ))
        }
        ContainerOp::Inspect { id, reveal_env } => {
            let v = json_call("GET", &format!("/containers/{id}/json"), QUICK).await?;
            let mut out = inspect_view(&v);
            // Environment *values* only on request: they often carry secrets.
            if *reveal_env {
                out["env"] = env_values(&v);
            }
            Ok(out)
        }
        ContainerOp::Start { id } | ContainerOp::Stop { id } | ContainerOp::Restart { id } => {
            let verb = match op {
                ContainerOp::Start { .. } => "start",
                ContainerOp::Stop { .. } => "stop",
                _ => "restart",
            };
            json_call("POST", &format!("/containers/{id}/{verb}"), Duration::from_secs(90)).await?;
            Ok(json!({ "id": id, "done": verb }))
        }
        ContainerOp::Remove { id } => {
            json_call("DELETE", &format!("/containers/{id}?force=1"), Duration::from_secs(90)).await?;
            Ok(json!({ "id": id, "done": "remove" }))
        }
        ContainerOp::Logs { id, tail, since } => {
            // `since` was validated as a decimal Unix time.
            let window = since.as_deref().map(|s| format!("&since={s}")).unwrap_or_default();
            let (status, body) = call(
                "GET",
                &format!("/containers/{id}/logs?stdout=1&stderr=1&timestamps=1&tail={tail}{window}"),
                QUICK,
            )
            .await?;
            if status != 200 {
                return Err(docker_error(status, &body));
            }
            let mut text = demux(&body);
            if text.len() > LOG_LIMIT {
                let mut cut = text.len() - LOG_LIMIT;
                while !text.is_char_boundary(cut) {
                    cut += 1;
                }
                text = text[cut..].to_owned();
            }
            Ok(json!({ "id": id, "logs": text }))
        }
        ContainerOp::Stats { id } => {
            let s = json_call("GET", &format!("/containers/{id}/stats?stream=false"), QUICK).await?;
            Ok(json!({
                "id": id,
                "cpu_percent": (cpu_percent(&s) * 10.0).round() / 10.0,
                "memory_bytes": s.pointer("/memory_stats/usage"),
                "memory_limit": s.pointer("/memory_stats/limit"),
                "pids": s.pointer("/pids_stats/current"),
            }))
        }
        ContainerOp::Pull { image } => {
            let (name, tag) = match image.rsplit_once(':') {
                Some((n, t)) if !t.contains('/') => (n, t),
                _ => (image.as_str(), "latest"),
            };
            let (status, body) =
                call("POST", &format!("/images/create?fromImage={name}&tag={tag}"), Duration::from_secs(20 * 60))
                    .await?;
            if status != 200 {
                return Err(docker_error(status, &body));
            }
            // The body is a JSON-lines progress stream; an error can arrive mid-stream.
            let text = String::from_utf8_lossy(&body);
            if let Some(err) = text
                .lines()
                .filter_map(|l| serde_json::from_str::<Value>(l).ok())
                .find_map(|v| v["error"].as_str().map(str::to_owned))
            {
                return Err(format!("Docker: {err}"));
            }
            Ok(json!({ "image": format!("{name}:{tag}"), "done": "pull" }))
        }
    }
}

/// How a command run in an app's container ended ([`exec_in_app`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecEnd {
    /// It ran to its end: exit code and the end of its output.
    Exited { code: i32, output: Vec<u8> },
    /// The timeout passed; the process was ended where that was possible.
    TimedOut,
    /// The container does not exist or is not running.
    NotRunning,
    /// A container of that name exists but does not carry Forge's label
    /// for the app: nothing was run in it.
    Foreign,
    /// An earlier run of the same check outlived its timeout and could not
    /// be ended: nothing new was started.
    StillRunning,
    /// Docker could not start the command (not found, not executable).
    StartFailed { detail: String },
}

/// Where a health check's command runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecTarget {
    /// `forge-app-<ns>-<app>`.
    pub container: String,
    /// The value of the container's `forge.app` label: `<ns>/<app>`.
    pub app_label: String,
    /// Names the check, so a run that could not be ended is not started again.
    pub check: String,
}

/// Bytes of a run's output returned (its end).
const EXEC_TAIL: usize = 512;
const EXEC_QUICK: Duration = Duration::from_secs(5);

/// Runs that outlived their timeout and could not be ended, by check: the
/// exec id and the container's full id. Owned by the helper's
/// [`crate::health_exec::Runner`].
#[derive(Debug, Default)]
pub struct StuckRuns(std::sync::Mutex<std::collections::HashMap<String, (String, String)>>);

impl StuckRuns {
    fn get(&self, check: &str) -> Option<(String, String)> {
        self.0.lock().ok()?.get(check).cloned()
    }

    fn forget(&self, check: &str) {
        if let Ok(mut s) = self.0.lock() {
            s.remove(check);
        }
    }

    fn remember(&self, check: &str, exec: &str, container: &str) {
        if let Ok(mut s) = self.0.lock() {
            s.insert(check.to_owned(), (exec.to_owned(), container.to_owned()));
        }
    }
}

/// The Engine API body that creates the exec: the argv as given, no TTY, no
/// stdin, and no `User`, so it runs as the container's default user. Never
/// privileged, never with extra environment.
#[must_use]
pub fn exec_create_body(cmd: &[String]) -> Value {
    json!({ "AttachStdin": false, "AttachStdout": true, "AttachStderr": true, "Tty": false, "Cmd": cmd })
}

/// Whether Docker's inspect answer is the container Forge created for the
/// app `label` names (`<ns>/<app>`): the name alone cannot tell
/// `a-b`/`c` from `a`/`b-c`.
#[must_use]
pub fn is_app_container(inspect: &Value, label: &str) -> bool {
    inspect.pointer("/Config/Labels/forge.app").and_then(Value::as_str) == Some(label)
}

/// Whether `/proc/<pid>/cgroup` (`cgroups`) places a process in the
/// container with the full id `container`.
#[must_use]
pub fn in_container_cgroup(cgroups: &str, container: &str) -> bool {
    container.len() == 64
        && container.bytes().all(|b| b.is_ascii_hexdigit())
        && cgroups.lines().any(|l| l.contains(container))
}

/// Ends the exec'd process `pid` (as Docker reports it, in the host's
/// numbering) if it belongs to `container`. The process is pinned with a
/// pidfd before its cgroup is read, and signalled through that pidfd, so a
/// recycled pid is never hit: if the process ended in between, the signal
/// goes nowhere.
#[cfg(target_os = "linux")]
fn kill_in_container(pid: i64, container: &str) -> bool {
    use rustix::process::{Pid, PidfdFlags, Signal, pidfd_open, pidfd_send_signal};
    let Some(pid) = i32::try_from(pid).ok().filter(|p| *p > 1).and_then(Pid::from_raw) else { return false };
    let Ok(fd) = pidfd_open(pid, PidfdFlags::empty()) else { return false };
    // Unreadable (the process is gone): an empty text matches no container.
    let cgroups = std::fs::read_to_string(format!("/proc/{}/cgroup", pid.as_raw_nonzero())).unwrap_or_default();
    in_container_cgroup(&cgroups, container) && pidfd_send_signal(&fd, Signal::KILL).is_ok()
}

#[cfg(not(target_os = "linux"))]
fn kill_in_container(_pid: i64, _container: &str) -> bool {
    false
}

/// `Some(pid)` while the exec still runs.
async fn exec_running(exec: &str) -> Result<Option<i64>, String> {
    // 404: Docker forgot the exec, so it does not run.
    let v = api("GET", &format!("/exec/{exec}/json"), None, EXEC_QUICK).await?.unwrap_or(Value::Null);
    // A missing pid is 0, which `kill_in_container` never signals.
    Ok((v["Running"].as_bool() == Some(true)).then(|| v["Pid"].as_i64().unwrap_or(0)))
}

/// Ends an exec that outlived its timeout. `true` when it no longer runs.
async fn end_exec(exec: &str, container: &str) -> bool {
    for _ in 0..3 {
        match exec_running(exec).await {
            Ok(None) => return true,
            Ok(Some(pid)) => {
                kill_in_container(pid, container);
            }
            Err(_) => return false,
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    matches!(exec_running(exec).await, Ok(None))
}

/// Ends a started exec when the run is dropped before it finished: the
/// check's timeout fired, or the agent disconnected and the helper dropped
/// the request. Ending needs calls to Docker, so it happens in a task of
/// its own; a run that cannot be ended is remembered as stuck.
struct ExecGuard {
    exec: String,
    container: String,
    check: String,
    stuck: std::sync::Arc<StuckRuns>,
    armed: bool,
}

impl ExecGuard {
    /// The run ended by itself: nothing to end.
    fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for ExecGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        // Until it is known to be gone, the check is not started again.
        self.stuck.remember(&self.check, &self.exec, &self.container);
        let (exec, container, check, stuck) =
            (self.exec.clone(), self.container.clone(), self.check.clone(), self.stuck.clone());
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            rt.spawn(async move {
                if end_exec(&exec, &container).await {
                    stuck.forget(&check);
                }
            });
        }
    }
}

/// Docker's stdout/stderr stream of a run without a TTY, demultiplexed as
/// it arrives: only the end of the payload is kept, however much there is.
#[derive(Debug, Default)]
struct StreamTail {
    header: Vec<u8>,
    /// Payload bytes of the current frame still to come.
    remaining: usize,
    tail: Vec<u8>,
}

impl StreamTail {
    fn feed(&mut self, mut data: &[u8]) {
        while !data.is_empty() {
            if self.remaining == 0 {
                let need = 8 - self.header.len();
                let (head, rest) = data.split_at(need.min(data.len()));
                self.header.extend_from_slice(head);
                data = rest;
                if let [_, _, _, _, a, b, c, d] = self.header[..] {
                    self.remaining = u32::from_be_bytes([a, b, c, d]) as usize;
                    self.header.clear();
                }
                continue;
            }
            let (payload, rest) = data.split_at(self.remaining.min(data.len()));
            self.remaining -= payload.len();
            self.tail.extend_from_slice(payload);
            let cut = self.tail.len().saturating_sub(EXEC_TAIL);
            self.tail.drain(..cut);
            data = rest;
        }
    }
}

/// Starts the exec and reads its output stream to the end, keeping the end
/// of the output. `Ok(Err(detail))`: Docker refused to start it.
async fn exec_start(exec: &str) -> Result<Result<Vec<u8>, String>, String> {
    let stream = tokio::net::UnixStream::connect(SOCKET)
        .await
        .map_err(|e| format!("{} ({SOCKET}: {e})", forge_catalog::apps::DOCKER_UNAVAILABLE))?;
    let (mut sender, conn) =
        hyper::client::conn::http1::handshake(TokioIo::new(stream)).await.map_err(|e| e.to_string())?;
    tokio::spawn(conn);
    let req = Request::builder()
        .method("POST")
        .uri(format!("http://docker/v1.43/exec/{exec}/start"))
        .header("Host", "docker")
        .header("Content-Type", "application/json")
        .body(Full::new(Bytes::from(json!({ "Detach": false, "Tty": false }).to_string())))
        .map_err(|e| e.to_string())?;
    let resp = sender.send_request(req).await.map_err(|e| e.to_string())?;
    let status = resp.status().as_u16();
    let mut body = resp.into_body();
    let mut tail = StreamTail::default();
    // An error answer is JSON, not a stream: its first bytes are kept as they are.
    let mut error = Vec::new();
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|e| format!("reading Docker response: {e}"))?;
        if let Some(data) = frame.data_ref() {
            if status == 200 {
                tail.feed(data);
            } else {
                let room = 4096usize.saturating_sub(error.len());
                // `get` cannot miss: the range ends at or before the data's end.
                error.extend_from_slice(data.get(..room.min(data.len())).unwrap_or_default());
            }
        }
    }
    Ok(if status == 200 { Ok(tail.tail) } else { Err(docker_error(status, &error)) })
}

/// Runs `cmd` in the Forge app container of `target` for a health check
/// (D-111) and waits at most `limit`. `Err` only when Docker itself cannot
/// be asked.
pub async fn exec_in_app(
    target: &ExecTarget,
    cmd: &[String],
    limit: Duration,
    stuck: &std::sync::Arc<StuckRuns>,
) -> Result<ExecEnd, String> {
    let name = target.container.as_str();
    if !crate::helper::valid_app_container(name) {
        return Err(format!("`{name}` is not a Forge app container"));
    }
    if let Some((exec, container)) = stuck.get(&target.check) {
        if !end_exec(&exec, &container).await {
            return Ok(ExecEnd::StillRunning);
        }
        stuck.forget(&target.check);
    }
    let Some(c) = api("GET", &format!("/containers/{name}/json"), None, EXEC_QUICK).await? else {
        return Ok(ExecEnd::NotRunning);
    };
    if !is_app_container(&c, &target.app_label) {
        return Ok(ExecEnd::Foreign);
    }
    if c.pointer("/State/Running").and_then(Value::as_bool) != Some(true) {
        return Ok(ExecEnd::NotRunning);
    }
    // An empty id matches no cgroup, so nothing is ever signalled for it.
    let container = c["Id"].as_str().unwrap_or_default().to_owned();
    // By id from here on: the name could be given to another container.
    let by_id = if container.is_empty() { name } else { container.as_str() };
    let (status, bytes) =
        call_with("POST", &format!("/containers/{by_id}/exec"), Some(&exec_create_body(cmd)), EXEC_QUICK).await?;
    let exec = match status {
        201 => serde_json::from_slice::<Value>(&bytes).ok().and_then(|v| v["Id"].as_str().map(str::to_owned)),
        // Gone, stopped or paused since the inspect.
        404 | 409 => return Ok(ExecEnd::NotRunning),
        _ => return Err(docker_error(status, &bytes)),
    };
    let Some(exec) = exec.filter(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_hexdigit())) else {
        return Err("Docker returned no exec id".into());
    };
    // From here until the exit code is read, dropping this future (timeout
    // below, or the agent going away) ends the process in the container.
    let guard = ExecGuard {
        exec: exec.clone(),
        container: container.clone(),
        check: target.check.clone(),
        stuck: stuck.clone(),
        armed: true,
    };
    let output = match tokio::time::timeout(limit, exec_start(&exec)).await {
        Ok(Ok(Ok(output))) => output,
        Ok(Ok(Err(detail))) => {
            guard.disarm();
            return Ok(ExecEnd::StartFailed { detail });
        }
        Ok(Err(e)) => return Err(e),
        // Closing the stream does not end the process: the guard does.
        Err(_) => return Ok(ExecEnd::TimedOut),
    };
    // The stream ends just before Docker records the exit code.
    for _ in 0..20 {
        // 404: Docker forgot the exec; there is no exit code to read.
        let v = api("GET", &format!("/exec/{exec}/json"), None, EXEC_QUICK).await?.unwrap_or(Value::Null);
        if v["Running"].as_bool() != Some(true)
            && let Some(code) = v["ExitCode"].as_i64()
        {
            guard.disarm();
            return Ok(ExecEnd::Exited { code: i32::try_from(code).unwrap_or(-1), output });
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Err("Docker did not report how the command ended".into())
}

#[cfg(test)]
mod tests {
    use super::{demux, env_values, inspect_view};

    /// A health check's exec: the argv untouched, the container's own user,
    /// no TTY, no stdin, nothing privileged.
    #[test]
    fn exec_bodies_carry_the_argv_and_nothing_that_widens_it() {
        let cmd = vec!["sh".to_owned(), "-c".to_owned(), "echo \"$(id)\"; exit 3".to_owned()];
        let v = super::exec_create_body(&cmd);
        assert_eq!(v["Cmd"], serde_json::json!(["sh", "-c", "echo \"$(id)\"; exit 3"]));
        assert_eq!((v["Tty"].as_bool(), v["AttachStdin"].as_bool()), (Some(false), Some(false)));
        for absent in ["User", "Privileged", "Env", "WorkingDir"] {
            assert!(v.get(absent).is_none(), "{absent}");
        }
    }

    #[test]
    fn only_processes_of_the_container_are_ended() {
        let id = "a".repeat(64);
        let v2 = format!("0::/system.slice/docker-{id}.scope\n");
        let v1 = format!("12:pids:/docker/{id}\n11:cpu:/docker/{id}\n");
        assert!(super::in_container_cgroup(&v2, &id) && super::in_container_cgroup(&v1, &id));
        assert!(!super::in_container_cgroup("0::/user.slice/user-1000.slice\n", &id));
        assert!(!super::in_container_cgroup(&v2, &"b".repeat(64)));
        // A short or empty id would match too much: refused.
        assert!(!super::in_container_cgroup(&v2, "aaaa") && !super::in_container_cgroup(&v2, ""));
    }

    #[tokio::test]
    async fn exec_is_limited_to_forge_app_containers() {
        let stuck = std::sync::Arc::new(super::StuckRuns::default());
        for name in ["postgres", "forge-app-", "forge-app-a/../b"] {
            let target = super::ExecTarget { container: name.into(), app_label: "a/b".into(), check: "k".into() };
            let r = super::exec_in_app(&target, &["true".to_owned()], std::time::Duration::from_secs(1), &stuck).await;
            assert!(r.is_err_and(|e| e.contains("not a Forge app container")), "{name}");
        }
    }

    /// `forge-app-a-b-c` is the name of `a-b`/`c` and of `a`/`b-c`: only
    /// the label Forge set says whose container it is.
    #[test]
    fn the_label_decides_whose_container_it_is() {
        let c = serde_json::json!({ "Name": "/forge-app-a-b-c", "Config": { "Labels": { "forge.app": "a-b/c" } } });
        assert!(super::is_app_container(&c, "a-b/c"));
        assert!(!super::is_app_container(&c, "a/b-c"));
        let unlabelled = serde_json::json!({ "Name": "/forge-app-a-b-c", "Config": { "Labels": {} } });
        assert!(!super::is_app_container(&unlabelled, "a-b/c"), "a container someone else named like ours");
        assert!(!super::is_app_container(&serde_json::json!({}), "a-b/c"));
    }

    /// The end of a run's output is kept, whatever came before and however
    /// the stream is cut into reads.
    #[test]
    fn the_end_of_the_exec_stream_is_kept() {
        let frame = |kind: u8, payload: &[u8]| {
            let mut f = vec![kind, 0, 0, 0];
            f.extend_from_slice(&u32::try_from(payload.len()).unwrap_or(0).to_be_bytes());
            f.extend_from_slice(payload);
            f
        };
        let mut raw = frame(1, &vec![b'a'; 200_000]);
        raw.extend(frame(2, b"warning\n"));
        raw.extend(frame(1, b""));
        raw.extend(frame(1, b"THE-END\n"));
        for chunk in [1usize, 7, 4096, raw.len()] {
            let mut t = super::StreamTail::default();
            for part in raw.chunks(chunk) {
                t.feed(part);
            }
            assert_eq!(t.tail.len(), super::EXEC_TAIL, "chunks of {chunk}");
            assert!(t.tail.ends_with(b"warning\nTHE-END\n"), "chunks of {chunk}");
            assert!(!t.tail.contains(&0), "no frame header in the output");
        }
    }

    /// Dropping a started run (timeout, or the agent went away) marks it
    /// stuck at once, so the check is not started again before it is gone.
    #[tokio::test]
    async fn a_dropped_exec_is_remembered_until_it_is_ended() {
        let stuck = std::sync::Arc::new(super::StuckRuns::default());
        let guard = |armed| super::ExecGuard {
            exec: "abc".into(),
            container: "c".repeat(64),
            check: "uat/api/liveness/cmd".into(),
            stuck: stuck.clone(),
            armed,
        };
        guard(true).disarm();
        assert!(stuck.get("uat/api/liveness/cmd").is_none(), "a finished run leaves nothing");
        drop(guard(true));
        assert_eq!(stuck.get("uat/api/liveness/cmd"), Some(("abc".to_owned(), "c".repeat(64))));
    }

    /// `inspect` returns environment names by default and values only on request.
    #[test]
    fn inspect_reveals_values_only_when_asked() {
        let v = serde_json::json!({
            "Id": "0123456789abcdef0123", "Name": "/web",
            "Config": { "Env": ["DB_PASSWORD=s3cret", "FLAG", "A=b=c"], "Image": "nginx" },
            "State": { "Running": true }
        });
        let plain = inspect_view(&v);
        assert_eq!(plain["env_keys"], serde_json::json!(["DB_PASSWORD", "FLAG", "A"]));
        assert!(plain.get("env").is_none() && !plain.to_string().contains("s3cret"));
        assert_eq!(
            env_values(&v),
            serde_json::json!([
                { "key": "DB_PASSWORD", "value": "s3cret" },
                { "key": "FLAG", "value": "" },
                { "key": "A", "value": "b=c" }
            ])
        );
    }

    /// Only four fields leave the helper for an unsigned ContainerState.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn container_state_returns_four_fields_only() {
        let inspect = serde_json::json!({
            "Id": "0123456789abcdef0123",
            "Name": "/forge-app-uat-web",
            "Config": { "Env": ["DB_PASSWORD=s3cret"], "Labels": { "forge.app": "web" } },
            "State": { "Running": true, "Status": "running", "Health": { "Status": "healthy", "Log": [{"Output": "secret"}] } },
            "Mounts": [{ "Source": "/srv" }]
        });
        let v = super::state_view(&inspect);
        assert_eq!(
            v,
            serde_json::json!({"id": "0123456789ab", "running": true, "status": "running", "health": "healthy"})
        );
        assert!(!v.to_string().contains("s3cret") && !v.to_string().contains("secret"));
    }
    #[test]
    fn demultiplexes_log_frames() {
        let mut raw = vec![1, 0, 0, 0, 0, 0, 0, 6];
        raw.extend_from_slice(b"hello\n");
        raw.extend_from_slice(&[2, 0, 0, 0, 0, 0, 0, 4]);
        raw.extend_from_slice(b"err\n");
        assert_eq!(demux(&raw), "hello\nerr\n");
        assert_eq!(demux(b"plain tty output"), "plain tty output");
    }
}
