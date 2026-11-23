//! Health policies: startup, readiness and liveness checks of one
//! service (issue #12, D-085).
//!
//! One typed model drives the console's form, its YAML view, the API and
//! the node's probe executor. [`HealthPolicy::validate`] is the
//! authoritative check; the console mirrors it for inline errors.
//!
//! Security rules (D-085):
//! - HTTP and TCP probes target the workload itself by default (`host`
//!   empty = 127.0.0.1 for native apps, the container's own address for
//!   containers). Any other host must be set explicitly with
//!   `allow_remote_host`, which needs `health.configure`; link-local and
//!   cloud metadata addresses are always refused. Hosts are re-checked on
//!   the node when the probe connects.
//! - Exec and shell probes run as the app's own account (native apps,
//!   D-081) or inside the container: never as root or the node helper.
//! - Secret header values are vault references (`vault:<id>`), resolved on
//!   the node at probe time; plain values of sensitive headers are refused.

use std::collections::HashSet;
use std::net::IpAddr;

use serde::{Deserialize, Serialize};

/// The app command action that carries an [`ApplySpec`] to a node.
pub const APPLY_ACTION: &str = "health";
/// Most checks per purpose.
pub const MAX_CHECKS: usize = 10;
/// Longest shell script, in bytes.
pub const MAX_SCRIPT: usize = 16 * 1024;
/// Most exec arguments.
pub const MAX_ARGS: usize = 64;
/// Most HTTP headers per check.
pub const MAX_HEADERS: usize = 20;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HealthPolicy {
    /// Forge's default checks (D-085): liveness = the systemd unit is active
    /// (plus a TCP connect to the app's port, if it declares one) or the
    /// container is running (plus its image HEALTHCHECK, else TCP on the
    /// published port); readiness = liveness unless readiness checks are
    /// configured. On unless turned off.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub default_checks: bool,
    /// Gate readiness and liveness until the instance started.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub startup: Vec<Check>,
    /// Can the instance take traffic now? All required checks must pass.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub readiness: Vec<Check>,
    /// Is the instance working rather than stuck? Report-only in v1.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub liveness: Vec<Check>,
}

impl Default for HealthPolicy {
    fn default() -> Self {
        Self { default_checks: true, startup: Vec::new(), readiness: Vec::new(), liveness: Vec::new() }
    }
}

#[allow(clippy::trivially_copy_pass_by_ref)] // serde's skip_serializing_if passes a reference
const fn is_true(b: &bool) -> bool {
    *b
}

/// Which section a check is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Purpose {
    Startup,
    Readiness,
    Liveness,
}

