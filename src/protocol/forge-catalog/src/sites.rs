//! Reverse-proxy endpoints (Nginx) shared by the control plane and the node helper.
//!
//! The helper renders the managed config (the read-only *main
//! configuration*) from a typed [`SiteSpec`]: domains and ports, path rules
//! (proxy, load balancer, redirect, static), cache, compression, access
//! control, redirects, TLS and logs. Only the *custom configuration* is free
//! text. It is kept in its own include file (`/etc/nginx/forge/custom/<site>.conf`),
//! checked against an allowlist, tested with `nginx -t` and rolled back
//! automatically if the test fails.

use serde::{Deserialize, Serialize};

mod features;

pub use features::{
    AccessSpec, BasicAuth, BasicAuthUser, CACHE_DIR, CacheSpec, GzipSpec, HTPASSWD_DIR, Hotlink, LOG_DIR, MatchKind,
    ProxyOptions, RedirectRule, RedirectTo, RouteRule, RuleKind, StaticRoot, valid_crypt_hash, valid_dir, valid_url,
};

pub const SITES_DIR: &str = "/etc/nginx/forge/sites";
pub const CUSTOM_DIR: &str = "/etc/nginx/forge/custom";
pub const ACME_ROOT: &str = "/var/www/forge-acme";
/// Uploaded certificates on the edges (root only, files 0600).
pub const TLS_DIR: &str = "/etc/nginx/forge/tls";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Balance {
    #[default]
    RoundRobin,
    LeastConn,
    IpHash,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Tls {
    /// Plain HTTP only.
    #[default]
    Off,
    /// Let's Encrypt via certbot (HTTP-01, webroot); needs Certbot on the node.
    /// Certbot's timer renews it.
    Acme,
    /// A certificate uploaded to Forge (`cert_id`); Forge sends the chain and
    /// key to the edges when the endpoint is applied.
    Uploaded,
}

/// Certificate chain and private key for an uploaded certificate. Filled in
/// by the control plane only on the command sent to an edge (never stored in
/// job payloads); `Debug` never prints the key.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CertMaterial {
    pub chain_pem: String,
    pub key_pem: String,
}

impl std::fmt::Debug for CertMaterial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CertMaterial")
            .field("chain_pem", &self.chain_pem.len())
            .field("key_pem", &"<redacted>")
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SiteSpec {
    /// Slug used for file names.
    pub name: String,
    /// Host names (`example.com`, `*.example.com`). The first is the primary.
    pub domains: Vec<String>,
    /// Port of the plain HTTP server (80 unless changed; Let's Encrypt needs 80).
    #[serde(default = "http_port", skip_serializing_if = "is_http_port")]
    pub http_port: u16,
    /// Port of the HTTPS server (with TLS).
    #[serde(default = "https_port", skip_serializing_if = "is_https_port")]
    pub https_port: u16,
    /// What the default `/` rule does. Old rows have none: a proxy.
    #[serde(default, skip_serializing_if = "RuleKind::is_proxy")]
    pub kind: RuleKind,
    /// Active upstreams `host:port`. May be left out when the API resolves
    /// them from typed targets; [`SiteSpec::validate`] still needs one for a
    /// proxy or load balancer.
    #[serde(default)]
    pub upstreams: Vec<String>,
    /// Standby upstreams (`backup`): used only when every active one is down.
    #[serde(default)]
    pub standby: Vec<String>,
    /// Relative weights of active upstreams (`host:port` → 1–1000); an
    /// upstream not listed weighs 100. Filled in by the control plane while
    /// a rollout shifts traffic between two copies of a service (#73).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub weights: std::collections::BTreeMap<String, u32>,
    #[serde(default)]
    pub balance: Balance,
    /// Proxy options of the default rule.
    #[serde(default, skip_serializing_if = "ProxyOptions::is_default")]
    pub proxy: ProxyOptions,
    /// The default rule's redirect (with [`RuleKind::Redirect`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redirect: Option<RedirectTo>,
    /// The default rule's directory (with [`RuleKind::Static`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub static_root: Option<StaticRoot>,
    /// Further path rules, matched before the default rule.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<RouteRule>,
    #[serde(default)]
    pub tls: Tls,
    /// Contact for Let's Encrypt.
    #[serde(default)]
    pub acme_email: String,
    /// Uploaded certificate id (with [`Tls::Uploaded`]).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cert_id: String,
    /// See [`CertMaterial`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub material: Option<CertMaterial>,
    /// The other edges serving this endpoint (`host` or `host:port`), filled
    /// in per edge by the control plane. With Let's Encrypt, a challenge this
    /// edge does not hold is asked of its peers, so every edge can issue its
    /// own certificate while DNS points at any of them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub acme_peers: Vec<String>,
    /// Redirect HTTP to HTTPS (with TLS).
    #[serde(default = "yes")]
    pub force_https: bool,
    /// `Strict-Transport-Security` on the HTTPS server.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub hsts: bool,
    #[serde(default = "yes")]
    pub websockets: bool,
    /// `client_max_body_size` in MB.
    #[serde(default = "body")]
    pub max_body_mb: u32,
    /// Requests per second per client IP (0 = unlimited).
    #[serde(default)]
    pub rate_limit: u32,
    /// Requests allowed above the rate before refusing (0 = twice the rate).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rate_burst: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache: Option<CacheSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gzip: Option<GzipSpec>,
    #[serde(default, skip_serializing_if = "AccessSpec::is_default")]
    pub access: AccessSpec,
    /// Domain and path redirects.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub redirects: Vec<RedirectRule>,
    /// The custom configuration file: free directives included inside the
    /// `server` block(s), checked against an allowlist.
    #[serde(default)]
    pub custom: String,
    /// Main access log format in this site's `server` blocks: `combined`
    /// (empty = default), `json` or `off`. Filled in per edge by the control
    /// plane from the cluster ingress profile; the Forge traffic log is
    /// written either way.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub access_log: String,
}

const fn yes() -> bool {
    true
}
#[allow(clippy::trivially_copy_pass_by_ref)] // reason: serde's skip_serializing_if passes a reference
const fn is_true(b: &bool) -> bool {
    *b
}
#[allow(clippy::trivially_copy_pass_by_ref)] // reason: serde's skip_serializing_if passes a reference
const fn is_zero(n: &u32) -> bool {
    *n == 0
}
const fn body() -> u32 {
    25
}
const fn http_port() -> u16 {
    80
}
const fn https_port() -> u16 {
    443
}
#[allow(clippy::trivially_copy_pass_by_ref)] // reason: serde's skip_serializing_if passes a reference
const fn is_http_port(p: &u16) -> bool {
    *p == 80
}
#[allow(clippy::trivially_copy_pass_by_ref)] // reason: serde's skip_serializing_if passes a reference
const fn is_https_port(p: &u16) -> bool {
    *p == 443
}

/// A refused custom configuration: where and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CustomError {
    /// 1-based line of the offending statement.
    pub line: usize,
    /// The refused directive, when one was read.
    pub directive: Option<String>,
    pub message: String,
}

impl std::fmt::Display for CustomError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for CustomError {}

impl CustomError {
    fn at(line: usize, message: impl Into<String>) -> Self {
        Self { line, directive: None, message: message.into() }
    }
}

