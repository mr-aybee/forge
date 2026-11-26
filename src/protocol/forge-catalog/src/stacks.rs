//! Compose stacks (issue #7): the security policy, port binding, placement,
//! per-node split and the operations the node helper runs.
//!
//! The control plane parses a compose file (YAML) into a JSON tree and works
//! on that tree only; the helper receives the tree for its node inside a
//! signed command, checks it again with the same [`check`] and writes it as
//! YAML with [`to_yaml`]. The helper therefore needs no YAML parser.
//!
//! Interpolation: Docker Compose substitutes `${VAR}` from the project's
//! `.env` file and the process environment. The helper runs `docker compose`
//! with a cleared environment and only the stack's `.env`, and the policy
//! checks the tree *after* [`interpolate`] with that same `.env`, so the
//! values Compose sees are the values that were checked.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

/// Largest compose document accepted (bytes of YAML or JSON).
pub const MAX_COMPOSE_BYTES: usize = 512 * 1024;
/// Most services in one stack.
pub const MAX_SERVICES: usize = 100;
/// Largest `.env` file.
pub const MAX_ENV_BYTES: usize = 64 * 1024;
/// Where stacks live on a node: `<STACKS_DIR>/<environment>/<stack>/`.
pub const STACKS_DIR: &str = "/opt/forge/stacks";
/// Runtime of the services Forge registers for stack services.
pub const STACK_RUNTIME: &str = "compose-service";
/// Label set on every service Forge deploys: `<environment>/<stack>`.
pub const STACK_LABEL: &str = "forge.stack";

/// Host paths a bind mount may never reach, even for privileged stacks
/// (only the Docker socket is allowed with `stack.privileged`).
const DOCKER_SOCKETS: &[&str] = &["/var/run/docker.sock", "/run/docker.sock", "/run/containerd/containerd.sock"];
/// Host paths that make a bind mount privileged even inside an approved root.
const SENSITIVE: &[&str] = &[
    "/",
    "/etc",
    "/root",
    "/boot",
    "/proc",
    "/sys",
    "/dev",
    "/run",
    "/var/run",
    "/var/lib/docker",
    "/var/lib/containerd",
    "/var/lib/forge-agent",
    "/etc/forge",
    "/usr",
    "/bin",
    "/sbin",
    "/lib",
    "/lib64",
];

/// What the checker allows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Policy {
    /// The caller holds `stack.privileged` (MFA): privileged settings pass.
    #[serde(default)]
    pub allow_privileged: bool,
    /// Absolute host directories bind mounts may use (approved file roots).
    #[serde(default)]
    pub bind_roots: Vec<String>,
    /// The stack's own directory on the node (relative binds land here).
    pub stack_dir: String,
}

/// How bad a finding is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Never allowed (`build:`, `extends`, `include`, paths escaping the stack).
    Blocked,
    /// Allowed only with `stack.privileged`.
    Privileged,
    /// For information (for example a port moved to an internal address).
    Info,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    /// Service name, or empty for the top level.
    pub service: String,
    pub severity: Severity,
    pub message: String,
}

/// The result of [`check`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    pub findings: Vec<Finding>,
    /// Some finding is `Privileged` (needs `stack.privileged`).
    pub privileged: bool,
}

impl Report {
    /// Findings that stop a deployment under `policy`.
    #[must_use]
    pub fn blocking(&self, allow_privileged: bool) -> Vec<&Finding> {
        self.findings
            .iter()
            .filter(|f| match f.severity {
                Severity::Blocked => true,
                Severity::Privileged => !allow_privileged,
                Severity::Info => false,
            })
            .collect()
    }

    /// `Err` with every blocking reason, one per line.
    pub fn ensure(&self, allow_privileged: bool) -> Result<(), String> {
        let b = self.blocking(allow_privileged);
        if b.is_empty() {
            return Ok(());
        }

        Err(b.iter().map(|f| describe(f)).collect::<Vec<_>>().join("\n"))
    }

    /// Adds a finding; a `Privileged` one marks the report privileged.
    pub fn push(&mut self, service: &str, severity: Severity, message: impl Into<String>) {
        if severity == Severity::Privileged {
            self.privileged = true;
        }

        self.findings.push(Finding { service: service.to_owned(), severity, message: message.into() });
    }
}

fn describe(f: &Finding) -> String {
    let what = match f.severity {
        Severity::Blocked => "not allowed",
        Severity::Privileged => "needs stack.privileged",
        Severity::Info => "note",
    };

    if f.service.is_empty() {
        format!("{what}: {}", f.message)
    } else {
        format!("{}: {what}: {}", f.service, f.message)
    }
}

// ---- names ----------------------------------------------------------------------

/// Stack and environment names on disk and in project names: `[a-z0-9][a-z0-9_-]{0,62}`.
#[must_use]
pub fn valid_stack_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 63
        && (s.as_bytes()[0].is_ascii_lowercase() || s.as_bytes()[0].is_ascii_digit())
        && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

/// Compose service names: `[a-zA-Z0-9][a-zA-Z0-9_.-]{0,62}`.
#[must_use]
pub fn valid_service_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 63
        && s.as_bytes()[0].is_ascii_alphanumeric()
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}

/// The Compose project name of a stack: `forge-<environment>-<stack>`.
#[must_use]
pub fn project_name(environment: &str, stack: &str) -> String {
    format!("forge-{environment}-{stack}")
}

/// Valid project names (as built by [`project_name`]).
#[must_use]
pub fn valid_project(s: &str) -> bool {
    s.starts_with("forge-")
        && s.len() <= 140
        && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

/// The default directory of a stack on a node.
#[must_use]
pub fn default_dir(environment: &str, stack: &str) -> String {
    format!("{STACKS_DIR}/{environment}/{stack}")
}

// ---- paths ----------------------------------------------------------------------

/// Normalizes an absolute POSIX path (`//`, `.`); `None` for `..` or relative paths.
#[must_use]
pub fn normalize_abs(p: &str) -> Option<String> {
    if !p.starts_with('/') || p.contains('\0') {
        return None;
    }

    let mut out = Vec::new();

    for c in p.split('/') {
        match c {
            "" | "." => {}
            ".." => return None,
            c => out.push(c),
        }
    }

    Some(format!("/{}", out.join("/")))
}

/// `child` equals `base` or lies inside it (both normalized).
#[must_use]
pub fn within(child: &str, base: &str) -> bool {
    base == "/" || child == base || child.strip_prefix(base).is_some_and(|r| r.starts_with('/'))
}

/// `path` (normalized) is `/` or lies in a system folder (`/etc`, `/var/lib/docker`, …).
#[must_use]
pub fn is_system_path(path: &str) -> bool {
    SENSITIVE.iter().any(|s| if *s == "/" { path == "/" } else { within(path, s) })
}

/// Forge's own folders on a node (stack folders hold other stacks' `.env`).
const FORGE_PATHS: &[&str] = &[STACKS_DIR, "/opt/forge", "/var/lib/forge", "/var/lib/forge-agent", "/etc/forge"];

/// Why an absolute host path may never be mounted into a service
/// container, or `None`: the Docker sockets, system paths and Forge's own
/// folders. `path` is normalized first; `..` and relative paths are refused.
#[must_use]
pub fn protected_host_path(path: &str) -> Option<&'static str> {
    let Some(p) = normalize_abs(path) else { return Some("the path must be absolute without `..`") };
    if DOCKER_SOCKETS.iter().any(|s| within(&p, s)) {
        Some("it gives control of Docker on the node")
    } else if is_system_path(&p) {
        Some("it is a system path")
    } else if FORGE_PATHS.iter().any(|s| within(&p, s)) {
        Some("it holds Forge's own files")
    } else {
        None
    }
}

