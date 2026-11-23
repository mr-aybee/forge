//! `forge nodes terminal|exec` and `forge sessions` (issue #5, D-075).
//!
//! Both open a WebSocket to the server's SSH broker with the bearer token in
//! the `Authorization` header (never in the URL), over rustls with the
//! system's root certificates (plus the context's CA file). The server
//! checks `terminal.open` (MFA), signs a five-minute certificate for the
//! login and relays bytes; the CLI never sees an SSH key.

use std::io::{IsTerminal, Write as _};
use std::sync::Arc;

use clap::Subcommand;
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

use crate::admincmd::enc;
use crate::api::Api;
use crate::cmds::{confirm, resolve_node};
use crate::error::{CliError, Exit, Result};
use crate::out::{self, Col};
use crate::{Global, Target, connect, target};

pub(crate) trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

pub(crate) type Ws = WebSocketStream<Box<dyn Io>>;

fn tls_config(ca_file: Option<&str>) -> Result<rustls::ClientConfig> {
    let mut roots = rustls::RootCertStore::empty();
    for cert in rustls_native_certs::load_native_certs().certs {
        let _ = roots.add(cert);
    }
    if let Some(path) = ca_file {
        use rustls_pki_types::pem::PemObject;
        let pem = std::fs::read(path).map_err(|e| CliError::usage(format!("CA file {path}: {e}")))?;
        for c in rustls_pki_types::CertificateDer::pem_slice_iter(&pem) {
            let c = c.map_err(|e| CliError::usage(format!("CA file {path}: {e}")))?;
            roots.add(c).map_err(|e| CliError::usage(format!("CA file {path}: {e}")))?;
        }
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut cfg = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| CliError::new(Exit::Transport, e.to_string()))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(cfg)
}

/// Opens `path` (with its query) as a WebSocket, authenticated with the bearer token.
async fn open_ws(api: &mut Api, t: &Target, path: &str) -> Result<Ws> {
    let token = api.access_token().await?.ok_or_else(|| CliError::auth("not signed in"))?;
    ws_connect(t, &token, path).await
}

/// Opens `path` as a WebSocket with a given bearer token (for many sockets
/// from concurrent tasks, such as tunnel connections).
pub(crate) async fn ws_connect(t: &Target, token: &str, path: &str) -> Result<Ws> {
    let url = url::Url::parse(&format!("{}{path}", t.server)).map_err(|e| CliError::usage(e.to_string()))?;
    let tls = match url.scheme() {
        "https" => true,
        "http" => false,
        other => return Err(CliError::usage(format!("unsupported scheme {other}"))),
    };
    let host = url.host_str().ok_or_else(|| CliError::usage("server URL has no host"))?.to_owned();
    let port = url.port_or_known_default().unwrap_or(if tls { 443 } else { 80 });
    let mut ws_url = url.clone();
    let _ = ws_url.set_scheme(if tls { "wss" } else { "ws" });
    let mut req = ws_url.as_str().into_client_request().map_err(|e| CliError::new(Exit::Transport, e.to_string()))?;
    let auth = format!("Bearer {token}")
        .parse()
        .map_err(|_| CliError::new(Exit::Auth, "the access token is not a valid header value"))?;
    req.headers_mut().insert("authorization", auth);
    req.headers_mut().insert(
        "user-agent",
        concat!("forge-cli/", env!("CARGO_PKG_VERSION")).parse().map_err(|_| CliError::usage("user agent"))?,
    );
    let tcp = tokio::net::TcpStream::connect((host.as_str(), port))
        .await
        .map_err(|e| CliError::new(Exit::Transport, format!("{host}:{port}: {e}")))?;
    let _ = tcp.set_nodelay(true);
    let stream: Box<dyn Io> = if tls {
        let name = rustls_pki_types::ServerName::try_from(host.clone())
            .map_err(|_| CliError::usage(format!("invalid server name {host}")))?;
        let conn = tokio_rustls::TlsConnector::from(Arc::new(tls_config(t.ca_file.as_deref())?));
        Box::new(conn.connect(name, tcp).await.map_err(|e| CliError::new(Exit::Transport, format!("TLS: {e}")))?)
    } else {
        Box::new(tcp)
    };
    let (ws, _) = tokio_tungstenite::client_async(req, stream).await.map_err(|e| {
        use tokio_tungstenite::tungstenite::Error as E;
        match e {
            E::Http(resp) => {
                let status = resp.status().as_u16();
                let body = resp.body().as_deref().map(String::from_utf8_lossy).unwrap_or_default().to_string();
                let v: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
                CliError::from_http(status, v["title"].as_str(), v["detail"].as_str(), None)
            }
            other => CliError::new(Exit::Transport, other.to_string()),
        }
    })?;
    Ok(ws)
}