/// Directives a custom block may use (site blocks sit inside `server {}`,
/// the profile's global block at `http {}` level). An allowlist: nothing that
/// takes a file path (nginx opens files as root), changes the nginx process,
/// or loads code. `nginx -t` on the edge still checks where each one may go.
const ALLOWED: &[&str] = &[
    // Blocks and flow.
    "location",
    "if",
    "limit_except",
    "upstream",
    "server",
    "map",
    "geo",
    "split_clients",
    "return",
    "rewrite",
    "break",
    "set",
    "internal",
    "error_page",
    "default_type",
    "charset",
    "listen",
    "server_name",
    "absolute_redirect",
    "port_in_redirect",
    "server_name_in_redirect",
    "merge_slashes",
    "if_modified_since",
    "etag",
    "expires",
    "add_header",
    "add_trailer",
    "server_tokens",
    "ignore_invalid_headers",
    "underscores_in_headers",
    "chunked_transfer_encoding",
    "recursive_error_pages",
    "msie_padding",
    "msie_refresh",
    "log_not_found",
    "log_subrequest",
    "rewrite_log",
    "uninitialized_variable_warn",
    // Upstream blocks.
    "keepalive",
    "keepalive_requests",
    "keepalive_time",
    "keepalive_timeout",
    "least_conn",
    "ip_hash",
    "hash",
    "random",
    "zone",
    // Proxying.
    "proxy_pass",
    "proxy_set_header",
    "proxy_hide_header",
    "proxy_pass_header",
    "proxy_http_version",
    "proxy_buffering",
    "proxy_buffers",
    "proxy_buffer_size",
    "proxy_busy_buffers_size",
    "proxy_read_timeout",
    "proxy_send_timeout",
    "proxy_connect_timeout",
    "proxy_redirect",
    "proxy_cookie_path",
    "proxy_cookie_domain",
    "proxy_cookie_flags",
    "proxy_intercept_errors",
    "proxy_next_upstream",
    "proxy_next_upstream_tries",
    "proxy_next_upstream_timeout",
    "proxy_request_buffering",
    "proxy_max_temp_file_size",
    "proxy_ignore_headers",
    "proxy_method",
    "proxy_set_body",
    "proxy_pass_request_body",
    "proxy_pass_request_headers",
    "proxy_ssl_server_name",
    "proxy_ssl_name",
    "proxy_ssl_protocols",
    "proxy_ssl_ciphers",
    "proxy_socket_keepalive",
    "proxy_bind",
    "proxy_headers_hash_max_size",
    "proxy_headers_hash_bucket_size",
    "grpc_pass",
    "grpc_set_header",
    "grpc_hide_header",
    "grpc_read_timeout",
    "grpc_send_timeout",
    "grpc_connect_timeout",
    "grpc_buffer_size",
    // Clients, limits and access.
    "client_max_body_size",
    "client_body_buffer_size",
    "client_body_timeout",
    "client_header_timeout",
    "client_header_buffer_size",
    "large_client_header_buffers",
    "send_timeout",
    "reset_timedout_connection",
    "limit_req",
    "limit_req_status",
    "limit_req_log_level",
    "limit_req_dry_run",
    "limit_req_zone",
    "limit_conn",
    "limit_conn_status",
    "limit_conn_log_level",
    "limit_conn_dry_run",
    "limit_conn_zone",
    "limit_rate",
    "limit_rate_after",
    "allow",
    "deny",
    "satisfy",
    "auth_request",
    "auth_request_set",
    "valid_referers",
    "real_ip_header",
    "set_real_ip_from",
    "real_ip_recursive",
    // Content.
    "gzip",
    "gzip_types",
    "gzip_proxied",
    "gzip_min_length",
    "gzip_comp_level",
    "gzip_vary",
    "gzip_disable",
    "gzip_http_version",
    "gzip_buffers",
    "sub_filter",
    "sub_filter_once",
    "sub_filter_types",
    "sub_filter_last_modified",
    "mirror",
    "mirror_request_body",
    "sendfile",
    "tcp_nopush",
    "tcp_nodelay",
    "postpone_output",
    "output_buffers",
    "resolver",
    "resolver_timeout",
    "http2",
    "http2_max_concurrent_streams",
    "ssl_protocols",
    "ssl_ciphers",
    "ssl_prefer_server_ciphers",
    "ssl_session_cache",
    "ssl_session_timeout",
    "ssl_session_tickets",
    "ssl_buffer_size",
    "ssl_early_data",
    "variables_hash_max_size",
    "variables_hash_bucket_size",
    "map_hash_max_size",
    "map_hash_bucket_size",
    "server_names_hash_max_size",
    "server_names_hash_bucket_size",
];
/// Blocks whose entries are data (`key value;`), not directives. Their only
/// directive that reads a file, `include`, is refused.
const DATA_BLOCKS: &[&str] = &["map", "geo", "split_clients"];

/// Cache and auth directives that take no file path, also allowed in custom
/// configuration (the cache zone and the user file are Forge's).
const ALLOWED_EXTRA: &[&str] = &[
    "auth_basic",
    "proxy_cache",
    "proxy_cache_key",
    "proxy_cache_valid",
    "proxy_cache_bypass",
    "proxy_no_cache",
    "proxy_cache_methods",
    "proxy_cache_min_uses",
    "proxy_cache_use_stale",
    "proxy_cache_lock",
    "proxy_cache_lock_timeout",
    "proxy_cache_revalidate",
    "proxy_cache_background_update",
];

/// One statement of a free nginx block: its words (the directive name
/// first), the block it sits in and its line.
#[derive(Debug)]
struct Stmt {
    words: Vec<String>,
    parent: Option<String>,
    line: usize,
}

/// Statements of a free nginx block, read the way nginx's own parser
/// (`ngx_conf_read_token`) splits words, so quoting, escapes and comments
/// cannot hide a directive. Errors on input nginx itself would refuse.
fn statements(text: &str) -> Result<Vec<Stmt>, CustomError> {
    fn unescape(raw: &[char]) -> String {
        let mut out = String::new();
        let mut i = 0;
        while i < raw.len() {
            if raw[i] == '\\' && i + 1 < raw.len() {
                let mapped = match raw[i + 1] {
                    c @ ('"' | '\'' | '\\') => Some(c),
                    't' => Some('\t'),
                    'r' => Some('\r'),
                    'n' => Some('\n'),
                    _ => None,
                };
                if let Some(c) = mapped {
                    out.push(c);
                    i += 2;
                    continue;
                }
            }
            out.push(raw[i]);
            i += 1;
        }
        out
    }
    let chars: Vec<char> = text.chars().collect();
    let mut line_of = Vec::with_capacity(chars.len());
    let mut line = 1usize;
    for &c in &chars {
        line_of.push(line);
        if c == '\n' {
            line += 1;
        }
    }
    let at = |pos: usize| line_of.get(pos).copied().unwrap_or(line);
    let mut stmts: Vec<Stmt> = Vec::new();
    let mut words = 0usize; // words of the current statement
    // Open blocks (their directive names); `cur` names the current statement.
    let mut stack: Vec<String> = Vec::new();
    let mut cur = String::new();
    let (mut last_space, mut need_space, mut sharp, mut quoted) = (true, false, false, false);
    let (mut d_quoted, mut s_quoted, mut variable) = (false, false, false);
    let mut start = 0usize;
    for (pos, &ch) in chars.iter().enumerate() {
        if ch == '\n' && sharp {
            sharp = false;
        }
        if sharp {
            continue;
        }
        if quoted {
            quoted = false;
            continue;
        }
        let space = matches!(ch, ' ' | '\t' | '\r' | '\n');
        if need_space {
            if space {
                last_space = true;
                need_space = false;
                continue;
            }
            match ch {
                ';' => {
                    words = 0;
                    last_space = true;
                    need_space = false;
                    continue;
                }
                '{' => {
                    words = 0;
                    stack.push(std::mem::take(&mut cur));
                    last_space = true;
                    need_space = false;
                    continue;
                }
                ')' => {
                    last_space = true;
                    need_space = false;
                }
                _ => return Err(CustomError::at(at(pos), format!("unexpected `{ch}`"))),
            }
        }
        if last_space {
            start = pos;
            if space {
                continue;
            }
            match ch {
                ';' | '{' => {
                    if words == 0 {
                        return Err(CustomError::at(at(pos), format!("unexpected `{ch}`")));
                    }
                    words = 0;
                    if ch == '{' {
                        stack.push(std::mem::take(&mut cur));
                    }
                    continue;
                }
                '}' => {
                    if words != 0 {
                        return Err(CustomError::at(at(pos), "unexpected `}` (missing `;`?)"));
                    }
                    if stack.pop().is_none() {
                        return Err(CustomError::at(at(pos), "unbalanced `}`"));
                    }
                    continue;
                }
                '#' => {
                    sharp = true;
                    continue;
                }
                '\\' => {
                    quoted = true;
                    last_space = false;
                    continue;
                }
                '"' => {
                    start = pos + 1;
                    d_quoted = true;
                    last_space = false;
                    continue;
                }
                '\'' => {
                    start = pos + 1;
                    s_quoted = true;
                    last_space = false;
                    continue;
                }
                '$' => {
                    variable = true;
                    last_space = false;
                    continue;
                }
                _ => {
                    last_space = false;
                    continue;
                }
            }
        }
        // Inside a word.
        if ch == '{' && variable {
            continue;
        }
        variable = false;
        if ch == '\\' {
            quoted = true;
            continue;
        }
        if ch == '$' {
            variable = true;
            continue;
        }
        let mut found = false;
        if d_quoted {
            if ch == '"' {
                d_quoted = false;
                need_space = true;
                found = true;
            }
        } else if s_quoted {
            if ch == '\'' {
                s_quoted = false;
                need_space = true;
                found = true;
            }
        } else if space || ch == ';' || ch == '{' {
            last_space = true;
            found = true;
        }
        if found {
            let word = unescape(&chars[start..pos]);
            if words == 0 {
                stmts.push(Stmt { words: vec![word.clone()], parent: stack.last().cloned(), line: at(start) });
                cur = word;
            } else if let Some(s) = stmts.last_mut() {
                s.words.push(word);
            }
            words += 1;
            if ch == ';' {
                words = 0;
            } else if ch == '{' {
                words = 0;
                stack.push(std::mem::take(&mut cur));
            }
        }
    }
    if !last_space || need_space || words != 0 || d_quoted || s_quoted {
        return Err(CustomError::at(line, "the file ends in the middle of a directive (missing `;`?)"));
    }
    if !stack.is_empty() {
        return Err(CustomError::at(line, "unbalanced `{`"));
    }
    Ok(stmts)
}