/// Resolves a bind source relative to the stack directory.
fn resolve_source(src: &str, stack_dir: &str) -> Option<String> {
    if src.starts_with('/') {
        return normalize_abs(src);
    }

    let rel = src.strip_prefix("./").unwrap_or(src);
    if rel == "." {
        return normalize_abs(stack_dir);
    }

    normalize_abs(&format!("{stack_dir}/{rel}"))
}

/// Is this short-syntax volume source a host path (rather than a named volume)?
fn is_path_source(src: &str) -> bool {
    src.starts_with('/') || src.starts_with('.') || src.starts_with('~')
}

/// Checks one host path used by a bind mount, `env_file`, config or secret.
fn check_host_path(r: &mut Report, svc: &str, what: &str, src: &str, p: &Policy) {
    if src.starts_with('~') {
        r.push(svc, Severity::Blocked, format!("{what} `{src}`: home-relative paths are not allowed"));
        return;
    }

    let Some(path) = resolve_source(src, &p.stack_dir) else {
        r.push(svc, Severity::Blocked, format!("{what} `{src}`: the path must not contain `..`"));
        return;
    };

    if DOCKER_SOCKETS.iter().any(|s| within(&path, s)) {
        r.push(svc, Severity::Privileged, format!("{what} `{path}` gives control of Docker on the node"));
        return;
    }

    // System paths first: a stack folder inside an approved root such as
    // `/var` must not make `./` reach `/var/lib/docker`.
    if is_system_path(&path) {
        r.push(svc, Severity::Privileged, format!("{what} `{path}` is a system path"));
        return;
    }

    let stack_dir = normalize_abs(&p.stack_dir).unwrap_or_default();
    if !stack_dir.is_empty() && within(&path, &stack_dir) {
        return;
    }

    let roots: Vec<String> = p.bind_roots.iter().filter_map(|b| normalize_abs(b)).collect();
    if !roots.iter().any(|b| within(&path, b)) {
        r.push(
            svc,
            Severity::Privileged,
            format!("{what} `{path}` is outside the stack folder and the approved file roots"),
        );
    }
}

/// A host directory or file mounted into a container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bind {
    /// Service name, or empty for a top-level volume's `device`.
    pub service: String,
    /// Absolute, normalized host path.
    pub path: String,
    /// Mounted read-write (containers can create links in it).
    pub writable: bool,
}

/// Bind mounts of the tree: bind sources of services and `device` of
/// bind-type volumes, resolved against the stack folder.
#[must_use]
pub fn binds(tree: &Value, stack_dir: &str) -> Vec<Bind> {
    let mut out = Vec::new();

    for (name, svc) in tree.get("services").and_then(Value::as_object).into_iter().flatten() {
        for v in svc.get("volumes").and_then(Value::as_array).into_iter().flatten() {
            let (src, writable) = match v {
                Value::String(s) if s.contains(':') => {
                    let mut parts = s.split(':');
                    let src = parts.next().filter(|s| is_path_source(s));
                    let opts = parts.nth(1).unwrap_or_default();
                    (src, !opts.split(',').any(|o| matches!(o, "ro" | "readonly")))
                }
                Value::Object(m) if m.get("type").and_then(Value::as_str) == Some("bind") => {
                    (m.get("source").and_then(Value::as_str), !truthy(m.get("read_only")))
                }
                _ => (None, false),
            };

            if let Some(path) = src.and_then(|s| resolve_source(s, stack_dir)) {
                out.push(Bind { service: name.clone(), path, writable });
            }
        }
    }

    for (_, v) in tree.get("volumes").and_then(Value::as_object).into_iter().flatten() {
        let Some(path) =
            v.pointer("/driver_opts/device").and_then(Value::as_str).and_then(|d| resolve_source(d, stack_dir))
        else {
            continue;
        };

        let o = v.pointer("/driver_opts/o").and_then(Value::as_str).unwrap_or_default();
        out.push(Bind { service: String::new(), path, writable: !o.split(',').any(|x| x == "ro") });
    }

    out
}

/// Host files Compose reads (as root) on the node: `env_file`,
/// `label_file` and the `file` of configs and secrets.
#[must_use]
pub fn host_files(tree: &Value, stack_dir: &str) -> Vec<String> {
    let mut out = Vec::new();

    for svc in tree.get("services").and_then(Value::as_object).into_iter().flatten().map(|(_, s)| s) {
        for key in ["env_file", "label_file"] {
            out.extend(env_files(svc.get(key)).iter().filter_map(|f| resolve_source(f, stack_dir)));
        }
    }

    for section in ["configs", "secrets"] {
        for (_, v) in tree.get(section).and_then(Value::as_object).into_iter().flatten() {
            out.extend(v.get("file").and_then(Value::as_str).and_then(|f| resolve_source(f, stack_dir)));
        }
    }

    out
}

/// Host paths the tree mounts or reads ([`binds`] then [`host_files`]),
/// resolved against the stack folder. The node re-checks them for links.
#[must_use]
pub fn host_paths(tree: &Value, stack_dir: &str) -> Vec<String> {
    let mut out: Vec<String> = binds(tree, stack_dir).into_iter().map(|b| b.path).collect();
    out.extend(host_files(tree, stack_dir));

    out
}

/// Replaces `.env` values in text shown to users (finding messages, port
/// lists, errors) with `${KEY}`. Values of secret-looking keys are always
/// replaced; other values from three characters on.
#[must_use]
pub fn redact(text: &str, env: &BTreeMap<String, String>) -> String {
    let secretish = |k: &str| {
        let k = k.to_ascii_uppercase();
        ["PASS", "SECRET", "TOKEN", "KEY", "CREDENTIAL", "PRIVATE", "AUTH", "PWD"].iter().any(|s| k.contains(s))
    };
    let mut vals: Vec<(&String, &String)> =
        env.iter().filter(|(k, v)| !v.is_empty() && (v.chars().count() >= 3 || secretish(k))).collect();

    // Longest first, so a value inside another is not replaced first.
    vals.sort_by_key(|(_, v)| std::cmp::Reverse(v.len()));
    let mut out = text.to_owned();

    for (k, v) in vals {
        if out.contains(v.as_str()) {
            out = out.replace(v.as_str(), &format!("${{{k}}}"));
        }
    }

    out
}

/// [`redact`] on every string of a JSON value (keys included).
#[must_use]
pub fn redact_value(v: &Value, env: &BTreeMap<String, String>) -> Value {
    match v {
        Value::String(s) => Value::String(redact(s, env)),
        Value::Array(a) => Value::Array(a.iter().map(|x| redact_value(x, env)).collect()),
        Value::Object(m) => Value::Object(m.iter().map(|(k, x)| (redact(k, env), redact_value(x, env))).collect()),
        other => other.clone(),
    }
}

/// Checks one absolute host path (for example after resolving symlinks).
#[must_use]
pub fn check_path(path: &str, p: &Policy) -> Report {
    let mut r = Report::default();
    check_host_path(&mut r, "", "host path", path, p);

    r
}

// ---- interpolation ----------------------------------------------------------------

