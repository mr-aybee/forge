//! Browser sign-in: OAuth 2.1 authorization code + PKCE (S256) with a
//! loopback redirect (RFC 8252), against Forge's existing `/oauth/authorize`
//! and `/oauth/token`. Password, MFA, passkeys and SSO all happen in the
//! browser, so the CLI never sees a password.

use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::rand::{SecureRandom, SystemRandom};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::api::{CLIENT_ID, http_client, problem, token_pair};
use crate::creds::Tokens;
use crate::error::{CliError, Exit, Result};

fn random_b64(n: usize) -> Result<String> {
    let mut buf = vec![0u8; n];
    SystemRandom::new().fill(&mut buf).map_err(|_| CliError::new(Exit::General, "no secure random source"))?;

    Ok(URL_SAFE_NO_PAD.encode(buf))
}

/// PKCE verifier (43+ chars) and its S256 challenge.
pub fn pkce() -> Result<(String, String)> {
    let verifier = random_b64(32)?;
    let digest = ring::digest::digest(&ring::digest::SHA256, verifier.as_bytes());

    Ok((verifier, URL_SAFE_NO_PAD.encode(digest.as_ref())))
}

fn open_browser(url: &str) -> bool {
    // The URL is built by us (server origin + encoded query); it is passed as
    // a single argument, never through a shell string.
    let status = if cfg!(windows) {
        std::process::Command::new("rundll32").args(["url.dll,FileProtocolHandler", url]).status()
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(url).status()
    } else {
        std::process::Command::new("xdg-open").arg(url).status()
    };

    status.is_ok_and(|s| s.success())
}

/// Pulls `code` and `state` from `GET /callback?code=…&state=… HTTP/1.1`.
fn parse_callback(request: &str) -> Option<(Option<String>, Option<String>, Option<String>)> {
    let line = request.lines().next()?;
    let target = line.strip_prefix("GET ")?.split(' ').next()?;
    let url = url::Url::parse(&format!("http://127.0.0.1{target}")).ok()?;
    if url.path() != "/callback" {
        return None;
    }

    let mut code = None;
    let mut state = None;
    let mut error = None;

    for (k, v) in url.query_pairs() {
        match k.as_ref() {
            "code" => code = Some(v.into_owned()),
            "state" => state = Some(v.into_owned()),
            "error" => error = Some(v.into_owned()),
            _ => {}
        }
    }

    Some((code, state, error))
}

const PAGE_OK: &str = "<!doctype html><meta charset=utf-8><title>Orbyn Forge</title><body style=\"font-family:system-ui;padding:3rem\"><h2>Signed in</h2><p>You can close this tab and return to the terminal.</p>";
const PAGE_ERR: &str = "<!doctype html><meta charset=utf-8><title>Orbyn Forge</title><body style=\"font-family:system-ui;padding:3rem\"><h2>Sign-in failed</h2><p>Return to the terminal for details.</p>";

/// Runs the browser flow and returns tokens for `server`.
pub async fn browser(server: &str, ca_file: Option<&str>, timeout: Duration) -> Result<Tokens> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let redirect_uri = format!("http://127.0.0.1:{port}/callback");
    let (verifier, challenge) = pkce()?;
    let state = random_b64(16)?;
    let mut url = url::Url::parse(&format!("{server}/oauth/authorize"))
        .map_err(|_| CliError::usage(format!("invalid server URL {server}")))?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", CLIENT_ID)
        .append_pair("redirect_uri", &redirect_uri)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", &state);

    eprintln!("Opening your browser to sign in to {server} …");

    if !open_browser(url.as_str()) {
        eprintln!("Could not open a browser. Open this URL on this machine:\n\n  {url}\n");
    } else {
        eprintln!("If nothing opened, visit:\n  {url}");
    }

    let code = tokio::time::timeout(timeout, async {
        loop {
            let (mut sock, _) = listener.accept().await?;
            let mut buf = vec![0u8; 8192];
            let n = sock.read(&mut buf).await?;
            let req = String::from_utf8_lossy(&buf[..n]).into_owned();

            let Some((code, got_state, error)) = parse_callback(&req) else {
                let _ = sock.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                continue;
            };

            let ok = error.is_none() && code.is_some() && got_state.as_deref() == Some(state.as_str());
            let page = if ok { PAGE_OK } else { PAGE_ERR };
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{page}",
                page.len()
            );
            let _ = sock.write_all(resp.as_bytes()).await;

            if let Some(e) = error {
                return Err(CliError::auth(format!("sign-in was refused: {e}")));
            }

            if got_state.as_deref() != Some(state.as_str()) {
                // Not our request (wrong or missing state): keep waiting.
                continue;
            }

            return code.ok_or_else(|| CliError::auth("the callback carried no code"));
        }
    })
    .await
    .map_err(|_| CliError::new(Exit::Timeout, "timed out waiting for the browser sign-in"))??;

    let http = http_client(ca_file)?;
    let resp = http
        .post(format!("{server}/oauth/token"))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("code_verifier", verifier.as_str()),
            ("client_id", CLIENT_ID),
            ("redirect_uri", redirect_uri.as_str()),
        ])
        .send()
        .await
        .map_err(|e| CliError::new(Exit::Transport, e.to_string()))?;

    if !resp.status().is_success() {
        return Err(problem(resp).await);
    }

    let v: Value = resp.json().await.map_err(|e| CliError::new(Exit::Transport, e.to_string()))?;

    token_pair(server, &v)
}

