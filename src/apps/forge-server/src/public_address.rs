//! The console's public address (D-120): a runtime setting that overrides
//! the configured `public_url` for every link Forge sends to people, and the
//! set of browser origins the WebSocket and MCP Origin guards accept.
//!
//! The value is an origin only (`scheme://host[:port]`): Forge is served at
//! the root of its address, so a path would produce broken links.

use axum::http::HeaderMap;
use forge_core::ports::SettingsRepository;
use forge_core::public_address::{PublicAddress, SETTING_KEY};
use forge_core::{DomainError, DomainResult, Timestamp};

use crate::throttle::TrustedNet;

/// Why a public address was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AddressError {
    #[error("not an absolute URL (use https://forge.example.com)")]
    NotAUrl,
    #[error("the scheme must be http or https")]
    Scheme,
    #[error("the URL has no host")]
    NoHost,
    #[error("the URL must not contain a user name or password")]
    UserInfo,
    #[error("use the origin only (scheme, host and port): no path, query or fragment")]
    NotOrigin,
    #[error("at most 255 characters")]
    TooLong,
}

/// Checks a public address and returns its canonical form
/// (`https://forge.example.com`, lowercase host, default port dropped).
pub fn validate(input: &str) -> Result<String, AddressError> {
    let s = input.trim();
    if s.len() > 255 {
        return Err(AddressError::TooLong);
    }
    let url = url::Url::parse(s).map_err(|_| AddressError::NotAUrl)?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(AddressError::Scheme);
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err(AddressError::NoHost);
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(AddressError::UserInfo);
    }
    if url.path() != "/" || url.query().is_some() || url.fragment().is_some() {
        return Err(AddressError::NotOrigin);
    }
    Ok(url.origin().ascii_serialization())
}

/// Reads the runtime setting at start. A stored value that no longer
/// validates is ignored with a warning (links fall back to `public_url`).
pub async fn load(settings: &dyn SettingsRepository, configured: &str) -> anyhow::Result<PublicAddress> {
    let addr = PublicAddress::new(configured);
    let stored = settings.all_settings().await?.into_iter().find(|(k, _)| k == SETTING_KEY).map(|(_, v)| v);
    if let Some(v) = stored.filter(|v| !v.trim().is_empty()) {
        match validate(&v) {
            Ok(v) => addr.set_runtime(Some(v)),
            Err(e) => tracing::warn!(error = %e, "ignoring the stored public address; links use public_url"),
        }
    }
    Ok(addr)
}

/// Stores (or with `None` clears) the runtime setting and applies it.
/// Store first, then memory: a failed write leaves both unchanged.
pub async fn save(
    settings: &dyn SettingsRepository,
    addr: &PublicAddress,
    value: Option<&str>,
    now: Timestamp,
) -> DomainResult<()> {
    match value {
        Some(v) => {
            let v = validate(v).map_err(|e| DomainError::Invalid(e.to_string()))?;
            settings.put_setting(SETTING_KEY, &v, now).await?;
            addr.set_runtime(Some(v));
        }
        None => {
            settings.delete_setting(SETTING_KEY).await?;
            addr.set_runtime(None);
        }
    }
    Ok(())
}

fn origin_of(url: &str) -> Option<String> {
    url::Url::parse(url).ok().map(|u| u.origin().ascii_serialization()).filter(|o| o != "null")
}

/// Browser origins allowed on WebSockets and `/mcp`: the configured
/// `public_url` and the runtime public address. Never derived from the
/// request's `Host` header.
#[must_use]
pub fn allowed_origins(addr: &PublicAddress) -> Vec<String> {
    let mut v: Vec<String> = [Some(addr.configured().to_owned()), addr.runtime()]
        .into_iter()
        .flatten()
        .filter_map(|u| origin_of(&u))
        .collect();
    v.dedup();
    v
}

/// Whether `origin` (an `Origin` header value) is one of [`allowed_origins`].
#[must_use]
pub fn origin_allowed(addr: &PublicAddress, origin: &str) -> bool {
    let o = origin.trim().trim_end_matches('/');
    allowed_origins(addr).iter().any(|a| a.eq_ignore_ascii_case(o))
}

