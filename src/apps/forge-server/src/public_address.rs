//! The console's public address (D-120): a runtime setting that overrides
//! the configured `public_url` for every link Forge sends to people, and the
//! set of browser origins the WebSocket and MCP Origin guards accept.
//!
//! The value is an origin only (`scheme://host[:port]`): Forge is served at
//! the root of its address, so a path would produce broken links.

use axum::http::HeaderMap;
use forge_core::ports::SettingsRepository;
use forge_core::public_address::{LAST_ADMIN_KEY, PublicAddress, SETTING_KEY};
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

/// Reads the runtime setting and the last administrator address at start.
/// A stored value that no longer validates is ignored with a warning.
/// `configured` is `public_url` from `forge.toml` (empty when not set).
pub async fn load(settings: &dyn SettingsRepository, configured: &str) -> anyhow::Result<PublicAddress> {
    let addr = PublicAddress::new(configured);
    let all = settings.all_settings().await?;
    let stored =
        |key: &str| all.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone()).filter(|v| !v.trim().is_empty());
    if let Some(v) = stored(SETTING_KEY) {
        match validate(&v) {
            Ok(v) => addr.set_runtime(Some(v)),
            Err(e) => tracing::warn!(error = %e, "ignoring the stored public address; links use public_url"),
        }
    }
    if let Some(v) = stored(LAST_ADMIN_KEY) {
        match validate(&v) {
            Ok(v) => addr.set_last_admin(Some(v)),
            Err(e) => tracing::warn!(error = %e, "ignoring the stored last administrator address"),
        }
    }
    Ok(addr)
}

/// Audit action: the last administrator address changed (D-167).
pub const AUDIT_LAST_ADMIN_UPDATE: &str = "server.last_admin_address.update";

/// This machine's own names and addresses: what a request host must be
/// (besides configured names) before it may become the last administrator
/// address.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocalNames {
    /// Host names (lowercase).
    pub hostnames: Vec<String>,
    /// Interface addresses.
    pub ips: Vec<std::net::IpAddr>,
}

impl LocalNames {
    /// Reads the host name and interface addresses. Blocking (system
    /// calls): run it in `spawn_blocking`.
    #[must_use]
    pub fn read() -> Self {
        let hostnames = sysinfo::System::host_name()
            .map(|h| h.trim().trim_end_matches('.').to_ascii_lowercase())
            .filter(|h| !h.is_empty())
            .into_iter()
            .collect();
        let nets = sysinfo::Networks::new_with_refreshed_list();
        let ips = nets.values().flat_map(|d| d.ip_networks().iter().map(|n| n.addr)).collect();
        Self { hostnames, ips }
    }
}

/// The host (no scheme or port) of a URL or a bare name, lowercase.
fn host_of(s: &str) -> Option<String> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let host = match url::Url::parse(s) {
        Ok(u) if u.host_str().is_some() => u.host_str().map(str::to_owned),
        _ => Some(s.to_owned()),
    }?;
    forge_core::allowed_hosts::normalize_host(&host).ok()
}

/// Whether `host` (canonical form, as `RequestOrigin::host`) may be recorded
/// as the last administrator address: an address of this machine, its host
/// name, a configured name (`configured`: URLs or names), or a match in an
/// explicit allowed-hosts list (not `*`). A `Host` header is
/// client-controlled, and the recorded value feeds emails, webhooks and
/// node downloads, so a name nobody configured is never trusted. Loopback
/// and `localhost` are never recorded, even when configured: no other
/// machine reaches Forge there (signing in on them still works).
#[must_use]
pub fn host_trusted(
    host: &str,
    local: &LocalNames,
    configured: &[&str],
    allowed: &forge_core::allowed_hosts::AllowedHosts,
) -> bool {
    let Ok(host) = forge_core::allowed_hosts::normalize_host(host) else { return false };
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = bare.parse::<std::net::IpAddr>() {
        if ip.is_loopback() || ip.is_unspecified() {
            return false;
        }
        if local.ips.contains(&ip) {
            return true;
        }
    } else if host == "localhost" || host.ends_with(".localhost") {
        return false;
    } else if local.hostnames.contains(&host) {
        return true;
    }
    if configured.iter().filter_map(|c| host_of(c)).any(|c| c == host) {
        return true;
    }
    !allowed.any() && allowed.allows(&host)
}