/// Checks a free nginx block (the profile's global block, at `http {}`
/// level) against [`ALLOWED`], and refuses Unix sockets (a local socket such
/// as Docker's is root on the node). `nginx -t` still checks the syntax;
/// saving or applying a non-empty block also needs `server.command` (MFA).
pub fn check_custom_block(text: &str) -> Result<(), CustomError> {
    check_custom(text, None)
}

/// [`check_custom_block`] for an endpoint's custom configuration: also allows
/// the endpoint's own managed files (its `htpasswd` and log files).
fn check_custom(text: &str, site: Option<&SiteSpec>) -> Result<(), CustomError> {
    if text.len() > 64 * 1024 || text.contains('\0') {
        return Err(CustomError::at(1, "the custom configuration is too large (64 KiB at most)"));
    }
    if let Some(i) = text.to_ascii_lowercase().find("unix:") {
        let line = text[..i].matches('\n').count() + 1;
        return Err(CustomError::at(line, "Unix sockets are not allowed"));
    }
    for s in statements(text)? {
        let name = s.words.first().cloned().unwrap_or_default();
        let ok = if s.parent.as_deref().is_some_and(|p| DATA_BLOCKS.contains(&p)) {
            name != "include"
        } else {
            ALLOWED.contains(&name.as_str())
                || ALLOWED_EXTRA.contains(&name.as_str())
                || site.is_some_and(|spec| spec.managed_file_ok(&s.words))
        };
        if !ok {
            return Err(CustomError {
                line: s.line,
                message: format!("`{name}` is not allowed here (see the list of supported directives)"),
                directive: Some(name),
            });
        }
    }
    Ok(())
}

pub(crate) fn valid_domain(d: &str) -> bool {
    let d = d.strip_prefix("*.").unwrap_or(d);
    !d.is_empty()
        && d.len() <= 253
        && d.contains('.')
        && d.split('.').all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}

fn valid_upstream(u: &str) -> bool {
    let Some((host, port)) = u.rsplit_once(':') else { return false };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    port.parse::<u16>().is_ok_and(|p| p > 0)
        && !host.is_empty()
        && host.len() <= 253
        && host.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':'))
}

/// `upstream` block body lines for active and standby servers.
fn upstream_block(
    name: &str,
    upstreams: &[String],
    standby: &[String],
    weights: &std::collections::BTreeMap<String, u32>,
    balance: Balance,
) -> String {
    let mut servers = String::new();
    for u in upstreams {
        let weight = weights.get(u).filter(|w| **w != 100).map(|w| format!(" weight={w}")).unwrap_or_default();
        servers.push_str(&format!("    server {u}{weight} max_fails=3 fail_timeout=10s;\n"));
    }
    for u in standby {
        servers.push_str(&format!("    server {u} backup;\n"));
    }
    let balance = match balance {
        Balance::RoundRobin => "",
        Balance::LeastConn => "    least_conn;\n",
        Balance::IpHash => "    ip_hash;\n",
    };
    format!("upstream {name} {{\n{balance}{servers}    keepalive 16;\n}}\n\n")
}

/// What one `location` does (the default rule's or a path rule's).
struct Route<'a> {
    open: String,
    kind: RuleKind,
    upstream: String,
    proxy: &'a ProxyOptions,
    websockets: bool,
    redirect: Option<&'a RedirectTo>,
    static_root: Option<&'a StaticRoot>,
}

impl SiteSpec {
    pub fn validate(&self) -> Result<(), String> {
        if !crate::apps::valid_slug(&self.name, 60) {
            return Err("site name: lowercase letters, digits and dashes".into());
        }
        if self.domains.is_empty() || self.domains.len() > 50 {
            return Err("between 1 and 50 domains".into());
        }
        if let Some(d) = self.domains.iter().find(|d| !valid_domain(d)) {
            return Err(format!("invalid domain `{d}`"));
        }
        if self.http_port == 0 || self.https_port == 0 || self.http_port == self.https_port {
            return Err("HTTP and HTTPS ports: two different ports 1–65535".into());
        }
        features::check_kind(
            self.kind,
            &self.upstreams,
            &self.standby,
            self.redirect.as_ref(),
            self.static_root.as_ref(),
        )?;
        if !self.kind.proxies() && (!self.upstreams.is_empty() || !self.standby.is_empty()) {
            return Err("a redirect or static site has no upstreams".into());
        }
        self.proxy.validate_default()?;
        for (u, w) in &self.weights {
            if !self.upstreams.contains(u) {
                return Err(format!("weight for `{u}`, which is not an active upstream"));
            }
            if *w == 0 || *w > 1000 {
                return Err(format!("the weight of `{u}` must be 1–1000"));
            }
        }
        if self.rules.len() > 50 {
            return Err("at most 50 path rules".into());
        }
        let mut paths = std::collections::BTreeSet::new();
        for r in &self.rules {
            r.validate()?;
            if !paths.insert((r.match_kind as u8, r.path.as_str())) {
                return Err(format!("two rules for `{}`", r.path));
            }
        }
        if self.redirects.len() > 100 {
            return Err("at most 100 redirects".into());
        }
        for r in &self.redirects {
            r.validate(&self.domains)?;
            if r.is_path() && !paths.insert((MatchKind::Exact as u8, r.from.as_str())) {
                return Err(format!("`{}` is both a redirect and an exact rule", r.from));
            }
        }
        features::validate_features(self.cache.as_ref(), self.gzip.as_ref(), &self.access)?;
        if self.tls == Tls::Acme {
            if self.domains.iter().any(|d| d.starts_with("*.")) {
                return Err("wildcard domains need DNS-01; use TLS off or explicit names".into());
            }
            if self.http_port != 80 {
                return Err("Let's Encrypt validates on port 80: keep the HTTP port at 80".into());
            }
            if !self.acme_email.contains('@')
                || self.acme_email.len() > 254
                || self.acme_email.contains([' ', '\n', ';', '\'', '"'])
            {
                return Err("a contact email is required for Let's Encrypt".into());
            }
        }
        if self.tls == Tls::Uploaded && self.cert_id.is_empty() {
            return Err("choose an uploaded certificate".into());
        }
        if self.tls != Tls::Uploaded && self.material.is_some() {
            return Err("certificate material without an uploaded certificate".into());
        }
        if let Some(m) = &self.material
            && (!m.chain_pem.contains("BEGIN CERTIFICATE")
                || !m.key_pem.contains("PRIVATE KEY")
                || m.chain_pem.len() > 64 * 1024
                || m.key_pem.len() > 16 * 1024)
        {
            return Err("invalid certificate material".into());
        }
        if self.acme_peers.len() > 16 || self.acme_peers.iter().any(|p| !valid_upstream(p) && !valid_domain(p)) {
            return Err("invalid ACME peers".into());
        }
        if self.max_body_mb == 0 || self.max_body_mb > 10_240 {
            return Err("max body size: 1–10240 MB".into());
        }
        if self.rate_burst > 100_000 {
            return Err("burst: 0–100000 requests".into());
        }
        if !matches!(self.access_log.as_str(), "" | "combined" | "json" | "off") {
            return Err("access_log must be combined, json or off".into());
        }
        self.check_custom().map_err(|e| format!("custom configuration, {e}"))
    }

    /// Checks the custom configuration file alone (line and directive of the
    /// first refusal), for live checks while editing.
    pub fn check_custom(&self) -> Result<(), CustomError> {
        check_custom(&self.custom, Some(self))
    }

