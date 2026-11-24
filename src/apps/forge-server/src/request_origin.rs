//! The address the client used (D-167): scheme, host and port of this
//! request, so links and same-origin checks work under any name or IP.
//!
//! * Normally the `Host` header (or the HTTP/2 authority). The API listener
//!   is plain HTTP, so the scheme is `http`, except when the host and port
//!   are those of the pinned public address: then its scheme (a TLS proxy
//!   in front that is not listed in `trusted_proxies`).
//! * When the TCP peer is in `[security] trusted_proxies`, the **last**
//!   `X-Forwarded-Host` and `X-Forwarded-Proto` values instead. From any
//!   other peer those headers are ignored (anyone can send them).
//!
//! Hosts are validated strictly (a host name or IP literal, an optional
//! port, at most 253 characters, no user info or spaces): anything else is
//! `400 Bad Request`. `X-Forwarded-Proto` must be `http` or `https`.
//!
//! Only signed-in handlers and the token-protected install scripts build
//! links from this; background links (emails, webhooks, OIDC) use the
//! pinned or last administrator address (`forge_core::public_address`).

use std::net::SocketAddr;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{HeaderMap, header};
use forge_core::allowed_hosts::{HostError, MAX_HOST_LEN, normalize_host};

use crate::error::ApiError;
use crate::state::AppState;
use crate::throttle::TrustedNet;

/// `http` or `https`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scheme {
    Http,
    Https,
}

impl Scheme {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Https => "https",
        }
    }

    const fn default_port(self) -> u16 {
        match self {
            Self::Http => 80,
            Self::Https => 443,
        }
    }

    fn parse(s: &str) -> Result<Self, OriginError> {
        match s.trim().to_ascii_lowercase().as_str() {
            "http" => Ok(Self::Http),
            "https" => Ok(Self::Https),
            _ => Err(OriginError::Proto),
        }
    }
}

/// Why the request's address was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OriginError {
    #[error("bad host: {0}")]
    Host(#[from] HostError),
    #[error("bad port in the host")]
    Port,
    #[error("X-Forwarded-Proto must be http or https")]
    Proto,
}

/// The scheme, host and port this request was addressed to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestOrigin {
    scheme: Scheme,
    /// Canonical host (lowercase; IPv6 in brackets).
    host: String,
    /// Port, when not the scheme's default.
    port: Option<u16>,
}

/// What [`RequestOrigin::resolve`] reads.
#[derive(Debug, Clone, Copy)]
pub struct Inputs<'a> {
    pub headers: &'a HeaderMap,
    /// The request URI's authority (HTTP/2 `:authority`, absolute-form).
    pub authority: Option<&'a str>,
    /// The TCP peer (`None` for in-process dispatch).
    pub peer: Option<SocketAddr>,
    pub trusted: &'a [TrustedNet],
    /// The pinned public address, if any (`scheme://host[:port]`).
    pub pinned: Option<&'a str>,
}

/// Splits `host[:port]` and validates both.
fn parse_authority(s: &str) -> Result<(String, Option<u16>), OriginError> {
    let s = s.trim();
    if s.len() > MAX_HOST_LEN + 8 {
        return Err(HostError::TooLong.into());
    }
    if s.contains('@') || s.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(HostError::Invalid(s.chars().take(80).collect()).into());
    }
    let (host, port) = if s.starts_with('[') {
        match s.find(']') {
            Some(end) => {
                let rest = &s[end + 1..];
                let port = match rest {
                    "" => None,
                    r => Some(r.strip_prefix(':').ok_or(OriginError::Port)?),
                };
                (&s[..=end], port)
            }
            None => return Err(HostError::Invalid(s.chars().take(80).collect()).into()),
        }
    } else {
        match s.split_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (s, None),
        }
    };
    let port = match port {
        None => None,
        Some(p) if !p.is_empty() && p.len() <= 5 && p.bytes().all(|b| b.is_ascii_digit()) => {
            Some(p.parse::<u16>().ok().filter(|p| *p != 0).ok_or(OriginError::Port)?)
        }
        Some(_) => return Err(OriginError::Port),
    };
    // A bare IPv6 address (`2001:db8::1`) is not a valid Host: brackets are
    // required there, and the `split_once` above already refused it.
    Ok((normalize_host(host)?, port))
}

/// The last comma-separated value of `name` (the one the nearest proxy set).
fn last_value<'h>(headers: &'h HeaderMap, name: &str) -> Option<&'h str> {
    headers
        .get_all(name)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .next_back()
        .map(str::trim)
        .filter(|v| !v.is_empty())
}