impl Purpose {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Purpose::Startup => "startup",
            Purpose::Readiness => "readiness",
            Purpose::Liveness => "liveness",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Check {
    /// Lowercase letters, digits and dashes; unique per purpose.
    pub name: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Readiness only: an optional check that fails shows "degraded"
    /// instead of making the instance unready.
    #[serde(default = "yes")]
    pub required: bool,
    pub probe: Probe,
    #[serde(default = "ten")]
    pub period_secs: u32,
    #[serde(default = "two")]
    pub timeout_secs: u32,
    #[serde(default)]
    pub initial_delay_secs: u32,
    /// Consecutive failures before the state turns failing.
    #[serde(default = "three")]
    pub failure_threshold: u32,
    /// Consecutive successes before it turns passing (startup and
    /// liveness: always 1).
    #[serde(default = "one")]
    pub success_threshold: u32,
}

const fn yes() -> bool {
    true
}
const fn ten() -> u32 {
    10
}
const fn two() -> u32 {
    2
}
const fn three() -> u32 {
    3
}
const fn one() -> u32 {
    1
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Probe {
    Http(HttpProbe),
    Tcp(TcpProbe),
    /// A program and its arguments, no shell.
    Exec(ExecProbe),
    /// A script run by an explicit interpreter.
    Shell(ShellProbe),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scheme {
    #[default]
    Http,
    Https,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Method {
    #[default]
    Get,
    Head,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpProbe {
    #[serde(default)]
    pub scheme: Scheme,
    /// Empty: the workload itself.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub host: String,
    /// Required to probe any host other than the workload.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub allow_remote_host: bool,
    pub port: u16,
    #[serde(default = "root_path")]
    pub path: String,
    #[serde(default)]
    pub method: Method,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<Header>,
    /// Accepted status codes as ranges; default 200–299.
    #[serde(default = "ok_statuses")]
    pub expected_statuses: Vec<StatusRange>,
    /// HTTPS: verify the certificate and host name (default on).
    #[serde(default = "yes")]
    pub tls_verify: bool,
    /// HTTPS: the name to verify and send as SNI, when it differs from `host`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub server_name: String,
}

fn root_path() -> String {
    "/".into()
}

fn ok_statuses() -> Vec<StatusRange> {
    vec![StatusRange { from: 200, to: 299 }]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatusRange {
    pub from: u16,
    pub to: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Header {
    pub name: String,
    /// A plain value; refused for sensitive headers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// A vault reference, `vault:<id>`, resolved on the node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TcpProbe {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub host: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub allow_remote_host: bool,
    pub port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecProbe {
    /// Program first, then its arguments. Never passed to a shell.
    pub argv: Vec<String>,
    #[serde(default = "zero_exit")]
    pub expected_exit_codes: Vec<i32>,
    /// Report the end of a failed run's output with the result. Off by
    /// default: a command can print secrets, and the detail is stored and
    /// shown in the console.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub show_output: bool,
}

fn zero_exit() -> Vec<i32> {
    vec![0]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Interpreter {
    #[default]
    Sh,
    Bash,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShellProbe {
    #[serde(default)]
    pub interpreter: Interpreter,
    pub script: String,
    #[serde(default = "zero_exit")]
    pub expected_exit_codes: Vec<i32>,
    /// Report the end of a failed run's output with the result (see
    /// [`ExecProbe::show_output`]).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub show_output: bool,
}

impl HttpProbe {
    /// Whether a header takes its value from the vault.
    #[must_use]
    pub fn has_secret(&self) -> bool {
        self.headers.iter().any(|h| h.secret.is_some())
    }

    /// Whether the target is the workload itself by its spelling: no host,
    /// `localhost`, or a loopback address.
    #[must_use]
    pub fn targets_workload(&self) -> bool {
        workload_host(&self.host)
    }

    /// HTTPS with certificate verification: the only way a vault value may
    /// leave the node.
    #[must_use]
    pub fn verified_tls(&self) -> bool {
        self.scheme == Scheme::Https && self.tls_verify
    }
}

/// No host, `localhost`, or a loopback address in any spelling.
#[must_use]
pub fn workload_host(host: &str) -> bool {
    host.is_empty() || host == "localhost" || host.parse::<IpAddr>().is_ok_and(|ip| canonical(ip).is_loopback())
}

/// One problem, with the field it belongs to (`readiness[0].probe.port`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldError {
    pub field: String,
    pub message: String,
}

impl Probe {
    /// Exec and shell probes run code on the node.
    #[must_use]
    pub const fn runs_code(&self) -> bool {
        matches!(self, Probe::Exec(_) | Probe::Shell(_))
    }

    /// The explicit remote host, if the probe targets one.
    #[must_use]
    pub fn remote_host(&self) -> Option<&str> {
        match self {
            Probe::Http(h) if !h.host.is_empty() => Some(&h.host),
            Probe::Tcp(t) if !t.host.is_empty() => Some(&t.host),
            _ => None,
        }
    }
}

impl HealthPolicy {
    #[must_use]
    /// Nothing configured and the default checks on (the stored form is
    /// then no policy at all).
    pub fn is_empty(&self) -> bool {
        self.default_checks && self.startup.is_empty() && self.readiness.is_empty() && self.liveness.is_empty()
    }

    /// Every check with its purpose.
    pub fn checks(&self) -> impl Iterator<Item = (Purpose, &Check)> {
        self.startup
            .iter()
            .map(|c| (Purpose::Startup, c))
            .chain(self.readiness.iter().map(|c| (Purpose::Readiness, c)))
            .chain(self.liveness.iter().map(|c| (Purpose::Liveness, c)))
    }

    /// Whether any check runs code (exec or shell).
    #[must_use]
    pub fn runs_code(&self) -> bool {
        self.checks().any(|(_, c)| c.probe.runs_code())
    }

    /// Every problem, empty when valid.
    #[must_use]
    pub fn validate(&self) -> Vec<FieldError> {
        let mut errs = Vec::new();
        for (purpose, list) in [
            (Purpose::Startup, &self.startup),
            (Purpose::Readiness, &self.readiness),
            (Purpose::Liveness, &self.liveness),
        ] {
            if list.len() > MAX_CHECKS {
                errs.push(err(purpose.as_str(), format!("at most {MAX_CHECKS} checks")));
            }
            let mut names = HashSet::new();
            for (i, c) in list.iter().enumerate() {
                let at = format!("{}[{i}]", purpose.as_str());
                if !names.insert(c.name.as_str()) {
                    errs.push(err(&format!("{at}.name"), format!("`{}` is used twice", c.name)));
                }
                c.validate(purpose, &at, &mut errs);
            }
        }
        errs
    }
}

fn err(field: &str, message: impl Into<String>) -> FieldError {
    FieldError { field: field.into(), message: message.into() }
}

/// A check name: lowercase letters, digits and dashes, at most 40.
#[must_use]
pub fn valid_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 40
        && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !s.starts_with('-')
        && !s.ends_with('-')
}

/// Header names that carry credentials: plain values are refused.
fn sensitive_header(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    ["authorization", "proxy-authorization", "cookie"].contains(&n.as_str())
        || ["token", "secret", "key", "password", "auth"].iter().any(|w| n.contains(w))
}

fn valid_vault_ref(s: &str) -> bool {
    s.strip_prefix("vault:").is_some_and(|id| {
        !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    })
}

/// Addresses no probe may target: link-local (cloud metadata lives there),
/// unspecified, multicast and broadcast.
#[must_use]
pub fn forbidden_address(ip: IpAddr) -> bool {
    match canonical(ip) {
        IpAddr::V4(v4) => v4.is_link_local() || v4.is_unspecified() || v4.is_multicast() || v4.is_broadcast(),
        IpAddr::V6(v6) => {
            let seg = v6.segments();
            // fe80::/10 link-local; fd00:ec2::254 is the AWS IPv6 metadata endpoint.
            (seg[0] & 0xffc0) == 0xfe80
                || v6.is_unspecified()
                || v6.is_multicast()
                || v6 == std::net::Ipv6Addr::new(0xfd00, 0xec2, 0, 0, 0, 0, 0, 0x254)
        }
    }
}

/// IPv6 prefixes whose addresses are delivered to an IPv4 destination the
/// address itself names, through a relay or translator outside Forge's
/// view: 6to4 (`2002::/16`), Teredo (`2001::/32`), local-use NAT64
/// (`64:ff9b:1::/48`) and IPv4-translated (`::ffff:0:0:0/96`).
/// `2002:a9fe:a9fe::1` is one way to write 169.254.169.254. A caller that
/// must not reach internal IPv4 addresses refuses these outright.
#[must_use]
pub fn transition_address(ip: IpAddr) -> bool {
    let IpAddr::V6(v6) = ip else { return false };
    let s = v6.segments();
    s[0] == 0x2002
        || (s[0] == 0x2001 && s[1] == 0)
        || (s[0] == 0x64 && s[1] == 0xff9b && s[2] == 1)
        || (s[..4] == [0, 0, 0, 0] && s[4] == 0xffff && s[5] == 0)
}

/// The IPv4 address an IPv6 address stands for, if it embeds one:
/// IPv4-mapped (`::ffff:a.b.c.d`), IPv4-compatible (`::a.b.c.d`, not `::` or
/// `::1`) and NAT64 (`64:ff9b::/96`). Checks run on that IPv4 address, so
/// `::a9fe:a9fe` is refused like `169.254.169.254`.
#[must_use]
pub fn canonical(ip: IpAddr) -> IpAddr {
    let IpAddr::V6(v6) = ip else { return ip };
    if let Some(v4) = v6.to_ipv4_mapped() {
        return IpAddr::V4(v4);
    }
    let seg = v6.segments();
    let low =
        std::net::Ipv4Addr::new((seg[6] >> 8) as u8, (seg[6] & 0xff) as u8, (seg[7] >> 8) as u8, (seg[7] & 0xff) as u8);
    let compatible = seg[..6].iter().all(|s| *s == 0) && u32::from(low) > 1;
    let nat64 = seg[0] == 0x64 && seg[1] == 0xff9b && seg[2..6].iter().all(|s| *s == 0);
    if compatible || nat64 { IpAddr::V4(low) } else { ip }
}

/// A host that isn't an IP literal but whose last label is all digits or
/// hex (`0x…`): resolvers may read it as an encoded IPv4 address
/// (`2852039166`, `0xa9fea9fe`, `0251.0376.0251.0376` for 169.254.169.254).
#[must_use]
pub fn numeric_host(host: &str) -> bool {
    if host.parse::<IpAddr>().is_ok() {
        return false;
    }
    let last = host.trim_end_matches('.').rsplit('.').next().unwrap_or_default().to_ascii_lowercase();
    !last.is_empty() && (last.bytes().all(|b| b.is_ascii_digit()) || last.starts_with("0x"))
}
/// Host names that resolve to metadata services.
#[must_use]
pub fn forbidden_name(host: &str) -> bool {
    let h = host.trim_end_matches('.').to_ascii_lowercase();
    ["metadata", "metadata.google.internal", "metadata.goog", "instance-data", "instance-data.ec2.internal"]
        .contains(&h.as_str())
}

fn valid_host(host: &str) -> bool {
    if host.parse::<IpAddr>().is_ok() {
        return true;
    }
    !host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                && !l.starts_with('-')
                && !l.ends_with('-')
        })
}

fn check_host(host: &str, allow_remote: bool, at: &str, errs: &mut Vec<FieldError>) {
    if host.is_empty() {
        if allow_remote {
            errs.push(err(&format!("{at}.allow_remote_host"), "set a host, or turn this off to probe the workload"));
        }
        return;
    }
    if !valid_host(host) {
        errs.push(err(&format!("{at}.host"), "not a valid host name or IP address"));
        return;
    }
    if numeric_host(host) {
        errs.push(err(&format!("{at}.host"), "write an IP address in its normal form, or use a host name"));
        return;
    }
    let local = workload_host(host);
    if host.parse::<IpAddr>().is_ok_and(forbidden_address) {
        errs.push(err(&format!("{at}.host"), "link-local and metadata addresses can't be probed"));
        return;
    }
    // 6to4, Teredo and local-use NAT64 name an IPv4 destination a relay
    // delivers to: `2002:a9fe:a9fe::1` is 169.254.169.254.
    if host.parse::<IpAddr>().is_ok_and(transition_address) {
        errs.push(err(
            &format!("{at}.host"),
            "IPv6 transition addresses (6to4, Teredo, NAT64) can't be probed; use the IPv4 address",
        ));
        return;
    }
    if forbidden_name(host) {
        errs.push(err(&format!("{at}.host"), "metadata service names can't be probed"));
        return;
    }
    if !local && !allow_remote {
        errs.push(err(
            &format!("{at}.allow_remote_host"),
            "probing a host other than the workload must be allowed explicitly",
        ));
    }
}

fn check_exit_codes(codes: &[i32], at: &str, errs: &mut Vec<FieldError>) {
    if codes.is_empty() || codes.len() > 16 || codes.iter().any(|c| !(0..=255).contains(c)) {
        errs.push(err(&format!("{at}.expected_exit_codes"), "1 to 16 exit codes between 0 and 255"));
    }
}

impl Check {
    fn validate(&self, purpose: Purpose, at: &str, errs: &mut Vec<FieldError>) {
        if !valid_name(&self.name) {
            errs.push(err(&format!("{at}.name"), "lowercase letters, digits and dashes, at most 40"));
        }
        if !(1..=3600).contains(&self.period_secs) {
            errs.push(err(&format!("{at}.period_secs"), "between 1 and 3600 seconds"));
        }
        if self.timeout_secs == 0 || self.timeout_secs >= self.period_secs.max(1) {
            errs.push(err(&format!("{at}.timeout_secs"), "at least 1 second and shorter than the period"));
        }
        if self.initial_delay_secs > 3600 {
            errs.push(err(&format!("{at}.initial_delay_secs"), "at most 3600 seconds"));
        }
        if !(1..=100).contains(&self.failure_threshold) {
            errs.push(err(&format!("{at}.failure_threshold"), "between 1 and 100"));
        }
        let max_success = if purpose == Purpose::Readiness { 100 } else { 1 };
        if !(1..=max_success).contains(&self.success_threshold) {
            errs.push(err(
                &format!("{at}.success_threshold"),
                if max_success == 1 { "must be 1 for startup and liveness checks" } else { "between 1 and 100" },
            ));
        }
        if !self.required && purpose != Purpose::Readiness {
            errs.push(err(&format!("{at}.required"), "only readiness checks can be optional"));
        }
        let p = format!("{at}.probe");
        match &self.probe {
            Probe::Http(h) => {
                check_host(&h.host, h.allow_remote_host, &p, errs);
                if h.port == 0 {
                    errs.push(err(&format!("{p}.port"), "between 1 and 65535"));
                }
                if !h.path.starts_with('/') || h.path.len() > 2048 || h.path.chars().any(char::is_control) {
                    errs.push(err(
                        &format!("{p}.path"),
                        "starts with `/`, at most 2048 characters, no control characters",
                    ));
                }
                if h.expected_statuses.is_empty()
                    || h.expected_statuses.len() > 10
                    || h.expected_statuses.iter().any(|r| r.from < 100 || r.to > 599 || r.from > r.to)
                {
                    errs.push(err(&format!("{p}.expected_statuses"), "1 to 10 ranges within 100–599"));
                }
                if !h.server_name.is_empty() && !valid_host(&h.server_name) {
                    errs.push(err(&format!("{p}.server_name"), "not a valid host name"));
                }
                // A vault value goes in clear text, or to a peer nobody
                // verified, only to the workload itself.
                if h.has_secret() && !h.targets_workload() && !h.verified_tls() {
                    let field = if h.scheme == Scheme::Https { "tls_verify" } else { "scheme" };
                    errs.push(err(
                        &format!("{p}.{field}"),
                        "a vault secret is sent to another host only over HTTPS with certificate verification",
                    ));
                }
                if h.headers.len() > MAX_HEADERS {
                    errs.push(err(&format!("{p}.headers"), format!("at most {MAX_HEADERS} headers")));
                }
                for (j, hd) in h.headers.iter().enumerate() {
                    let hp = format!("{p}.headers[{j}]");
                    let token = |b: u8| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b);
                    if hd.name.is_empty() || hd.name.len() > 64 || !hd.name.bytes().all(token) {
                        errs.push(err(&format!("{hp}.name"), "not a valid header name"));
                    }
                    match (&hd.value, &hd.secret) {
                        (Some(_), Some(_)) | (None, None) => {
                            errs.push(err(&hp, "set either a value or a vault secret"));
                        }
                        (Some(v), None) => {
                            if sensitive_header(&hd.name) {
                                errs.push(err(
                                    &format!("{hp}.value"),
                                    "credentials must be a vault secret, not a plain value",
                                ));
                            }
                            if v.len() > 1024 || v.chars().any(char::is_control) {
                                errs.push(err(
                                    &format!("{hp}.value"),
                                    "at most 1024 characters, no control characters",
                                ));
                            }
                        }
                        (None, Some(s)) => {
                            if !valid_vault_ref(s) {
                                errs.push(err(&format!("{hp}.secret"), "a vault reference, `vault:<id>`"));
                            }
                        }
                    }
                }
            }
            Probe::Tcp(t) => {
                check_host(&t.host, t.allow_remote_host, &p, errs);
                if t.port == 0 {
                    errs.push(err(&format!("{p}.port"), "between 1 and 65535"));
                }
            }
            Probe::Exec(e) => {
                if e.argv.is_empty() || e.argv.len() > MAX_ARGS {
                    errs.push(err(&format!("{p}.argv"), format!("a program and up to {} arguments", MAX_ARGS - 1)));
                } else if e.argv[0].trim().is_empty() {
                    errs.push(err(&format!("{p}.argv"), "the program can't be empty"));
                }
                if e.argv.iter().any(|a| a.len() > 4096 || a.contains('\0')) {
                    errs.push(err(&format!("{p}.argv"), "arguments: at most 4096 bytes, no NUL"));
                }
                check_exit_codes(&e.expected_exit_codes, &p, errs);
            }
            Probe::Shell(s) => {
                if s.script.trim().is_empty() || s.script.len() > MAX_SCRIPT || s.script.contains('\0') {
                    errs.push(err(&format!("{p}.script"), format!("1 to {MAX_SCRIPT} bytes, no NUL")));
                }
                check_exit_codes(&s.expected_exit_codes, &p, errs);
            }
        }
    }
}

/// A plain-language summary of a check, from the same typed state the
/// console edits ("Check GET /health/ready on port 8080 every 10 s…").
#[must_use]
pub fn summary(purpose: Purpose, c: &Check) -> String {
    let what = match &c.probe {
        Probe::Http(h) => {
            let host = if h.host.is_empty() { "the workload".to_owned() } else { h.host.clone() };
            format!(
                "Check {} {}://{host}:{}{}",
                match h.method {
                    Method::Get => "GET",
                    Method::Head => "HEAD",
                },
                match h.scheme {
                    Scheme::Http => "http",
                    Scheme::Https => "https",
                },
                h.port,
                h.path
            )
        }
        Probe::Tcp(t) => {
            let host = if t.host.is_empty() { "the workload" } else { &t.host };
            format!("Open a TCP connection to {host} port {}", t.port)
        }
        Probe::Exec(e) => format!("Run `{}` as the app's account", e.argv.first().map_or("", String::as_str)),
        Probe::Shell(s) => format!(
            "Run a {} script as the app's account",
            match s.interpreter {
                Interpreter::Sh => "sh",
                Interpreter::Bash => "bash",
            }
        ),
    };
    let n = |k: u32, one: &str, many: &str| format!("{k} {}", if k == 1 { one } else { many });
    let delay =
        if c.initial_delay_secs > 0 { format!(", starting after {} s", c.initial_delay_secs) } else { String::new() };
    let fail = match purpose {
        Purpose::Startup => format!(
            "Give up after {} (the startup budget); readiness and liveness wait until it succeeds",
            n(c.failure_threshold, "failure", "failures")
        ),
        Purpose::Readiness => format!(
            "Mark unready after {}; ready again after {}",
            n(c.failure_threshold, "failure", "consecutive failures"),
            n(c.success_threshold, "success", "successes")
        ),
        Purpose::Liveness => {
            format!(
                "Report not live after {}. Do not restart",
                n(c.failure_threshold, "failure", "consecutive failures")
            )
        }
    };
    format!("{what} every {} s (timeout {} s){delay}. {fail}.", c.period_secs, c.timeout_secs)
}

/// What a node needs to run a service's checks: sent with the signed
/// app command `health` when the policy changes without a redeploy, and
/// again when a node reports results under another revision. No
/// environment, no secrets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplySpec {
    pub environment: String,
    pub name: String,
    pub runtime: crate::apps::Runtime,
    #[serde(default)]
    pub listen_port: Option<u16>,
    #[serde(default)]
    pub policy: HealthPolicy,
    /// [`revision`] of `listen_port` and `policy`.
    pub revision: i64,
    /// Addresses of the fleet's other nodes when the policy was sent, at
    /// most [`MAX_FLEET_ADDRESSES`]: a probe to a remote host is refused on
    /// the node when the host resolves to one of them. Not part of the
    /// revision, so a fleet change alone does not send the policy again.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fleet: Vec<IpAddr>,
    /// The copy the checks run against (#73): `a`/`b` for a slot, empty for
    /// the single instance. `listen_port` and the policy's ports are the
    /// slot's already. Not part of the revision.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub slot: String,
}

/// Most fleet addresses sent with a policy ([`ApplySpec::fleet`]).
pub const MAX_FLEET_ADDRESSES: usize = 2048;

/// Identifies what a node checks a service with. `0` stands for "no
/// policy, default checks only", which is also what a node reports right
/// after a deploy; every other policy gets a positive number from its
/// content (FNV-1a over the JSON form), so the same policy always has the
/// same revision.
#[must_use]
pub fn revision(listen_port: Option<u16>, policy: &HealthPolicy) -> i64 {
    if policy.is_empty() {
        return 0;
    }
    let text = serde_json::to_string(&(listen_port, policy)).unwrap_or_default();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    i64::try_from(h >> 1).unwrap_or(i64::MAX).max(1)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;