    /// Whether a custom statement only points at this endpoint's own managed
    /// files: `auth_basic_user_file` at its `htpasswd`, `access_log` and
    /// `error_log` at its log files.
    fn managed_file_ok(&self, words: &[String]) -> bool {
        let up = self.upstream_name();
        match words {
            [d, path] if d == "auth_basic_user_file" => *path == self.htpasswd_path(),
            [d, path] if d == "access_log" => *path == self.access_log_path(),
            [d, path, format] if d == "access_log" => {
                *path == self.access_log_path()
                    && (format == "combined" || format.strip_prefix(&up).is_some_and(|f| f.starts_with('_')))
            }
            [d, path] if d == "error_log" => *path == self.error_log_path(),
            [d, path, level] if d == "error_log" => {
                *path == self.error_log_path()
                    && matches!(level.as_str(), "debug" | "info" | "notice" | "warn" | "error" | "crit")
            }
            _ => false,
        }
    }

    /// Per-site log formats (`http {}` level). Named after the site, so they
    /// never clash with the profile's `forge_traffic` (nginx refuses a
    /// duplicate `log_format` name) or with another site's.
    fn log_formats(&self) -> String {
        let up = self.upstream_name();
        let mut o = format!("log_format {up}_traffic {};\n", crate::ingress::TRAFFIC_LOG_FORMAT);
        if self.access_log == "json" {
            o.push_str(&format!("log_format {up}_json escape=json {};\n", crate::ingress::JSON_LOG_FORMAT));
        }
        o
    }

    /// Log lines for each `server` block. Server-level `access_log` replaces
    /// the inherited `http {}` ones, so each request is written once to the
    /// traffic log (also on edges with a profile) and once to the endpoint's
    /// own access log in the profile's format; errors go to its error log.
    fn log_lines(&self) -> String {
        let up = self.upstream_name();
        let mut o = format!("    access_log {} {up}_traffic;\n", crate::ingress::TRAFFIC_LOG);
        let own = self.access_log_path();
        match self.access_log.as_str() {
            "off" => {}
            "json" => o.push_str(&format!("    access_log {own} {up}_json;\n")),
            _ => o.push_str(&format!("    access_log {own} combined;\n")),
        }
        o.push_str(&format!("    error_log {};\n", self.error_log_path()));
        o
    }

    #[must_use]
    pub fn site_path(&self) -> String {
        format!("{SITES_DIR}/{}.conf", self.name)
    }

    #[must_use]
    pub fn custom_path(&self) -> String {
        format!("{CUSTOM_DIR}/{}.conf", self.name)
    }

    /// The managed `htpasswd` file of the endpoint's basic authentication.
    #[must_use]
    pub fn htpasswd_path(&self) -> String {
        format!("{HTPASSWD_DIR}/{}", self.name)
    }

    /// The endpoint's proxy cache directory.
    #[must_use]
    pub fn cache_path(&self) -> String {
        format!("{CACHE_DIR}/{}", self.name)
    }

    #[must_use]
    pub fn access_log_path(&self) -> String {
        format!("{LOG_DIR}/{}.access.log", self.name)
    }

    #[must_use]
    pub fn error_log_path(&self) -> String {
        format!("{LOG_DIR}/{}.error.log", self.name)
    }

    #[must_use]
    pub fn cert_dir(&self) -> String {
        format!("/etc/letsencrypt/live/{}", self.domains[0].trim_start_matches("*."))
    }

    /// `(certificate chain, private key)` paths on the edge.
    #[must_use]
    pub fn cert_files(&self) -> (String, String) {
        match self.tls {
            Tls::Uploaded => (format!("{TLS_DIR}/{}.crt", self.name), format!("{TLS_DIR}/{}.key", self.name)),
            _ => {
                let dir = self.cert_dir();
                (format!("{dir}/fullchain.pem"), format!("{dir}/privkey.pem"))
            }
        }
    }

    /// Every directory a static route serves (the default rule's and the
    /// path rules'); the control plane checks each one.
    #[must_use]
    pub fn static_roots(&self) -> Vec<&str> {
        let default = (self.kind == RuleKind::Static).then_some(self.static_root.as_ref()).flatten();
        default
            .into_iter()
            .chain(self.rules.iter().filter(|r| r.kind == RuleKind::Static).filter_map(|r| r.static_root.as_ref()))
            .map(|s| s.root.as_str())
            .collect()
    }

    fn upstream_name(&self) -> String {
        format!("forge_{}", self.name.replace('-', "_"))
    }

    /// One `location`, by the route's kind.
    fn location(&self, r: &Route<'_>) -> String {
        let up = self.upstream_name();
        match r.kind {
            RuleKind::Redirect => match r.redirect {
                Some(to) => format!("    {} {{\n        {}\n    }}\n", r.open, features::redirect_return(to)),
                None => String::new(),
            },
            RuleKind::Static => match r.static_root {
                Some(s) => format!(
                    "    {} {{\n        root {};\n        index {};\n        disable_symlinks if_not_owner from=$document_root;\n\
                     \x20       if ($uri ~ \"/\\.\") {{\n            return 404;\n        }}\n        try_files $uri $uri/ {};\n    }}\n",
                    r.open,
                    s.root,
                    s.index,
                    if s.spa { format!("/{}", s.index) } else { "=404".into() },
                ),
                None => String::new(),
            },
            RuleKind::Proxy | RuleKind::Balancer => {
                let ws = if r.websockets {
                    format!(
                        "        proxy_set_header Upgrade $http_upgrade;\n        proxy_set_header Connection ${up}_upgrade;\n"
                    )
                } else {
                    String::new()
                };
                let limit = if self.rate_limit > 0 {
                    let burst = if self.rate_burst > 0 { self.rate_burst } else { self.rate_limit * 2 };
                    format!("        limit_req zone={up}_rl burst={burst} nodelay;\n")
                } else {
                    String::new()
                };
                let o = r.proxy;
                let host = if o.preserve_host { "$host" } else { "$proxy_host" };
                let strip = if o.strip_prefix { "/" } else { "" };
                let mut extra = String::new();
                if !o.buffering {
                    extra.push_str("        proxy_buffering off;\n");
                }
                if let Some(c) = &self.cache {
                    extra.push_str(&format!(
                        "        proxy_cache {up}_cache;\n        proxy_cache_key \"{}\";\n        proxy_cache_valid 200 301 302 {}m;\n",
                        c.key, c.ttl_minutes
                    ));
                    if c.ignore_headers {
                        extra.push_str("        proxy_ignore_headers Cache-Control Expires Set-Cookie;\n");
                    }
                    if !c.extensions.is_empty() {
                        extra.push_str(&format!(
                            "        proxy_no_cache ${up}_nocache;\n        proxy_cache_bypass ${up}_nocache;\n"
                        ));
                    }
                }
                format!(
                    "    {open} {{\n{limit}        proxy_pass http://{upstream}{strip};\n        proxy_http_version 1.1;\n{ws}\
                     \x20       proxy_set_header Host {host};\n        proxy_set_header X-Real-IP $remote_addr;\n\
                     \x20       proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;\n        proxy_set_header X-Forwarded-Proto $scheme;\n\
                     \x20       proxy_set_header X-Forwarded-Host $host;\n\
                     \x20       proxy_read_timeout {read}s;\n        proxy_connect_timeout {connect}s;\n\
                     \x20       proxy_next_upstream error timeout http_502 http_503 http_504;\n{extra}    }}\n",
                    open = r.open,
                    upstream = r.upstream,
                    read = o.read_timeout_s,
                    connect = o.connect_timeout_s,
                )
            }
        }
    }

    /// Every `location` serving requests: path redirects, path rules, then
    /// the default `/` rule.
    fn locations(&self) -> String {
        let up = self.upstream_name();
        let mut o = String::new();
        for r in self.redirects.iter().filter(|r| r.is_path()) {
            o.push_str(&format!("    location = {} {{\n        {}\n    }}\n", r.from, r.ret()));
        }
        for (i, r) in self.rules.iter().enumerate() {
            o.push_str(&self.location(&Route {
                open: r.location(),
                kind: r.kind,
                upstream: format!("{up}_r{}", i + 1),
                proxy: &r.proxy,
                websockets: r.websockets,
                redirect: r.redirect.as_ref(),
                static_root: r.static_root.as_ref(),
            }));
        }
        o.push_str(&self.location(&Route {
            open: "location /".into(),
            kind: self.kind,
            upstream: up,
            proxy: &self.proxy,
            websockets: self.websockets,
            redirect: self.redirect.as_ref(),
            static_root: self.static_root.as_ref(),
        }));
        o
    }

