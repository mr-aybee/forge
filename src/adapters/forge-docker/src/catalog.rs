//! Every Docker Engine API endpoint Forge forwards, and what each needs.
//!
//! The catalog is deny-by-default: a request whose method and path match no
//! entry is refused before anything reaches a node. Each entry names its
//! [`Class`] (which Forge action authorizes it) and its [`Transfer`] (how
//! the body moves). Paths are written as in the Engine API reference, with
//! `{}` for one path segment and `{*}` for the rest of the path (image and
//! plugin names may contain `/`).
//!
//! The list follows Engine API 1.47 and stays valid on newer engines: Forge
//! sends the path without a version prefix unless the caller gave one, so
//! the daemon answers in its own current version.

use serde::Serialize;

/// What a call can do, and so which Forge action it needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Class {
    /// Lists, inspects, logs, stats, events: `server.manage`.
    Read,
    /// Changes that stay inside Docker's sandbox (containers without host
    /// access, images, networks, volumes, prune): `container.manage`.
    Manage,
    /// Runs or reads arbitrary things inside a container (exec, attach,
    /// archive, export): `container.exec`.
    Exec,
    /// Root on the node or the cluster (swarm, plugins, build, sessions,
    /// registry login, swarm secrets, services): `server.command` (MFA).
    Admin,
}

/// How the request and response bodies move.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Transfer {
    /// A JSON request body is read (bounded) so Forge can inspect it.
    Inspect,
    /// Bodies are streamed both ways (pull progress, logs, tar archives).
    Stream,
    /// The connection is upgraded to a raw duplex stream (attach, exec
    /// start, BuildKit sessions).
    Hijack,
}

/// One Engine API endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Endpoint {
    pub method: &'static str,
    pub path: &'static str,
    pub class: Class,
    pub transfer: Transfer,
    /// Short operation name, used in audit entries (`docker.api.<op>`).
    pub op: &'static str,
}

const fn e(method: &'static str, path: &'static str, class: Class, transfer: Transfer, op: &'static str) -> Endpoint {
    Endpoint { method, path, class, transfer, op }
}

use Class::{Admin, Exec, Manage, Read};
use Transfer::{Hijack, Inspect, Stream};

