//! Allowed hosts (D-167): the names and addresses the console answers on.
//!
//! The runtime setting [`SETTING_KEY`] holds a list of patterns, default
//! `["*"]` (any host). A request whose host matches none of them is refused
//! with `421 Misdirected Request`. Patterns:
//!
//! * `*`: any host;
//! * an exact host name (`forge.example.com`) or IP address (`192.0.2.1`,
//!   `[2001:db8::1]` or `2001:db8::1`);
//! * `*.example.com`: one or more labels under `example.com` (not
//!   `example.com` itself, and never `example.com.evil.test`).
//!
//! Ports are not part of a pattern: a host is allowed on every port.

use std::net::{IpAddr, Ipv6Addr};
use std::sync::{Arc, PoisonError, RwLock};

/// Settings key of the allowed hosts (a JSON array of patterns).
pub const SETTING_KEY: &str = "server.allowed_hosts";

/// Most patterns in the list.
pub const MAX_PATTERNS: usize = 64;

/// Longest host name (RFC 1035).
pub const MAX_HOST_LEN: usize = 253;

/// Why a host or pattern was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HostError {
    #[error("empty host")]
    Empty,
    #[error("longer than {MAX_HOST_LEN} characters")]
    TooLong,
    #[error("`{0}` is not a host name or IP address")]
    Invalid(String),
    #[error("a wildcard must be `*` or `*.<domain>` with at least two labels after it")]
    Wildcard,
    #[error("at most {MAX_PATTERNS} patterns")]
    TooMany,
    #[error("the list is empty (use `*` to allow any host)")]
    EmptyList,
}

/// Checks a host (no port) and returns its canonical form: lowercase, an
/// IPv6 address in brackets. Host names are letters, digits, `-` and `_`
/// in dot-separated labels of at most 63 characters.
pub fn normalize_host(input: &str) -> Result<String, HostError> {
    let s = input.trim();
    if s.is_empty() {
        return Err(HostError::Empty);
    }
    if s.len() > MAX_HOST_LEN {
        return Err(HostError::TooLong);
    }
    let invalid = || HostError::Invalid(s.chars().take(80).collect());
    if let Some(inner) = s.strip_prefix('[') {
        let inner = inner.strip_suffix(']').ok_or_else(invalid)?;
        let ip: Ipv6Addr = inner.parse().map_err(|_| invalid())?;
        return Ok(format!("[{ip}]"));
    }
    if s.contains(':') {
        let ip: Ipv6Addr = s.parse().map_err(|_| invalid())?;
        return Ok(format!("[{ip}]"));
    }
    if let Ok(ip) = s.parse::<IpAddr>() {
        return Ok(ip.to_string());
    }
    let lower = s.to_ascii_lowercase();
    let label_ok = |l: &str| {
        !l.is_empty()
            && l.len() <= 63
            && !l.starts_with('-')
            && !l.ends_with('-')
            && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    };
    if !lower.split('.').all(label_ok) {
        return Err(invalid());
    }
    // An all-digit dotted name that is not an IPv4 address (`1.2.3`,
    // `999.1.1.1`) is refused: resolvers read such names inconsistently.
    if lower.split('.').next_back().is_some_and(|l| l.bytes().all(|b| b.is_ascii_digit())) {
        return Err(invalid());
    }
    Ok(lower)
}

/// One allowed-host pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostPattern {
    /// `*`: any host.
    Any,
    /// One host (canonical form).
    Exact(String),
    /// `*.<suffix>`: hosts with one or more labels before `.<suffix>`.
    Subdomains(String),
}

impl HostPattern {
    /// Parses and validates a pattern.
    pub fn parse(input: &str) -> Result<Self, HostError> {
        let s = input.trim();
        if s == "*" {
            return Ok(Self::Any);
        }
        if let Some(suffix) = s.strip_prefix("*.") {
            let suffix = normalize_host(suffix).map_err(|_| HostError::Wildcard)?;
            // Two labels at least (`*.com` would match a whole TLD), and
            // never under an IP address.
            if suffix.starts_with('[') || suffix.parse::<IpAddr>().is_ok() || !suffix.contains('.') {
                return Err(HostError::Wildcard);
            }
            return Ok(Self::Subdomains(suffix));
        }
        if s.contains('*') {
            return Err(HostError::Wildcard);
        }
        normalize_host(s).map(Self::Exact)
    }