/// A change of the last administrator address, for the audit log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddressChange {
    pub from: Option<String>,
    pub to: String,
}

/// Records `origin` as the address an administrator last signed in on,
/// for links built without a request (D-167). The caller has checked the
/// host with [`host_trusted`]. Writes only on a change, which it returns
/// for the audit log. Store first, then memory.
pub async fn record_last_admin(
    settings: &dyn SettingsRepository,
    addr: &PublicAddress,
    origin: &str,
    now: Timestamp,
) -> DomainResult<Option<AddressChange>> {
    let v = validate(origin).map_err(|e| DomainError::Invalid(e.to_string()))?;
    let from = addr.last_admin();
    if from.as_deref() == Some(v.as_str()) {
        return Ok(None);
    }
    settings.put_setting(LAST_ADMIN_KEY, &v, now).await?;
    addr.set_last_admin(Some(v.clone()));
    Ok(Some(AddressChange { from, to: v }))
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

/// Browser origins always allowed on WebSockets and `/mcp`, besides the
/// request's own origin (see `crate::request_origin::origin_header_ok`):
/// the configured `public_url` and the runtime public address.
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
    }

    #[test]
    fn only_local_or_configured_hosts_are_trusted() {
        use forge_core::allowed_hosts::{AllowedHosts, parse_list};
        let local = LocalNames { hostnames: vec!["lab-a".into()], ips: vec!["192.0.2.5".parse().unwrap()] };
        let any = AllowedHosts::default();
        let configured = ["https://forge.example.com", "forge.internal", "", "localhost", "127.0.0.1"];
        for h in ["192.0.2.5", "lab-a", "forge.example.com", "forge.internal"] {
            assert!(host_trusted(h, &local, &configured, &any), "{h}");
        }
        // Loopback is never recorded, even when configured or allowed.
        let loopback_ok = AllowedHosts::default();
        loopback_ok.set(Some(parse_list(&["localhost", "127.0.0.1", "app.localhost"]).unwrap()));
        for h in ["127.0.0.1", "127.0.0.2", "[::1]", "localhost", "app.localhost", "0.0.0.0"] {
            assert!(!host_trusted(h, &local, &configured, &any), "{h}");
            assert!(!host_trusted(h, &local, &configured, &loopback_ok), "{h} allowed explicitly");
        }
        for h in ["evil.test", "192.0.2.99", "lab-a.evil.test", "[2001:db8::1]"] {
            assert!(!host_trusted(h, &local, &configured, &any), "{h} with allowed hosts *");
        }
        let narrow = AllowedHosts::default();
        narrow.set(Some(parse_list(&["*.corp.example"]).unwrap()));
        assert!(host_trusted("forge.corp.example", &local, &[], &narrow));
        assert!(!host_trusted("evil.test", &local, &[], &narrow));
    }

    #[tokio::test]
    async fn last_admin_change_is_reported_once() {
        let dir = tempfile::tempdir().unwrap();
        let store = forge_store::Store::open_sqlite(&dir.path().join("forge.db")).await.unwrap();
        let a = PublicAddress::new("");
        let now = Timestamp(1);
        let c = record_last_admin(&store, &a, "http://192.0.2.5:55130", now).await.unwrap().unwrap();
        assert_eq!(c, AddressChange { from: None, to: "http://192.0.2.5:55130".into() });
        assert!(record_last_admin(&store, &a, "http://192.0.2.5:55130", now).await.unwrap().is_none());
        let c = record_last_admin(&store, &a, "http://lab-a:55130", now).await.unwrap().unwrap();
        assert_eq!(c.from.as_deref(), Some("http://192.0.2.5:55130"));
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