/// Parses a `.env` file (`KEY=value`, `#` comments, optional quotes,
/// `export KEY=value`). Keys that steer Compose itself (`COMPOSE_*`,
/// `DOCKER_*`) are refused.
pub fn parse_env(text: &str) -> Result<BTreeMap<String, String>, String> {
    if text.len() > MAX_ENV_BYTES {
        return Err(format!("the .env file is larger than {} KiB", MAX_ENV_BYTES / 1024));
    }

    let mut out = BTreeMap::new();

    for (n, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let line = line.strip_prefix("export ").unwrap_or(line);
        let (k, v) = line.split_once('=').ok_or_else(|| format!(".env line {}: expected KEY=value", n + 1))?;
        let k = k.trim();
        if k.is_empty()
            || k.as_bytes()[0].is_ascii_digit()
            || !k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            return Err(format!(".env line {}: `{k}` is not a valid variable name", n + 1));
        }

        let upper = k.to_ascii_uppercase();
        if upper.starts_with("COMPOSE_") || upper.starts_with("DOCKER_") {
            return Err(format!(".env line {}: `{k}` changes how Compose runs and is not allowed", n + 1));
        }

        let v = v.trim();
        let v = if v.len() >= 2
            && ((v.starts_with('"') && v.ends_with('"')) || (v.starts_with('\'') && v.ends_with('\'')))
        {
            &v[1..v.len() - 1]
        } else {
            // Unquoted: an inline ` #` starts a comment.
            v.split(" #").next().unwrap_or(v).trim_end()
        };

        out.insert(k.to_owned(), v.to_owned());
    }

    Ok(out)
}

/// Renders `.env` content from variables (values double-quoted).
#[must_use]
pub fn render_env(vars: &BTreeMap<String, String>) -> String {
    vars.iter().map(|(k, v)| format!("{k}={}\n", serde_json::to_string(v).unwrap_or_default())).collect()
}

/// Substitutes `${VAR}`, `${VAR:-d}`, `${VAR-d}`, `${VAR:?e}`, `${VAR?e}`,
/// `${VAR:+r}`, `${VAR+r}`, `$VAR` and `$$` the way Compose does, in every
/// string of the tree (keys included). Unknown variables become empty.
pub fn interpolate(tree: &Value, env: &BTreeMap<String, String>) -> Result<Value, String> {
    Ok(match tree {
        Value::String(s) => Value::String(interpolate_str(s, env)?),
        Value::Array(a) => Value::Array(a.iter().map(|v| interpolate(v, env)).collect::<Result<_, _>>()?),
        Value::Object(m) => {
            let mut out = Map::new();

            for (k, v) in m {
                out.insert(interpolate_str(k, env)?, interpolate(v, env)?);
            }

            Value::Object(out)
        }
        v => v.clone(),
    })
}

fn interpolate_str(s: &str, env: &BTreeMap<String, String>) -> Result<String, String> {
    if !s.contains('$') {
        return Ok(s.to_owned());
    }

    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;

    while i < b.len() {
        if b[i] != b'$' {
            let ch = s[i..].chars().next().unwrap_or_default();
            out.push(ch);
            i += ch.len_utf8();

            continue;
        }

        match b.get(i + 1) {
            Some(b'$') => {
                out.push('$');
                i += 2;
            }
            Some(b'{') => {
                // Find the matching `}` (nesting allowed in defaults).
                let mut depth = 0usize;
                let mut end = None;

                for (j, &c) in b.iter().enumerate().skip(i + 1) {
                    match c {
                        b'{' => depth += 1,
                        b'}' => {
                            depth -= 1;

                            if depth == 0 {
                                end = Some(j);
                                break;
                            }
                        }
                        _ => {}
                    }
                }

                let end = end.ok_or_else(|| format!("unclosed `${{` in `{s}`"))?;
                out.push_str(&expand(&s[i + 2..end], env)?);
                i = end + 1;
            }
            Some(c) if c.is_ascii_alphabetic() || *c == b'_' => {
                let start = i + 1;
                let mut j = start;

                while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                    j += 1;
                }

                out.push_str(env.get(&s[start..j]).map_or("", String::as_str));
                i = j;
            }
            _ => {
                out.push('$');
                i += 1;
            }
        }
    }

    Ok(out)
}

fn expand(inner: &str, env: &BTreeMap<String, String>) -> Result<String, String> {
    let name_end = inner.bytes().position(|c| !(c.is_ascii_alphanumeric() || c == b'_')).unwrap_or(inner.len());
    let (name, rest) = inner.split_at(name_end);
    if name.is_empty() {
        return Err(format!("invalid variable `${{{inner}}}`"));
    }

    let val = env.get(name);
    let set = val.is_some();
    let nonempty = val.is_some_and(|v| !v.is_empty());
    let v = val.cloned().unwrap_or_default();

    let (op, arg) = if let Some(a) = rest.strip_prefix(":-") {
        (":-", a)
    } else if let Some(a) = rest.strip_prefix(":?") {
        (":?", a)
    } else if let Some(a) = rest.strip_prefix(":+") {
        (":+", a)
    } else if let Some(a) = rest.strip_prefix('-') {
        ("-", a)
    } else if let Some(a) = rest.strip_prefix('?') {
        ("?", a)
    } else if let Some(a) = rest.strip_prefix('+') {
        ("+", a)
    } else if rest.is_empty() {
        ("", "")
    } else {
        return Err(format!("invalid variable `${{{inner}}}`"));
    };

    let arg = || interpolate_str(arg, env);

    Ok(match op {
        ":-" => {
            if nonempty {
                v
            } else {
                arg()?
            }
        }
        "-" => {
            if set {
                v
            } else {
                arg()?
            }
        }
        ":?" if !nonempty => return Err(format!("variable {name} is required: {}", arg()?)),
        "?" if !set => return Err(format!("variable {name} is required: {}", arg()?)),
        ":+" => {
            if nonempty {
                arg()?
            } else {
                String::new()
            }
        }
        "+" => {
            if set {
                arg()?
            } else {
                String::new()
            }
        }
        _ => v,
    })
}

// ---- the policy ---------------------------------------------------------------------

/// Service keys that can never be used.
const BLOCKED_KEYS: &[(&str, &str)] = &[
    ("build", "images only: `build:` is not supported"),
    ("extends", "`extends` reads other files and is not supported"),
];

/// Service keys of the Compose specification Forge accepts (plus `x-*`).
/// Anything else is refused, so a new Compose feature that reaches the host
/// is never let through by default. Some of these are checked further.
const SERVICE_KEYS: &[&str] = &[
    "annotations",
    "attach",
    "blkio_config",
    "build",
    "cap_add",
    "cap_drop",
    "cgroup",
    "cgroup_parent",
    "command",
    "configs",
    "container_name",
    "cpu_count",
    "cpu_percent",
    "cpu_period",
    "cpu_quota",
    "cpu_rt_period",
    "cpu_rt_runtime",
    "cpu_shares",
    "cpus",
    "cpuset",
    "depends_on",
    "deploy",
    "develop",
    "device_cgroup_rules",
    "devices",
    "dns",
    "dns_opt",
    "dns_search",
    "domainname",
    "entrypoint",
    "env_file",
    "environment",
    "expose",
    "extends",
    "external_links",
    "extra_hosts",
    "gpus",
    "group_add",
    "healthcheck",
    "hostname",
    "image",
    "init",
    "ipc",
    "label_file",
    "labels",
    "links",
    "logging",
    "mac_address",
    "mem_limit",
    "mem_reservation",
    "mem_swappiness",
    "memswap_limit",
    "network_mode",
    "networks",
    "oom_kill_disable",
    "oom_score_adj",
    "pid",
    "pids_limit",
    "platform",
    "ports",
    "post_start",
    "pre_stop",
    "privileged",
    "profiles",
    "pull_policy",
    "read_only",
    "restart",
    "runtime",
    "scale",
    "secrets",
    "security_opt",
    "shm_size",
    "stdin_open",
    "stop_grace_period",
    "stop_signal",
    "storage_opt",
    "sysctls",
    "tmpfs",
    "tty",
    "ulimits",
    "use_api_socket",
    "user",
    "userns_mode",
    "uts",
    "volumes",
    "volumes_from",
    "working_dir",
];

