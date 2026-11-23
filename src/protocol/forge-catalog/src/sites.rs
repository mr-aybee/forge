//! Reverse-proxy sites (Nginx) shared by the control plane and the node helper.
//!
//! The helper renders the managed config from a typed [`SiteSpec`]; only the
//! optional *custom block* is free text. It is kept in its own include file
//! (`/etc/nginx/forge/custom/<site>.conf`), checked with `nginx -t`, and
//! rolled back automatically if the test fails.

use serde::{Deserialize, Serialize};

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
    /// Active upstreams `host:port`. May be left out when the API resolves
    /// them from typed targets; [`SiteSpec::validate`] still needs one.
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
    #[serde(default = "yes")]
    pub websockets: bool,
    /// `client_max_body_size` in MB.
    #[serde(default = "body")]
    pub max_body_mb: u32,
    /// Requests per second per client IP (0 = unlimited).
    #[serde(default)]
    pub rate_limit: u32,
    /// Free-form directives included inside the `server` block(s).
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
const fn body() -> u32 {
    25
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

/// Directive names of a free nginx block, read the way nginx's own parser
/// (`ngx_conf_read_token`) splits words, so quoting, escapes and comments
/// cannot hide a directive. Errors on input nginx itself would refuse.
/// Returns each statement's name with the block it sits in.
fn directive_names(text: &str) -> Result<Vec<(String, Option<String>)>, String> {
    fn unescape(raw: &[char]) -> String {
        let mut out = String::new();
        let mut i = 0;
        while i < raw.len() {
            if raw[i] == '\\' && i + 1 < raw.len() {
                match raw[i + 1] {
                    '"' | '\'' | '\\' => {
                        out.push(raw[i + 1]);
                        i += 2;
                        continue;
                    }
                    't' => {
                        out.push('\t');
                        i += 2;
                        continue;
                    }
                    'r' => {
                        out.push('\r');
                        i += 2;
                        continue;
                    }
                    'n' => {
                        out.push('\n');
                        i += 2;
                        continue;
                    }
                    _ => {}
                }
            }
            out.push(raw[i]);
            i += 1;
        }
        out
    }
    let chars: Vec<char> = text.chars().collect();
    let mut names = Vec::new();
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
                _ => return Err(format!("unexpected `{ch}` in the custom block")),
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
                        return Err(format!("unexpected `{ch}` in the custom block"));
                    }
                    words = 0;
                    if ch == '{' {
                        stack.push(std::mem::take(&mut cur));
                    }
                    continue;
                }
                '}' => {
                    if words != 0 {
                        return Err("unexpected `}` in the custom block".into());
                    }
                    if stack.pop().is_none() {
                        return Err("unbalanced `}` in the custom block".into());
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
                names.push((word.clone(), stack.last().cloned()));
                cur = word;
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
        return Err("the custom block ends in the middle of a directive (missing `;`?)".into());
    }
    if !stack.is_empty() {
        return Err("unbalanced `{` in the custom block".into());
    }
    Ok(names)
}

/// Checks a free nginx block (a site's custom block or the cluster profile's
/// global block) against [`ALLOWED`], and refuses Unix sockets (a local socket
/// such as Docker's is root on the node). `nginx -t` still checks the syntax;
/// saving or applying a non-empty block also needs `server.command` (MFA).
pub fn check_custom_block(text: &str) -> Result<(), String> {
    if text.len() > 64 * 1024 || text.contains('\0') {
        return Err("custom block too large".into());
    }
    if text.to_ascii_lowercase().contains("unix:") {
        return Err("Unix sockets are not allowed in custom blocks".into());
    }
    for (name, parent) in directive_names(text)? {
        let ok = if parent.as_deref().is_some_and(|p| DATA_BLOCKS.contains(&p)) {
            name != "include"
        } else {
            ALLOWED.contains(&name.as_str())
        };
        if !ok {
            return Err(format!("`{name}` is not allowed in custom blocks (see the list of supported directives)"));
        }
    }
    Ok(())
}

