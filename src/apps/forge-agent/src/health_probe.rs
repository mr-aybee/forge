//! Configured HTTP and TCP health checks, run on the node (issue #12, D-085).
//!
//! Targets follow the policy's rules once more, here where the connection is
//! made: an empty host is the workload itself (127.0.0.1); any other host is
//! resolved once, every resolved address is checked (never link-local or a
//! metadata address; loopback only unless `allow_remote_host`), and the
//! probe connects to a checked address without resolving again. HTTP
//! redirects are not followed: a 3xx answer is just a status.
//!
//! A resolved address is also refused when it is an IPv6 transition address
//! (6to4, Teredo, local-use NAT64) or an address of another node of the
//! fleet, as Forge listed them with the policy ([`Context::fleet`]).
//!
//! Headers whose value is a vault reference are filled from
//! [`Context::secrets`], which the scheduler fetched from Forge for this
//! check ([`crate::health_secrets`]); the values are written to the request
//! and nowhere else. Exec and shell checks are not run here: the node's
//! helper runs them ([`crate::health_exec`]).

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use forge_catalog::health::{HttpProbe, Method, Probe, Scheme, canonical, forbidden_address, transition_address};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;

/// What an attempt says about the workload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Pass,
    Fail,
    /// The check could not be run: neither a failure nor a recovery.
    Unknown,
}

/// Why a check did not run, as far as waiting is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Unknown {
    /// It may run next time (a policy on its way, Forge not answering, a
    /// refusal that is cached for a while). A startup check keeps waiting.
    #[default]
    Transient,
    /// The node will not run it until someone changes the node or the app
    /// (no Landlock, not Linux, no account). A startup check in this state
    /// holds nothing back.
    Permanent,
    /// The node had no free place for it. Fine once; after a few periods
    /// in a row it counts as a timeout.
    Throttled,
}

/// What one probe attempt found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub verdict: Verdict,
    /// For [`Verdict::Unknown`]: whether waiting can help.
    pub unknown: Unknown,
    /// A `ReasonCode` name, empty on success.
    pub reason: &'static str,
    pub detail: String,
}

impl Outcome {
    #[must_use]
    pub const fn pass() -> Self {
        Self { verdict: Verdict::Pass, unknown: Unknown::Transient, reason: "", detail: String::new() }
    }

    pub fn fail(reason: &'static str, detail: impl Into<String>) -> Self {
        Self { verdict: Verdict::Fail, unknown: Unknown::Transient, reason, detail: detail.into() }
    }

    /// The check did not run (the node refuses it, or Forge refused what
    /// it needs): reported as `unknown`, never as a failure. `kind` says
    /// whether waiting can help.
    pub fn unknown(kind: Unknown, reason: &'static str, detail: impl Into<String>) -> Self {
        Self { verdict: Verdict::Unknown, unknown: kind, reason, detail: detail.into() }
    }

    #[must_use]
    pub fn ok(&self) -> bool {
        self.verdict == Verdict::Pass
    }
}

/// What a probe needs besides its own fields.
#[derive(Debug, Clone, Copy, Default)]
pub struct Context<'a> {
    /// Addresses of the fleet's other nodes: never a probe target.
    pub fleet: &'a [IpAddr],
    /// The check's vault-backed header values, in policy order.
    pub secrets: &'a [crate::health_secrets::ResolvedHeader],
}

/// Runs one HTTP or TCP attempt within `timeout`.
pub async fn run(probe: &Probe, timeout: Duration, ctx: Context<'_>) -> Outcome {
    let attempt = async {
        match probe {
            Probe::Tcp(t) => match connect(&t.host, t.allow_remote_host, t.port, ctx.fleet).await {
                Ok(_) => Outcome::pass(),
                Err(o) => o,
            },
            Probe::Http(h) => http(h, ctx).await,
            Probe::Exec(_) | Probe::Shell(_) => Outcome::unknown(
                Unknown::Permanent,
                "unsupported_executor",
                "command checks are run by the node's helper",
            ),
        }
    };
    tokio::time::timeout(timeout, attempt)
        .await
        .unwrap_or_else(|_| Outcome::fail("timeout", format!("no answer within {} s", timeout.as_secs())))
}