/// Top-level keys allowed in a compose file (plus `x-*` extensions).
const TOP_KEYS: &[&str] = &["version", "name", "services", "networks", "volumes", "configs", "secrets"];

/// Checks an (interpolated) compose tree against the policy.
#[must_use]
pub fn check(tree: &Value, p: &Policy) -> Report {
    let mut r = Report::default();

    let Some(top) = tree.as_object() else {
        r.push("", Severity::Blocked, "the compose file must be a mapping");
        return r;
    };

    for k in top.keys() {
        if k == "include" {
            r.push("", Severity::Blocked, "`include` reads other files and is not supported");
        } else if !TOP_KEYS.contains(&k.as_str()) && !k.starts_with("x-") {
            r.push("", Severity::Blocked, format!("unknown top-level key `{k}`"));
        }
    }

    let services = top.get("services").and_then(Value::as_object);
    match services {
        None => r.push("", Severity::Blocked, "the compose file has no services"),
        Some(s) if s.is_empty() => r.push("", Severity::Blocked, "the compose file has no services"),
        Some(s) if s.len() > MAX_SERVICES => {
            r.push("", Severity::Blocked, format!("more than {MAX_SERVICES} services"));
        }
        Some(s) => {
            let names: BTreeSet<&str> = s.keys().map(String::as_str).collect();

            for (name, svc) in s {
                check_service(&mut r, name, svc, p, &names);
            }
        }
    }

    for (name, v) in top.get("volumes").and_then(Value::as_object).into_iter().flatten() {
        let opts = v.get("driver_opts");
        let device = opts.and_then(|o| o.get("device")).and_then(Value::as_str);
        let bind = opts.and_then(|o| o.get("o")).and_then(Value::as_str).is_some_and(|o| o.contains("bind"));

        match device {
            Some(d) if bind || d.starts_with('/') => {
                check_host_path(&mut r, "", &format!("volume `{name}` device"), d, p)
            }
            Some(d) => r.push("", Severity::Privileged, format!("volume `{name}` mounts device `{d}`")),
            None => {}
        }
    }

    for (name, v) in top.get("networks").and_then(Value::as_object).into_iter().flatten() {
        if name == MESH_NETWORK_KEY {
            r.push("", Severity::Blocked, format!("the network key `{MESH_NETWORK_KEY}` is reserved"));
        }

        if v.get("driver").and_then(Value::as_str) == Some("host") || docker_network_name(name, v) == Some("host") {
            r.push("", Severity::Privileged, format!("network `{name}` uses the host network"));
        }
    }

    for section in ["configs", "secrets"] {
        for (name, v) in top.get(section).and_then(Value::as_object).into_iter().flatten() {
            if let Some(f) = v.get("file").and_then(Value::as_str) {
                check_host_path(&mut r, "", &format!("{section} `{name}` file"), f, p);
            }
        }
    }

    if let Some(dir) = normalize_abs(&p.stack_dir)
        && is_system_path(&dir)
    {
        r.push("", Severity::Blocked, format!("the stack folder `{dir}` is a system folder"));
    }

    check_nesting(&mut r, tree, p);

    r
}

/// The Docker name of a top-level network when it is fixed (`name:`, or
/// the key of an `external` network); `None` for project networks.
fn docker_network_name<'a>(key: &'a str, v: &'a Value) -> Option<&'a str> {
    if let Some(n) = v.get("name").and_then(Value::as_str) {
        return Some(n);
    }

    match v.get("external") {
        Some(Value::Object(m)) => Some(m.get("name").and_then(Value::as_str).unwrap_or(key)),
        e if truthy(e) => Some(key),
        _ => None,
    }
}

/// Paths that pass through a read-write bind mount can be redirected by a
/// container (it replaces a folder with a link), and Docker resolves bind
/// sources again at every container start. So no bind source and no file
/// Compose reads may lie inside another writable bind of the stack, and no
/// writable bind may contain the stack folder (`compose.yaml`, `.env`).
fn check_nesting(r: &mut Report, tree: &Value, p: &Policy) {
    let all = binds(tree, &p.stack_dir);

    // A writable bind of a system path already needs `stack.privileged` and
    // hands over the host; nesting under it adds nothing.
    let writable: Vec<&Bind> = all.iter().filter(|b| b.writable && !is_system_path(&b.path)).collect();
    let mut seen = BTreeSet::new();

    for b in &all {
        if let Some(w) = writable.iter().find(|w| w.path != b.path && within(&b.path, &w.path))
            && seen.insert(b.path.clone())
        {
            r.push(
                &b.service,
                Severity::Blocked,
                format!(
                    "bind mount `{}` lies inside the read-write bind mount `{}`; mount a separate folder",
                    b.path, w.path
                ),
            );
        }
    }

    for f in host_files(tree, &p.stack_dir) {
        if let Some(w) = writable.iter().find(|w| within(&f, &w.path)) {
            r.push(
                &w.service,
                Severity::Blocked,
                format!("`{f}` is read by Compose but lies inside the read-write bind mount `{}`", w.path),
            );
        }
    }

    if let Some(dir) = normalize_abs(&p.stack_dir) {
        for w in writable.iter().filter(|w| within(&dir, &w.path)) {
            r.push(
                &w.service,
                Severity::Blocked,
                format!(
                    "the read-write bind mount `{}` contains the stack folder (compose.yaml, .env); mount it read-only (`:ro`) or mount a subfolder",
                    w.path
                ),
            );
        }
    }
}

fn truthy(v: Option<&Value>) -> bool {
    match v {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => matches!(s.to_ascii_lowercase().as_str(), "true" | "yes" | "on" | "1"),
        Some(Value::Number(n)) => n.as_i64() != Some(0),
        _ => false,
    }
}

fn strings(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::String(s)) => vec![s.clone()],
        Some(Value::Array(a)) => a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect(),
        _ => Vec::new(),
    }
}