fn valid_domain(d: &str) -> bool {
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
        if self.upstreams.is_empty() {
            return Err("at least one upstream (host:port) is required".into());
        }
        if let Some(u) = self.upstreams.iter().chain(&self.standby).find(|u| !valid_upstream(u)) {
            return Err(format!("invalid upstream `{u}` (use host:port)"));
        }
        for (u, w) in &self.weights {
            if !self.upstreams.contains(u) {
                return Err(format!("weight for `{u}`, which is not an active upstream"));
            }
            if *w == 0 || *w > 1000 {
                return Err(format!("the weight of `{u}` must be 1–1000"));
            }
        }
        if self.tls == Tls::Acme {
            if self.domains.iter().any(|d| d.starts_with("*.")) {
                return Err("wildcard domains need DNS-01; use TLS off or explicit names".into());
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
        if !matches!(self.access_log.as_str(), "" | "combined" | "json" | "off") {
            return Err("access_log must be combined, json or off".into());
        }
        check_custom_block(&self.custom)
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

    /// `access_log` lines for each `server` block. Server-level `access_log`
    /// replaces the inherited `http {}` ones, so each request is written once
    /// to the traffic log (also on edges with a profile) and once to the main
    /// log in the profile's format.
    fn log_lines(&self) -> String {
        let up = self.upstream_name();
        let mut o = format!("    access_log {} {up}_traffic;\n", crate::ingress::TRAFFIC_LOG);
        match self.access_log.as_str() {
            "off" => {}
            "json" => o.push_str(&format!("    access_log /var/log/nginx/access.log {up}_json;\n")),
            _ => o.push_str("    access_log /var/log/nginx/access.log combined;\n"),
        }
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

    fn upstream_name(&self) -> String {
        format!("forge_{}", self.name.replace('-', "_"))
    }

    fn locations(&self) -> String {
        let ws = if self.websockets {
            "        proxy_set_header Upgrade $http_upgrade;\n        proxy_set_header Connection $forge_connection_upgrade;\n"
        } else {
            ""
        };
        let limit = if self.rate_limit > 0 {
            format!("        limit_req zone={}_rl burst={} nodelay;\n", self.upstream_name(), self.rate_limit * 2)
        } else {
            String::new()
        };
        format!(
            "    location / {{\n{limit}        proxy_pass http://{up};\n        proxy_http_version 1.1;\n{ws}\
             \x20       proxy_set_header Host $host;\n        proxy_set_header X-Real-IP $remote_addr;\n\
             \x20       proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;\n        proxy_set_header X-Forwarded-Proto $scheme;\n\
             \x20       proxy_read_timeout 300s;\n        proxy_connect_timeout 5s;\n\
             \x20       proxy_next_upstream error timeout http_502 http_503 http_504;\n    }}\n",
            up = self.upstream_name(),
        )
    }

    /// Renders the managed config. `with_tls` adds the HTTPS server (the
    /// certificate must exist); without it only HTTP (+ ACME challenge) is served.
    #[must_use]
    pub fn render(&self, with_tls: bool) -> String {
        let names = self.domains.join(" ");
        let up = self.upstream_name();
        let mut servers = String::new();
        for u in &self.upstreams {
            let weight = self.weights.get(u).filter(|w| **w != 100).map(|w| format!(" weight={w}")).unwrap_or_default();
            servers.push_str(&format!("    server {u}{weight} max_fails=3 fail_timeout=10s;\n"));
        }
        for u in &self.standby {
            servers.push_str(&format!("    server {u} backup;\n"));
        }
        let balance = match self.balance {
            Balance::RoundRobin => "",
            Balance::LeastConn => "    least_conn;\n",
            Balance::IpHash => "    ip_hash;\n",
        };
        let rate = if self.rate_limit > 0 {
            format!("limit_req_zone $binary_remote_addr zone={up}_rl:10m rate={}r/s;\n", self.rate_limit)
        } else {
            String::new()
        };
        let common = format!("    client_max_body_size {}m;\n    include {};\n", self.max_body_mb, self.custom_path());
        let acme = if self.tls == Tls::Acme && !self.acme_peers.is_empty() {
            // Not here: ask the peers once (the hop header stops loops); a
            // peer answering 404 passes to the next one.
            format!(
                "    location ^~ /.well-known/acme-challenge/ {{\n        root {ACME_ROOT};\n        try_files $uri @{up}_acme;\n    }}\n\
                 \x20   location @{up}_acme {{\n        if ($http_x_forge_acme_hop) {{\n            return 404;\n        }}\n\
                 \x20       proxy_pass http://{up}_acme;\n        proxy_set_header Host $host;\n        proxy_set_header X-Forge-Acme-Hop 1;\n\
                 \x20       proxy_next_upstream error timeout http_404;\n        proxy_connect_timeout 3s;\n    }}\n"
            )
        } else {
            format!("    location ^~ /.well-known/acme-challenge/ {{\n        root {ACME_ROOT};\n    }}\n")
        };
        let http_body = if with_tls && self.force_https {
            "    location / {\n        return 301 https://$host$request_uri;\n    }\n".to_owned()
        } else {
            format!("{common}{}", self.locations())
        };
        let mut out = format!(
            "# Managed by Orbyn Forge — site `{name}`. Edit the custom block in the console instead.\n\
             map $http_upgrade $forge_connection_upgrade {{\n    default upgrade;\n    ''      close;\n}}\n",
            name = self.name
        );
        // `map` must be unique per http block: name it per site.
        out = out.replace("$forge_connection_upgrade", &format!("${up}_upgrade"));
        out.push_str(&self.log_formats());
        let logs = self.log_lines();
        out.push_str(&format!("{rate}upstream {up} {{\n{balance}{servers}    keepalive 16;\n}}\n\n"));
        if self.tls == Tls::Acme && !self.acme_peers.is_empty() {
            let peers: String = self
                .acme_peers
                .iter()
                .map(|p| if p.contains(':') { format!("    server {p};\n") } else { format!("    server {p}:80;\n") })
                .collect();
            out.push_str(&format!("upstream {up}_acme {{\n{peers}}}\n\n"));
        }
        out.push_str(&format!(
            "server {{\n    listen 80;\n    listen [::]:80;\n    server_name {names};\n{logs}{acme}{http_body}}}\n"
        ));
        if with_tls {
            let (crt, key) = self.cert_files();
            out.push_str(&format!(
                "\nserver {{\n    listen 443 ssl;\n    listen [::]:443 ssl;\n    http2 on;\n    server_name {names};\n{logs}\
                 \x20   ssl_certificate {crt};\n    ssl_certificate_key {key};\n\
                 \x20   ssl_protocols TLSv1.2 TLSv1.3;\n    add_header Strict-Transport-Security \"max-age=31536000\" always;\n\
                 {common}{loc}}}\n",
                loc = self.locations().replace("$forge_connection_upgrade", &format!("${up}_upgrade")),
            ));
        }
        out.replace("$forge_connection_upgrade", &format!("${up}_upgrade"))
    }

    /// The custom include file (always present, possibly empty).
    #[must_use]
    pub fn render_custom(&self) -> String {
        format!("# Custom directives for site `{}` (edited in Orbyn Forge)\n{}\n", self.name, self.custom.trim_end())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn site() -> SiteSpec {
        SiteSpec {
            weights: std::collections::BTreeMap::new(),
            name: "shop".into(),
            domains: vec!["shop.example.com".into(), "www.shop.example.com".into()],
            upstreams: vec!["10.0.0.11:8000".into()],
            standby: vec!["10.0.0.12:8000".into()],
            balance: Balance::RoundRobin,
            tls: Tls::Off,
            acme_email: String::new(),
            cert_id: String::new(),
            material: None,
            acme_peers: vec![],
            force_https: true,
            websockets: true,
            max_body_mb: 25,
            rate_limit: 10,
            custom: "location /static/ { proxy_pass http://10.0.0.13:8080; }".into(),
            access_log: String::new(),
        }
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
        assert!(tc.contains("listen 443 ssl;") && tc.contains("/etc/letsencrypt/live/shop.example.com/fullchain.pem"));
        assert!(tc.contains("return 301 https://"));
    }

    /// Golden render (F-12): every site writes the Forge traffic log in its
    /// own `server` blocks with a per-site `log_format`, so edge-based sites
    /// (no cluster profile) are observable and a profile's `forge_traffic`
    /// format is never defined twice (nginx refuses duplicate names).
    #[test]
    fn golden_http_site_with_traffic_log() {
        let s = SiteSpec { standby: vec![], rate_limit: 0, custom: String::new(), ..site() };
        s.validate().unwrap();
        let expected = "# Managed by Orbyn Forge — site `shop`. Edit the custom block in the console instead.\n\
map $http_upgrade $forge_shop_upgrade {\n    default upgrade;\n    ''      close;\n}\n\
log_format forge_shop_traffic '$msec $server_name $upstream_addr $request_length $bytes_sent $upstream_bytes_sent $upstream_bytes_received us=$upstream_status rt=$upstream_response_time';\n\
upstream forge_shop {\n    server 10.0.0.11:8000 max_fails=3 fail_timeout=10s;\n    keepalive 16;\n}\n\n\
server {\n    listen 80;\n    listen [::]:80;\n    server_name shop.example.com www.shop.example.com;\n\
\x20   access_log /var/log/nginx/forge-traffic.log forge_shop_traffic;\n\
\x20   access_log /var/log/nginx/access.log combined;\n\
\x20   location ^~ /.well-known/acme-challenge/ {\n        root /var/www/forge-acme;\n    }\n\
\x20   client_max_body_size 25m;\n    include /etc/nginx/forge/custom/shop.conf;\n\
\x20   location / {\n        proxy_pass http://forge_shop;\n        proxy_http_version 1.1;\n\
\x20       proxy_set_header Upgrade $http_upgrade;\n        proxy_set_header Connection $forge_shop_upgrade;\n\
\x20       proxy_set_header Host $host;\n        proxy_set_header X-Real-IP $remote_addr;\n\
\x20       proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;\n        proxy_set_header X-Forwarded-Proto $scheme;\n\
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
        assert!(c.contains("access_log /var/log/nginx/access.log forge_shop_json;"));
        let off = SiteSpec { access_log: "off".into(), ..site() };
        let c = off.render(false);
        assert!(!c.contains("/var/log/nginx/access.log"));
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
}