/// The addresses a probe may connect to, from what `host` resolved to.
/// Every address must be allowed, or the whole target is refused: a name
/// that answers with one good and one forbidden address is not trusted.
fn allowed_targets(addrs: &[SocketAddr], allow_remote: bool, fleet: &[IpAddr]) -> Result<Vec<SocketAddr>, Outcome> {
    if addrs.is_empty() {
        return Err(Outcome::fail("connect_failed", "the host name has no address"));
    }

    for a in addrs {
        // A relay or translator delivers these to the IPv4 address they
        // name, outside what the checks below can see.
        if transition_address(a.ip()) {
            return Err(Outcome::fail(
                "invalid_config",
                format!("{} is an IPv6 transition address (6to4, Teredo or NAT64)", a.ip()),
            ));
        }

        let ip = canonical(a.ip());
        if forbidden_address(ip) {
            return Err(Outcome::fail("invalid_config", format!("{ip} is a link-local or metadata address")));
        }

        if !allow_remote && !ip.is_loopback() {
            return Err(Outcome::fail(
                "invalid_config",
                format!("{ip} is not the workload itself; allow a remote host in the check to probe it"),
            ));
        }

        if fleet.iter().any(|f| canonical(*f) == ip) {
            return Err(Outcome::fail("invalid_config", format!("{ip} is another node of the fleet")));
        }
    }

    Ok(addrs.to_vec())
}

/// Resolves `host` once, checks every address and connects to the first.
async fn connect(host: &str, allow_remote: bool, port: u16, fleet: &[IpAddr]) -> Result<TcpStream, Outcome> {
    let addrs: Vec<SocketAddr> = if host.is_empty() {
        vec![SocketAddr::from(([127, 0, 0, 1], port))]
    } else if let Ok(ip) = host.parse::<IpAddr>() {
        allowed_targets(&[SocketAddr::new(ip, port)], allow_remote, fleet)?
    } else {
        let found: Vec<SocketAddr> = tokio::net::lookup_host((host, port))
            .await
            .map_err(|e| Outcome::fail("connect_failed", format!("{host}: {e}")))?
            .collect();
        allowed_targets(&found, allow_remote, fleet)?
    };

    let Some(addr) = addrs.first() else { return Err(Outcome::fail("connect_failed", "no address")) };

    TcpStream::connect(addr).await.map_err(|e| Outcome::fail("connect_failed", format!("port {port}: {e}")))
}

/// Whether the probe's vault values may go to `peer`, the address the
/// connection was made to: always over HTTPS with a verified certificate,
/// otherwise only to this machine. A probe without vault values may go
/// anywhere the target rules allow.
fn may_send_secrets(h: &HttpProbe, peer: Option<IpAddr>) -> bool {
    !h.has_secret() || h.verified_tls() || peer.is_some_and(|ip| canonical(ip).is_loopback())
}

async fn http(h: &HttpProbe, ctx: Context<'_>) -> Outcome {
    let request = match request(h, ctx.secrets) {
        Ok(r) => r,
        Err(e) => return Outcome::fail("invalid_config", e),
    };

    let stream = match connect(&h.host, h.allow_remote_host, h.port, ctx.fleet).await {
        Ok(s) => s,
        Err(o) => return o,
    };

    // A vault value goes in clear text, or to a peer nobody verified, only
    // to this machine. Checked on the connection that was made, not on how
    // the host was spelled: `localhost` is whatever the resolver says.
    if !may_send_secrets(h, stream.peer_addr().ok().map(|a| a.ip())) {
        return Outcome::fail(
            "invalid_config",
            "a vault secret is sent to another host only over HTTPS with certificate verification",
        );
    }

    let status = match h.scheme {
        Scheme::Http => exchange(stream, &request).await,
        Scheme::Https => {
            let name = if !h.server_name.is_empty() {
                h.server_name.clone()
            } else if h.host.is_empty() {
                "localhost".to_owned()
            } else {
                h.host.clone()
            };

            let Ok(server_name) = rustls_pki_types::ServerName::try_from(name) else {
                return Outcome::fail("invalid_config", "the TLS server name is not valid");
            };

            let cfg = match crate::tls::health_probe(h.tls_verify) {
                Ok(c) => c,
                Err(e) => return Outcome::fail("tls", e.to_string()),
            };

            match tokio_rustls::TlsConnector::from(cfg).connect(server_name, stream).await {
                Ok(tls) => exchange(tls, &request).await,
                Err(e) => return Outcome::fail("tls", e.to_string()),
            }
        }
    };

    match status {
        Ok(code) if h.expected_statuses.iter().any(|r| (r.from..=r.to).contains(&code)) => Outcome::pass(),
        Ok(code @ (401 | 403)) => Outcome::fail("auth", format!("status {code}")),
        Ok(code) => Outcome::fail("app_failure", format!("status {code}")),
        Err(e) => Outcome::fail("app_failure", e),
    }
}

