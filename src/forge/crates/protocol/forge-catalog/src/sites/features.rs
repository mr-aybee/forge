//! Typed endpoint features (D-169): path rules, cache, compression, access
//! control, redirects and per-endpoint logs. Each part validates its own
//! input so the renderer only ever writes checked values into the file.

use serde::{Deserialize, Serialize};

use super::Balance;
use crate::ingress::Target;

/// Directory of the managed `htpasswd` files (one per endpoint).
pub const HTPASSWD_DIR: &str = "/etc/nginx/forge/htpasswd";
/// Directory of the per-endpoint proxy caches.
pub const CACHE_DIR: &str = "/var/cache/nginx/forge";
/// Directory of the per-endpoint access and error logs.
pub const LOG_DIR: &str = "/var/log/nginx/forge";

const fn yes() -> bool {
    true
}

/// What a path rule (or the endpoint's default `/` rule) does with a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RuleKind {
    /// Reverse proxy to one target (a service may still have several replicas).
    #[default]
    Proxy,
    /// Several targets with weights, standbys and a balancing method.
    Balancer,
    /// An HTTP redirect to another URL.
    Redirect,
    /// Files from a directory on the edge (inside an approved file root).
    Static,
}

impl RuleKind {
    /// Whether this rule proxies to upstreams.
    #[must_use]
    pub fn is_proxy(&self) -> bool {
        *self == Self::Proxy
    }
    /// Whether requests go to upstreams.
    #[must_use]
    pub fn proxies(self) -> bool {
        matches!(self, Self::Proxy | Self::Balancer)
    }
}

/// How a rule's path matches the request URI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MatchKind {
    #[default]
    Prefix,
    Exact,
    /// A case-sensitive PCRE regex (`location ~`).
    Regex,
}

/// Per-rule proxy options.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProxyOptions {
    /// Send the client's `Host` (else the upstream's address).
    #[serde(default = "yes")]
    pub preserve_host: bool,
    /// Remove the matched prefix before proxying (prefix rules ending in `/`).
    #[serde(default)]
    pub strip_prefix: bool,
    #[serde(default = "read_timeout")]
    pub read_timeout_s: u32,
    #[serde(default = "connect_timeout")]
    pub connect_timeout_s: u32,
    /// Response buffering; turn off for streaming responses (SSE).
    #[serde(default = "yes")]
    pub buffering: bool,
}

const fn read_timeout() -> u32 {
    300
}
const fn connect_timeout() -> u32 {
    5
}

impl Default for ProxyOptions {
    fn default() -> Self {
        Self {
            preserve_host: true,
            strip_prefix: false,
            read_timeout_s: read_timeout(),
            connect_timeout_s: connect_timeout(),
            buffering: true,
        }
    }
}

impl ProxyOptions {
    /// Whether every option has its default value.
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    pub(super) fn validate(&self) -> Result<(), String> {
        if !(1..=3600).contains(&self.read_timeout_s) || !(1..=300).contains(&self.connect_timeout_s) {
            return Err("timeouts: read 1–3600 s, connect 1–300 s".into());
        }

        Ok(())
    }
}

/// Where a redirect rule sends the client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RedirectTo {
    /// `https://host[:port][/path]`.
    pub to: String,
    #[serde(default = "moved")]
    pub code: u16,
    /// Append the request's path and query to `to`.
    #[serde(default = "yes")]
    pub keep_path: bool,
}

const fn moved() -> u16 {
    301
}

impl RedirectTo {
    fn validate(&self) -> Result<(), String> {
        valid_code(self.code)?;
        valid_url(&self.to)
    }

    /// The `return` target: with `keep_path`, the request URI (which starts
    /// with `/`) follows the URL without a doubled slash.
    fn target(&self, uri_var: &str) -> String {
        if self.keep_path { format!("{}{uri_var}", self.to.trim_end_matches('/')) } else { self.to.clone() }
    }
}

/// A static site: a directory on the edge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StaticRoot {
    /// Absolute directory; the control plane checks it is inside an approved
    /// file root on every edge.
    pub root: String,
    #[serde(default = "index_html")]
    pub index: String,
    /// Single-page app: unknown paths serve `/index.html`.
    #[serde(default)]
    pub spa: bool,
}

fn index_html() -> String {
    "index.html".into()
}

impl StaticRoot {
    fn validate(&self) -> Result<(), String> {
        valid_dir(&self.root)?;

        if self.index.is_empty()
            || self.index.len() > 64
            || !self.index.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
            || self.index.starts_with('.')
        {
            return Err("index file: a file name such as index.html".into());
        }

        Ok(())
    }
}