/// The login to use: `--login`, else the node's first saved login, else `root`.
async fn pick_login(api: &mut Api, node_id: &str, login: Option<String>) -> Result<String> {
    if let Some(l) = login {
        return Ok(l);
    }
    let v = api.get(&format!("/api/v1/servers/{node_id}/logins")).await.unwrap_or(Value::Null);
    Ok(v["logins"].as_array().and_then(|a| a.first()).and_then(Value::as_str).unwrap_or("root").to_owned())
}

/// Runs the server's checks first as plain JSON: errors (MFA step-up,
/// permissions, no SSH endpoint) come back readable, and the CLI's step-up
/// prompt runs here, before the WebSocket.
async fn precheck(api: &mut Api, node_id: &str, login: &str) -> Result<()> {
    api.get(&format!("/api/v1/servers/{node_id}/terminal/check?login={}", enc(login))).await.map(|_| ())
}

fn status_of(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or(Value::Null)
}

// ------------------------------------------------------------------ terminal

/// Restores the terminal when dropped (also on errors and panics).
struct RawMode;

impl RawMode {
    fn enable() -> Result<Self> {
        crossterm::terminal::enable_raw_mode().map_err(|e| CliError::new(Exit::General, format!("raw mode: {e}")))?;
        Ok(Self)
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

fn term_size() -> (u16, u16) {
    crossterm::terminal::size().unwrap_or((120, 32))
}

/// Keyboard input as bytes for the remote terminal, and size changes.
enum Input {
    Bytes(Vec<u8>),
    Resize(u16, u16),
    Closed,
}

/// Reads the local terminal. Unix: raw bytes from stdin (exact, including
/// pastes), size polled. Windows: console key events translated to xterm
/// sequences, since the console does not deliver escape sequences.
fn spawn_input() -> tokio::sync::mpsc::Receiver<Input> {
    let (tx, rx) = tokio::sync::mpsc::channel(64);
    #[cfg(not(windows))]
    {
        let tx2 = tx.clone();
        tokio::spawn(async move {
            let mut stdin = tokio::io::stdin();
            let mut buf = vec![0u8; 8192];
            loop {
                match stdin.read(&mut buf).await {
                    Ok(0) | Err(_) => {
                        let _ = tx2.send(Input::Closed).await;
                        break;
                    }
                    Ok(n) => {
                        if tx2.send(Input::Bytes(buf[..n].to_vec())).await.is_err() {
                            break;
                        }
                    }
                }
            }
        });
        tokio::spawn(async move {
            let mut last = term_size();
            let mut tick = tokio::time::interval(std::time::Duration::from_millis(400));
            loop {
                tick.tick().await;
                let now = term_size();
                if now != last {
                    last = now;
                    if tx.send(Input::Resize(now.0, now.1)).await.is_err() {
                        break;
                    }
                }
            }
        });
    }
    #[cfg(windows)]
    {
        std::thread::spawn(move || {
            use crossterm::event::{Event, KeyEventKind, read};
            loop {
                let msg = match read() {
                    Ok(Event::Key(k)) if k.kind != KeyEventKind::Release => match keys::encode(&k) {
                        Some(b) => Input::Bytes(b),
                        None => continue,
                    },
                    Ok(Event::Resize(c, r)) => Input::Resize(c, r),
                    Ok(_) => continue,
                    Err(_) => Input::Closed,
                };
                let closed = matches!(msg, Input::Closed);
                if tx.blocking_send(msg).is_err() || closed {
                    break;
                }
            }
        });
    }
    rx
}

/// Key events → the bytes an xterm sends (Windows console input).
#[cfg_attr(not(windows), allow(dead_code))]
mod keys {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[must_use]
    pub fn encode(k: &KeyEvent) -> Option<Vec<u8>> {
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let mut out: Vec<u8> = match k.code {
            KeyCode::Char(c) if ctrl => {
                let l = c.to_ascii_lowercase();
                match l {
                    'a'..='z' => vec![(l as u8) - b'a' + 1],
                    ' ' | '@' | '2' => vec![0],
                    '[' | '3' => vec![0x1b],
                    '\\' | '4' => vec![0x1c],
                    ']' | '5' => vec![0x1d],
                    '^' | '6' => vec![0x1e],
                    '_' | '7' | '/' => vec![0x1f],
                    _ => c.to_string().into_bytes(),
                }
            }
            KeyCode::Char(c) => c.to_string().into_bytes(),
            KeyCode::Enter => vec![b'\r'],
            KeyCode::Tab => vec![b'\t'],
            KeyCode::BackTab => b"\x1b[Z".to_vec(),
            KeyCode::Backspace => vec![0x7f],
            KeyCode::Esc => vec![0x1b],
            KeyCode::Up => b"\x1b[A".to_vec(),
            KeyCode::Down => b"\x1b[B".to_vec(),
            KeyCode::Right => b"\x1b[C".to_vec(),
            KeyCode::Left => b"\x1b[D".to_vec(),
            KeyCode::Home => b"\x1b[H".to_vec(),
            KeyCode::End => b"\x1b[F".to_vec(),
            KeyCode::Insert => b"\x1b[2~".to_vec(),
            KeyCode::Delete => b"\x1b[3~".to_vec(),
            KeyCode::PageUp => b"\x1b[5~".to_vec(),
            KeyCode::PageDown => b"\x1b[6~".to_vec(),
            KeyCode::F(n) => match n {
                1 => b"\x1bOP".to_vec(),
                2 => b"\x1bOQ".to_vec(),
                3 => b"\x1bOR".to_vec(),
                4 => b"\x1bOS".to_vec(),
                5 => b"\x1b[15~".to_vec(),
                6 => b"\x1b[17~".to_vec(),
                7 => b"\x1b[18~".to_vec(),
                8 => b"\x1b[19~".to_vec(),
                9 => b"\x1b[20~".to_vec(),
                10 => b"\x1b[21~".to_vec(),
                11 => b"\x1b[23~".to_vec(),
                12 => b"\x1b[24~".to_vec(),
                _ => return None,
            },
            _ => return None,
        };
        if alt {
            out.insert(0, 0x1b);
        }
        Some(out)
    }

    #[cfg(test)]
    mod tests {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        use super::encode;

        fn k(code: KeyCode, m: KeyModifiers) -> Option<Vec<u8>> {
            encode(&KeyEvent::new(code, m))
        }

        #[test]
        fn keys_map_to_xterm_bytes() {
            assert_eq!(k(KeyCode::Char('a'), KeyModifiers::NONE), Some(b"a".to_vec()));
            assert_eq!(k(KeyCode::Char('c'), KeyModifiers::CONTROL), Some(vec![3]));
            assert_eq!(k(KeyCode::Char('D'), KeyModifiers::CONTROL | KeyModifiers::SHIFT), Some(vec![4]));
            assert_eq!(k(KeyCode::Up, KeyModifiers::NONE), Some(b"\x1b[A".to_vec()));
            assert_eq!(k(KeyCode::Char('b'), KeyModifiers::ALT), Some(b"\x1bb".to_vec()));
            assert_eq!(k(KeyCode::Enter, KeyModifiers::NONE), Some(b"\r".to_vec()));
            assert_eq!(k(KeyCode::Backspace, KeyModifiers::NONE), Some(vec![0x7f]));
            assert_eq!(k(KeyCode::F(5), KeyModifiers::NONE), Some(b"\x1b[15~".to_vec()));
            assert_eq!(k(KeyCode::Char('é'), KeyModifiers::NONE), Some("é".as_bytes().to_vec()));
            assert_eq!(k(KeyCode::CapsLock, KeyModifiers::NONE), None);
        }
    }
}

/// `forge nodes terminal`: an interactive shell on the node.
pub async fn terminal(g: &Global, node: &str, login: Option<String>) -> Result<()> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err(CliError::usage("`forge nodes terminal` needs a terminal; use `forge nodes exec` in scripts"));
    }
    let t = target(g)?;
    let mut api = connect(g, &t).await?;
    let n = resolve_node(&mut api, node).await?;
    let id = n["id"].as_str().unwrap_or_default().to_owned();
    let login = pick_login(&mut api, &id, login).await?;
    precheck(&mut api, &id, &login).await?;
    let (cols, rows) = term_size();
    let mut ws =
        open_ws(&mut api, &t, &format!("/api/v1/servers/{id}/terminal?login={}&cols={cols}&rows={rows}", enc(&login)))
            .await?;
    #[cfg(windows)]
    let _ = crossterm::ansi_support::supports_ansi();
    eprintln!(
        "Connecting to {login}@{}… (the session ends when the shell exits)",
        n["hostname"].as_str().unwrap_or(node)
    );
    let raw = RawMode::enable()?;
    let mut input = spawn_input();
    let mut stdout = std::io::stdout();
    let mut outcome: std::result::Result<String, CliError> = Ok("closed".into());
    loop {
        tokio::select! {
            msg = ws.next() => match msg {
                Some(Ok(Message::Binary(b))) => {
                    let _ = stdout.write_all(&b);
                    let _ = stdout.flush();
                }
                Some(Ok(Message::Text(t))) => {
                    let v = status_of(&t);
                    match v["type"].as_str() {
                        Some("error") => {
                            outcome = Err(CliError::new(Exit::General, v["message"].as_str().unwrap_or("terminal error").to_owned()));
                            break;
                        }
                        Some("closed") => {
                            outcome = Ok(v["reason"].as_str().unwrap_or("closed").to_owned());
                            break;
                        }
                        _ => {}
                    }
                }
                Some(Ok(Message::Close(_))) | None => break,
                Some(Ok(_)) => {}
                Some(Err(e)) => {
                    outcome = Err(CliError::new(Exit::Transport, e.to_string()));
                    break;
                }
            },
            inp = input.recv() => match inp {
                Some(Input::Bytes(b)) => {
                    if ws.send(Message::Binary(b.into())).await.is_err() {
                        break;
                    }
                }
                Some(Input::Resize(c, r)) => {
                    let _ = ws.send(Message::Text(json!({"type": "resize", "cols": c, "rows": r}).to_string().into())).await;
                }
                Some(Input::Closed) | None => break,
            },
        }
    }
    let _ = ws.close(None).await;
    drop(raw);
    match outcome {
        Ok(reason) => {
            match reason.as_str() {
                "shell_exited" | "client_closed" | "closed" => eprintln!("\r\nConnection closed."),
                "revoked" => return Err(CliError::new(Exit::General, "the session was revoked").with_code("revoked")),
                other => eprintln!("\r\nConnection closed ({other})."),
            }
            Ok(())
        }
        Err(e) => Err(e),
    }
}