/// The request bytes. Refuses anything that could add a line of its own.
/// A header with a vault reference takes the next value of `secrets`, which
/// must be for that header; errors name the header, never a value.
fn request(h: &HttpProbe, secrets: &[crate::health_secrets::ResolvedHeader]) -> Result<Vec<u8>, String> {
    use std::fmt::Write as _;
    let clean = |s: &str| !s.bytes().any(|b| b == b'\r' || b == b'\n' || b == 0);
    if !h.path.starts_with('/') || !clean(&h.path) || h.path.contains(' ') {
        return Err("the path is not valid".into());
    }

    let method = match h.method {
        Method::Get => "GET",
        Method::Head => "HEAD",
    };

    let host = if h.host.is_empty() { "localhost" } else { h.host.as_str() };
    let host = if host.contains(':') { format!("[{host}]") } else { host.to_owned() };
    let mut out = format!(
        "{method} {} HTTP/1.1\r\nHost: {host}:{}\r\nUser-Agent: forge-health\r\nAccept: */*\r\nConnection: close\r\n",
        h.path, h.port
    );
    let mut resolved = secrets.iter();

    for hd in &h.headers {
        let value = match (&hd.value, &hd.secret) {
            (Some(v), _) => v.as_str(),
            (None, Some(_)) => match resolved.next() {
                Some(r) if r.name == hd.name => r.value(),
                _ => return Err(format!("header `{}` has no value from the vault", hd.name)),
            },
            (None, None) => return Err(format!("header `{}` has no value", hd.name)),
        };

        let token = !hd.name.is_empty() && hd.name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
        if !token || !clean(value) {
            return Err(format!("header `{}` is not valid", hd.name));
        }

        let _ = write!(out, "{}: {value}\r\n", hd.name);
    }

    out.push_str("\r\n");

    Ok(out.into_bytes())
}

/// Sends the request and reads the status code; the body is not read.
async fn exchange<S: AsyncRead + AsyncWrite + Unpin>(mut stream: S, request: &[u8]) -> Result<u16, String> {
    stream.write_all(request).await.map_err(|e| format!("sending the request: {e}"))?;
    let mut buf = Vec::with_capacity(64);
    let mut chunk = [0u8; 64];

    // Stop at the end of the status line.
    while buf.len() < 1024 && !buf.contains(&b'\n') {
        let n = stream.read(&mut chunk).await.map_err(|e| format!("reading the answer: {e}"))?;
        if n == 0 {
            break;
        }

        buf.extend_from_slice(chunk.get(..n).unwrap_or_default());
    }

    status_code(&buf).ok_or_else(|| "the answer is not HTTP".to_owned())
}