/// A path rule: requests whose URI matches go to its targets, a redirect or
/// a static directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteRule {
    /// `/api/` (prefix), `/health` (exact) or `\.php$` (regex).
    pub path: String,
    #[serde(default)]
    pub match_kind: MatchKind,
    #[serde(default)]
    pub kind: RuleKind,
    /// Typed targets, resolved by the control plane into `upstreams`/`standby`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub targets: Vec<Target>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub standby_targets: Vec<Target>,
    #[serde(default)]
    pub upstreams: Vec<String>,
    #[serde(default)]
    pub standby: Vec<String>,
    #[serde(default)]
    pub balance: Balance,
    #[serde(default = "yes")]
    pub websockets: bool,
    #[serde(default, skip_serializing_if = "ProxyOptions::is_default")]
    pub proxy: ProxyOptions,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redirect: Option<RedirectTo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub static_root: Option<StaticRoot>,
}

impl RouteRule {
    pub(super) fn validate(&self) -> Result<(), String> {
        valid_path(&self.path, self.match_kind)?;

        if self.match_kind == MatchKind::Prefix && self.path == "/" {
            return Err("the `/` rule is the endpoint's default route; set its type on the Routing page".into());
        }

        if self.proxy.strip_prefix && (self.match_kind != MatchKind::Prefix || !self.path.ends_with('/')) {
            return Err(format!("strip prefix needs a prefix rule ending in `/` (`{}`)", self.path));
        }

        self.proxy.validate()?;

        check_kind(self.kind, &self.upstreams, &self.standby, self.redirect.as_ref(), self.static_root.as_ref())
            .map_err(|e| format!("rule `{}`: {e}", self.path))
    }

    /// The `location` line opening this rule.
    pub(super) fn location(&self) -> String {
        match self.match_kind {
            MatchKind::Prefix => format!("location {}", self.path),
            MatchKind::Exact => format!("location = {}", self.path),
            MatchKind::Regex => format!("location ~ \"{}\"", self.path),
        }
    }
}

/// Checks what a rule of `kind` needs (shared by path rules and the default rule).
pub(super) fn check_kind(
    kind: RuleKind,
    upstreams: &[String],
    standby: &[String],
    redirect: Option<&RedirectTo>,
    static_root: Option<&StaticRoot>,
) -> Result<(), String> {
    match kind {
        RuleKind::Proxy | RuleKind::Balancer => {
            if upstreams.is_empty() {
                return Err("at least one upstream (host:port) is required".into());
            }

            if let Some(u) = upstreams.iter().chain(standby).find(|u| !super::valid_upstream(u)) {
                return Err(format!("invalid upstream `{u}` (use host:port)"));
            }
        }
        RuleKind::Redirect => redirect.ok_or("a redirect needs a target URL")?.validate()?,
        RuleKind::Static => static_root.ok_or("a static site needs a directory")?.validate()?,
    }

    Ok(())
}

/// Proxy cache (`proxy_cache`) for the endpoint's proxied routes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CacheSpec {
    /// Variables only, such as `$scheme$request_method$host$request_uri`.
    #[serde(default = "cache_key")]
    pub key: String,
    /// File extensions to cache (`css`, `js`); empty caches every response
    /// the upstream allows.
    #[serde(default)]
    pub extensions: Vec<String>,
    /// How long `200`/`301`/`302` responses stay fresh.
    #[serde(default = "cache_ttl")]
    pub ttl_minutes: u32,
    /// Ignore the upstream's `Cache-Control`, `Expires` and `Set-Cookie`.
    #[serde(default)]
    pub ignore_headers: bool,
    #[serde(default = "cache_size")]
    pub max_size_mb: u32,
}

fn cache_key() -> String {
    "$scheme$request_method$host$request_uri".into()
}
const fn cache_ttl() -> u32 {
    60
}
const fn cache_size() -> u32 {
    1024
}

impl CacheSpec {
    fn validate(&self) -> Result<(), String> {
        let k = &self.key;
        if k.is_empty()
            || k.len() > 200
            || !k.starts_with('$')
            || !k.bytes().all(|b| b == b'$' || b == b'_' || b.is_ascii_alphanumeric())
            || k.contains("$$")
        {
            return Err("cache key: nginx variables only, such as $scheme$host$request_uri".into());
        }

        valid_extensions(&self.extensions)?;

        if !(1..=525_600).contains(&self.ttl_minutes) || !(1..=1_048_576).contains(&self.max_size_mb) {
            return Err("cache: TTL 1–525600 minutes, size 1–1048576 MB".into());
        }

        Ok(())
    }
}