impl RequestOrigin {
    /// Works out the request's address; see the module docs.
    pub fn resolve(i: Inputs<'_>) -> Result<Self, OriginError> {
        let trusted_peer = i.peer.is_some_and(|p| i.trusted.iter().any(|t| t.contains(p.ip())));
        let (fwd_host, fwd_proto) = if trusted_peer {
            (last_value(i.headers, "x-forwarded-host"), last_value(i.headers, "x-forwarded-proto"))
        } else {
            (None, None)
        };
        let fwd_proto = fwd_proto.map(Scheme::parse).transpose()?;
        let raw = fwd_host.or(i.authority).or_else(|| i.headers.get(header::HOST).and_then(|v| v.to_str().ok()));
        let pinned = i.pinned.and_then(Self::from_origin);
        let Some(raw) = raw else {
            // No host at all: in-process dispatch or an HTTP/1.0 client.
            return Ok(pinned.unwrap_or_else(|| Self::localhost(fwd_proto.unwrap_or(Scheme::Http))));
        };
        let (host, port) = parse_authority(raw)?;
        let scheme = match fwd_proto {
            Some(s) => s,
            // The pinned address's own host and port: its scheme (a TLS
            // proxy in front). Anything else reached this plain listener.
            None => match &pinned {
                Some(p) if p.host == host && p.port == port.filter(|n| *n != p.scheme.default_port()) => p.scheme,
                _ => Scheme::Http,
            },
        };
        Ok(Self { scheme, host, port: port.filter(|p| *p != scheme.default_port()) })
    }

    /// Parses an origin (`scheme://host[:port]`, as stored for the pinned
    /// address).
    #[must_use]
    pub fn from_origin(origin: &str) -> Option<Self> {
        let (scheme, rest) = origin.trim().split_once("://")?;
        let scheme = Scheme::parse(scheme).ok()?;
        let (host, port) = parse_authority(rest.trim_end_matches('/')).ok()?;
        Some(Self { scheme, host, port: port.filter(|p| *p != scheme.default_port()) })
    }

    fn localhost(scheme: Scheme) -> Self {
        Self { scheme, host: "localhost".into(), port: None }
    }

    #[must_use]
    pub const fn scheme(&self) -> Scheme {
        self.scheme
    }

    /// Whether the client used HTTPS (directly or through a trusted proxy).
    #[must_use]
    pub fn is_https(&self) -> bool {
        self.scheme == Scheme::Https
    }

    /// The canonical host, without port (IPv6 in brackets).
    #[must_use]
    pub fn host(&self) -> &str {
        &self.host
    }

    /// `scheme://host[:port]`, the browser's `Origin` for this address.
    #[must_use]
    pub fn origin(&self) -> String {
        match self.port {
            Some(p) => format!("{}://{}:{p}", self.scheme.as_str(), self.host),
            None => format!("{}://{}", self.scheme.as_str(), self.host),
        }
    }

    /// The agent link on the same host: `https://<host>:<agent_port>`.
    #[must_use]
    pub fn agent_url(&self, agent_port: u16) -> String {
        format!("https://{}:{agent_port}", self.host)
    }
}

/// The console and agent URLs a node is told to use, from the address the
/// person or node used for this request (D-167).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeUrls {
    /// Where the node downloads the installer and agent:
    /// `node_public_url` when configured, else this request's origin.
    pub forge_url: String,
    /// The agent link: `agent_public_url` when this request came in on the
    /// pinned address (existing installs keep their URL), else
    /// `https://<request host>:<agent_listen port>`.
    pub agent_url: String,
}

impl NodeUrls {
    #[must_use]
    pub fn for_request(config: &crate::config::ServerConfig, pinned: Option<&str>, origin: &RequestOrigin) -> Self {
        let forge_url =
            config.node_public_url.as_deref().map_or_else(|| origin.origin(), |u| u.trim_end_matches('/').to_owned());
        let on_pinned = pinned.and_then(RequestOrigin::from_origin).is_some_and(|p| p.host == origin.host);
        let agent_url = if on_pinned {
            config.agent_public_url.trim_end_matches('/').to_owned()
        } else {
            origin.agent_url(config.agent_listen.port())
        };
        Self { forge_url, agent_url }
    }

    /// [`Self::for_request`] with the server's pinned address.
    #[must_use]
    pub fn from_state(state: &AppState, origin: &RequestOrigin) -> Self {
        Self::for_request(&state.config, state.public_address.pinned().as_deref(), origin)
    }
}

/// The inputs from request parts and the server state.
fn resolve_parts(parts: &Parts, state: &AppState) -> Result<RequestOrigin, OriginError> {
    let peer = parts.extensions.get::<axum::extract::ConnectInfo<SocketAddr>>().map(|c| c.0);
    let trusted = state.config.trusted_proxies();
    let pinned = state.public_address.pinned();
    RequestOrigin::resolve(Inputs {
        headers: &parts.headers,
        authority: parts.uri.authority().map(axum::http::uri::Authority::as_str),
        peer,
        trusted: &trusted,
        pinned: pinned.as_deref(),
    })
}

