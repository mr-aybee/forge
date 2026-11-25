//! Container shells through the agent and the root helper (D-114).
//!
//! 1. Forge signs a `ContainerExecOpen`. The agent checks it
//!    ([`validate_open`]), takes one of the node's [`MAX_EXEC`] slots and
//!    forwards the signed command to the helper ([`open_helper`]).
//! 2. The helper verifies the signature with its own pin (replays refused by
//!    operation id), re-validates ([`ExecSpec::validate`]) and drives a Docker
//!    exec with a TTY ([`serve`]): create, start (hijacked stream), resize,
//!    and the exit code at the end. It answers one JSON line, then speaks
//!    [`forge_proto::exec_frame`] frames on the same socket.
//! 3. The agent dials the control plane and relays the frames unchanged
//!    (`crate::exec_stream`).
//!
//! The shell runs as the image's default user. Docker has no API to end an
//! exec: dropping the hijacked stream closes the PTY, which hangs up the
//! shell.

use forge_proto::messages::ContainerExecOpen;
use serde::{Deserialize, Serialize};

/// Container shells on one node at most (the helper's limit is
/// authoritative; the agent checks the same number early).
pub const MAX_EXEC: usize = 8;
/// Longest a shell may run, in seconds (the server's limit is 12 h).
pub const MAX_LIFETIME_SECS: u32 = 12 * 3600;
/// The command run in the container: bash when the image has it, else sh.
pub const SHELL: [&str; 3] = ["/bin/sh", "-c", "command -v bash >/dev/null 2>&1 && exec bash || exec sh"];

/// What the helper runs, derived only from a signed `ContainerExecOpen`.
/// The helper enforces the limits itself: it does not rely on the agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecSpec {
    pub container: String,
    pub cols: u16,
    pub rows: u16,
    /// The shell ends after this long without traffic either way.
    pub idle_secs: u32,
    /// The shell ends this long after it opened.
    pub lifetime_secs: u32,
    /// The request must be taken before this (epoch ms).
    pub expires_at_ms: i64,
}

/// Terminal sizes are clamped to this.
const MAX_DIM: u32 = 1000;
/// Longest idle limit a command may carry, in seconds.
const MAX_IDLE_SECS: u32 = 86_400;

fn dim(v: u32) -> u16 {
    u16::try_from(v.clamp(1, MAX_DIM)).unwrap_or(1)
}

impl ExecSpec {
    #[must_use]
    pub fn from_wire(x: &ContainerExecOpen) -> Self {
        Self {
            container: x.container.clone(),
            cols: dim(x.cols),
            rows: dim(x.rows),
            idle_secs: x.idle_secs,
            lifetime_secs: x.lifetime_secs,
            expires_at_ms: x.expires_at_ms,
        }
    }

    /// Structural checks (no clock).
    pub fn validate(&self) -> Result<(), String> {
        if !crate::helper::valid_container_id(&self.container) {
            return Err(format!("invalid container id `{}`", self.container));
        }

        if self.cols == 0 || self.rows == 0 || u32::from(self.cols) > MAX_DIM || u32::from(self.rows) > MAX_DIM {
            return Err("invalid terminal size".into());
        }

        if !(1..=MAX_IDLE_SECS).contains(&self.idle_secs) {
            return Err("invalid idle timeout".into());
        }

        if !(1..=MAX_LIFETIME_SECS).contains(&self.lifetime_secs) {
            return Err("invalid lifetime".into());
        }

        Ok(())
    }

    /// [`Self::validate`] plus the request's expiry.
    pub fn validate_at(&self, now_ms: i64) -> Result<(), String> {
        self.validate()?;

        if self.expires_at_ms <= now_ms {
            return Err("the container shell request has expired".into());
        }

        Ok(())
    }
}