/// Gzip compression for this endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GzipSpec {
    #[serde(default = "gzip_level")]
    pub level: u8,
    /// MIME types besides `text/html` (always compressed).
    #[serde(default = "gzip_types")]
    pub types: Vec<String>,
}

const fn gzip_level() -> u8 {
    5
}
fn gzip_types() -> Vec<String> {
    ["text/css", "text/plain", "application/javascript", "application/json", "image/svg+xml"].map(String::from).to_vec()
}

impl GzipSpec {
    fn validate(&self) -> Result<(), String> {
        if !(1..=9).contains(&self.level) {
            return Err("gzip level: 1–9".into());
        }

        let mime = |t: &str| {
            let ok = |s: &str| {
                !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'+' | b'-'))
            };
            t.len() <= 100 && t.split_once('/').is_some_and(|(a, b)| ok(a) && (b == "*" || ok(b)))
        };

        if self.types.len() > 50 {
            return Err("at most 50 gzip types".into());
        }

        match self.types.iter().find(|t| !mime(t)) {
            Some(t) => Err(format!("`{t}` is not a MIME type")),
            None => Ok(()),
        }
    }
}

/// A user of the endpoint's basic authentication. Forge stores only the
/// SHA-512 crypt hash; `password` is accepted on input and never serialized.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BasicAuthUser {
    pub user: String,
    /// `$6$<salt>$<hash>` (crypt SHA-512), set by the control plane.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub hash: String,
    /// A new password (input only).
    #[serde(default, skip_serializing)]
    pub password: Option<String>,
}

impl std::fmt::Debug for BasicAuthUser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BasicAuthUser").field("user", &self.user).finish_non_exhaustive()
    }
}

/// Whether `h` is a crypt SHA-512 hash (`$6$[rounds=N$]salt$hash`).
#[must_use]
pub fn valid_crypt_hash(h: &str) -> bool {
    let b64 = |s: &str| s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'/');
    let Some(rest) = h.strip_prefix("$6$") else { return false };
    let rest = match rest.strip_prefix("rounds=") {
        Some(r) => match r.split_once('$') {
            Some((n, tail)) if n.parse::<u32>().is_ok_and(|n| (1000..=999_999_999).contains(&n)) => tail,
            _ => return false,
        },
        None => rest,
    };

    rest.split_once('$')
        .is_some_and(|(salt, hash)| (1..=16).contains(&salt.len()) && b64(salt) && hash.len() == 86 && b64(hash))
}

/// Basic authentication on an endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BasicAuth {
    #[serde(default = "realm")]
    pub realm: String,
    pub users: Vec<BasicAuthUser>,
}

fn realm() -> String {
    "Restricted".into()
}

/// Hotlink protection: these file types load only from the listed referers
/// (and the endpoint's own domains).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hotlink {
    #[serde(default)]
    pub referers: Vec<String>,
    pub extensions: Vec<String>,
}

/// Who may reach the endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct AccessSpec {
    /// Addresses or CIDRs refused first.
    #[serde(default)]
    pub deny: Vec<String>,
    /// When not empty, only these addresses or CIDRs are served.
    #[serde(default)]
    pub allow: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub basic_auth: Option<BasicAuth>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hotlink: Option<Hotlink>,
}

impl AccessSpec {
    /// Whether every option has its default value.
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Whether requests can be refused before routing (the ACME challenge
    /// location must then opt out).
    #[must_use]
    pub fn restricts(&self) -> bool {
        !self.deny.is_empty() || !self.allow.is_empty() || self.basic_auth.is_some()
    }

    pub(super) fn validate(&self) -> Result<(), String> {
        if self.deny.len() + self.allow.len() > 500 {
            return Err("at most 500 allow and deny entries".into());
        }

        if let Some(a) = self.deny.iter().chain(&self.allow).find(|a| !valid_ip_or_cidr(a)) {
            return Err(format!("`{a}` is not an IP address or CIDR"));
        }

        if let Some(b) = &self.basic_auth {
            if b.realm.is_empty()
                || b.realm.len() > 64
                || !b.realm.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b' ' | b'.' | b'-' | b'_'))
            {
                return Err("basic auth realm: letters, digits, spaces, `.`, `-` and `_`".into());
            }

            if b.users.is_empty() || b.users.len() > 100 {
                return Err("basic auth needs 1–100 users".into());
            }

            let mut seen = std::collections::BTreeSet::new();

            for u in &b.users {
                if u.user.is_empty()
                    || u.user.len() > 64
                    || !u.user.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-' | b'_' | b'@'))
                {
                    return Err(format!("basic auth user `{}`: letters, digits, `.`, `-`, `_` and `@`", u.user));
                }

                if !seen.insert(u.user.as_str()) {
                    return Err(format!("basic auth user `{}` is listed twice", u.user));
                }

                if !valid_crypt_hash(&u.hash) {
                    return Err(format!("basic auth user `{}` has no password", u.user));
                }
            }
        }