    /// Whether `host` (canonical form, from [`normalize_host`]) matches.
    #[must_use]
    pub fn matches(&self, host: &str) -> bool {
        match self {
            Self::Any => true,
            Self::Exact(h) => h == host,
            Self::Subdomains(suffix) => host
                .strip_suffix(suffix.as_str())
                .and_then(|head| head.strip_suffix('.'))
                .is_some_and(|head| !head.is_empty() && !host.starts_with('[')),
        }
    }
}

impl std::fmt::Display for HostPattern {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Any => f.write_str("*"),
            Self::Exact(h) => f.write_str(h),
            Self::Subdomains(s) => write!(f, "*.{s}"),
        }
    }
}

/// Parses a whole list: not empty, at most [`MAX_PATTERNS`], duplicates
/// dropped.
pub fn parse_list<S: AsRef<str>>(items: &[S]) -> Result<Vec<HostPattern>, HostError> {
    if items.is_empty() {
        return Err(HostError::EmptyList);
    }
    if items.len() > MAX_PATTERNS {
        return Err(HostError::TooMany);
    }
    let mut out: Vec<HostPattern> = Vec::with_capacity(items.len());
    for i in items {
        let p = HostPattern::parse(i.as_ref())?;
        if !out.contains(&p) {
            out.push(p);
        }
    }
    Ok(out)
}

/// Shared, runtime-editable allowed hosts (default: any host).
#[derive(Debug, Clone)]
pub struct AllowedHosts {
    patterns: Arc<RwLock<Vec<HostPattern>>>,
}

impl Default for AllowedHosts {
    fn default() -> Self {
        Self { patterns: Arc::new(RwLock::new(vec![HostPattern::Any])) }
    }
}

impl AllowedHosts {
    /// Whether `host` (canonical form) is allowed.
    #[must_use]
    pub fn allows(&self, host: &str) -> bool {
        self.patterns.read().unwrap_or_else(PoisonError::into_inner).iter().any(|p| p.matches(host))
    }

    /// The patterns, as text.
    #[must_use]
    pub fn list(&self) -> Vec<String> {
        self.patterns.read().unwrap_or_else(PoisonError::into_inner).iter().map(ToString::to_string).collect()
    }

    /// Whether any host is allowed (`*` in the list).
    #[must_use]
    pub fn any(&self) -> bool {
        self.patterns.read().unwrap_or_else(PoisonError::into_inner).contains(&HostPattern::Any)
    }