impl FromRequestParts<AppState> for RequestOrigin {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        if let Some(o) = parts.extensions.get::<Self>() {
            return Ok(o.clone());
        }
        let o = resolve_parts(parts, state).map_err(|e| ApiError::bad_request(e.to_string()))?;
        parts.extensions.insert(o.clone());
        Ok(o)
    }
}

/// The same-origin guard for WebSockets, tunnels and `/mcp`: no `Origin`
/// (a CLI or server-to-server client) passes. A browser's must equal the
/// request's own origin on an allowed host, or be the pinned address
/// (kept for proxies that are not in `trusted_proxies`).
#[must_use]
pub fn origin_header_ok(state: &AppState, request: &RequestOrigin, headers: &HeaderMap) -> bool {
    let Some(v) = headers.get(header::ORIGIN) else { return true };
    let Ok(o) = v.to_str() else { return false };
    let o = o.trim().trim_end_matches('/');
    (o.eq_ignore_ascii_case(&request.origin()) && state.allowed_hosts.allows(request.host()))
        || crate::public_address::origin_allowed(&state.public_address, o)
}

/// Middleware for every route except `/api/health`: refuses a request
/// whose host is malformed (400) or not allowed (`421 Misdirected
/// Request`, no detail), and adds HSTS to HTTPS responses only.
pub async fn guard(
    axum::extract::State(state): axum::extract::State<AppState>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if req.uri().path() == HEALTH_PATH {
        return next.run(req).await;
    }
    let (mut parts, body) = req.into_parts();
    let origin = match resolve_parts(&parts, &state) {
        Ok(o) => o,
        Err(e) => return ApiError::bad_request(e.to_string()).into_response(),
    };
    if !state.allowed_hosts.allows(origin.host()) {
        return axum::http::StatusCode::MISDIRECTED_REQUEST.into_response();
    }
    let https = origin.is_https();
    parts.extensions.insert(origin);
    let mut resp = next.run(axum::extract::Request::from_parts(parts, body)).await;
    if https && !resp.headers().contains_key(header::STRICT_TRANSPORT_SECURITY) {
        resp.headers_mut()
            .insert(header::STRICT_TRANSPORT_SECURITY, axum::http::HeaderValue::from_static("max-age=31536000"));
    }
    resp
}

/// The liveness probe answers on any host.
pub const HEALTH_PATH: &str = "/api/health";