    /// `http {}`-level definitions: rate zone, cache, maps and upstreams.
    fn http_level(&self) -> String {
        let up = self.upstream_name();
        let mut o = String::new();
        if self.rate_limit > 0 {
            o.push_str(&format!("limit_req_zone $binary_remote_addr zone={up}_rl:10m rate={}r/s;\n", self.rate_limit));
        }
        if let Some(c) = &self.cache {
            o.push_str(&format!(
                "proxy_cache_path {} levels=1:2 keys_zone={up}_cache:10m max_size={}m inactive={}m use_temp_path=off;\n",
                self.cache_path(),
                c.max_size_mb,
                c.ttl_minutes.max(10),
            ));
            if !c.extensions.is_empty() {
                o.push_str(&format!(
                    "map $uri ${up}_nocache {{\n    default 1;\n    \"~*\\.{}$\" 0;\n}}\n",
                    features::ext_group(&c.extensions)
                ));
            }
        }
        if let Some(h) = &self.access.hotlink {
            o.push_str(&format!(
                "map $uri ${up}_hotlink {{\n    default 0;\n    \"~*\\.{}$\" 1;\n}}\n",
                features::ext_group(&h.extensions)
            ));
        }
        if self.kind.proxies() {
            o.push_str(&upstream_block(&up, &self.upstreams, &self.standby, &self.weights, self.balance));
        } else {
            o.push('\n');
        }
        for (i, r) in self.rules.iter().enumerate().filter(|(_, r)| r.kind.proxies()) {
            o.push_str(&upstream_block(
                &format!("{up}_r{}", i + 1),
                &r.upstreams,
                &r.standby,
                &std::collections::BTreeMap::new(),
                r.balance,
            ));
        }
        o
    }

    /// Server-level access control, compression and domain redirects (in
    /// every `server` block that serves the endpoint).
    fn server_level(&self) -> String {
        let up = self.upstream_name();
        let mut o = String::new();
        let a = &self.access;
        for d in &a.deny {
            o.push_str(&format!("    deny {d};\n"));
        }
        for d in &a.allow {
            o.push_str(&format!("    allow {d};\n"));
        }
        if !a.allow.is_empty() {
            o.push_str("    deny all;\n");
        }
        if let Some(b) = &a.basic_auth {
            o.push_str(&format!(
                "    auth_basic \"{}\";\n    auth_basic_user_file {};\n",
                b.realm,
                self.htpasswd_path()
            ));
        }
        if let Some(h) = &a.hotlink {
            let refs: String = h.referers.iter().map(|r| format!(" {r}")).collect();
            o.push_str(&format!(
                "    valid_referers none blocked server_names{refs};\n    set ${up}_hl \"${{{up}_hotlink}}${{invalid_referer}}\";\n\
                 \x20   if (${up}_hl = 11) {{\n        return 403;\n    }}\n"
            ));
        }
        if let Some(g) = &self.gzip {
            o.push_str(&format!(
                "    gzip on;\n    gzip_comp_level {};\n    gzip_types {};\n    gzip_proxied any;\n    gzip_vary on;\n",
                g.level,
                if g.types.is_empty() { "text/plain".into() } else { g.types.join(" ") },
            ));
        }
        for r in self.redirects.iter().filter(|r| !r.is_path()) {
            o.push_str(&format!("    if ($host = {}) {{\n        {}\n    }}\n", r.from, r.ret()));
        }
        o
    }

    /// The ACME challenge locations (always served, whatever access rules say).
    fn acme_locations(&self) -> String {
        let up = self.upstream_name();
        let open = if self.access.restricts() { "        allow all;\n        auth_basic off;\n" } else { "" };
        if self.tls == Tls::Acme && !self.acme_peers.is_empty() {
            // Not here: ask the peers once (the hop header stops loops); a
            // peer answering 404 passes to the next one.
            format!(
                "    location ^~ /.well-known/acme-challenge/ {{\n{open}        root {ACME_ROOT};\n        try_files $uri @{up}_acme;\n    }}\n\
                 \x20   location @{up}_acme {{\n{open}        if ($http_x_forge_acme_hop) {{\n            return 404;\n        }}\n\
                 \x20       proxy_pass http://{up}_acme;\n        proxy_set_header Host $host;\n        proxy_set_header X-Forge-Acme-Hop 1;\n\
                 \x20       proxy_next_upstream error timeout http_404;\n        proxy_connect_timeout 3s;\n    }}\n"
            )
        } else {
            format!("    location ^~ /.well-known/acme-challenge/ {{\n{open}        root {ACME_ROOT};\n    }}\n")
        }
    }

    /// Renders the managed config (the read-only main configuration).
    /// `with_tls` adds the HTTPS server (the certificate must exist); without
    /// it only HTTP (+ ACME challenge) is served.
    #[must_use]
    pub fn render(&self, with_tls: bool) -> String {
        let names = self.domains.join(" ");
        let up = self.upstream_name();
        let common = format!(
            "{}    client_max_body_size {}m;\n    include {};\n",
            self.server_level(),
            self.max_body_mb,
            self.custom_path()
        );
        let acme = self.acme_locations();
        let http_body = if with_tls && self.force_https {
            let port = if self.https_port == 443 { String::new() } else { format!(":{}", self.https_port) };
            format!("    location / {{\n        return 301 https://$host{port}$request_uri;\n    }}\n")
        } else {
            format!("{common}{}", self.locations())
        };
        let mut out = format!(
            "# Managed by Orbyn Forge — endpoint `{name}`. Forge rewrites this file on every apply; put your own directives in {custom}.\n\
             map $http_upgrade ${up}_upgrade {{\n    default upgrade;\n    ''      close;\n}}\n",
            name = self.name,
            custom = self.custom_path(),
        );
        out.push_str(&self.log_formats());
        let logs = self.log_lines();
        out.push_str(&self.http_level());
        if self.tls == Tls::Acme && !self.acme_peers.is_empty() {
            let peers: String = self
                .acme_peers
                .iter()
                .map(|p| if p.contains(':') { format!("    server {p};\n") } else { format!("    server {p}:80;\n") })
                .collect();
            out.push_str(&format!("upstream {up}_acme {{\n{peers}}}\n\n"));
        }
        let hp = self.http_port;
        out.push_str(&format!(
            "server {{\n    listen {hp};\n    listen [::]:{hp};\n    server_name {names};\n{logs}{acme}{http_body}}}\n"
        ));
        if with_tls {
            let (crt, key) = self.cert_files();
            let sp = self.https_port;
            let hsts =
                if self.hsts { "    add_header Strict-Transport-Security \"max-age=31536000\" always;\n" } else { "" };
            // HTTP/2 uses the `listen ... ssl http2` form, not `http2 on;`:
            // the App Store installs the distro package (1.18 on Debian 11 /
            // Ubuntu 22.04, 1.20 on RHEL 9, 1.22 on Debian 12, 1.24 on
            // Ubuntu 24.04) and `http2 on;` needs 1.25.1. On 1.25.1+ the
            // listen flag only logs a deprecation warning and `nginx -t`
            // still passes, so one form serves every version without
            // plumbing the edge's Nginx version into rendering.
            out.push_str(&format!(
                "\nserver {{\n    listen {sp} ssl http2;\n    listen [::]:{sp} ssl http2;\n    server_name {names};\n{logs}\
                 \x20   ssl_certificate {crt};\n    ssl_certificate_key {key};\n\
                 \x20   ssl_protocols TLSv1.2 TLSv1.3;\n{hsts}{common}{loc}}}\n",
                loc = self.locations(),
            ));
        }
        out
    }

    /// The custom include file (always present, possibly empty).
    #[must_use]
    pub fn render_custom(&self) -> String {
        format!(
            "# Custom configuration for endpoint `{}` (edited in Orbyn Forge)\n{}\n",
            self.name,
            self.custom.trim_end()
        )
    }
}