// ------------------------------------------------------------------ exec

/// Largest output kept for `--output json`.
const MAX_JSON_OUTPUT: usize = 16 * 1024 * 1024;

/// `forge nodes exec`: one command, no terminal; stdout and stderr apart.
pub async fn exec(
    g: &Global,
    node: &str,
    login: Option<String>,
    shell: bool,
    propagate: bool,
    argv: Vec<String>,
) -> Result<()> {
    if argv.is_empty() {
        return Err(CliError::usage("name the command after `--`, e.g. `forge nodes exec web-01 -- uptime`"));
    }
    let t = target(g)?;
    let mut api = connect(g, &t).await?;
    let n = resolve_node(&mut api, node).await?;
    let id = n["id"].as_str().unwrap_or_default().to_owned();
    let login = pick_login(&mut api, &id, login).await?;
    precheck(&mut api, &id, &login).await?;
    let mut ws = open_ws(&mut api, &t, &format!("/api/v1/servers/{id}/exec?login={}", enc(&login))).await?;
    let first =
        if shell { json!({"type": "exec", "command": argv.join(" ")}) } else { json!({"type": "exec", "argv": argv}) };
    ws.send(Message::Text(first.to_string().into()))
        .await
        .map_err(|e| CliError::new(Exit::Transport, e.to_string()))?;

    let json_mode = g.output.is_machine();
    let (mut out_buf, mut err_buf) = (Vec::new(), Vec::new());
    let mut code: Option<u32> = None;
    let mut session = Value::Null;
    // Stdin: piped input is forwarded, then end-of-file; a terminal sends none.
    let (stdin_tx, mut stdin_rx) = tokio::sync::mpsc::channel::<Option<Vec<u8>>>(16);
    if std::io::stdin().is_terminal() {
        let _ = stdin_tx.send(None).await;
    } else {
        tokio::spawn(async move {
            let mut stdin = tokio::io::stdin();
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                match stdin.read(&mut buf).await {
                    Ok(0) | Err(_) => {
                        let _ = stdin_tx.send(None).await;
                        break;
                    }
                    Ok(n) => {
                        if stdin_tx.send(Some(buf[..n].to_vec())).await.is_err() {
                            break;
                        }
                    }
                }
            }
        });
    }
    let mut ready = false;
    let mut stdin_open = true;
    let reason = loop {
        tokio::select! {
            msg = ws.next() => match msg {
                Some(Ok(Message::Binary(b))) if !b.is_empty() => {
                    let (tag, data) = (b[0], &b[1..]);
                    if json_mode {
                        let buf = if tag == 2 { &mut err_buf } else { &mut out_buf };
                        if buf.len() < MAX_JSON_OUTPUT {
                            buf.extend_from_slice(data);
                        }
                    } else if tag == 2 {
                        let mut e = std::io::stderr();
                        let _ = e.write_all(data);
                        let _ = e.flush();
                    } else {
                        let mut o = std::io::stdout();
                        let _ = o.write_all(data);
                        let _ = o.flush();
                    }
                }
                Some(Ok(Message::Text(t))) => {
                    let v = status_of(&t);
                    match v["type"].as_str() {
                        Some("ready") => {
                            ready = true;
                            session = v["session"].clone();
                        }
                        Some("exit") => code = v["code"].as_u64().and_then(|c| u32::try_from(c).ok()),
                        Some("error") => {
                            return Err(CliError::new(Exit::General, v["message"].as_str().unwrap_or("exec failed").to_owned()));
                        }
                        Some("closed") => break v["reason"].as_str().unwrap_or("closed").to_owned(),
                        _ => {}
                    }
                }
                Some(Ok(Message::Close(_))) | None => break "closed".to_owned(),
                Some(Ok(_)) => {}
                Some(Err(e)) => return Err(CliError::new(Exit::Transport, e.to_string())),
            },
            chunk = stdin_rx.recv(), if ready && stdin_open => match chunk {
                Some(Some(b)) => {
                    if ws.send(Message::Binary(b.into())).await.is_err() {
                        break "client_gone".to_owned();
                    }
                }
                Some(None) | None => {
                    stdin_open = false;
                    let _ = ws.send(Message::Text(json!({"type": "eof"}).to_string().into())).await;
                }
            },
        }
    };
    let _ = ws.close(None).await;
    if reason == "revoked" {
        return Err(CliError::new(Exit::General, "the session was revoked").with_code("revoked"));
    }
    if code.is_none() && reason != "exited" {
        return Err(CliError::new(Exit::General, format!("the command did not finish ({reason})")).with_code(reason));
    }
    let status = code.unwrap_or(255);
    if json_mode {
        out::result(
            g.output,
            &json!({
                "node": n["hostname"], "login": login, "session": session, "exit_code": status,
                "stdout": String::from_utf8_lossy(&out_buf), "stderr": String::from_utf8_lossy(&err_buf),
                "truncated": out_buf.len() >= MAX_JSON_OUTPUT || err_buf.len() >= MAX_JSON_OUTPUT,
            }),
            None,
        );
    } else if status != 0 && !propagate {
        eprintln!("forge: the remote command exited with status {status}");
    }
    if propagate && status != 0 {
        std::process::exit(i32::try_from(status.min(255)).unwrap_or(255));
    }
    Ok(())
}