        if let Some(h) = &self.hotlink {
            if h.extensions.is_empty() {
                return Err("hotlink protection needs at least one file type".into());
            }

            valid_extensions(&h.extensions)?;

            if h.referers.len() > 50 {
                return Err("at most 50 allowed referers".into());
            }

            if let Some(r) = h.referers.iter().find(|r| !super::valid_domain(r)) {
                return Err(format!("referer `{r}`: a domain or *.domain"));
            }
        }

        Ok(())
    }

    /// The managed `htpasswd` file content.
    #[must_use]
    pub fn htpasswd(&self) -> Option<String> {
        self.basic_auth.as_ref().map(|b| b.users.iter().map(|u| format!("{}:{}\n", u.user, u.hash)).collect())
    }
}

/// A redirect from one of the endpoint's domains, or from a path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RedirectRule {
    /// `www.example.com` (one of the endpoint's domains) or `/old-path`.
    pub from: String,
    pub to: String,
    #[serde(default = "moved")]
    pub code: u16,
    #[serde(default = "yes")]
    pub keep_path: bool,
}

impl RedirectRule {
    /// Whether the redirect matches a path rather than a domain.
    #[must_use]
    pub fn is_path(&self) -> bool {
        self.from.starts_with('/')
    }

    pub(super) fn validate(&self, domains: &[String]) -> Result<(), String> {
        valid_code(self.code)?;
        valid_url(&self.to)?;

        if self.is_path() {
            valid_path(&self.from, MatchKind::Exact)
        } else if domains.iter().any(|d| *d == self.from && !d.starts_with("*.")) {
            Ok(())
        } else {
            Err(format!("redirect from `{}`: add it to the endpoint's domains first", self.from))
        }
    }

    /// The `return` statement (path redirects keep the query string).
    pub(super) fn ret(&self) -> String {
        let to = RedirectTo { to: self.to.clone(), code: self.code, keep_path: self.keep_path };

        if self.is_path() {
            let t = if self.keep_path { format!("{}$is_args$args", self.to) } else { self.to.clone() };
            format!("return {} {t};", self.code)
        } else {
            format!("return {} {};", self.code, to.target("$request_uri"))
        }
    }
}

pub(super) fn redirect_return(r: &RedirectTo) -> String {
    format!("return {} {};", r.code, r.target("$request_uri"))
}

fn valid_code(code: u16) -> Result<(), String> {
    if matches!(code, 301 | 302 | 307 | 308) { Ok(()) } else { Err("redirect code: 301, 302, 307 or 308".into()) }
}

/// `http(s)://host[:port][/path]` with no characters nginx would read as syntax.
pub fn valid_url(u: &str) -> Result<(), String> {
    let rest = u.strip_prefix("https://").or_else(|| u.strip_prefix("http://"));
    let ok = rest.is_some_and(|r| {
        let (authority, path) = r.split_once('/').map_or((r, ""), |(a, p)| (a, p));
        let (host, port) = authority.rsplit_once(':').map_or((authority, None), |(h, p)| (h, Some(p)));
        let host_ok = super::valid_domain(host) || host.parse::<std::net::Ipv4Addr>().is_ok() || host == "localhost";
        let port_ok = port.is_none_or(|p| p.parse::<u16>().is_ok_and(|p| p > 0));
        let path_ok = path.bytes().all(|b| {
            b.is_ascii_alphanumeric()
                || matches!(b, b'/' | b'.' | b'-' | b'_' | b'~' | b'%' | b'?' | b'=' | b'&' | b'+' | b'#')
        });
        u.len() <= 2048 && host_ok && port_ok && path_ok
    });

    if ok { Ok(()) } else { Err(format!("`{u}` is not a URL (https://host[:port][/path])")) }
}