fn check_service(r: &mut Report, name: &str, svc: &Value, p: &Policy, stack_services: &BTreeSet<&str>) {
    if !valid_service_name(name) {
        r.push(name, Severity::Blocked, "service names use letters, digits, `.`, `_` and `-`");
    }

    let Some(s) = svc.as_object() else {
        r.push(name, Severity::Blocked, "a service must be a mapping");
        return;
    };

    for (k, why) in BLOCKED_KEYS {
        if s.contains_key(*k) {
            r.push(name, Severity::Blocked, *why);
        }
    }

    if s.contains_key("provider") {
        r.push(name, Severity::Blocked, "`provider` runs a program on the node and is not supported");
    }

    for k in s.keys() {
        if !SERVICE_KEYS.contains(&k.as_str()) && !k.starts_with("x-") && k != "provider" {
            r.push(name, Severity::Blocked, format!("unknown service key `{k}`"));
        }
    }

    if s.get("image").and_then(Value::as_str).is_none_or(str::is_empty) {
        r.push(name, Severity::Blocked, "every service needs an `image`");
    }

    if truthy(s.get("privileged")) {
        r.push(name, Severity::Privileged, "`privileged: true` gives the container full control of the node");
    }

    for hook in ["post_start", "pre_stop"] {
        let hooks = match s.get(hook) {
            Some(Value::Array(a)) => a.iter().collect(),
            Some(v @ Value::Object(_)) => vec![v],
            _ => Vec::new(),
        };

        if hooks.iter().any(|h| truthy(h.get("privileged"))) {
            r.push(name, Severity::Privileged, format!("`{hook}` runs a command with `privileged: true`"));
        }
    }

    if truthy(s.get("use_api_socket")) {
        r.push(name, Severity::Privileged, "`use_api_socket` gives the container control of Docker on the node");
    }

    // `service:<name>` must name a service of this stack (on this node).
    let outside = |m: &str| m.strip_prefix("service:").is_some_and(|x| !stack_services.contains(x));

    for key in ["pid", "ipc", "uts", "userns_mode", "cgroup", "network_mode"] {
        match s.get(key).and_then(Value::as_str) {
            Some("host") if key == "network_mode" => {
                r.push(name, Severity::Privileged, "`network_mode: host` puts the container on the node's network");
            }
            Some("host") => {
                r.push(name, Severity::Privileged, format!("`{key}: host` shares the node's {key} environment"));
            }
            Some(m) if m.starts_with("container:") || outside(m) => {
                r.push(name, Severity::Privileged, format!("`{key}: {m}` joins a container outside the stack"));
            }
            _ => {}
        }
    }

    for (key, why) in [
        ("cap_add", "adds kernel capabilities"),
        ("devices", "passes host devices into the container"),
        ("device_cgroup_rules", "changes device access rules"),
        ("sysctls", "changes kernel parameters"),
        ("cgroup_parent", "places the container in another cgroup"),
        ("runtime", "uses another container runtime"),
        ("gpus", "passes GPUs into the container"),
    ] {
        let present = match s.get(key) {
            None | Some(Value::Null) => false,
            Some(Value::Array(a)) => !a.is_empty(),
            Some(Value::Object(m)) => !m.is_empty(),
            Some(Value::String(v)) => !v.is_empty(),
            Some(_) => true,
        };

        if present {
            r.push(name, Severity::Privileged, format!("`{key}` {why}"));
        }
    }

    let reservations = svc.pointer("/deploy/resources/reservations/devices");
    if reservations.and_then(Value::as_array).is_some_and(|a| !a.is_empty()) {
        r.push(name, Severity::Privileged, "`deploy.resources.reservations.devices` passes host devices");
    }

    for opt in strings(s.get("security_opt")) {
        let o = opt.replace(':', "=").to_ascii_lowercase();
        if o.contains("unconfined") || o.starts_with("label=disable") || o.contains("no-new-privileges=false") {
            r.push(name, Severity::Privileged, format!("`security_opt: {opt}` turns off a protection"));
        }
    }

    for v in strings(s.get("volumes_from")) {
        if v.starts_with("container:") {
            r.push(name, Severity::Privileged, format!("`volumes_from: {v}` reads a container outside the stack"));
        }
    }

    for key in ["env_file", "label_file"] {
        for f in env_files(s.get(key)) {
            if f.starts_with('/') || f.starts_with('~') || f.split('/').any(|c| c == "..") {
                r.push(name, Severity::Blocked, format!("`{key}: {f}` must be a file inside the stack folder"));
            }
        }
    }

    for v in s.get("volumes").and_then(Value::as_array).into_iter().flatten() {
        check_volume(r, name, v, p);
    }
}

fn env_files(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::String(s)) => vec![s.clone()],
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|x| x.as_str().map(str::to_owned).or_else(|| x.get("path")?.as_str().map(str::to_owned)))
            .collect(),
        _ => Vec::new(),
    }
}

fn check_volume(r: &mut Report, svc: &str, v: &Value, p: &Policy) {
    match v {
        Value::String(spec) => {
            let src = spec.split(':').next().unwrap_or_default();

            // A single field is an anonymous volume (container path only).
            if spec.contains(':') && is_path_source(src) {
                check_host_path(r, svc, "bind mount", src, p);
            }
        }
        Value::Object(m) => {
            let kind = m.get("type").and_then(Value::as_str).unwrap_or("volume");
            let src = m.get("source").and_then(Value::as_str).unwrap_or_default();

            match kind {
                "bind" => check_host_path(r, svc, "bind mount", src, p),
                "volume" | "tmpfs" | "image" => {}
                "npipe" | "cluster" => r.push(svc, Severity::Blocked, format!("volume type `{kind}` is not supported")),
                k => r.push(svc, Severity::Blocked, format!("unknown volume type `{k}`")),
            }
        }
        _ => r.push(svc, Severity::Blocked, "a volume must be a string or a mapping"),
    }
}

// ---- ports ---------------------------------------------------------------------------

fn is_public_addr(ip: &str) -> bool {
    matches!(ip, "" | "0.0.0.0" | "::" | "[::]")
}

fn bracket(ip: &str) -> String {
    if ip.contains(':') && !ip.starts_with('[') { format!("[{ip}]") } else { ip.to_owned() }
}

/// Ports the service marks public: `x-forge: { public: [8080, "443"] }`.
fn public_ports(svc: &Value) -> BTreeSet<String> {
    match svc.pointer("/x-forge/public") {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|v| match v {
                Value::Number(n) => Some(n.to_string()),
                Value::String(s) => Some(s.clone()),
                _ => None,
            })
            .collect(),
        Some(Value::Bool(true)) => BTreeSet::from(["*".to_owned()]),
        _ => BTreeSet::new(),
    }
}

/// A published port after [`bind_ports`]: what Nginx or users can reach.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishedPort {
    pub service: String,
    pub host_ip: String,
    /// Host port (may be empty for an ephemeral port, or a range).
    pub published: String,
    pub target: String,
    pub protocol: String,
    pub public: bool,
}

/// Rewrites every published port to bind to `internal_ip` unless the
/// service marks it public (`x-forge.public`), and returns them.
pub fn bind_ports(tree: &mut Value, internal_ip: &str, r: &mut Report) -> Vec<PublishedPort> {
    let mut out = Vec::new();
    let Some(services) = tree.get_mut("services").and_then(Value::as_object_mut) else { return out };

    for (name, svc) in services.iter_mut() {
        let public = public_ports(svc);
        let Some(ports) = svc.get_mut("ports").and_then(Value::as_array_mut) else { continue };

        for port in ports.iter_mut() {
            let parsed = match port {
                Value::String(s) => parse_short_port(s),
                Value::Number(n) => Some((String::new(), String::new(), n.to_string(), "tcp".to_owned())),
                Value::Object(m) => Some((
                    m.get("host_ip").and_then(Value::as_str).unwrap_or_default().to_owned(),
                    m.get("published").map(scalar).unwrap_or_default(),
                    m.get("target").map(scalar).unwrap_or_default(),
                    m.get("protocol").and_then(Value::as_str).unwrap_or("tcp").to_owned(),
                )),
                _ => None,
            };

            let Some((ip, published, target, protocol)) = parsed else {
                r.push(name, Severity::Blocked, format!("cannot read port `{port}`"));
                continue;
            };

            let key = if published.is_empty() { &target } else { &published };
            let is_public = public.contains("*") || public.contains(key);
            let host_ip = if is_public {
                if ip.is_empty() { "0.0.0.0".to_owned() } else { ip.clone() }
            } else if is_public_addr(&ip) {
                if !ip.is_empty() {
                    r.push(
                        name,
                        Severity::Info,
                        format!("port {key} binds to {internal_ip}; list it in `x-forge.public` to publish it on all addresses"),
                    );
                }

                internal_ip.to_owned()
            } else {
                ip.clone()
            };

            if let Value::Object(m) = port {
                m.insert("host_ip".into(), json!(host_ip));
            } else {
                *port = json!(format!("{}:{published}:{target}/{protocol}", bracket(&host_ip)));
            }

            out.push(PublishedPort { service: name.clone(), host_ip, published, target, protocol, public: is_public });
        }
    }

    out
}