fn status_code(answer: &[u8]) -> Option<u16> {
    let line = answer.split(|b| *b == b'\n').next()?;
    let line = std::str::from_utf8(line).ok()?;
    let mut parts = line.split_whitespace();
    if !parts.next()?.starts_with("HTTP/1.") {
        return None;
    }

    parts.next()?.parse::<u16>().ok().filter(|c| (100..=599).contains(c))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::health_secrets::ResolvedHeader;
    use serde_json::json;

    fn probe(v: serde_json::Value) -> Probe {
        serde_json::from_value(v).unwrap()
    }

    /// Answers every connection with `answer`, after reading the request.
    async fn server(answer: &'static str) -> (u16, tokio::sync::mpsc::UnboundedReceiver<String>) {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = l.accept().await {
                let mut buf = vec![0u8; 4096];
                let n = s.read(&mut buf).await.unwrap_or(0);
                let _ = tx.send(String::from_utf8_lossy(&buf[..n]).into_owned());
                let _ = s.write_all(answer.as_bytes()).await;
            }
        });

        (port, rx)
    }

    const T: Duration = Duration::from_secs(2);

    async fn plain(p: &Probe, timeout: Duration) -> Outcome {
        run(p, timeout, Context::default()).await
    }

    #[tokio::test]
    async fn http_checks_compare_the_status_and_send_the_headers() {
        let (port, mut seen) = server("HTTP/1.1 204 No Content\r\n\r\n").await;
        let p = probe(json!({ "type": "http", "port": port, "path": "/healthz?deep=1",
            "headers": [{ "name": "X-Probe", "value": "forge" }] }));
        assert_eq!(plain(&p, T).await, Outcome::pass());
        let req = seen.recv().await.unwrap();
        assert!(req.starts_with("GET /healthz?deep=1 HTTP/1.1\r\n"), "{req}");
        assert!(req.contains(&format!("Host: localhost:{port}\r\n")) && req.contains("X-Probe: forge\r\n"), "{req}");

        let p = probe(json!({ "type": "http", "port": port, "method": "HEAD",
            "expected_statuses": [{ "from": 200, "to": 200 }] }));
        let o = plain(&p, T).await;
        assert_eq!((o.verdict, o.reason, o.detail.as_str()), (Verdict::Fail, "app_failure", "status 204"));
    }

    /// D-111: a vault-backed header is sent with the value Forge handed
    /// out for this check; without one, nothing is sent at all.
    #[tokio::test]
    async fn vault_headers_are_sent_only_with_a_resolved_value() {
        let (port, mut seen) = server("HTTP/1.1 200 OK\r\n\r\n").await;
        let p = probe(json!({ "type": "http", "port": port, "headers": [
            { "name": "X-Plain", "value": "1" },
            { "name": "Authorization", "secret": "vault:abc" }
        ] }));
        let secrets = [ResolvedHeader::new("Authorization", "Bearer s3cret")];
        let o = run(&p, T, Context { fleet: &[], secrets: &secrets }).await;
        assert_eq!(o, Outcome::pass());
        let req = seen.recv().await.unwrap();
        assert!(req.contains("X-Plain: 1\r\nAuthorization: Bearer s3cret\r\n"), "{req}");
        assert!(!req.contains("vault:abc"), "the reference itself is never sent");

        // No value, or a value for another header: refused before connecting.
        for secrets in [vec![], vec![ResolvedHeader::new("Cookie", "x")]] {
            let o = run(&p, T, Context { fleet: &[], secrets: &secrets }).await;
            assert_eq!((o.verdict, o.reason), (Verdict::Fail, "invalid_config"));
            assert!(o.detail.contains("Authorization") && !o.detail.contains("s3cret"), "{}", o.detail);
        }

        assert!(seen.try_recv().is_err(), "no request reached the target");

        // A value that would add a line of its own is refused, and not quoted.
        let split = [ResolvedHeader::new("Authorization", "x\r\nHost: evil")];
        let o = run(&p, T, Context { fleet: &[], secrets: &split }).await;
        assert!(o.verdict == Verdict::Fail && !o.detail.contains("evil"), "{}", o.detail);
    }

    /// D-111: a vault value is never sent in clear text, or to a peer
    /// nobody verified, unless the peer is this machine. The policy is
    /// refused when saved; this is the node's own check, on the address it
    /// connected to.
    #[test]
    fn vault_values_need_verified_https_beyond_this_machine() {
        let h = |v: serde_json::Value| -> HttpProbe { serde_json::from_value(v).unwrap() };
        let ip = |s: &str| Some(s.parse::<IpAddr>().unwrap());
        let secret = json!([{ "name": "Authorization", "secret": "vault:abc" }]);
        let plain = h(json!({ "port": 80, "host": "api.internal", "allow_remote_host": true, "headers": secret }));
        let unverified = h(json!({ "port": 443, "scheme": "https", "tls_verify": false, "host": "api.internal",
            "allow_remote_host": true, "headers": secret }));
        let verified = h(json!({ "port": 443, "scheme": "https", "host": "api.internal",
            "allow_remote_host": true, "headers": secret }));

        for remote in ["10.0.0.5", "2001:db8::1", "192.0.2.1"] {
            assert!(!may_send_secrets(&plain, ip(remote)), "plain HTTP to {remote}");
            assert!(!may_send_secrets(&unverified, ip(remote)), "unverified TLS to {remote}");
            assert!(may_send_secrets(&verified, ip(remote)));
        }

        for local in ["127.0.0.1", "::1", "::ffff:127.0.0.1"] {
            assert!(may_send_secrets(&plain, ip(local)) && may_send_secrets(&unverified, ip(local)), "{local}");
        }

        assert!(!may_send_secrets(&plain, None), "an unknown peer is not this machine");

        // `localhost` that resolves elsewhere gets no value either: the
        // spelling of the host decides nothing here.
        let named = h(json!({ "port": 80, "host": "localhost", "allow_remote_host": true, "headers": secret }));
        assert!(!may_send_secrets(&named, ip("10.0.0.5")));

        // Without a vault value nothing is restricted.
        let open = h(json!({ "port": 80, "host": "api.internal", "allow_remote_host": true }));
        assert!(may_send_secrets(&open, ip("10.0.0.5")));
    }

    #[tokio::test]
    async fn refused_statuses_and_redirects_fail() {
        let (port, _seen) = server("HTTP/1.1 302 Found\r\nLocation: http://169.254.169.254/\r\n\r\n").await;
        let o = plain(&probe(json!({ "type": "http", "port": port })), T).await;
        assert_eq!((o.ok(), o.detail.as_str()), (false, "status 302"), "a redirect is not followed");
        let (port, _seen) = server("HTTP/1.1 401 Unauthorized\r\n\r\n").await;
        let o = plain(&probe(json!({ "type": "http", "port": port })), T).await;
        assert_eq!(o.reason, "auth");
        let (port, _seen) = server("SSH-2.0-OpenSSH_9.6\r\n").await;
        let o = plain(&probe(json!({ "type": "http", "port": port })), T).await;
        assert_eq!(o.detail, "the answer is not HTTP");
    }

    #[tokio::test]
    async fn tcp_checks_connect_and_report_refusals_and_timeouts() {
        let (port, _seen) = server("").await;
        assert_eq!(plain(&probe(json!({ "type": "tcp", "port": port })), T).await, Outcome::pass());

        // A port nothing listens on.
        let free = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };

        let o = plain(&probe(json!({ "type": "tcp", "port": free })), T).await;

        // Windows retries a refused connect for longer than the timeout.
        assert!(!o.ok() && matches!(o.reason, "connect_failed" | "timeout"), "{o:?}");

        // A server that accepts and never answers.
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let o = plain(&probe(json!({ "type": "http", "port": port })), Duration::from_millis(300)).await;
        assert_eq!(o.reason, "timeout");
        drop(l);
    }

    #[test]
    fn targets_are_checked_where_the_connection_is_made() {
        let a = |s: &str| -> SocketAddr { format!("{s}:80").parse().unwrap() };

        // The workload itself needs no permission.
        assert!(allowed_targets(&[a("127.0.0.1")], false, &[]).is_ok());

        // Another host needs `allow_remote_host`.
        assert_eq!(allowed_targets(&[a("10.0.0.5")], false, &[]).unwrap_err().reason, "invalid_config");
        assert!(allowed_targets(&[a("10.0.0.5")], true, &[]).is_ok());

        // Metadata and link-local are refused even when allowed, also as one
        // of several answers and in IPv4-mapped form.
        assert!(allowed_targets(&[a("169.254.169.254")], true, &[]).is_err());
        assert!(allowed_targets(&[a("10.0.0.5"), a("169.254.169.254")], true, &[]).is_err());
        assert!(allowed_targets(&["[::ffff:169.254.169.254]:80".parse().unwrap()], true, &[]).is_err());
        assert_eq!(allowed_targets(&[], true, &[]).unwrap_err().reason, "connect_failed");
    }

    /// 6to4, Teredo and local-use NAT64 name an IPv4 address a relay
    /// delivers to: refused whatever they resolve from.
    #[test]
    fn transition_addresses_are_refused_at_probe_time() {
        for ip in ["2002:a9fe:a9fe::1", "64:ff9b:1::a9fe:a9fe", "2001:0:a9fe:a9fe::1", "2002:a00:5::1"] {
            let target: SocketAddr = format!("[{ip}]:80").parse().unwrap();
            let o = allowed_targets(&[target], true, &[]).unwrap_err();
            assert_eq!(o.reason, "invalid_config", "{ip}");
            assert!(o.detail.contains("transition"), "{ip}: {}", o.detail);

            // Also as one of several answers of a name.
            let ok: SocketAddr = "10.0.0.5:80".parse().unwrap();
            assert!(allowed_targets(&[ok, target], true, &[]).is_err(), "{ip}");
        }

        // Ordinary global IPv6 still passes.
        assert!(allowed_targets(&["[2001:db8::1]:80".parse().unwrap()], true, &[]).is_ok());
    }

    /// A remote host that resolves to another node of the fleet is refused
    /// on the node, in every spelling of that address.
    #[tokio::test]
    async fn other_nodes_of_the_fleet_are_refused_at_probe_time() {
        let fleet: Vec<IpAddr> = vec!["10.0.0.7".parse().unwrap(), "fd00::7".parse().unwrap()];
        let a = |s: &str| -> SocketAddr { s.parse().unwrap() };

        for target in ["10.0.0.7:80", "[::ffff:10.0.0.7]:80", "[64:ff9b::a00:7]:80", "[fd00::7]:80"] {
            let o = allowed_targets(&[a(target)], true, &fleet).unwrap_err();
            assert_eq!(o.reason, "invalid_config", "{target}");
            assert!(o.detail.contains("another node of the fleet"), "{target}: {}", o.detail);
        }

        assert!(allowed_targets(&[a("10.0.0.5:80"), a("10.0.0.7:80")], true, &fleet).is_err(), "one of several");
        assert!(allowed_targets(&[a("10.0.0.8:80")], true, &fleet).is_ok());

        // Through the probe itself: refused before any connection.
        let p = probe(json!({ "type": "tcp", "host": "10.0.0.7", "allow_remote_host": true, "port": 9 }));
        let o = run(&p, T, Context { fleet: &fleet, secrets: &[] }).await;
        assert_eq!((o.verdict, o.reason), (Verdict::Fail, "invalid_config"));
    }

    #[test]
    fn requests_cannot_be_split() {
        let h = |v: serde_json::Value| -> HttpProbe { serde_json::from_value(v).unwrap() };
        assert!(request(&h(json!({ "port": 80, "path": "/a\r\nX: 1" })), &[]).is_err());
        assert!(request(&h(json!({ "port": 80, "path": "/a b" })), &[]).is_err());
        let bad = h(json!({ "port": 80, "headers": [{ "name": "X-A", "value": "1\r\nHost: evil" }] }));
        assert!(request(&bad, &[]).is_err());
        let v6 = h(json!({ "port": 8443, "host": "fd00::7", "allow_remote_host": true }));
        assert!(String::from_utf8(request(&v6, &[]).unwrap()).unwrap().contains("Host: [fd00::7]:8443\r\n"));
    }

    #[tokio::test]
    async fn command_checks_are_not_run_by_this_executor() {
        let o = plain(&probe(json!({ "type": "exec", "argv": ["/bin/true"] })), T).await;
        assert_eq!((o.verdict, o.reason), (Verdict::Unknown, "unsupported_executor"));
    }

    #[test]
    fn status_lines_are_parsed() {
        assert_eq!(status_code(b"HTTP/1.1 200 OK\r\n"), Some(200));
        assert_eq!(status_code(b"HTTP/1.0 503\r\n"), Some(503));
        assert_eq!(status_code(b"HTTP/2 200\r\n"), None);
        assert_eq!(status_code(b"HTTP/1.1 999 X\r\n"), None);
        assert_eq!(status_code(b""), None);
    }
}