/// The full catalog.
pub const ENDPOINTS: &[Endpoint] = &[
    // System
    e("GET", "/_ping", Read, Stream, "system.ping"),
    e("HEAD", "/_ping", Read, Stream, "system.ping"),
    e("GET", "/version", Read, Stream, "system.version"),
    e("GET", "/info", Read, Stream, "system.info"),
    e("GET", "/events", Read, Stream, "system.events"),
    e("GET", "/system/df", Read, Stream, "system.df"),
    e("POST", "/auth", Admin, Inspect, "system.auth"),
    // Containers
    e("GET", "/containers/json", Read, Stream, "container.list"),
    e("POST", "/containers/create", Manage, Inspect, "container.create"),
    e("POST", "/containers/prune", Manage, Stream, "container.prune"),
    e("GET", "/containers/{}/json", Read, Stream, "container.inspect"),
    e("GET", "/containers/{}/top", Read, Stream, "container.top"),
    e("GET", "/containers/{}/logs", Read, Stream, "container.logs"),
    e("GET", "/containers/{}/changes", Read, Stream, "container.changes"),
    e("GET", "/containers/{}/export", Exec, Stream, "container.export"),
    e("GET", "/containers/{}/stats", Read, Stream, "container.stats"),
    e("POST", "/containers/{}/resize", Manage, Stream, "container.resize"),
    e("POST", "/containers/{}/start", Manage, Stream, "container.start"),
    e("POST", "/containers/{}/stop", Manage, Stream, "container.stop"),
    e("POST", "/containers/{}/restart", Manage, Stream, "container.restart"),
    e("POST", "/containers/{}/kill", Manage, Stream, "container.kill"),
    e("POST", "/containers/{}/update", Manage, Inspect, "container.update"),
    e("POST", "/containers/{}/rename", Manage, Stream, "container.rename"),
    e("POST", "/containers/{}/pause", Manage, Stream, "container.pause"),
    e("POST", "/containers/{}/unpause", Manage, Stream, "container.unpause"),
    e("POST", "/containers/{}/attach", Exec, Hijack, "container.attach"),
    e("GET", "/containers/{}/attach/ws", Exec, Hijack, "container.attach_ws"),
    e("POST", "/containers/{}/wait", Read, Stream, "container.wait"),
    e("DELETE", "/containers/{}", Manage, Stream, "container.delete"),
    e("HEAD", "/containers/{}/archive", Exec, Stream, "container.archive_info"),
    e("GET", "/containers/{}/archive", Exec, Stream, "container.archive_get"),
    e("PUT", "/containers/{}/archive", Exec, Stream, "container.archive_put"),
    // Exec
    e("POST", "/containers/{}/exec", Exec, Inspect, "exec.create"),
    e("POST", "/exec/{}/start", Exec, Hijack, "exec.start"),
    e("POST", "/exec/{}/resize", Exec, Stream, "exec.resize"),
    e("GET", "/exec/{}/json", Read, Stream, "exec.inspect"),
    // Images
    e("GET", "/images/json", Read, Stream, "image.list"),
    e("POST", "/build", Admin, Stream, "image.build"),
    e("POST", "/build/prune", Manage, Stream, "build.prune"),
    e("POST", "/images/create", Manage, Stream, "image.create"),
    e("GET", "/images/search", Read, Stream, "image.search"),
    e("POST", "/images/prune", Manage, Stream, "image.prune"),
    e("POST", "/commit", Manage, Stream, "image.commit"),
    e("GET", "/images/get", Read, Stream, "image.get_all"),
    e("POST", "/images/load", Admin, Stream, "image.load"),
    // Image names may contain `/`: these match on the suffix.
    e("GET", "/images/{*}/json", Read, Stream, "image.inspect"),
    e("GET", "/images/{*}/history", Read, Stream, "image.history"),
    e("POST", "/images/{*}/push", Manage, Stream, "image.push"),
    e("POST", "/images/{*}/tag", Manage, Stream, "image.tag"),
    e("GET", "/images/{*}/get", Read, Stream, "image.get"),
    e("DELETE", "/images/{*}", Manage, Stream, "image.delete"),
    e("GET", "/distribution/{*}/json", Read, Stream, "distribution.inspect"),
    // Networks
    e("GET", "/networks", Read, Stream, "network.list"),
    e("POST", "/networks/create", Manage, Inspect, "network.create"),
    e("POST", "/networks/prune", Manage, Stream, "network.prune"),
    e("GET", "/networks/{}", Read, Stream, "network.inspect"),
    e("DELETE", "/networks/{}", Manage, Stream, "network.delete"),
    e("POST", "/networks/{}/connect", Manage, Inspect, "network.connect"),
    e("POST", "/networks/{}/disconnect", Manage, Inspect, "network.disconnect"),
    // Volumes
    e("GET", "/volumes", Read, Stream, "volume.list"),
    e("POST", "/volumes/create", Manage, Inspect, "volume.create"),
    e("POST", "/volumes/prune", Manage, Stream, "volume.prune"),
    e("GET", "/volumes/{}", Read, Stream, "volume.inspect"),
    e("PUT", "/volumes/{}", Admin, Inspect, "volume.update"),
    e("DELETE", "/volumes/{}", Manage, Stream, "volume.delete"),
    // Swarm
    e("GET", "/swarm", Read, Stream, "swarm.inspect"),
    e("POST", "/swarm/init", Admin, Inspect, "swarm.init"),
    e("POST", "/swarm/join", Admin, Inspect, "swarm.join"),
    e("POST", "/swarm/leave", Admin, Stream, "swarm.leave"),
    e("POST", "/swarm/update", Admin, Inspect, "swarm.update"),
    e("GET", "/swarm/unlockkey", Admin, Stream, "swarm.unlockkey"),
    e("POST", "/swarm/unlock", Admin, Inspect, "swarm.unlock"),
    // Nodes
    e("GET", "/nodes", Read, Stream, "node.list"),
    e("GET", "/nodes/{}", Read, Stream, "node.inspect"),
    e("DELETE", "/nodes/{}", Admin, Stream, "node.delete"),
    e("POST", "/nodes/{}/update", Admin, Inspect, "node.update"),
    // Services
    e("GET", "/services", Read, Stream, "service.list"),
    e("POST", "/services/create", Admin, Inspect, "service.create"),
    e("GET", "/services/{}", Read, Stream, "service.inspect"),
    e("DELETE", "/services/{}", Admin, Stream, "service.delete"),
    e("POST", "/services/{}/update", Admin, Inspect, "service.update"),
    e("GET", "/services/{}/logs", Read, Stream, "service.logs"),
    // Tasks
    e("GET", "/tasks", Read, Stream, "task.list"),
    e("GET", "/tasks/{}", Read, Stream, "task.inspect"),
    e("GET", "/tasks/{}/logs", Read, Stream, "task.logs"),
    // Secrets (the API never returns secret data)
    e("GET", "/secrets", Read, Stream, "secret.list"),
    e("POST", "/secrets/create", Admin, Inspect, "secret.create"),
    e("GET", "/secrets/{}", Read, Stream, "secret.inspect"),
    e("DELETE", "/secrets/{}", Admin, Stream, "secret.delete"),
    e("POST", "/secrets/{}/update", Admin, Inspect, "secret.update"),
    // Configs
    e("GET", "/configs", Read, Stream, "config.list"),
    e("POST", "/configs/create", Admin, Inspect, "config.create"),
    e("GET", "/configs/{}", Read, Stream, "config.inspect"),
    e("DELETE", "/configs/{}", Admin, Stream, "config.delete"),
    e("POST", "/configs/{}/update", Admin, Inspect, "config.update"),
    // Plugins (run as root on the node)
    e("GET", "/plugins", Read, Stream, "plugin.list"),
    e("GET", "/plugins/privileges", Read, Stream, "plugin.privileges"),
    e("POST", "/plugins/pull", Admin, Inspect, "plugin.pull"),
    e("POST", "/plugins/create", Admin, Stream, "plugin.create"),
    e("GET", "/plugins/{*}/json", Read, Stream, "plugin.inspect"),
    e("POST", "/plugins/{*}/enable", Admin, Stream, "plugin.enable"),
    e("POST", "/plugins/{*}/disable", Admin, Stream, "plugin.disable"),
    e("POST", "/plugins/{*}/upgrade", Admin, Inspect, "plugin.upgrade"),
    e("POST", "/plugins/{*}/push", Admin, Stream, "plugin.push"),
    e("POST", "/plugins/{*}/set", Admin, Inspect, "plugin.set"),
    e("DELETE", "/plugins/{*}", Admin, Stream, "plugin.delete"),
    // BuildKit sessions
    e("POST", "/session", Admin, Hijack, "session.start"),
    e("POST", "/grpc", Admin, Hijack, "session.grpc"),
];