impl ProxyOptions {
    /// The default rule cannot strip a prefix (it has none).
    fn validate_default(&self) -> Result<(), String> {
        if self.strip_prefix {
            return Err("the default rule has no prefix to strip".into());
        }
        self.validate()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn site() -> SiteSpec {
        serde_json::from_value(serde_json::json!({
            "name": "shop",
            "domains": ["shop.example.com", "www.shop.example.com"],
            "upstreams": ["10.0.0.11:8000"],
            "standby": ["10.0.0.12:8000"],
            "rate_limit": 10,
            "custom": "location /static/ { proxy_pass http://10.0.0.13:8080; }",
        }))
        .unwrap()
    }

    #[test]
    fn acme_challenges_fall_back_to_peer_edges() {
        let s = SiteSpec {
            tls: Tls::Acme,
            acme_email: "ops@example.com".into(),
            acme_peers: vec!["10.64.0.17".into(), "10.64.0.18:8080".into()],
            ..site()
        };
        s.validate().unwrap();
        let c = s.render(false);
        assert!(c.contains("try_files $uri @forge_shop_acme;"));
        assert!(c.contains("upstream forge_shop_acme {\n    server 10.64.0.17:80;\n    server 10.64.0.18:8080;\n}"));
        assert!(c.contains("if ($http_x_forge_acme_hop)"));
        let solo = SiteSpec { acme_peers: vec![], ..s.clone() };
        assert!(!solo.render(false).contains("_acme"));
        let bad = SiteSpec { acme_peers: vec!["x; evil".into()], ..s };
        assert!(bad.validate().is_err());
    }

    #[test]
    fn uploaded_certificates_use_forge_paths_and_hide_the_key() {
        let mut s = SiteSpec { tls: Tls::Uploaded, ..site() };
        assert!(s.validate().is_err(), "needs a certificate id");
        s.cert_id = "c1".into();
        s.validate().unwrap();
        let c = s.render(true);
        assert!(c.contains("ssl_certificate /etc/nginx/forge/tls/shop.crt;"));
        assert!(c.contains("ssl_certificate_key /etc/nginx/forge/tls/shop.key;"));
        s.material = Some(CertMaterial {
            chain_pem: "-----BEGIN CERTIFICATE-----".into(),
            key_pem: "-----BEGIN PRIVATE KEY----- secret".into(),
        });
        s.validate().unwrap();
        assert!(!format!("{s:?}").contains("secret"));
        let off = SiteSpec { tls: Tls::Off, ..s };
        assert!(off.validate().is_err(), "material only with an uploaded certificate");
    }

    #[test]
    fn weights_render_and_are_checked() {
        let mut s = site();
        s.upstreams = vec!["10.0.0.11:21000".into(), "10.0.0.11:21001".into()];
        s.weights = std::collections::BTreeMap::from([("10.0.0.11:21000".into(), 10), ("10.0.0.11:21001".into(), 100)]);
        s.validate().unwrap();
        let c = s.render(false);
        assert!(c.contains("server 10.0.0.11:21000 weight=10 max_fails=3"), "{c}");
        assert!(c.contains("server 10.0.0.11:21001 max_fails=3"), "100 is the default");
        s.weights.insert("10.9.9.9:1".into(), 5);
        assert!(s.validate().is_err(), "a weight for an upstream that is not there");
        s.weights = std::collections::BTreeMap::from([("10.0.0.11:21000".into(), 0)]);
        assert!(s.validate().is_err());
        // An agent that does not know weights still reads a site without them.
        assert!(!serde_json::to_string(&site()).unwrap().contains("weights"));
    }

    #[test]
    fn renders_upstreams_with_standby_and_custom_include() {
        let s = site();
        s.validate().unwrap();
        let c = s.render(false);
        assert!(c.contains("upstream forge_shop {"));
        assert!(c.contains("server 10.0.0.12:8000 backup;"));
        assert!(c.contains("server_name shop.example.com www.shop.example.com;"));
        assert!(c.contains("include /etc/nginx/forge/custom/shop.conf;"));
        assert!(c.contains("limit_req zone=forge_shop_rl"));
        assert!(!c.contains("listen 443"));
        assert!(!c.contains("$forge_connection_upgrade"));
        let t = SiteSpec { tls: Tls::Acme, acme_email: "ops@example.com".into(), ..s };
        t.validate().unwrap();
        let tc = t.render(true);
        assert!(
            tc.contains("listen 443 ssl http2;") && tc.contains("/etc/letsencrypt/live/shop.example.com/fullchain.pem")
        );
        assert!(tc.contains("return 301 https://"));
    }

    /// The TLS server must load on the distro Nginx the App Store installs
    /// (1.18 to 1.24), where `http2 on;` is an unknown directive.
    #[test]
    fn tls_server_uses_portable_http2_form() {
        let tc = SiteSpec { tls: Tls::Uploaded, cert_id: "c1".into(), ..site() }.render(true);
        assert!(tc.contains("    listen 443 ssl http2;\n    listen [::]:443 ssl http2;\n"));
        assert!(!tc.contains("http2 on"));
        assert!(tc.contains("proxy_set_header X-Forwarded-Host $host;"));
    }

    /// Golden render (F-12): every site writes the Forge traffic log in its
    /// own `server` blocks with a per-site `log_format`, so edge-based sites
    /// (no cluster profile) are observable and a profile's `forge_traffic`
    /// format is never defined twice (nginx refuses duplicate names).
    #[test]
    fn golden_http_site_with_traffic_log() {
        let s = SiteSpec { standby: vec![], rate_limit: 0, custom: String::new(), ..site() };
        s.validate().unwrap();
        let expected = "# Managed by Orbyn Forge — endpoint `shop`. Forge rewrites this file on every apply; put your own directives in /etc/nginx/forge/custom/shop.conf.\n\
map $http_upgrade $forge_shop_upgrade {\n    default upgrade;\n    ''      close;\n}\n\
log_format forge_shop_traffic '$msec $server_name $upstream_addr $request_length $bytes_sent $upstream_bytes_sent $upstream_bytes_received us=$upstream_status rt=$upstream_response_time';\n\
upstream forge_shop {\n    server 10.0.0.11:8000 max_fails=3 fail_timeout=10s;\n    keepalive 16;\n}\n\n\
server {\n    listen 80;\n    listen [::]:80;\n    server_name shop.example.com www.shop.example.com;\n\
\x20   access_log /var/log/nginx/forge-traffic.log forge_shop_traffic;\n\
\x20   access_log /var/log/nginx/forge/shop.access.log combined;\n\
\x20   error_log /var/log/nginx/forge/shop.error.log;\n\
\x20   location ^~ /.well-known/acme-challenge/ {\n        root /var/www/forge-acme;\n    }\n\
\x20   client_max_body_size 25m;\n    include /etc/nginx/forge/custom/shop.conf;\n\
\x20   location / {\n        proxy_pass http://forge_shop;\n        proxy_http_version 1.1;\n\
\x20       proxy_set_header Upgrade $http_upgrade;\n        proxy_set_header Connection $forge_shop_upgrade;\n\
\x20       proxy_set_header Host $host;\n        proxy_set_header X-Real-IP $remote_addr;\n\
\x20       proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;\n        proxy_set_header X-Forwarded-Proto $scheme;\n\
\x20       proxy_set_header X-Forwarded-Host $host;\n\
\x20       proxy_read_timeout 300s;\n        proxy_connect_timeout 5s;\n\
\x20       proxy_next_upstream error timeout http_502 http_503 http_504;\n    }\n}\n";
        assert_eq!(s.render(false), expected);
    }

    #[test]
    fn traffic_log_in_every_server_block_and_never_the_profile_format() {
        let s = SiteSpec { tls: Tls::Acme, acme_email: "ops@example.com".into(), ..site() };
        let c = s.render(true);
        // One definition, used by the HTTP and the HTTPS server.
        assert_eq!(c.matches("log_format forge_shop_traffic ").count(), 1);
        assert_eq!(c.matches("access_log /var/log/nginx/forge-traffic.log forge_shop_traffic;").count(), 2);
        // The profile's global config owns `forge_traffic`: never redefined here.
        assert!(!c.contains("log_format forge_traffic "));
        // Every log_format name in the file is unique (nginx -t would refuse).
        let names: Vec<&str> =
            c.lines().filter_map(|l| l.strip_prefix("log_format ")).filter_map(|l| l.split(' ').next()).collect();
        let mut dedup = names.clone();
        dedup.dedup();
        assert_eq!(names, dedup);
        // Formats sit at http level (before the first server block).
        assert!(c.find("log_format").unwrap() < c.find("server {").unwrap());
    }

    #[test]
    fn main_access_log_follows_the_profile_format() {
        let json = SiteSpec { access_log: "json".into(), ..site() };
        json.validate().unwrap();
        let c = json.render(false);
        assert!(c.contains("log_format forge_shop_json escape=json '{"));
        assert!(c.contains("access_log /var/log/nginx/forge/shop.access.log forge_shop_json;"));
        let off = SiteSpec { access_log: "off".into(), ..site() };
        let c = off.render(false);
        assert!(!c.contains("/var/log/nginx/forge/shop.access.log"));
        assert!(c.contains("forge-traffic.log forge_shop_traffic;"), "the traffic log stays on");
        assert!(SiteSpec { access_log: "x; evil".into(), ..site() }.validate().is_err());
        // Old stored specs (no field) still deserialize; empty is not serialized.
        let v = serde_json::to_value(site()).unwrap();
        assert!(v.get("access_log").is_none());
    }

    #[test]
    fn custom_blocks_refuse_file_process_and_code_directives() {
        for bad in [
            "log_format x '$http_x';\naccess_log /etc/cron.d/x x;",
            "access_log /etc/cron.d/x;",
            "error_log /root/x;",
            "location /a { alias /etc/; }",
            "root /;",
            "include /etc/shadow;",
            "\"access_log\" /etc/cron.d/x;",
            "'alias' /etc/;",
            "acc\\ess_log /x;\nalias /etc/;",
            "set_by_lua $x 'os.execute(1)';",
            "rewrite_by_lua_block { os.execute('id') }",
            "content_by_lua_file /tmp/x.lua;",
            "js_import /tmp/x.js;",
            "perl_modules /tmp;",
            "load_module /tmp/x.so;",
            "client_body_temp_path /etc/cron.d;",
            "proxy_cache_path /etc/x keys_zone=a:1m;",
            "ssl_certificate_key /etc/ssh/ssh_host_ed25519_key;",
            "proxy_ssl_trusted_certificate /etc/shadow;",
            "auth_basic_user_file /etc/shadow;",
            "ssl_password_file /etc/shadow;",
            "pid /etc/x;",
            "user root;",
            "env LD_PRELOAD=/tmp/x.so;",
            "worker_processes 1;",
            "dav_methods PUT;",
            "proxy_store on;",
            "location / { proxy_pass http://unix:/var/run/docker.sock:; }",
            "# comment\nadd_header X 1;\nlocation / {\n  root /etc;\n}",
            "add_header X \"a\\\"; root /etc;\";\nroot /;",
            "add_header X 1",
            "location / {",
            "}",
            "map $uri $x { default 0; include /etc/shadow; }",
            "some_module_directive on;",
            "location / { autoindex on; }",
            "location / { try_files $uri =404; }",
        ] {
            assert!(check_custom_block(bad).is_err(), "{bad}");
        }
        for ok in [
            "",
            "# just a comment",
            "add_header X-Test 1;",
            "location /api/ { proxy_pass http://10.0.0.5:8000; proxy_set_header X-A \"b;c\"; }",
            "add_header X-Q 'root /etc;';",
            "set $x ${host}; return 301 https://$host$request_uri;",
            "proxy_cookie_path / /app;\nclient_body_buffer_size 1m;",
            "if ($request_method = POST) { return 405; }",
            "map $uri $legacy { default 0; ~^/old/ 1; \"\" 0; }",
            "upstream b { least_conn; server 10.0.0.7:80 max_fails=2; keepalive 8; }",
        ] {
            check_custom_block(ok).unwrap_or_else(|e| panic!("{ok}: {e}"));
        }
    }

    #[test]
    fn rejects_bad_input() {
        let mut s = site();
        s.domains = vec!["bad domain".into()];
        assert!(s.validate().is_err());
        let mut s = site();
        s.upstreams = vec!["10.0.0.1; evil".into()];
        assert!(s.validate().is_err());
        let mut s = site();
        s.custom = "load_module /tmp/x.so;".into();
        assert!(s.validate().is_err());
        let mut s = site();
        s.tls = Tls::Acme;
        assert!(s.validate().is_err(), "email required");
    }

    fn spec(v: serde_json::Value) -> SiteSpec {
        let mut base = serde_json::json!({ "name": "shop", "domains": ["shop.example.com", "www.shop.example.com"] });
        if let (Some(b), Some(o)) = (base.as_object_mut(), v.as_object()) {
            for (k, x) in o {
                b.insert(k.clone(), x.clone());
            }
        }
        let s: SiteSpec = serde_json::from_value(base).unwrap();
        s.validate().unwrap_or_else(|e| panic!("{e}"));
        s
    }

    fn hash() -> String {
        format!("$6$saltsalt${}", "a".repeat(86))
    }

    /// Old rows (no kind, rules or features) read as a proxy and serialize
    /// without any of the new fields, so older agents still accept them.
    #[test]
    fn old_rows_default_to_a_proxy_and_round_trip() {
        let s = spec(serde_json::json!({ "upstreams": ["10.0.0.11:8000"] }));
        assert_eq!(s.kind, RuleKind::Proxy);
        let v = serde_json::to_value(&s).unwrap();
        for k in ["kind", "rules", "cache", "gzip", "access", "redirects", "http_port", "https_port", "hsts", "proxy"] {
            assert!(v.get(k).is_none(), "{k} serialized");
        }
    }

    #[test]
    fn golden_redirect_endpoint() {
        let s =
            spec(serde_json::json!({ "kind": "redirect", "redirect": { "to": "https://example.com/", "code": 308 } }));
        let c = s.render(false);
        assert!(c.contains("    location / {\n        return 308 https://example.com$request_uri;\n    }\n"), "{c}");
        assert!(!c.contains("upstream forge_shop "), "no upstream for a redirect");
        let no_path = spec(
            serde_json::json!({ "kind": "redirect", "redirect": { "to": "https://example.com/x", "keep_path": false } }),
        );
        assert!(no_path.render(false).contains("return 301 https://example.com/x;"));
        for bad in ["javascript:alert(1)", "https://x.com/$host", "https://x.com/a;b"] {
            let mut s = s.clone();
            s.redirect = Some(RedirectTo { to: bad.into(), code: 301, keep_path: true });
            assert!(s.validate().is_err(), "{bad}");
        }
        let mut s2 = s.clone();
        s2.redirect = Some(RedirectTo { to: "https://example.com".into(), code: 200, keep_path: true });
        assert!(s2.validate().is_err(), "code");
    }

    #[test]
    fn golden_static_endpoint() {
        let s = spec(serde_json::json!({ "kind": "static", "static_root": { "root": "/srv/www/shop", "spa": true } }));
        assert_eq!(s.static_roots(), vec!["/srv/www/shop"]);
        let c = s.render(false);
        assert!(
            c.contains("    location / {\n        root /srv/www/shop;\n        index index.html;\n        disable_symlinks if_not_owner from=$document_root;\n        if ($uri ~ \"/\\.\") {\n            return 404;\n        }\n        try_files $uri $uri/ /index.html;\n    }\n"),
            "{c}"
        );
        let plain = spec(serde_json::json!({ "kind": "static", "static_root": { "root": "/srv/www/shop" } }));
        assert!(plain.render(false).contains("try_files $uri $uri/ =404;"));
        let mut bad = plain.clone();
        bad.static_root = Some(StaticRoot { root: "/srv/../etc".into(), index: "index.html".into(), spa: false });
        assert!(bad.validate().is_err());
        bad.static_root = Some(StaticRoot { root: "/srv/x".into(), index: "../x".into(), spa: false });
        assert!(bad.validate().is_err());
    }

    #[test]
    fn golden_path_rules() {
        let s = spec(serde_json::json!({
            "upstreams": ["10.0.0.11:8000"],
            "rules": [
                { "path": "/api/", "kind": "balancer", "upstreams": ["10.0.0.21:9000", "10.0.0.22:9000"], "balance": "least_conn",
                  "websockets": false, "proxy": { "strip_prefix": true, "preserve_host": false, "buffering": false, "read_timeout_s": 30 } },
                { "path": "/health", "match_kind": "exact", "kind": "redirect", "redirect": { "to": "https://status.example.com" } },
                { "path": "\\.(png|jpg)$", "match_kind": "regex", "kind": "static", "static_root": { "root": "/srv/img" } },
            ],
        }));
        let c = s.render(false);
        assert!(
            c.contains(
                "upstream forge_shop_r1 {\n    least_conn;\n    server 10.0.0.21:9000 max_fails=3 fail_timeout=10s;\n"
            ),
            "{c}"
        );
        assert!(c.contains("    location /api/ {\n        proxy_pass http://forge_shop_r1/;\n        proxy_http_version 1.1;\n        proxy_set_header Host $proxy_host;\n"), "{c}");
        assert!(c.contains("        proxy_read_timeout 30s;\n") && c.contains("        proxy_buffering off;\n"));
        assert!(
            c.contains("    location = /health {\n        return 301 https://status.example.com$request_uri;\n    }\n")
        );
        assert!(c.contains("    location ~ \"\\.(png|jpg)$\" {\n        root /srv/img;\n"));
        // Rules come before the default route.
        assert!(c.find("location /api/").unwrap() < c.find("location / {").unwrap());
        assert_eq!(s.static_roots(), vec!["/srv/img"]);
        // A rule for `/`, a strip on an exact rule and duplicate rules are refused.
        let mut bad = s.clone();
        bad.rules[0].path = "/".into();
        assert!(bad.validate().is_err());
        let mut bad = s.clone();
        bad.rules[0].proxy.strip_prefix = true;
        bad.rules[0].match_kind = MatchKind::Exact;
        assert!(bad.validate().is_err());
        let mut bad = s.clone();
        bad.rules.push(bad.rules[0].clone());
        assert!(bad.validate().is_err());
        let mut bad = s;
        bad.rules[2].path = "x\" { root /;".into();
        assert!(bad.validate().is_err());
    }

    #[test]
    fn golden_cache_and_gzip() {
        let s = spec(serde_json::json!({
            "upstreams": ["10.0.0.11:8000"],
            "cache": { "extensions": ["css", "js"], "ttl_minutes": 30, "ignore_headers": true, "max_size_mb": 512 },
            "gzip": { "level": 6, "types": ["text/css", "application/json"] },
        }));
        let c = s.render(false);
        assert!(c.contains("proxy_cache_path /var/cache/nginx/forge/shop levels=1:2 keys_zone=forge_shop_cache:10m max_size=512m inactive=30m use_temp_path=off;\n"), "{c}");
        assert!(c.contains("map $uri $forge_shop_nocache {\n    default 1;\n    \"~*\\.(css|js)$\" 0;\n}\n"));
        assert!(c.contains("        proxy_cache forge_shop_cache;\n        proxy_cache_key \"$scheme$request_method$host$request_uri\";\n        proxy_cache_valid 200 301 302 30m;\n"));
        assert!(c.contains("        proxy_ignore_headers Cache-Control Expires Set-Cookie;\n        proxy_no_cache $forge_shop_nocache;\n"));
        assert!(c.contains("    gzip on;\n    gzip_comp_level 6;\n    gzip_types text/css application/json;\n    gzip_proxied any;\n    gzip_vary on;\n"));
        let mut bad = s.clone();
        bad.cache = Some(CacheSpec { key: "$host; root /".into(), ..s.cache.clone().unwrap() });
        assert!(bad.validate().is_err());
        let mut bad = s.clone();
        bad.gzip = Some(GzipSpec { level: 6, types: vec!["text/css;".into()] });
        assert!(bad.validate().is_err());
    }

    #[test]
    fn golden_access_controls() {
        let s = spec(serde_json::json!({
            "upstreams": ["10.0.0.11:8000"],
            "tls": "acme", "acme_email": "ops@example.com",
            "rate_limit": 5, "rate_burst": 20,
            "access": {
                "deny": ["192.0.2.66"], "allow": ["192.0.2.0/24", "2001:db8::/32"],
                "basic_auth": { "realm": "Staff only", "users": [{ "user": "ops", "hash": hash() }] },
                "hotlink": { "referers": ["partner.example.org"], "extensions": ["png", "jpg"] },
            },
        }));
        let c = s.render(false);
        assert!(
            c.contains("    deny 192.0.2.66;\n    allow 192.0.2.0/24;\n    allow 2001:db8::/32;\n    deny all;\n"),
            "{c}"
        );
        assert!(
            c.contains("    auth_basic \"Staff only\";\n    auth_basic_user_file /etc/nginx/forge/htpasswd/shop;\n")
        );
        assert!(c.contains("map $uri $forge_shop_hotlink {\n    default 0;\n    \"~*\\.(png|jpg)$\" 1;\n}\n"));
        assert!(c.contains("    valid_referers none blocked server_names partner.example.org;\n    set $forge_shop_hl \"${forge_shop_hotlink}${invalid_referer}\";\n    if ($forge_shop_hl = 11) {\n        return 403;\n    }\n"));
        assert!(c.contains("limit_req zone=forge_shop_rl burst=20 nodelay;"));
        // Let's Encrypt still reaches the challenge.
        assert!(
            c.contains("    location ^~ /.well-known/acme-challenge/ {\n        allow all;\n        auth_basic off;\n")
        );
        assert_eq!(s.access.htpasswd().unwrap(), format!("ops:{}\n", hash()));
        for bad in [
            serde_json::json!({ "allow": ["all"] }),
            serde_json::json!({ "deny": ["10.0.0.0/40"] }),
            serde_json::json!({ "basic_auth": { "users": [{ "user": "ops" }] } }),
            serde_json::json!({ "basic_auth": { "users": [{ "user": "o:ps", "hash": hash() }] } }),
            serde_json::json!({ "basic_auth": { "realm": "a\"b", "users": [{ "user": "ops", "hash": hash() }] } }),
            serde_json::json!({ "hotlink": { "extensions": [] } }),
            serde_json::json!({ "hotlink": { "extensions": ["png"], "referers": ["~.*"] } }),
        ] {
            let mut s = s.clone();
            s.access = serde_json::from_value(bad.clone()).unwrap();
            assert!(s.validate().is_err(), "{bad}");
        }
    }

    #[test]
    fn golden_tls_options_ports_and_redirects() {
        let s = spec(serde_json::json!({
            "upstreams": ["10.0.0.11:8000"],
            "tls": "uploaded", "cert_id": "c1", "hsts": false, "http_port": 8080, "https_port": 8443,
            "redirects": [
                { "from": "www.shop.example.com", "to": "https://shop.example.com" },
                { "from": "/old", "to": "https://shop.example.com/new", "code": 302 },
            ],
        }));
        let c = s.render(true);
        assert!(c.contains("    listen 8080;\n    listen [::]:8080;\n"));
        assert!(c.contains("    listen 8443 ssl http2;\n"));
        assert!(c.contains("return 301 https://$host:8443$request_uri;"));
        assert!(!c.contains("Strict-Transport-Security"));
        assert!(c.contains("    if ($host = www.shop.example.com) {\n        return 301 https://shop.example.com$request_uri;\n    }\n"), "{c}");
        assert!(
            c.contains("    location = /old {\n        return 302 https://shop.example.com/new$is_args$args;\n    }\n")
        );
        let mut bad = s.clone();
        bad.redirects[0].from = "other.example.com".into();
        assert!(bad.validate().is_err(), "from must be one of the domains");
        let mut bad = s.clone();
        bad.tls = Tls::Acme;
        bad.acme_email = "ops@example.com".into();
        bad.cert_id.clear();
        assert!(bad.validate().is_err(), "ACME needs port 80");
        let mut bad = s;
        bad.https_port = 8080;
        assert!(bad.validate().is_err());
    }

    #[test]
    fn custom_errors_name_line_and_directive() {
        let e = check_custom_block("add_header X 1;\n\n  root /etc;\n").unwrap_err();
        assert_eq!((e.line, e.directive.as_deref()), (3, Some("root")));
        assert_eq!(e.to_string(), "line 3: `root` is not allowed here (see the list of supported directives)");
        let e = check_custom_block("add_header X 1;\nlocation / {\n").unwrap_err();
        assert!(e.directive.is_none());
        let e = check_custom_block("a 1;\nproxy_pass http://unix:/x;").unwrap_err();
        assert_eq!(e.line, 2);
    }

    #[test]
    fn custom_config_may_point_only_at_its_own_managed_files() {
        let s = spec(serde_json::json!({ "upstreams": ["10.0.0.11:8000"] }));
        let with = |custom: &str| SiteSpec { custom: custom.into(), ..s.clone() };
        for ok in [
            "auth_basic \"x\";\nauth_basic_user_file /etc/nginx/forge/htpasswd/shop;",
            "location /x/ { access_log /var/log/nginx/forge/shop.access.log combined; }",
            "error_log /var/log/nginx/forge/shop.error.log warn;",
            "location /c/ { proxy_cache forge_shop_cache; proxy_cache_valid 200 5m; }",
        ] {
            with(ok).validate().unwrap_or_else(|e| panic!("{ok}: {e}"));
        }
        for bad in [
            "auth_basic_user_file /etc/shadow;",
            "auth_basic_user_file /etc/nginx/forge/htpasswd/other;",
            "access_log /var/log/nginx/forge/other.access.log;",
            "access_log /var/log/nginx/forge/shop.access.log other_format;",
            "error_log /etc/cron.d/x;",
            "proxy_cache_path /etc/x keys_zone=a:1m;",
        ] {
            assert!(with(bad).validate().is_err(), "{bad}");
        }
        // The profile's global block knows no endpoint files.
        assert!(check_custom_block("auth_basic_user_file /etc/nginx/forge/htpasswd/shop;").is_err());
    }
}