fn scalar(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

/// `[ip:][published:]target[/proto]` → (ip, published, target, proto).
fn parse_short_port(s: &str) -> Option<(String, String, String, String)> {
    let (rest, proto) = match s.rsplit_once('/') {
        Some((r, p)) if matches!(p, "tcp" | "udp" | "sctp") => (r, p.to_owned()),
        Some(_) => return None,
        None => (s, "tcp".to_owned()),
    };

    // A bracketed IPv6 address first.
    let (ip, rest) = if let Some(r) = rest.strip_prefix('[') {
        let (ip, r) = r.split_once("]:")?;
        (ip.to_owned(), r)
    } else {
        let parts: Vec<&str> = rest.split(':').collect();
        match parts.len() {
            1 | 2 => (String::new(), rest),
            3 => (parts[0].to_owned(), &rest[parts[0].len() + 1..]),
            _ => return None,
        }
    };

    let (published, target) = match rest.split_once(':') {
        Some((p, t)) => (p.to_owned(), t.to_owned()),
        None => (String::new(), rest.to_owned()),
    };

    let ok = |x: &str| x.bytes().all(|b| b.is_ascii_digit() || b == b'-');
    if target.is_empty() || !ok(&target) || !ok(&published) {
        return None;
    }

    Some((ip, published, target, proto))
}

// ---- placement and split ------------------------------------------------------------

/// Where a service runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "name", rename_all = "snake_case")]
pub enum Placement {
    /// The node the stack was created from.
    Default,
    /// A node by hostname.
    Node(String),
    /// Every node of a cluster (by name).
    Cluster(String),
}

/// Reads each service's placement from `x-forge.node`, `x-forge.cluster`
/// or `deploy.placement.constraints: ["node.hostname == x"]`.
pub fn placements(tree: &Value) -> Result<BTreeMap<String, Placement>, String> {
    let mut out = BTreeMap::new();

    for (name, svc) in tree.get("services").and_then(Value::as_object).into_iter().flatten() {
        let node = svc.pointer("/x-forge/node").and_then(Value::as_str);
        let cluster = svc.pointer("/x-forge/cluster").and_then(Value::as_str);
        let mut hostnames = Vec::new();

        for c in strings(svc.pointer("/deploy/placement/constraints")) {
            let c = c.replace(' ', "");
            if let Some(h) = c.strip_prefix("node.hostname==") {
                hostnames.push(h.to_owned());
            } else {
                return Err(format!(
                    "{name}: placement constraint `{c}` is not supported; use `node.hostname == <node>` or `x-forge.node`"
                ));
            }
        }

        let p = match (node, cluster, hostnames.as_slice()) {
            (Some(_), Some(_), _) => return Err(format!("{name}: set either x-forge.node or x-forge.cluster")),
            (Some(n), None, []) => Placement::Node(n.to_owned()),
            (Some(n), None, [h]) if h == n => Placement::Node(n.to_owned()),
            (None, Some(c), []) => Placement::Cluster(c.to_owned()),
            (None, None, []) => Placement::Default,
            (None, None, [h]) => Placement::Node(h.clone()),
            _ => return Err(format!("{name}: the placement names more than one node")),
        };

        out.insert(name.clone(), p);
    }

    Ok(out)
}

/// Services each service depends on (`depends_on` list or map).
#[must_use]
pub fn dependencies(svc: &Value) -> Vec<String> {
    match svc.get("depends_on") {
        Some(Value::Array(a)) => a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect(),
        Some(Value::Object(m)) => m.keys().cloned().collect(),
        _ => Vec::new(),
    }
}

/// `selected` plus everything they depend on, transitively.
#[must_use]
pub fn with_dependencies(tree: &Value, selected: &[String]) -> BTreeSet<String> {
    let services = tree.get("services").and_then(Value::as_object);
    let mut out = BTreeSet::new();
    let mut todo: Vec<String> = selected.to_vec();

    while let Some(s) = todo.pop() {
        if out.insert(s.clone())
            && let Some(svc) = services.and_then(|m| m.get(&s))
        {
            todo.extend(dependencies(svc));
        }
    }

    out
}

fn named_volumes(svc: &Value) -> Vec<String> {
    let mut out = Vec::new();

    for v in svc.get("volumes").and_then(Value::as_array).into_iter().flatten() {
        match v {
            Value::String(s) => {
                let src = s.split(':').next().unwrap_or_default();
                if s.contains(':') && !is_path_source(src) {
                    out.push(src.to_owned());
                }
            }
            Value::Object(m) if m.get("type").and_then(Value::as_str).unwrap_or("volume") == "volume" => {
                if let Some(s) = m.get("source").and_then(Value::as_str) {
                    out.push(s.to_owned());
                }
            }
            _ => {}
        }
    }

    out
}

fn named_refs(svc: &Value, key: &str) -> Vec<String> {
    match svc.get(key) {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|v| v.as_str().map(str::to_owned).or_else(|| v.get("source")?.as_str().map(str::to_owned)))
            .collect(),
        Some(Value::Object(m)) => m.keys().cloned().collect(),
        _ => Vec::new(),
    }
}

/// The part of `tree` for one node: only `services`, plus the networks,
/// volumes, configs and secrets they use. `x-forge` and placement
/// constraints are removed and every service gets the [`STACK_LABEL`].
/// Fails when a service depends on one that runs elsewhere.
pub fn part_for(tree: &Value, services: &BTreeSet<String>, stack_label: &str) -> Result<Value, String> {
    let all = tree.get("services").and_then(Value::as_object).cloned().unwrap_or_default();
    let mut svcs = Map::new();
    let (mut nets, mut vols, mut cfgs, mut secs) = (BTreeSet::new(), BTreeSet::new(), BTreeSet::new(), BTreeSet::new());

    for name in services {
        let mut svc = all.get(name).cloned().ok_or_else(|| format!("unknown service `{name}`"))?;

        for d in dependencies(&svc) {
            if !services.contains(&d) {
                return Err(format!("{name} depends on {d}, which runs on another node"));
            }
        }

        nets.extend(named_refs(&svc, "networks"));
        vols.extend(named_volumes(&svc));
        cfgs.extend(named_refs(&svc, "configs"));
        secs.extend(named_refs(&svc, "secrets"));

        if let Some(m) = svc.as_object_mut() {
            // Placement keys are spent here, but `public` must survive: the
            // server and the agent both run `bind_ports` on this part, and
            // without it every port fell back to the internal address.
            // Compose accepts `x-*` extension fields on a service.
            // `mesh` survives too: the helper reads it when it attaches
            // services to the cluster network.
            let mut kept = Map::new();

            for key in ["public", "mesh"] {
                if let Some(v) = m.get("x-forge").and_then(|x| x.get(key)) {
                    kept.insert(key.into(), v.clone());
                }
            }

            m.remove("x-forge");

            if !kept.is_empty() {
                m.insert("x-forge".into(), Value::Object(kept));
            }

            if let Some(dep) = m.get_mut("deploy").and_then(Value::as_object_mut) {
                dep.remove("placement");

                if dep.is_empty() {
                    m.remove("deploy");
                }
            }

            match m.entry("labels").or_insert_with(|| json!({})) {
                Value::Object(l) => {
                    l.insert(STACK_LABEL.into(), json!(stack_label));
                }
                Value::Array(l) => {
                    l.retain(|x| !x.as_str().is_some_and(|s| s.starts_with(&format!("{STACK_LABEL}="))));
                    l.push(json!(format!("{STACK_LABEL}={stack_label}")));
                }
                other => *other = json!({ STACK_LABEL: stack_label }),
            }
        }

        svcs.insert(name.clone(), svc);
    }

    let mut out = Map::new();

    if let Some(n) = tree.get("name") {
        out.insert("name".into(), n.clone());
    }

    out.insert("services".into(), Value::Object(svcs));

    for (key, used) in [("networks", nets), ("volumes", vols), ("configs", cfgs), ("secrets", secs)] {
        let Some(defs) = tree.get(key).and_then(Value::as_object) else { continue };
        let picked: Map<String, Value> = defs
            .iter()
            .filter(|(k, _)| used.contains(*k) || (key == "networks" && *k == "default"))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        if !picked.is_empty() {
            out.insert(key.into(), Value::Object(picked));
        }
    }

    // Extension fields (`x-…`) at the top level are only anchors' homes.
    Ok(Value::Object(out))
}