fn hex(s: &str, n: usize) -> bool {
    s.len() == n && s.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// The agent's checks of a signed `ContainerExecOpen` before anything runs.
pub fn validate_open(x: &ContainerExecOpen, now_ms: i64) -> Result<ExecSpec, String> {
    if !hex(&x.session_id, 32) {
        return Err("invalid session id".into());
    }

    if !hex(&x.stream_token, 64) {
        return Err("invalid stream token".into());
    }

    let spec = ExecSpec::from_wire(x);
    spec.validate_at(now_ms)?;

    Ok(spec)
}

/// Open shells, counted by whoever owns the counter (the agent, or one run
/// of the helper's `serve`).
pub type ShellCount = std::sync::Arc<std::sync::atomic::AtomicUsize>;

/// A counted slot: at most `max` held at once.
#[derive(Debug)]
pub struct Slot(ShellCount);

impl Slot {
    pub fn take(counter: &ShellCount, max: usize) -> Result<Self, String> {
        use std::sync::atomic::Ordering;

        if counter.fetch_add(1, Ordering::SeqCst) >= max {
            counter.fetch_sub(1, Ordering::SeqCst);
            return Err(format!("this node already has {max} open container shells"));
        }

        Ok(Self(counter.clone()))
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// The helper's side of a shell: frames to and from the agent.
pub struct ExecPipe {
    pub reader: Box<dyn tokio::io::AsyncRead + Unpin + Send>,
    pub writer: Box<dyn tokio::io::AsyncWrite + Unpin + Send>,
}

impl std::fmt::Debug for ExecPipe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ExecPipe")
    }
}

/// Asks the helper for a shell with the command as Forge signed it. On
/// success the helper has started the exec and the pipe carries frames.
#[cfg(unix)]
pub async fn open_helper(signed: &forge_proto::messages::SignedCommand) -> Result<ExecPipe, String> {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

    use crate::helper::{HelperRequest, HelperResponse, SOCKET};
    let stream = tokio::net::UnixStream::connect(SOCKET)
        .await
        .map_err(|e| format!("forge-agent-helper is not running ({SOCKET}: {e}); re-run the node installer"))?;
    let (read, mut write) = stream.into_split();
    let envelope =
        HelperRequest::Signed { command: forge_secrets::crypto::b64url(&prost::Message::encode_to_vec(signed)) };
    let mut line = serde_json::to_string(&envelope).map_err(|e| e.to_string())?;
    line.push('\n');
    write.write_all(line.as_bytes()).await.map_err(|e| e.to_string())?;

    // The answer line is short; frames may follow it at once, so the
    // buffered reader stays the pipe's reader.
    let mut reader = BufReader::new(read);
    let mut answer = String::new();
    let read_line = async {
        let mut limited = (&mut reader).take(64 * 1024);
        limited.read_line(&mut answer).await
    };

    match tokio::time::timeout(std::time::Duration::from_secs(30), read_line).await {
        Ok(Ok(n)) if n > 0 => {}
        Ok(Ok(_)) => return Err("the helper closed the connection".into()),
        Ok(Err(e)) => return Err(e.to_string()),
        Err(_) => return Err("the helper did not answer in time".into()),
    }

    let r: HelperResponse = serde_json::from_str(&answer).map_err(|e| format!("bad helper response: {e}"))?;
    if !r.ok {
        return Err(r.error.unwrap_or_else(|| "the helper refused the container shell".into()));
    }

    Ok(ExecPipe { reader: Box::new(reader), writer: Box::new(write) })
}

/// Windows nodes have no container shells.
#[cfg(not(unix))]
pub async fn open_helper(_: &forge_proto::messages::SignedCommand) -> Result<ExecPipe, String> {
    Err("container shells are not supported on Windows nodes".into())
}

#[cfg(unix)]
pub use docker_exec::serve;

#[cfg(unix)]
mod docker_exec {
    use std::path::Path;
    use std::time::Duration;

    use forge_proto::exec_frame::{Decoder, Frame, MAX_PAYLOAD};
    use serde_json::{Value, json};
    use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
    use tokio::net::UnixStream;

    use std::sync::atomic::{AtomicI64, Ordering};

    use super::{ExecSpec, MAX_EXEC, SHELL, ShellCount, Slot};
    use crate::docker::{call_at, docker_error};
    use crate::helper::HelperResponse;

    /// The API version prefix for the hand-written start request (`call_at`
    /// adds it itself).
    const API: &str = "/v1.43";
    const CALL: Duration = Duration::from_secs(10);
    const MAX_HEAD: usize = 16 * 1024;

    /// A started exec: its id, the hijacked stream and any output that came
    /// with the response head.
    struct Started {
        id: String,
        stream: UnixStream,
        early: Vec<u8>,
    }

    async fn create(socket: &Path, spec: &ExecSpec) -> Result<String, String> {
        let body = json!({
            "AttachStdin": true, "AttachStdout": true, "AttachStderr": true, "Tty": true,
            "Env": ["TERM=xterm-256color"],
            "Cmd": SHELL,
            "ConsoleSize": [spec.rows, spec.cols],
        });
        let path = format!("/containers/{}/exec", spec.container);
        let (status, bytes) = call_at(socket, "POST", &path, Some(&body), CALL).await?;
        match status {
            200..=299 => {}
            404 => return Err(format!("no container `{}` on this node", spec.container)),
            _ => return Err(docker_error(status, &bytes)),
        }

        let v: Value = serde_json::from_slice(&bytes).map_err(|e| format!("Docker: {e}"))?;
        let id = v["Id"].as_str().unwrap_or_default().to_owned();
        if !crate::helper::valid_container_id(&id) {
            return Err("Docker returned no exec id".into());
        }

        Ok(id)
    }

    /// `POST /exec/{id}/start` with an upgrade: Docker answers `101` (or
    /// `200` from older engines) and the connection becomes the TTY.
    async fn start(socket: &Path, spec: &ExecSpec, id: &str) -> Result<Started, String> {
        let run = async {
            let mut stream = UnixStream::connect(socket).await.map_err(|e| format!("Docker: {e}"))?;
            let body = json!({ "Detach": false, "Tty": true, "ConsoleSize": [spec.rows, spec.cols] }).to_string();
            let head = format!(
                "POST {API}/exec/{id}/start HTTP/1.1\r\nHost: docker\r\nContent-Type: application/json\r\n\
                 Connection: Upgrade\r\nUpgrade: tcp\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(head.as_bytes()).await.map_err(|e| format!("Docker: {e}"))?;
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];

            let end = loop {
                if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    break i + 4;
                }

                if buf.len() > MAX_HEAD {
                    return Err("Docker sent an oversized response".to_owned());
                }

                let n = stream.read(&mut chunk).await.map_err(|e| format!("Docker: {e}"))?;
                if n == 0 {
                    return Err("Docker closed the connection".to_owned());
                }

                buf.extend_from_slice(&chunk[..n]);
            };

            let status_line = String::from_utf8_lossy(&buf[..end]).lines().next().unwrap_or_default().to_owned();
            let code: u16 = status_line.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
            if code != 101 && code != 200 {
                return Err(docker_error(code, &buf[end..]));
            }

            Ok(Started { id: id.to_owned(), stream, early: buf[end..].to_vec() })
        };
        tokio::time::timeout(CALL, run).await.map_err(|_| "Docker did not start the shell in time".to_owned())?
    }

    async fn resize(socket: &Path, id: &str, cols: u16, rows: u16) -> Result<(), String> {
        let (status, bytes) =
            call_at(socket, "POST", &format!("/exec/{id}/resize?h={rows}&w={cols}"), None, CALL).await?;
        if (200..300).contains(&status) { Ok(()) } else { Err(docker_error(status, &bytes)) }
    }

    /// The exec's exit code once it stopped (waits up to two seconds).
    async fn exit_code(socket: &Path, id: &str) -> Option<i32> {
        for _ in 0..10 {
            let (status, bytes) = call_at(socket, "GET", &format!("/exec/{id}/json"), None, CALL).await.ok()?;
            if status != 200 {
                return None;
            }

            let v: Value = serde_json::from_slice(&bytes).ok()?;
            if v["Running"] != true {
                return v["ExitCode"].as_i64().and_then(|c| i32::try_from(c).ok());
            }

            tokio::time::sleep(Duration::from_millis(200)).await;
        }

        None
    }

    async fn answer<W: AsyncWrite + Unpin>(to_agent: &mut W, r: &HelperResponse) -> std::io::Result<()> {
        let mut line = serde_json::to_string(r).unwrap_or_else(|_| "{\"ok\":false}".into());
        line.push('\n');

        to_agent.write_all(line.as_bytes()).await
    }

    /// Runs one container shell for the agent: answers one JSON line, then
    /// relays frames until the shell exits or the agent hangs up.
    pub async fn serve<R, W>(spec: ExecSpec, socket: &Path, shells: &ShellCount, mut from_agent: R, mut to_agent: W)
    where
        R: AsyncRead + Unpin,
        W: AsyncWrite + Unpin,
    {
        let opened = async {
            spec.validate_at(crate::tunnel::now_ms())?;
            let slot = Slot::take(shells, MAX_EXEC)?;
            let id = create(socket, &spec).await?;
            let started = start(socket, &spec, &id).await?;

            // Docker sizes the TTY only once the exec runs.
            if let Err(e) = resize(socket, &id, spec.cols, spec.rows).await {
                eprintln!("forge-agent-helper: container shell resize failed: {e}");
            }

            Ok::<_, String>((slot, started))
        };

        let (slot, started) = match opened.await {
            Ok(v) => v,
            Err(e) => {
                let _ = answer(&mut to_agent, &HelperResponse::failed(e)).await;
                return;
            }
        };

        let ok = HelperResponse { ok: true, state: None, error: None, output: None, details: None, data: None };
        if answer(&mut to_agent, &ok).await.is_err() {
            return;
        }

        let _slot = slot;
        let Started { id, stream, early } = started;
        let (mut docker_rd, mut docker_wr) = stream.into_split();
        let last = AtomicI64::new(crate::tunnel::now_ms());
        let touch = || last.store(crate::tunnel::now_ms(), Ordering::Relaxed);

        // Docker → agent: output, then the exit code.
        let output = async {
            if !early.is_empty() && to_agent.write_all(&Frame::Data(early).encode()).await.is_err() {
                return;
            }

            let mut buf = vec![0u8; MAX_PAYLOAD];

            loop {
                match docker_rd.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        touch();

                        if to_agent.write_all(&Frame::Data(buf[..n].to_vec()).encode()).await.is_err() {
                            return;
                        }
                    }
                }
            }

            let end = match exit_code(socket, &id).await {
                Some(code) => Frame::Exit(code),
                None => Frame::Eof,
            };

            let _ = to_agent.write_all(&end.encode()).await;
            let _ = to_agent.shutdown().await;
        };

        // Agent → Docker: keystrokes and resizes.
        let input = async {
            let mut dec = Decoder::new();
            let mut buf = vec![0u8; MAX_PAYLOAD];

            loop {
                let n = match from_agent.read(&mut buf).await {
                    Ok(0) | Err(_) => return,
                    Ok(n) => n,
                };

                dec.push(&buf[..n]);

                loop {
                    match dec.next_frame() {
                        Ok(Some(Frame::Data(d))) => {
                            touch();

                            if docker_wr.write_all(&d).await.is_err() {
                                return;
                            }
                        }
                        Ok(Some(Frame::Resize { cols, rows })) => {
                            let (cols, rows) = (cols.clamp(1, 1000), rows.clamp(1, 1000));
                            if let Err(e) = resize(socket, &id, cols, rows).await {
                                eprintln!("forge-agent-helper: container shell resize failed: {e}");
                            }
                        }
                        Ok(Some(Frame::Eof)) => {
                            let _ = docker_wr.shutdown().await;
                        }
                        Ok(Some(_)) => {}
                        Ok(None) => break,
                        Err(e) => {
                            eprintln!("forge-agent-helper: container shell stream: {e}");
                            return;
                        }
                    }
                }
            }
        };

        // The shell exiting ends the session; the agent hanging up drops the
        // hijacked stream, which closes the PTY and hangs up the shell.
        let idle_ms = i64::from(spec.idle_secs) * 1000;
        let idle = async {
            // Checked often enough to end within a quarter of the limit.
            let tick = Duration::from_millis(u64::try_from(idle_ms / 4).unwrap_or(1000).clamp(100, 5000));

            loop {
                tokio::time::sleep(tick).await;

                if crate::tunnel::now_ms() - last.load(Ordering::Relaxed) >= idle_ms {
                    return;
                }
            }
        };
        let lifetime = tokio::time::sleep(Duration::from_secs(u64::from(spec.lifetime_secs)));
        let limit = tokio::select! {
            () = output => None,
            () = input => None,
            () = idle => Some("the container shell was idle for too long"),
            () = lifetime => Some("the container shell reached its lifetime"),
        };

        // The helper enforces both limits itself, whatever the agent does.
        if let Some(why) = limit {
            let _ = to_agent.write_all(&Frame::Error(why.into()).encode()).await;
            let _ = to_agent.shutdown().await;
        }
    }

    #[cfg(test)]
    #[allow(clippy::unwrap_used)]
    mod tests {
        use std::sync::{Arc, Mutex};

        use tokio::io::{AsyncBufReadExt, BufReader};
        use tokio::net::UnixListener;

        use super::*;

        /// A fake Docker Engine: exec create/start (upgraded, echoing until
        /// `exit 3`), resize and inspect with exit code 3.
        fn fake_docker(dir: &Path, not_running: bool) -> (std::path::PathBuf, Arc<Mutex<Vec<String>>>) {
            let path = dir.join("docker.sock");
            let listener = UnixListener::bind(&path).unwrap();
            let seen = Arc::new(Mutex::new(Vec::new()));
            let log = seen.clone();
            tokio::spawn(async move {
                while let Ok((mut s, _)) = listener.accept().await {
                    let log = log.clone();
                    tokio::spawn(async move {
                        let mut buf = Vec::new();
                        let mut chunk = [0u8; 4096];

                        let end = loop {
                            if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                                break i + 4;
                            }

                            let n = s.read(&mut chunk).await.unwrap();
                            if n == 0 {
                                return;
                            }

                            buf.extend_from_slice(&chunk[..n]);
                        };

                        let head = String::from_utf8_lossy(&buf[..end]).to_string();
                        let len: usize = head
                            .lines()
                            .find_map(|l| {
                                let (k, v) = l.split_once(':')?;
                                k.eq_ignore_ascii_case("content-length").then(|| v.trim().parse().ok())?
                            })
                            .unwrap_or(0);

                        while buf.len() < end + len {
                            let n = s.read(&mut chunk).await.unwrap();
                            buf.extend_from_slice(&chunk[..n]);
                        }

                        let body = String::from_utf8_lossy(&buf[end..end + len]).to_string();

                        // hyper sends the absolute form (`POST http://docker/v1.43/…`).
                        let line = head.lines().next().unwrap().replacen("http://docker", "", 1);
                        log.lock().unwrap().push(format!("{line} {body}"));
                        let reply = |code: &str, body: &str| {
                            format!(
                                "HTTP/1.1 {code}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                                body.len()
                            )
                        };

                        if line.starts_with("POST /v1.43/containers/web-1/exec ") {
                            let r = if not_running {
                                reply("409 Conflict", r#"{"message":"Container web-1 is not running"}"#)
                            } else {
                                reply("201 Created", r#"{"Id":"e1"}"#)
                            };

                            s.write_all(r.as_bytes()).await.unwrap();
                        } else if line.starts_with("POST /v1.43/exec/e1/start ") {
                            s.write_all(b"HTTP/1.1 101 UPGRADED\r\nConnection: Upgrade\r\nUpgrade: tcp\r\n\r\n$ ")
                                .await
                                .unwrap();
                            let mut b = [0u8; 1024];

                            loop {
                                let n = s.read(&mut b).await.unwrap_or(0);
                                if n == 0 {
                                    break;
                                }

                                s.write_all(&b[..n]).await.unwrap();

                                if String::from_utf8_lossy(&b[..n]).contains("exit 3") {
                                    break;
                                }
                            }
                        } else if line.starts_with("POST /v1.43/exec/e1/resize") {
                            s.write_all(reply("201 Created", "").as_bytes()).await.unwrap();
                        } else if line.starts_with("GET /v1.43/exec/e1/json") {
                            s.write_all(reply("200 OK", r#"{"Running":false,"ExitCode":3}"#).as_bytes()).await.unwrap();
                        } else {
                            s.write_all(reply("404 Not Found", r#"{"message":"no route"}"#).as_bytes()).await.unwrap();
                        }
                    });
                }
            });

            (path, seen)
        }

        fn spec() -> ExecSpec {
            ExecSpec {
                container: "web-1".into(),
                cols: 120,
                rows: 32,
                idle_secs: 600,
                lifetime_secs: 3600,
                expires_at_ms: crate::tunnel::now_ms() + 30_000,
            }
        }

        type AgentRead = BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>;
        type AgentWrite = tokio::io::WriteHalf<tokio::io::DuplexStream>;

        /// Runs `serve` on a fresh pipe; returns the agent's ends.
        fn run(spec: ExecSpec, sock: std::path::PathBuf) -> (AgentRead, AgentWrite, tokio::task::JoinHandle<()>) {
            let (agent, helper) = tokio::io::duplex(64 * 1024);
            let (h_rd, h_wr) = tokio::io::split(helper);
            let shells = ShellCount::default();
            let task = tokio::spawn(async move { serve(spec, &sock, &shells, h_rd, h_wr).await });
            let (a_rd, a_wr) = tokio::io::split(agent);

            (BufReader::new(a_rd), a_wr, task)
        }

        /// Every frame until the helper closes the stream.
        async fn frames(rd: &mut AgentRead) -> Vec<Frame> {
            let mut dec = Decoder::new();
            let mut all = Vec::new();
            let mut b = [0u8; 4096];

            loop {
                let n = rd.read(&mut b).await.unwrap();
                if n == 0 {
                    return all;
                }

                dec.push(&b[..n]);

                while let Some(f) = dec.next_frame().unwrap() {
                    all.push(f);
                }
            }
        }

        #[tokio::test]
        async fn the_helper_drives_a_docker_exec_with_resize_and_exit_code() {
            let dir = tempfile::tempdir().unwrap();
            let (sock, seen) = fake_docker(dir.path(), false);
            let (mut a_rd, mut a_wr, task) = run(spec(), sock);
            let mut line = String::new();
            a_rd.read_line(&mut line).await.unwrap();
            assert!(line.contains("\"ok\":true"), "{line}");
            a_wr.write_all(&Frame::Resize { cols: 100, rows: 40 }.encode()).await.unwrap();
            a_wr.write_all(&Frame::Data(b"echo hi\n".to_vec()).encode()).await.unwrap();
            let mut dec = Decoder::new();
            let mut out = Vec::new();
            let mut sent_exit = false;

            let exit = loop {
                let mut b = [0u8; 4096];
                let n = a_rd.read(&mut b).await.unwrap();
                assert!(n > 0, "stream ended without an exit frame");
                dec.push(&b[..n]);
                let mut code = None;

                while let Some(f) = dec.next_frame().unwrap() {
                    match f {
                        Frame::Data(d) => out.extend_from_slice(&d),
                        Frame::Exit(c) => code = Some(c),
                        other => panic!("unexpected {other:?}"),
                    }
                }

                if let Some(c) = code {
                    break c;
                }

                if !sent_exit && String::from_utf8_lossy(&out).contains("echo hi") {
                    sent_exit = true;
                    a_wr.write_all(&Frame::Data(b"exit 3\n".to_vec()).encode()).await.unwrap();
                }
            };

            assert_eq!(exit, 3);
            let text = String::from_utf8_lossy(&out);
            assert!(text.starts_with("$ ") && text.contains("echo hi") && text.contains("exit 3"), "{text}");
            task.await.unwrap();
            let seen = seen.lock().unwrap().join("\n");
            assert!(seen.contains("\"Tty\":true") && seen.contains("exec bash || exec sh"), "{seen}");
            assert!(seen.contains("\"TERM=xterm-256color\""), "{seen}");
            assert!(!seen.contains("\"User\""), "the image's default user: {seen}");
            assert!(seen.contains("/exec/e1/resize?h=32&w=120"), "initial size: {seen}");
            assert!(seen.contains("/exec/e1/resize?h=40&w=100"), "resize: {seen}");
            assert!(seen.contains("GET /v1.43/exec/e1/json"), "exit code inspected: {seen}");
        }

        async fn answer_line(spec: ExecSpec, sock: std::path::PathBuf) -> String {
            let (mut rd, _wr, task) = run(spec, sock);
            let mut line = String::new();
            rd.read_line(&mut line).await.unwrap();
            task.await.unwrap();

            line
        }

        #[tokio::test]
        async fn docker_errors_are_answered_and_invalid_or_expired_specs_refused() {
            let dir = tempfile::tempdir().unwrap();
            let (sock, seen) = fake_docker(dir.path(), true);
            let line = answer_line(spec(), sock.clone()).await;
            assert!(line.contains("\"ok\":false") && line.contains("not running"), "{line}");
            let calls = seen.lock().unwrap().len();
            let line = answer_line(ExecSpec { container: "web;id".into(), ..spec() }, sock.clone()).await;
            assert!(line.contains("invalid container id"), "{line}");

            // The helper checks the signed expiry and limits itself.
            let expired = ExecSpec { expires_at_ms: crate::tunnel::now_ms() - 1, ..spec() };
            let line = answer_line(expired, sock.clone()).await;
            assert!(line.contains("expired"), "{line}");
            let line = answer_line(ExecSpec { idle_secs: 0, ..spec() }, sock.clone()).await;
            assert!(line.contains("invalid idle timeout"), "{line}");
            assert_eq!(seen.lock().unwrap().len(), calls, "nothing reached Docker");
        }

        /// The helper ends a quiet shell, and a shell past its lifetime, on
        /// its own, whatever the agent does.
        #[tokio::test]
        async fn the_helper_enforces_idle_and_lifetime_limits() {
            let dir = tempfile::tempdir().unwrap();
            let (sock, _) = fake_docker(dir.path(), false);

            for (s, why) in
                [(ExecSpec { idle_secs: 1, ..spec() }, "idle"), (ExecSpec { lifetime_secs: 1, ..spec() }, "lifetime")]
            {
                let started = std::time::Instant::now();
                let (mut rd, _wr, task) = run(s, sock.clone());
                let mut line = String::new();
                rd.read_line(&mut line).await.unwrap();
                assert!(line.contains("\"ok\":true"), "{line}");
                let all = tokio::time::timeout(Duration::from_secs(10), frames(&mut rd)).await.unwrap();
                assert!(matches!(all.last(), Some(Frame::Error(m)) if m.contains(why)), "{why}: {all:?}");
                assert!(started.elapsed() < Duration::from_secs(5), "{why}: {:?}", started.elapsed());
                task.await.unwrap();
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn cmd() -> ContainerExecOpen {
        ContainerExecOpen {
            session_id: "a".repeat(32),
            container: "web-1".into(),
            cols: 120,
            rows: 32,
            stream_token: "b".repeat(64),
            idle_secs: 1800,
            expires_at_ms: 2_000,
            lifetime_secs: 3600,
        }
    }

    #[test]
    fn open_commands_are_checked_before_anything_runs() {
        let spec = validate_open(&cmd(), 1_000).unwrap();
        assert_eq!((spec.container.as_str(), spec.cols, spec.rows), ("web-1", 120, 32));
        assert_eq!((spec.idle_secs, spec.lifetime_secs, spec.expires_at_ms), (1800, 3600, 2_000), "limits travel");

        let bad = |f: fn(&mut ContainerExecOpen)| {
            let mut c = cmd();
            f(&mut c);

            validate_open(&c, 1_000)
        };

        assert!(bad(|c| c.session_id = "../x".into()).is_err());
        assert!(bad(|c| c.stream_token = "B".repeat(64)).is_err());
        assert!(bad(|c| c.container = "-web".into()).is_err());
        assert!(bad(|c| c.container = "web;id".into()).is_err());
        assert!(bad(|c| c.idle_secs = 0).is_err());
        assert!(bad(|c| c.lifetime_secs = MAX_LIFETIME_SECS + 1).is_err());
        assert!(bad(|c| c.expires_at_ms = 1_000).unwrap_err().contains("expired"));

        // Sizes are clamped, never refused.
        assert_eq!(bad(|c| (c.cols, c.rows) = (0, 99_999)).unwrap().cols, 1);
        assert_eq!(bad(|c| (c.cols, c.rows) = (0, 99_999)).unwrap().rows, 1000);
    }

    #[test]
    fn slots_are_counted_per_owner_bounded_and_released() {
        let c = ShellCount::default();
        let held: Vec<Slot> = (0..MAX_EXEC).map(|_| Slot::take(&c, MAX_EXEC).unwrap()).collect();
        assert!(Slot::take(&c, MAX_EXEC).unwrap_err().contains("8 open container shells"));
        assert!(Slot::take(&ShellCount::default(), MAX_EXEC).is_ok(), "another owner's counter");
        drop(held);
        assert!(Slot::take(&c, MAX_EXEC).is_ok());
    }
}