#[cfg(test)]
#[allow(clippy::unwrap_used)] // tests
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.append(axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(), v.parse().unwrap());
        }
        h
    }

    fn resolve(
        h: &HeaderMap,
        peer: &str,
        trusted: &[&str],
        pinned: Option<&str>,
    ) -> Result<RequestOrigin, OriginError> {
        let trusted: Vec<TrustedNet> = trusted.iter().map(|t| t.parse().unwrap()).collect();
        RequestOrigin::resolve(Inputs {
            headers: h,
            authority: None,
            peer: Some(peer.parse().unwrap()),
            trusted: &trusted,
            pinned,
        })
    }

    #[test]
    fn direct_requests_use_the_host_header() {
        let o = resolve(&headers(&[("host", "192.0.2.5:3880")]), "198.51.100.1:5000", &[], None).unwrap();
        assert_eq!(o.origin(), "http://192.0.2.5:3880");
        assert_eq!(o.agent_url(3843), "https://192.0.2.5:3843");
        let o = resolve(&headers(&[("host", "Forge.Example.com")]), "198.51.100.1:5000", &[], None).unwrap();
        assert_eq!(o.origin(), "http://forge.example.com");
        let o = resolve(&headers(&[("host", "[2001:DB8::1]:3880")]), "198.51.100.1:5000", &[], None).unwrap();
        assert_eq!(o.origin(), "http://[2001:db8::1]:3880");
        assert_eq!(o.agent_url(3843), "https://[2001:db8::1]:3843");
        let o = resolve(&headers(&[("host", "x.example:80")]), "198.51.100.1:5000", &[], None).unwrap();
        assert_eq!(o.origin(), "http://x.example", "default port dropped");
    }

    #[test]
    fn the_pinned_address_lends_its_scheme_to_its_own_host() {
        let pinned = Some("https://forge.example.com");
        let h = headers(&[("host", "forge.example.com")]);
        assert_eq!(resolve(&h, "127.0.0.1:5000", &[], pinned).unwrap().origin(), "https://forge.example.com");
        let h = headers(&[("host", "forge.example.com:3880")]);
        assert_eq!(resolve(&h, "127.0.0.1:5000", &[], pinned).unwrap().origin(), "http://forge.example.com:3880");
        let h = headers(&[("host", "other.example.com")]);
        assert_eq!(resolve(&h, "127.0.0.1:5000", &[], pinned).unwrap().origin(), "http://other.example.com");
    }

    #[test]
    fn trusted_proxies_set_host_and_scheme_with_the_last_value() {
        let h = headers(&[
            ("host", "127.0.0.1:3880"),
            ("x-forwarded-host", "evil.test, forge.example.com"),
            ("x-forwarded-proto", "http"),
            ("x-forwarded-proto", "https"),
        ]);
        let o = resolve(&h, "10.0.0.2:5000", &["10.0.0.0/8"], None).unwrap();
        assert_eq!(o.origin(), "https://forge.example.com");
        assert!(o.is_https());
        let h = headers(&[("host", "forge.example.com"), ("x-forwarded-proto", "https")]);
        assert_eq!(resolve(&h, "10.0.0.2:5000", &["10.0.0.0/8"], None).unwrap().origin(), "https://forge.example.com");
    }

    #[test]
    fn forwarded_headers_from_untrusted_peers_are_ignored() {
        let h =
            headers(&[("host", "192.0.2.5:3880"), ("x-forwarded-host", "evil.test"), ("x-forwarded-proto", "https")]);
        let o = resolve(&h, "198.51.100.1:5000", &["10.0.0.0/8"], None).unwrap();
        assert_eq!(o.origin(), "http://192.0.2.5:3880");
        let o = resolve(&h, "198.51.100.1:5000", &[], None).unwrap();
        assert_eq!(o.origin(), "http://192.0.2.5:3880");
        // A garbage proto from an untrusted peer is ignored too.
        let h = headers(&[("host", "x.example"), ("x-forwarded-proto", "gopher")]);
        assert!(resolve(&h, "198.51.100.1:5000", &[], None).is_ok());
    }

    #[test]
    fn bad_hosts_and_protos_are_refused() {
        for bad in [
            "user@forge.example.com",
            "forge.example.com:0",
            "forge.example.com:99999",
            "forge.example.com:80a",
            "forge.example.com:",
            "forge example.com",
            "2001:db8::1",
            "[2001:db8::1",
            "[2001:db8::1]x",
            "a..b",
            "-x.example",
            "x.example/path",
        ] {
            assert!(resolve(&headers(&[("host", bad)]), "198.51.100.1:5000", &[], None).is_err(), "{bad}");
        }
        let long = format!("{}.example", "a".repeat(250));
        assert!(resolve(&headers(&[("host", &long)]), "198.51.100.1:5000", &[], None).is_err());
        let h = headers(&[("host", "x.example"), ("x-forwarded-proto", "ftp")]);
        assert_eq!(resolve(&h, "10.0.0.2:5000", &["10.0.0.0/8"], None), Err(OriginError::Proto));
        let h = headers(&[("host", "x.example"), ("x-forwarded-host", "bad host")]);
        assert!(resolve(&h, "10.0.0.2:5000", &["10.0.0.0/8"], None).is_err());
    }

    #[test]
    fn node_urls_follow_the_request_unless_it_is_the_pinned_address() {
        let mut c = crate::config::ServerConfig {
            agent_public_url: "https://forge.example.com:7443".into(),
            ..Default::default()
        };
        let o = resolve(&headers(&[("host", "192.0.2.5:3880")]), "198.51.100.1:1", &[], None).unwrap();
        let u = NodeUrls::for_request(&c, Some("https://forge.example.com"), &o);
        assert_eq!(u.forge_url, "http://192.0.2.5:3880");
        assert_eq!(u.agent_url, "https://192.0.2.5:3843");
        let o = RequestOrigin::from_origin("https://forge.example.com").unwrap();
        let u = NodeUrls::for_request(&c, Some("https://forge.example.com"), &o);
        assert_eq!(u.agent_url, "https://forge.example.com:7443", "existing installs keep their agent URL");
        c.node_public_url = Some("http://10.0.0.1:3880/".into());
        assert_eq!(NodeUrls::for_request(&c, None, &o).forge_url, "http://10.0.0.1:3880");
    }

    #[test]
    fn without_a_host_the_pinned_address_or_localhost_is_used() {
        let h = HeaderMap::new();
        assert_eq!(resolve(&h, "127.0.0.1:1", &[], None).unwrap().origin(), "http://localhost");
        assert_eq!(
            resolve(&h, "127.0.0.1:1", &[], Some("https://forge.example.com")).unwrap().origin(),
            "https://forge.example.com"
        );
    }
}