/// The OAuth device grant type (RFC 8628).
pub const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";

/// Device sign-in (RFC 8628) for machines without a browser (SSH sessions,
/// servers, containers): prints a URL and a short code, then waits while a
/// person approves it in the console on another device.
pub async fn device(server: &str, ca_file: Option<&str>, timeout: Duration) -> Result<Tokens> {
    let http = http_client(ca_file)?;
    let resp = http
        .post(format!("{server}/oauth/device_authorization"))
        .form(&[("client_id", CLIENT_ID)])
        .send()
        .await
        .map_err(|e| CliError::new(Exit::Transport, e.to_string()))?;

    if !resp.status().is_success() {
        return Err(problem(resp).await);
    }

    let start: Value = resp.json().await.map_err(|e| CliError::new(Exit::Transport, e.to_string()))?;
    let field = |k: &str| start.get(k).and_then(Value::as_str).map(str::to_owned);
    let device_code = field("device_code").ok_or_else(|| CliError::auth("the server did not return a device code"))?;
    let user_code = field("user_code").unwrap_or_default();
    let uri = field("verification_uri").unwrap_or_else(|| format!("{server}/device"));
    let complete = field("verification_uri_complete");
    let mut interval = Duration::from_secs(start.get("interval").and_then(Value::as_u64).unwrap_or(5).max(1));
    let expires = Duration::from_secs(start.get("expires_in").and_then(Value::as_u64).unwrap_or(600));

    eprintln!("To sign in, open this page on any device:\n\n  {uri}\n\nand enter the code:  {user_code}\n");

    if let Some(c) = &complete {
        eprintln!("Or open: {c}\n");
    }

    eprintln!("Check that the code matches before you approve. Waiting…");

    let deadline = tokio::time::Instant::now() + timeout.min(expires);

    loop {
        tokio::time::sleep(interval).await;

        if tokio::time::Instant::now() >= deadline {
            return Err(CliError::new(
                Exit::Timeout,
                "the device code expired before it was approved; run the command again",
            ));
        }

        let resp = http
            .post(format!("{server}/oauth/token"))
            .form(&[("grant_type", DEVICE_GRANT), ("device_code", device_code.as_str()), ("client_id", CLIENT_ID)])
            .send()
            .await
            .map_err(|e| CliError::new(Exit::Transport, e.to_string()))?;
        let ok = resp.status().is_success();
        let v: Value = resp.json().await.unwrap_or(Value::Null);

        if ok {
            return token_pair(server, &v);
        }

        match v.get("error").and_then(Value::as_str).unwrap_or_default() {
            "authorization_pending" => {}
            "slow_down" => interval += Duration::from_secs(5),
            "access_denied" => return Err(CliError::auth("the sign-in was denied in the console")),
            "expired_token" => {
                return Err(CliError::new(Exit::Timeout, "the device code expired; run the command again"));
            }
            other => {
                let d = v.get("error_description").and_then(Value::as_str).unwrap_or(other);
                return Err(CliError::auth(format!("device sign-in failed: {d}")));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_challenge_is_s256_of_verifier() {
        let (v, c) = pkce().unwrap_or_default();
        assert!(v.len() >= 43);
        let d = ring::digest::digest(&ring::digest::SHA256, v.as_bytes());
        assert_eq!(c, URL_SAFE_NO_PAD.encode(d.as_ref()));
    }

    #[test]
    fn callback_is_parsed_strictly() {
        let r = parse_callback("GET /callback?code=abc&state=xyz HTTP/1.1\r\nHost: x\r\n\r\n");
        assert_eq!(r, Some((Some("abc".into()), Some("xyz".into()), None)));
        assert_eq!(parse_callback("GET /other?code=abc HTTP/1.1"), None);
        assert_eq!(parse_callback("POST /callback HTTP/1.1"), None);
        let e = parse_callback("GET /callback?error=access_denied&state=s HTTP/1.1");
        assert_eq!(e, Some((None, Some("s".into()), Some("access_denied".into()))));
    }
}