// ---- YAML output ------------------------------------------------------------------------

/// Writes a JSON tree as block YAML. Strings are double-quoted with JSON
/// escapes (valid YAML), so nothing is reinterpreted (`on`, `1.10`, `08`).
#[must_use]
pub fn to_yaml(v: &Value) -> String {
    let mut out = String::new();

    match v {
        Value::Object(m) if !m.is_empty() => write_map(&mut out, m, 0),
        Value::Array(a) if !a.is_empty() => write_seq(&mut out, a, 0),
        other => {
            out.push_str(&scalar_yaml(other));
            out.push('\n');
        }
    }

    out
}

fn yaml_key(k: &str) -> String {
    let plain = !k.is_empty()
        && k.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b'/'))
        && k.as_bytes()[0].is_ascii_alphabetic()
        && !matches!(
            k.to_ascii_lowercase().as_str(),
            "true" | "false" | "yes" | "no" | "on" | "off" | "null" | "y" | "n"
        );
    if plain { k.to_owned() } else { serde_json::to_string(k).unwrap_or_default() }
}

fn scalar_yaml(v: &Value) -> String {
    match v {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => serde_json::to_string(s).unwrap_or_default(),
        Value::Array(_) => "[]".into(),
        Value::Object(_) => "{}".into(),
    }
}

fn is_block(v: &Value) -> bool {
    match v {
        Value::Object(m) => !m.is_empty(),
        Value::Array(a) => !a.is_empty(),
        _ => false,
    }
}

fn write_map(out: &mut String, m: &Map<String, Value>, indent: usize) {
    for (k, v) in m {
        out.push_str(&" ".repeat(indent));
        out.push_str(&yaml_key(k));
        out.push(':');

        match v {
            Value::Object(c) if !c.is_empty() => {
                out.push('\n');
                write_map(out, c, indent + 2);
            }
            Value::Array(c) if !c.is_empty() => {
                out.push('\n');
                write_seq(out, c, indent + 2);
            }
            other => {
                out.push(' ');
                out.push_str(&scalar_yaml(other));
                out.push('\n');
            }
        }
    }
}

fn write_seq(out: &mut String, a: &[Value], indent: usize) {
    for v in a {
        out.push_str(&" ".repeat(indent));
        out.push('-');

        if is_block(v) {
            match v {
                Value::Object(m) => {
                    // First key on the dash line, the rest indented under it.
                    let mut inner = String::new();
                    write_map(&mut inner, m, indent + 2);
                    out.push(' ');
                    out.push_str(inner.trim_start());
                }
                Value::Array(c) => {
                    out.push('\n');
                    write_seq(out, c, indent + 2);
                }
                _ => {}
            }
        } else {
            out.push(' ');
            out.push_str(&scalar_yaml(v));
            out.push('\n');
        }
    }
}

// ---- helper operations ----------------------------------------------------------------

/// One node's part of a stack, as the helper receives it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StackPart {
    pub environment: String,
    pub stack: String,
    /// Absolute directory on the node.
    pub dir: String,
    /// The compose tree for this node (not yet interpolated).
    pub compose: Value,
    /// `.env` variables (may hold secrets; written 0600).
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    pub policy: Policy,
    /// Address unmarked ports bind to.
    pub internal_ip: String,
    /// Docker name of the node's cluster container network, when every
    /// service not opted out must join it. `None`: no injection. Only set
    /// for agents that report the `mesh_network` fact, because older
    /// helpers refuse unknown fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mesh_network: Option<String>,
}

impl StackPart {
    #[must_use]
    pub fn project(&self) -> String {
        project_name(&self.environment, &self.stack)
    }

    /// Re-checks names, directory and policy; returns the tree to write (ports bound).
    pub fn prepare(&self) -> Result<(Value, Vec<PublishedPort>), String> {
        if !valid_stack_name(&self.environment) || !valid_stack_name(&self.stack) {
            return Err("invalid environment or stack name".into());
        }

        let dir = normalize_abs(&self.dir).ok_or("the stack folder must be an absolute path without `..`")?;
        if dir != self.policy.stack_dir {
            return Err("the stack folder does not match the policy".into());
        }

        let default = default_dir(&self.environment, &self.stack);
        if dir != default && !self.policy.bind_roots.iter().filter_map(|b| normalize_abs(b)).any(|b| within(&dir, &b)) {
            return Err("the stack folder must be inside an approved file root".into());
        }

        if is_system_path(&dir) {
            return Err("the stack folder cannot be a system folder".into());
        }

        if self.env.keys().any(|k| {
            let u = k.to_ascii_uppercase();
            u.starts_with("COMPOSE_") || u.starts_with("DOCKER_")
        }) {
            return Err("COMPOSE_* and DOCKER_* variables are not allowed in .env".into());
        }

        let ip: std::net::IpAddr = self.internal_ip.parse().map_err(|_| "invalid internal address")?;

        // The written file is the interpolated, checked tree with `$`
        // escaped, so Compose runs exactly what was checked. Errors go back
        // to the control plane (job errors), so `.env` values are redacted.
        let hide = |e: String| redact(&e, &self.env);
        let mut tree = interpolate(&self.compose, &self.env).map_err(hide)?;
        let mut report = check(&tree, &self.policy);
        report.ensure(self.policy.allow_privileged).map_err(hide)?;
        let ports = bind_ports(&mut tree, &ip.to_string(), &mut report);
        report.ensure(self.policy.allow_privileged).map_err(hide)?;
        check_mesh_refs(&tree, self.mesh_network.as_deref())?;
        add_log_url(&mut tree);

        if let Some(net) = &self.mesh_network {
            add_mesh_network(&mut tree, net);
        }

        Ok((escape_dollars(&tree), ports))
    }
}

/// Compose key of the cluster container network in a stack part. Reserved:
/// a stack that defines it is refused.
pub const MESH_NETWORK_KEY: &str = "forge-mesh";

/// Prefix of every cluster container network's Docker name
/// (`fabric::network_name` of a `forge…` interface).
const MESH_NETWORK_PREFIX: &str = "forge-forge";