/// The Origin guard for browser-facing sockets: no header (a CLI or
/// server-to-server client) passes; a browser's must be allowed.
#[must_use]
pub fn origin_header_ok(addr: &PublicAddress, headers: &HeaderMap) -> bool {
    match headers.get(axum::http::header::ORIGIN) {
        None => true,
        Some(v) => v.to_str().is_ok_and(|o| origin_allowed(addr, o)),
    }
}

/// Whether session cookies get `Secure`: always with `cookie_secure`, and
/// otherwise when a trusted proxy says the browser used HTTPS
/// (`X-Forwarded-Proto: https`). The header from any other peer is ignored.
#[must_use]
pub fn cookies_secure(
    cookie_secure: bool,
    headers: &HeaderMap,
    peer: std::net::SocketAddr,
    trusted: &[TrustedNet],
) -> bool {
    cookie_secure || crate::throttle::forwarded_https(headers, peer, trusted)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // tests
mod tests {
    use super::*;

    #[test]
    fn validation_keeps_origins_and_refuses_the_rest() {
        assert_eq!(validate("https://Forge.Example.com/").unwrap(), "https://forge.example.com");
        assert_eq!(validate(" https://forge.example.com:443 ").unwrap(), "https://forge.example.com");
        assert_eq!(validate("http://192.0.2.1:55130").unwrap(), "http://192.0.2.1:55130");
        assert_eq!(validate("forge.example.com"), Err(AddressError::NotAUrl));
        assert_eq!(validate("ftp://forge.example.com"), Err(AddressError::Scheme));
        assert_eq!(validate("javascript:alert(1)"), Err(AddressError::Scheme));
        assert_eq!(validate("https://u:p@forge.example.com"), Err(AddressError::UserInfo));
        assert_eq!(validate("https://forge.example.com/forge"), Err(AddressError::NotOrigin));
        assert_eq!(validate("https://forge.example.com/?a=1"), Err(AddressError::NotOrigin));
        assert_eq!(validate("https://forge.example.com/#x"), Err(AddressError::NotOrigin));
        assert_eq!(validate(&format!("https://{}.com", "a".repeat(260))), Err(AddressError::TooLong));
    }

    #[test]
    fn origin_allow_list_has_both_addresses_and_nothing_else() {
        let a = PublicAddress::new("http://192.0.2.1:55130");
        assert!(origin_allowed(&a, "http://192.0.2.1:55130"));
        assert!(!origin_allowed(&a, "https://forge.example.com"));
        a.set_runtime(Some("https://forge.example.com".into()));
        assert!(origin_allowed(&a, "https://forge.example.com"));
        assert!(origin_allowed(&a, "https://FORGE.example.com/"));
        assert!(origin_allowed(&a, "http://192.0.2.1:55130"), "the direct address keeps working");
        assert!(!origin_allowed(&a, "http://forge.example.com"), "scheme is part of the origin");
        assert!(!origin_allowed(&a, "https://forge.example.com.evil.test"));
        assert!(!origin_allowed(&a, "null"));
        assert!(!origin_allowed(&a, ""));
        let mut h = HeaderMap::new();
        assert!(origin_header_ok(&a, &h));
        h.insert(axum::http::header::ORIGIN, "https://evil.test".parse().unwrap());
        assert!(!origin_header_ok(&a, &h));
    }

    #[test]
    fn secure_cookies_trust_forwarded_proto_only_from_trusted_proxies() {
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-proto", "https".parse().unwrap());
        let edge: std::net::SocketAddr = "192.0.2.10:40000".parse().unwrap();
        let other: std::net::SocketAddr = "192.0.2.99:40000".parse().unwrap();
        let trusted: Vec<TrustedNet> = vec!["192.0.2.10".parse().unwrap()];
        assert!(cookies_secure(false, &h, edge, &trusted));
        assert!(!cookies_secure(false, &h, other, &trusted), "untrusted peer's header is ignored");
        assert!(!cookies_secure(false, &h, edge, &[]), "no trusted proxies: header ignored");
        assert!(cookies_secure(true, &HeaderMap::new(), other, &[]), "cookie_secure always wins");
        h.insert("x-forwarded-proto", "http".parse().unwrap());
        assert!(!cookies_secure(false, &h, edge, &trusted));
    }
}