// ------------------------------------------------------------------ sessions

#[derive(Subcommand, Debug)]
pub enum SessionsCmd {
    /// Your open terminal and exec sessions (`--all`: everyone's, user.manage).
    List {
        /// Everyone's sessions (needs user.manage).
        #[arg(long)]
        all: bool,
        #[command(flatten)]
        paging: out::PageArgs,
    },
    /// End an open session within a second.
    Revoke {
        id: String,
        #[arg(long)]
        yes: bool,
    },
}

pub async fn sessions(g: &Global, cmd: SessionsCmd) -> Result<()> {
    let t = target(g)?;
    let mut api = connect(g, &t).await?;
    match cmd {
        SessionsCmd::List { all, paging } => {
            let v =
                api.get(if all { "/api/v1/terminal-sessions?all=true" } else { "/api/v1/terminal-sessions" }).await?;
            let cols: &[Col] = &[
                ("ID", "/id"),
                ("USER", "/user"),
                ("NODE", "/server_id"),
                ("LOGIN", "/login"),
                ("KIND", "/kind"),
                ("CLIENT", "/client"),
                ("STARTED", "/started_at"),
            ];
            out::list_value(g.output, v, "/sessions", Some(cols), paging);
            Ok(())
        }
        SessionsCmd::Revoke { id, yes } => {
            confirm(g, yes, &format!("End session {id}?"))?;
            let v = api.post(&format!("/api/v1/terminal-sessions/{}/revoke", enc(&id)), &json!({})).await?;
            out::result(g.output, &v, None);
            Ok(())
        }
    }
}