    /// Replaces the patterns (already validated by [`parse_list`]); `None`
    /// goes back to the default (any host).
    pub fn set(&self, patterns: Option<Vec<HostPattern>>) {
        *self.patterns.write().unwrap_or_else(PoisonError::into_inner) =
            patterns.filter(|p| !p.is_empty()).unwrap_or_else(|| vec![HostPattern::Any]);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // tests
mod tests {
    use super::*;

    fn allowed(patterns: &[&str], host: &str) -> bool {
        let h = normalize_host(host).unwrap();
        parse_list(patterns).unwrap().iter().any(|p| p.matches(&h))
    }

    #[test]
    fn hosts_are_normalized_or_refused() {
        assert_eq!(normalize_host("Forge.Example.COM").unwrap(), "forge.example.com");
        assert_eq!(normalize_host("192.0.2.1").unwrap(), "192.0.2.1");
        assert_eq!(normalize_host("[2001:DB8::1]").unwrap(), "[2001:db8::1]");
        assert_eq!(normalize_host("2001:db8::1").unwrap(), "[2001:db8::1]");
        assert_eq!(normalize_host("forge_server").unwrap(), "forge_server");
        for bad in [
            "",
            "a b",
            "user@host",
            "host/path",
            "-lead.example.com",
            "trail-.example.com",
            "a..b",
            ".example.com",
            "example.com.",
            "[::1",
            "[nope]",
            "1.2.3",
            "999.1.1.1",
            "h\u{e9}llo",
            "a\tb",
            "host%00",
        ] {
            assert!(normalize_host(bad).is_err(), "{bad:?} must be refused");
        }
        assert_eq!(normalize_host(&"a".repeat(254)), Err(HostError::TooLong));
        assert!(normalize_host(&format!("{}.com", "a".repeat(64))).is_err(), "label over 63");
    }

    #[test]
    fn patterns_are_validated() {
        assert_eq!(HostPattern::parse("*").unwrap(), HostPattern::Any);
        assert_eq!(HostPattern::parse("*.Example.com").unwrap(), HostPattern::Subdomains("example.com".into()));
        for bad in ["*.com", "*example.com", "foo.*.com", "**", "*.", "*.192.0.2.1", "*.[::1]", "a*b", "", "a b"] {
            assert!(HostPattern::parse(bad).is_err(), "{bad:?} must be refused");
        }
        assert_eq!(parse_list::<&str>(&[]), Err(HostError::EmptyList));
        let many: Vec<String> = (0..=MAX_PATTERNS).map(|i| format!("h{i}.example.com")).collect();
        assert_eq!(parse_list(&many), Err(HostError::TooMany));
        assert_eq!(parse_list(&["a.example.com", "A.example.com"]).unwrap().len(), 1, "duplicates dropped");
    }

    /// Security review: the tricky cases of the matcher, as a table.
    #[test]
    fn matcher_table() {
        let cases: &[(&[&str], &str, bool)] = &[
            (&["*"], "anything.example", true),
            (&["*"], "192.0.2.1", true),
            (&["forge.example.com"], "forge.example.com", true),
            (&["forge.example.com"], "FORGE.example.com", true),
            (&["forge.example.com"], "forge.example.com.evil.test", false),
            (&["forge.example.com"], "evilforge.example.com", false),
            (&["forge.example.com"], "x.forge.example.com", false),
            (&["*.example.com"], "forge.example.com", true),
            (&["*.example.com"], "a.b.example.com", true),
            (&["*.example.com"], "example.com", false),
            (&["*.example.com"], "example.com.evil.test", false),
            (&["*.example.com"], "evilexample.com", false),
            (&["*.example.com"], "forge.example.com.evil.test", false),
            (&["*.example.com"], "192.0.2.1", false),
            (&["192.0.2.1"], "192.0.2.1", true),
            (&["192.0.2.1"], "192.0.2.10", false),
            (&["[2001:db8::1]"], "2001:db8:0::1", true),
            (&["2001:db8::1"], "[2001:db8::1]", true),
            (&["2001:db8::1"], "[2001:db8::2]", false),
            (&["forge.example.com", "192.0.2.1"], "192.0.2.1", true),
            (&["forge.example.com", "192.0.2.1"], "198.51.100.1", false),
        ];
        for (patterns, host, want) in cases {
            assert_eq!(allowed(patterns, host), *want, "{patterns:?} vs {host}");
        }
    }

    /// Property-style: a wildcard never matches a host that merely ends in
    /// the suffix without a dot boundary, nor one where the suffix appears
    /// in the middle.
    #[test]
    fn wildcard_needs_a_label_boundary() {
        let p = HostPattern::parse("*.example.com").unwrap();
        for prefix in ["a", "ab", "a-b", "x.y", "1", "_"] {
            let h = normalize_host(&format!("{prefix}.example.com")).unwrap();
            assert!(p.matches(&h), "{h}");
            let glued = normalize_host(&format!("{prefix}example.com")).unwrap();
            assert!(!p.matches(&glued), "{glued}");
            for tail in ["evil.test", "example.com.evil.test", "com"] {
                let h = normalize_host(&format!("{prefix}.example.com.{tail}")).unwrap();
                assert!(!p.matches(&h), "{h}");
            }
        }
    }

    #[test]
    fn shared_list_defaults_to_any_and_resets() {
        let a = AllowedHosts::default();
        assert!(a.any() && a.allows("x.example"));
        let clone = a.clone();
        clone.set(Some(parse_list(&["forge.example.com"]).unwrap()));
        assert!(!a.allows("x.example") && a.allows("forge.example.com"));
        assert_eq!(a.list(), vec!["forge.example.com".to_owned()]);
        a.set(None);
        assert!(a.any());
    }
}