/// Refuses a network whose Docker name is a cluster container network
/// unless it is this part's own (`mesh`): a stack cannot join another
/// cluster's network, or this one where the server did not put it.
fn check_mesh_refs(tree: &Value, mesh: Option<&str>) -> Result<(), String> {
    for (key, v) in tree.get("networks").and_then(Value::as_object).into_iter().flatten() {
        let Some(name) = docker_network_name(key, v) else { continue };

        if name.starts_with(MESH_NETWORK_PREFIX) && Some(name) != mesh {
            return Err(format!("network `{key}` uses the cluster network `{name}`, which this node does not offer"));
        }
    }

    Ok(())
}

/// Whether `x-forge.mesh` is explicitly `false` on `v` (a service or the
/// top level). Absent or anything else keeps the default, `true`.
fn mesh_off(v: &Value) -> bool {
    v.pointer("/x-forge/mesh") == Some(&Value::Bool(false))
}

/// Attaches every service to the node's cluster container network
/// `docker_name` under the reserved key [`MESH_NETWORK_KEY`], in addition to
/// its own networks. Skipped for a service with any `network_mode` or
/// `x-forge.mesh: false`, and for the whole stack with a top-level
/// `x-forge.mesh: false`. Declares the network as external only when a
/// service was attached.
pub fn add_mesh_network(tree: &mut Value, docker_name: &str) {
    if mesh_off(tree) {
        return;
    }

    let Some(services) = tree.get_mut("services").and_then(Value::as_object_mut) else { return };
    let mut attached = false;

    for svc in services.values_mut() {
        if svc.get("network_mode").is_some() || mesh_off(svc) {
            continue;
        }

        let Some(m) = svc.as_object_mut() else { continue };

        match m.entry("networks").or_insert_with(|| json!(["default"])) {
            Value::Array(list) => {
                if !list.iter().any(|n| n.as_str() == Some(MESH_NETWORK_KEY)) {
                    list.push(json!(MESH_NETWORK_KEY));
                }
            }
            Value::Object(map) => {
                map.entry(MESH_NETWORK_KEY).or_insert_with(|| json!({}));
            }
            _ => continue,
        }

        attached = true;
    }

    if !attached {
        return;
    }

    let Some(top) = tree.as_object_mut() else { return };

    if let Value::Object(nets) = top.entry("networks").or_insert_with(|| json!({})) {
        nets.entry(MESH_NETWORK_KEY).or_insert_with(|| json!({ "external": true, "name": docker_name }));
    }
}

/// Gives every service of a stack the node's log listener (D-142), like a
/// single container: `FORGE_LOG_URL` in its `environment` and a
/// `forge-node:host-gateway` host entry. A value the stack sets for either
/// wins. With `network_mode: host` the URL is the loopback listener and no
/// host entry is added; a service sharing another's network
/// (`service:`/`container:`) cannot take host entries and gets only the
/// URL; `network_mode: none` gets neither.
pub fn add_log_url(tree: &mut Value) {
    use crate::apps::{LOG_HOST_ENTRY, LOG_URL_CONTAINER, LOG_URL_ENV, LOG_URL_HOST};
    let Some(services) = tree.get_mut("services").and_then(Value::as_object_mut) else { return };

    for svc in services.values_mut().filter_map(Value::as_object_mut) {
        let mode = svc.get("network_mode").and_then(Value::as_str).unwrap_or_default().to_owned();
        let (url, hosts) = match mode.as_str() {
            "none" => continue,
            "host" => (LOG_URL_HOST, false),
            m if m.starts_with("service:") || m.starts_with("container:") => (LOG_URL_CONTAINER, false),
            _ => (LOG_URL_CONTAINER, true),
        };

        match svc.entry("environment").or_insert_with(|| Value::Object(Map::new())) {
            Value::Object(env) => {
                env.entry(LOG_URL_ENV).or_insert_with(|| Value::String(url.to_owned()));
            }
            Value::Array(env) => {
                let set = env.iter().filter_map(Value::as_str).any(|e| e.split('=').next() == Some(LOG_URL_ENV));
                if !set {
                    env.push(Value::String(format!("{LOG_URL_ENV}={url}")));
                }
            }
            _ => {}
        }

        if !hosts {
            continue;
        }

        let (name, target) = LOG_HOST_ENTRY.split_once(':').unwrap_or((LOG_HOST_ENTRY, ""));

        match svc.entry("extra_hosts").or_insert_with(|| Value::Array(Vec::new())) {
            Value::Array(h) => {
                let set = h.iter().filter_map(Value::as_str).any(|e| e.split([':', '=']).next() == Some(name));
                if !set {
                    h.push(Value::String(LOG_HOST_ENTRY.to_owned()));
                }
            }
            Value::Object(h) => {
                h.entry(name).or_insert_with(|| Value::String(target.to_owned()));
            }
            _ => {}
        }
    }
}

/// Doubles every `$` so Compose does not interpolate again.
#[must_use]
pub fn escape_dollars(v: &Value) -> Value {
    match v {
        Value::String(s) => Value::String(s.replace('$', "$$")),
        Value::Array(a) => Value::Array(a.iter().map(escape_dollars).collect()),
        Value::Object(m) => Value::Object(m.iter().map(|(k, v)| (k.clone(), escape_dollars(v))).collect()),
        other => other.clone(),
    }
}

/// Operations of the node helper on stacks (`docker compose`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum StackOp {
    /// Write `compose.yaml` and `.env`, pull images and start `services`
    /// (all when empty) with `up -d --no-build`; returns `ps`.
    Deploy {
        part: Box<StackPart>,
        #[serde(default)]
        services: Vec<String>,
        /// Pull images before starting (`--pull always` otherwise `missing`).
        #[serde(default)]
        pull: bool,
    },
    /// `docker compose ps --all --format json`.
    Ps { project: String, dir: String },
    /// `start`, `stop`, `restart` or `pull` for `services` (all when empty).
    Service {
        project: String,
        dir: String,
        action: String,
        #[serde(default)]
        services: Vec<String>,
    },
    /// `docker compose down` (`--volumes` when `volumes`), then remove the folder's files.
    Down {
        project: String,
        dir: String,
        #[serde(default)]
        volumes: bool,
    },
    /// Last lines of a service's logs.
    Logs {
        project: String,
        dir: String,
        service: String,
        #[serde(default)]
        tail: u32,
    },
}

impl StackOp {
    pub fn validate(&self) -> Result<(), String> {
        let check_dir = |dir: &str| -> Result<(), String> {
            let d = normalize_abs(dir).ok_or("invalid stack folder")?;
            if is_system_path(&d) { Err("invalid stack folder".into()) } else { Ok(()) }
        };
        let check_services = |s: &[String]| -> Result<(), String> {
            if s.len() > MAX_SERVICES || !s.iter().all(|x| valid_service_name(x)) {
                return Err("invalid service names".into());
            }

            Ok(())
        };

        match self {
            Self::Deploy { part, services, .. } => {
                check_services(services)?;
                check_dir(&part.dir)?;
                part.prepare().map(|_| ())
            }
            Self::Ps { project, dir } | Self::Down { project, dir, .. } => {
                if !valid_project(project) {
                    return Err("invalid project name".into());
                }

                check_dir(dir)
            }
            Self::Service { project, dir, action, services } => {
                if !valid_project(project) {
                    return Err("invalid project name".into());
                }

                if !matches!(action.as_str(), "start" | "stop" | "restart" | "pull") {
                    return Err(format!("unknown stack action `{action}`"));
                }

                check_services(services)?;
                check_dir(dir)
            }
            Self::Logs { project, dir, service, .. } => {
                if !valid_project(project) || !valid_service_name(service) {
                    return Err("invalid project or service name".into());
                }

                check_dir(dir)
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;