/// A rule path: prefix and exact paths are plain URI paths; a regex may not
/// contain whitespace, quotes, `;`, braces or a trailing backslash.
fn valid_path(p: &str, kind: MatchKind) -> Result<(), String> {
    let ok = match kind {
        MatchKind::Prefix | MatchKind::Exact => {
            p.starts_with('/')
                && p.len() <= 512
                && !p.split('/').any(|s| s == "..")
                && p.bytes().all(|b| {
                    b.is_ascii_alphanumeric() || matches!(b, b'/' | b'.' | b'-' | b'_' | b'~' | b'%' | b'@' | b'+')
                })
        }
        MatchKind::Regex => {
            !p.is_empty()
                && p.len() <= 512
                && !p.ends_with('\\')
                && !p.chars().any(|c| c.is_whitespace() || c.is_control() || matches!(c, '"' | '\'' | ';' | '{' | '}'))
        }
    };

    if ok { Ok(()) } else { Err(format!("invalid path `{p}`")) }
}

/// An absolute directory without `..`, `//` or characters nginx would parse.
pub fn valid_dir(d: &str) -> Result<(), String> {
    let ok = d.starts_with('/')
        && d.len() > 1
        && d.len() <= 1024
        && !d.contains("//")
        && !d.split('/').any(|s| s == ".." || s == ".")
        && d.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'.' | b'-' | b'_' | b'@' | b'+'));
    if ok { Ok(()) } else { Err(format!("`{d}` is not an absolute directory path")) }
}

fn valid_extensions(exts: &[String]) -> Result<(), String> {
    if exts.len() > 50 {
        return Err("at most 50 file types".into());
    }

    match exts.iter().find(|e| e.is_empty() || e.len() > 10 || !e.bytes().all(|b| b.is_ascii_alphanumeric())) {
        Some(e) => Err(format!("file type `{e}`: letters and digits, without the dot")),
        None => Ok(()),
    }
}

fn valid_ip_or_cidr(s: &str) -> bool {
    let (ip, bits) = s.split_once('/').map_or((s, None), |(a, b)| (a, Some(b)));
    match ip.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(_)) => bits.is_none_or(|b| b.parse::<u8>().is_ok_and(|b| b <= 32)),
        Ok(std::net::IpAddr::V6(_)) => bits.is_none_or(|b| b.parse::<u8>().is_ok_and(|b| b <= 128)),
        Err(_) => false,
    }
}

/// `(a|b|c)` for a regex over validated extensions.
pub(super) fn ext_group(exts: &[String]) -> String {
    format!("({})", exts.join("|"))
}

pub(super) fn validate_features(
    cache: Option<&CacheSpec>,
    gzip: Option<&GzipSpec>,
    access: &AccessSpec,
) -> Result<(), String> {
    if let Some(c) = cache {
        c.validate()?;
    }

    if let Some(g) = gzip {
        g.validate()?;
    }

    access.validate()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_paths_and_hashes() {
        for ok in ["https://example.com", "http://example.com:8080/a/b?x=1", "https://10.0.0.1/"] {
            assert!(valid_url(ok).is_ok(), "{ok}");
        }

        for bad in ["ftp://x.com", "https://x.com/a b", "https://x.com/;", "https://x.com/$host", "https://", "x.com"] {
            assert!(valid_url(bad).is_err(), "{bad}");
        }

        assert!(valid_path("/api/", MatchKind::Prefix).is_ok());
        assert!(valid_path("/a/../b", MatchKind::Prefix).is_err());
        assert!(valid_path("\\.php$", MatchKind::Regex).is_ok());
        assert!(valid_path("a\" { root /;", MatchKind::Regex).is_err());
        assert!(valid_dir("/srv/www/site").is_ok());

        for bad in ["/", "srv", "/srv/../etc", "/srv//x", "/srv/x;", "/srv/a b"] {
            assert!(valid_dir(bad).is_err(), "{bad}");
        }

        let h = format!("$6$saltsalt${}", "a".repeat(86));
        assert!(valid_crypt_hash(&h));
        assert!(valid_crypt_hash(&format!("$6$rounds=5000$salt${}", "b".repeat(86))));
        assert!(!valid_crypt_hash("$1$x$y"));
        assert!(!valid_crypt_hash(&format!("$6$sa:lt${}", "a".repeat(86))));
        assert!(valid_ip_or_cidr("192.0.2.0/24") && valid_ip_or_cidr("2001:db8::/32") && !valid_ip_or_cidr("all"));
    }

    #[test]
    fn passwords_are_never_serialized_or_printed() {
        let u: BasicAuthUser =
            serde_json::from_str(r#"{"user":"ops","password":"hunter2"}"#).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(u.password.as_deref(), Some("hunter2"));
        assert!(!serde_json::to_string(&u).unwrap_or_default().contains("hunter2"));
        assert!(!format!("{u:?}").contains("hunter2"));
    }
}