/// A request matched against the catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Matched {
    pub endpoint: Endpoint,
    /// The path to send, without any version prefix.
    pub path: String,
    /// `v1.xx` when the caller asked for a version, sent back as a prefix.
    pub version: Option<String>,
}

/// Why a request was refused before reaching the node.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MatchError {
    #[error("not a Docker Engine API path Forge forwards: {method} {path}")]
    Unknown { method: String, path: String },
    #[error("invalid path: {0}")]
    BadPath(&'static str),
}

/// Whether `s` is a version prefix like `v1.43`.
fn is_version(s: &str) -> bool {
    s.strip_prefix('v').and_then(|r| r.split_once('.')).is_some_and(|(a, b)| {
        !a.is_empty() && !b.is_empty() && a.bytes().all(|c| c.is_ascii_digit()) && b.bytes().all(|c| c.is_ascii_digit())
    })
}

/// Rejects paths that could escape their meaning on the daemon: `..`,
/// empty segments, control characters, encoded slashes or dots, `?` or `#`.
fn check_path(path: &str) -> Result<(), MatchError> {
    if !path.starts_with('/') {
        return Err(MatchError::BadPath("must start with /"));
    }
    if path.len() > 1024 {
        return Err(MatchError::BadPath("too long"));
    }
    if path.bytes().any(|b| b.is_ascii_control() || matches!(b, b'?' | b'#' | b'\\' | b' ')) {
        return Err(MatchError::BadPath("unexpected character"));
    }
    let lower = path.to_ascii_lowercase();
    if lower.contains("%2f") || lower.contains("%2e") || lower.contains("%5c") || lower.contains("%00") {
        return Err(MatchError::BadPath("encoded separator"));
    }
    let segments: Vec<&str> = path[1..].split('/').collect();
    if path.len() > 1 && segments.iter().any(|s| s.is_empty() || *s == "." || *s == "..") {
        return Err(MatchError::BadPath("empty or dot segment"));
    }
    Ok(())
}

/// Whether one pattern matches `segments`.
fn matches(pattern: &str, segments: &[&str]) -> bool {
    let pat: Vec<&str> = pattern[1..].split('/').collect();
    match pat.iter().position(|p| *p == "{*}") {
        None => pat.len() == segments.len() && pat.iter().zip(segments).all(|(p, s)| *p == "{}" || p == s),
        Some(star) => {
            // Prefix before `{*}`, at least one segment for it, then the
            // exact suffix after it.
            let (head, tail) = (&pat[..star], &pat[star + 1..]);
            segments.len() > head.len() + tail.len()
                && head.iter().zip(segments).all(|(p, s)| *p == "{}" || p == s)
                && tail.iter().rev().zip(segments.iter().rev()).all(|(p, s)| *p == "{}" || p == s)
        }
    }
}

/// Matches `method path` (path without query) against the catalog. A
/// leading `/v1.xx` is taken off and returned separately.
pub fn lookup(method: &str, path: &str) -> Result<Matched, MatchError> {
    check_path(path)?;
    let mut segments: Vec<&str> = path[1..].split('/').collect();
    let version = match segments.first() {
        Some(first) if is_version(first) => {
            let v = (*first).to_owned();
            segments.remove(0);
            Some(v)
        }
        _ => None,
    };
    let bare = format!("/{}", segments.join("/"));
    let method = method.to_ascii_uppercase();
    // Exact (no wildcard) entries first, so `/images/json` is a list, not an
    // inspect of an image called `json`.
    let exact = ENDPOINTS.iter().find(|e| e.method == method && !e.path.contains("{*}") && matches(e.path, &segments));
    let found = exact.or_else(|| {
        ENDPOINTS.iter().find(|e| e.method == method && e.path.contains("{*}") && matches(e.path, &segments))
    });
    found
        .map(|endpoint| Matched { endpoint: *endpoint, path: bare.clone(), version })
        .ok_or(MatchError::Unknown { method, path: bare })
}

/// A container's `Config.Env` holds secrets often: replace each value with
/// `KEY=<redacted>` in an inspect answer for callers without
/// `container.inspect_env`. Returns whether anything was redacted.
pub fn redact_env(inspect: &mut serde_json::Value) -> bool {
    let Some(env) = inspect.pointer_mut("/Config/Env").and_then(serde_json::Value::as_array_mut) else {
        return false;
    };
    for v in env.iter_mut() {
        if let Some(s) = v.as_str() {
            let key = s.split_once('=').map_or(s, |(k, _)| k);
            *v = serde_json::Value::String(format!("{key}=<redacted>"));
        }
    }
    true
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn exact_entries_win_over_wildcards() {
        assert_eq!(lookup("GET", "/images/json").unwrap().endpoint.op, "image.list");
        assert_eq!(lookup("GET", "/images/get").unwrap().endpoint.op, "image.get_all");
        assert_eq!(lookup("GET", "/images/library/nginx:1.27/json").unwrap().endpoint.op, "image.inspect");
        assert_eq!(lookup("DELETE", "/images/ghcr.io/org/app:1").unwrap().endpoint.op, "image.delete");
        assert_eq!(lookup("GET", "/plugins/privileges").unwrap().endpoint.op, "plugin.privileges");
        assert_eq!(lookup("POST", "/plugins/vieux/sshfs:latest/enable").unwrap().endpoint.op, "plugin.enable");
    }

    #[test]
    fn versions_are_split_off() {
        let m = lookup("get", "/v1.43/containers/abc/json").unwrap();
        assert_eq!(m.endpoint.op, "container.inspect");
        assert_eq!(m.version.as_deref(), Some("v1.43"));
        assert_eq!(m.path, "/containers/abc/json");
    }

    #[test]
    fn unknown_and_unsafe_paths_are_refused() {
        assert!(matches!(lookup("GET", "/containers/abc/secret"), Err(MatchError::Unknown { .. })));
        assert!(matches!(lookup("PATCH", "/containers/json"), Err(MatchError::Unknown { .. })));
        for bad in ["/containers/../info", "/images/a%2Fb/json", "//info", "/info?x", "/containers/a b/json", "info"] {
            assert!(matches!(lookup("GET", bad), Err(MatchError::BadPath(_))), "{bad}");
        }
    }

    #[test]
    fn root_on_the_node_is_admin() {
        for (m, p) in [
            ("POST", "/swarm/init"),
            ("POST", "/plugins/pull"),
            ("POST", "/build"),
            ("POST", "/session"),
            ("POST", "/auth"),
            ("GET", "/swarm/unlockkey"),
            ("POST", "/services/create"),
            ("POST", "/images/load"),
        ] {
            assert_eq!(lookup(m, p).unwrap().endpoint.class, Class::Admin, "{m} {p}");
        }
        assert_eq!(lookup("POST", "/containers/x/exec").unwrap().endpoint.class, Class::Exec);
        assert_eq!(lookup("GET", "/containers/x/archive").unwrap().endpoint.class, Class::Exec);
    }

    #[test]
    fn every_entry_is_well_formed_and_unique() {
        let mut seen = std::collections::HashSet::new();
        for e in ENDPOINTS {
            assert!(e.path.starts_with('/'), "{}", e.path);
            assert!(seen.insert((e.method, e.path)), "duplicate {} {}", e.method, e.path);
            assert!(matches!(e.method, "GET" | "HEAD" | "POST" | "PUT" | "DELETE"));
        }
        assert!(ENDPOINTS.len() >= 100, "{}", ENDPOINTS.len());
    }

    #[test]
    fn env_values_are_redacted() {
        let mut v = serde_json::json!({ "Config": { "Env": ["A=1", "PASSWORD=x=y", "BARE"] } });
        assert!(redact_env(&mut v));
        assert_eq!(v["Config"]["Env"], serde_json::json!(["A=<redacted>", "PASSWORD=<redacted>", "BARE=<redacted>"]));
    }
}
