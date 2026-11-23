//! Service specs shared by the control plane and the node helper.
//!
//! A service runs as a **system service** (`systemd`) or a **container**. Every node uses the same layout:
//!
//! | Path | Contents |
//! | --- | --- |
//! | `/srv/forge/apps/<ns>/<app>/` | Code and data, owned by the app user |
//! | `/srv/forge/apps/<ns>/<app>/.forge/run.sh` | Launcher (`exec <command>`), root-owned |
//! | `/etc/forge/apps/<ns>/<app>.env` | `KEY=value` lines: the unit's `EnvironmentFile=` (0640, root and the app user) and the container env (0600) |
//! | `forge-app-<ns>-<app>.service` | systemd unit (system services) |
//!
//! Native apps never run as root: each gets its own system account named
//! after it, `<ns>-<app>` (a hash suffix only when that is too long or taken
//! by an account Forge did not create), marked as Forge's in its GECOS
//! comment (D-081).
//! Environment values may not contain newlines or NUL, so one file works for
//! systemd and Docker alike without quoting rules.

use serde::{Deserialize, Serialize};

/// The variable that tells an app where to send its logs (D-142).
pub const LOG_URL_ENV: &str = "FORGE_LOG_URL";
/// `FORGE_LOG_URL` for apps on the host: the agent's loopback listener.
pub const LOG_URL_HOST: &str = "http://127.0.0.1:5341";
/// `FORGE_LOG_URL` for containers: the node through Docker's host gateway.
pub const LOG_URL_CONTAINER: &str = "http://forge-node:5341";
/// The host entry that makes `forge-node` the Docker host gateway.
pub const LOG_HOST_ENTRY: &str = "forge-node:host-gateway";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Runtime {
    /// A native service: a systemd unit (or Forge's supervisor / a Windows service where there is no systemd).
    Systemd,
    Container,
}

/// Everything the node needs to run one service.
///
/// `Debug` prints environment keys but never their values: they include
/// resolved secrets.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppSpec {
    pub environment: String,
    pub name: String,
    pub runtime: Runtime,
    /// Service / job: the command line (`exec`-ed by `/bin/sh`), arguments included.
    #[serde(default)]
    pub command: String,
    /// Service / job: working directory (default: the app directory).
    #[serde(default)]
    pub working_dir: String,
    /// Container: image reference.
    #[serde(default)]
    pub image: String,
    /// Container: `[ip:]host:container[/udp]` port mappings.
    #[serde(default)]
    pub ports: Vec<String>,
    /// Container: `host_path:container_path[:ro]` bind mounts.
    #[serde(default)]
    pub volumes: Vec<String>,
    /// Ordered environment variables (secrets already resolved).
    #[serde(default)]
    pub env: Vec<(String, String)>,
    /// App Store packages whose `/etc/forge/pkgs/<id>.env` is loaded first.
    #[serde(default)]
    pub env_files: Vec<String>,
    /// Restart when it exits (services, containers).
    #[serde(default = "yes")]
    pub restart: bool,
    /// App Store runtimes (Python, .NET, Java, Node.js, PHP, Go) the app needs
    /// on its node. Forge installs missing ones before deploying; their
    /// `/etc/forge/pkgs/<id>.env` is loaded like an `env_files` entry.
    #[serde(default)]
    pub dependencies: Vec<Dependency>,
    /// CPU limit in thousandths of a core (`1500` = 1.5 cores, `0` = none).
    #[serde(default)]
    pub cpu_millis: u32,
    /// Memory limit in MiB (`0` = none).
    #[serde(default)]
    pub memory_mb: u32,
    /// Container: entrypoint override (words separated by spaces).
    #[serde(default)]
    pub entrypoint: String,
    /// Container: user to run as (`name`, `uid` or `uid:gid`).
    #[serde(default)]
    pub user: String,
    /// Container: host name inside the container.
    #[serde(default)]
    pub hostname: String,
    /// Container: labels (`forge.*` is reserved).
    #[serde(default)]
    pub labels: Vec<(String, String)>,
    /// Container: extra `/etc/hosts` entries, `name:ipv4`.
    #[serde(default)]
    pub extra_hosts: Vec<String>,
    /// Container: health check.
    #[serde(default)]
    pub healthcheck: Option<HealthCheck>,
    /// Native service: the TCP port the app listens on, if any (D-085). Used by
    /// the default health check (TCP connect); not a published port.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listen_port: Option<u16>,
    /// Startup, readiness and liveness checks (D-085). Run by the node, not
    /// by Docker; independent of `healthcheck`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health: Option<crate::health::HealthPolicy>,
    /// Container: seconds to wait for a graceful stop (`0` = Docker's default).
    #[serde(default)]
    pub stop_timeout_secs: u32,
    /// Container: read-only root filesystem.
    #[serde(default)]
    pub read_only: bool,
    /// Container: rotate the JSON log at this size in MiB (`0` = Docker's default).
    #[serde(default)]
    pub log_max_mb: u32,
    /// The copy this spec runs as (#73): `""` for the service's single
    /// instance, `a` or `b` for one of two side-by-side slots, each with its
    /// own container or unit, env file and launcher.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub slot: String,
    /// The release number this copy runs (a label; empty outside deployments).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub release: String,
    /// Native services and jobs (#73): code to fetch into
    /// `<home>/releases/<release>` before the copy starts (a pinned Git
    /// commit, or an archive checked by its SHA-256).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact: Option<crate::deploy::ArtifactRef>,
    /// Container services in a Swarm cluster (D-154): replicas, placement,
    /// restart and update behaviour. Never sent to a node: Swarm clusters
    /// run the service through a manager.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swarm: Option<crate::swarm_service::SwarmSettings>,
}

impl std::fmt::Debug for AppSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Destructured, so a new field cannot be added without deciding here
        // whether it may be printed.
        let Self {
            environment,
            name,
            runtime,
            command,
            working_dir,
            image,
            ports,
            volumes,
            env,
            env_files,
            restart,
            dependencies,
            cpu_millis,
            memory_mb,
            entrypoint,
            user,
            hostname,
            labels,
            extra_hosts,
            healthcheck,
            listen_port,
            health,
            stop_timeout_secs,
            read_only,
            log_max_mb,
            slot,
            release,
            artifact,
            swarm,
        } = self;
        let env: Vec<String> = env.iter().map(|(k, _)| format!("{k}=***")).collect();
        f.debug_struct("AppSpec")
            .field("environment", environment)
            .field("name", name)
            .field("runtime", runtime)
            .field("command", command)
            .field("working_dir", working_dir)
            .field("image", image)
            .field("ports", ports)
            .field("volumes", volumes)
            .field("env", &env)
            .field("env_files", env_files)
            .field("restart", restart)
            .field("dependencies", dependencies)
            .field("cpu_millis", cpu_millis)
            .field("memory_mb", memory_mb)
            .field("entrypoint", entrypoint)
            .field("user", user)
            .field("hostname", hostname)
            .field("labels", labels)
            .field("extra_hosts", extra_hosts)
            .field("healthcheck", healthcheck)
            .field("listen_port", listen_port)
            .field("health", health)
            .field("stop_timeout_secs", stop_timeout_secs)
            .field("read_only", read_only)
            .field("log_max_mb", log_max_mb)
            .field("slot", slot)
            .field("release", release)
            // URLs may carry a signed query: the kind only.
            .field("artifact", &artifact.as_ref().map(crate::deploy::ArtifactRef::kind))
            .field("swarm", swarm)
            .finish()
    }
}

/// An App Store runtime a service needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dependency {
    pub package: String,
    pub version: String,
    /// Package parameters, e.g. `.NET` `flavor = sdk`.
    #[serde(default)]
    pub params: std::collections::BTreeMap<String, String>,
}

/// Container health check (Docker `HEALTHCHECK`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HealthCheck {
    /// Shell command; exit 0 = healthy.
    pub command: String,
    #[serde(default = "thirty")]
    pub interval_secs: u32,
    #[serde(default = "five")]
    pub timeout_secs: u32,
    #[serde(default = "three")]
    pub retries: u32,
    #[serde(default)]
    pub start_period_secs: u32,
}

const fn thirty() -> u32 {
    30
}
const fn five() -> u32 {
    5
}
const fn three() -> u32 {
    3
}

/// Runtimes a service may depend on (App Store category "runtimes").
#[must_use]
pub fn runtime_packages() -> Vec<&'static crate::Package> {
    crate::CATALOG.iter().filter(|p| p.category == crate::Category::Runtimes).collect()
}

const fn yes() -> bool {
    true
}

/// Start of the error a node answers when its Docker Engine cannot be
/// reached (not installed, or stopped). The agent's result carries only text,
/// so this one definition is what the control plane recognises it by.
pub const DOCKER_UNAVAILABLE: &str = "Docker is not available on this node";

/// The App Store package that provides the container runtime.
pub const DOCKER_PACKAGE: &str = "docker";

/// Actions on a service on a node. `prepare` creates a native app's
/// account and folder without deploying it, so code can be uploaded first.
pub const APP_ACTIONS: &[&str] =
    &["prepare", "deploy", "start", "stop", "restart", "status", "logs", "follow", "remove"];

/// Actions only rollouts send (#73): `probe` runs the copy's readiness
/// checks once and answers `{ready, checks}`; `retire` stops and removes one
/// copy (its container or unit, env file and launcher) and keeps the app's
/// account, folder and data.
pub const ROLLOUT_ACTIONS: &[&str] = &["probe", "retire"];

#[must_use]
pub fn valid_slug(s: &str, max: usize) -> bool {
    !s.is_empty()
        && s.len() <= max
        && s.as_bytes()[0].is_ascii_lowercase()
        && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !s.ends_with('-')
}

#[must_use]
pub fn valid_env_key(k: &str) -> bool {
    !k.is_empty()
        && k.len() <= 128
        && (k.as_bytes()[0].is_ascii_alphabetic() || k.starts_with('_'))
        && k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

fn clean_text(s: &str, max: usize) -> bool {
    s.len() <= max && !s.contains(['\n', '\r', '\0'])
}

fn valid_abs_path(p: &str) -> bool {
    p.starts_with('/')
        && p.len() <= 512
        && !p.split('/').any(|c| c == "..")
        && clean_text(p, 512)
        && !p.contains([':', ' '])
}

fn valid_port_map(p: &str) -> bool {
    let (body, proto) = p.split_once('/').map_or((p, "tcp"), |(b, pr)| (b, pr));
    let parts: Vec<&str> = body.split(':').collect();
    let port = |s: &str| s.parse::<u16>().is_ok_and(|n| n > 0);
    matches!(proto, "tcp" | "udp")
        && match parts.as_slice() {
            [h, c] => port(h) && port(c),
            [ip, h, c] => ip.parse::<std::net::IpAddr>().is_ok() && port(h) && port(c),
            _ => false,
        }
}

fn valid_volume(v: &str) -> bool {
    let parts: Vec<&str> = v.split(':').collect();
    match parts.as_slice() {
        [h, c] => valid_abs_path(h) && valid_abs_path(c),
        [h, c, "ro" | "rw"] => valid_abs_path(h) && valid_abs_path(c),
        _ => false,
    }
}

/// A container image reference: `registry/repo[:tag][@sha256:…]`.
#[must_use]
pub fn valid_image_ref(s: &str) -> bool {
    valid_image(s)
}

fn valid_image(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 255
        && s.as_bytes()[0].is_ascii_alphanumeric()
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b':' | b'.' | b'_' | b'-' | b'@'))
}

/// 32-bit FNV-1a, hex: a short stable suffix for user names.
/// Longest Linux user name `useradd` accepts.
pub const APP_USER_MAX: usize = 32;

/// Where native apps live: `<APP_HOMES>/<ns>/<app>`.
pub const APP_HOMES: &str = "/srv/forge/apps";

/// An app's home folder.
#[must_use]
pub fn app_home(environment: &str, app: &str) -> String {
    format!("{APP_HOMES}/{environment}/{app}")
}

/// The GECOS comment that marks an app's own account as Forge's (D-081).
#[must_use]
pub fn app_user_mark(environment: &str, app: &str) -> String {
    format!("Orbyn Forge app {environment}/{app}")
}

/// Names for an app's own system account, in order of preference (D-081):
/// `<environment>-<app>` when it fits Linux's 32 characters, then the same
/// shortened with a hash suffix (too long, or taken by an account Forge did
/// not create).
#[must_use]
pub fn app_user_candidates(environment: &str, app: &str) -> Vec<String> {
    let readable = format!("{environment}-{app}");
    let short: String = readable.chars().take(APP_USER_MAX - 7).collect();
    let hashed = format!("{}-{}", short.trim_end_matches('-'), short_hash(&readable));
    if readable.len() <= APP_USER_MAX { vec![readable, hashed] } else { vec![hashed] }
}

fn short_hash(s: &str) -> String {
    let mut h: u32 = 0x811c_9dc5;
    for b in s.bytes() {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x0100_0193);
    }
    format!("{:06x}", h & 0x00ff_ffff)
}

impl AppSpec {
    /// Validates everything the helper will write or run.
    pub fn validate(&self) -> Result<(), String> {
        if let Some(e) = self.health.as_ref().and_then(|h| h.validate().into_iter().next()) {
            return Err(format!("health.{}: {}", e.field, e.message));
        }
        if !matches!(self.slot.as_str(), "" | "a" | "b") {
            return Err("slot must be a or b".into());
        }
        if self.release.len() > 12 || !self.release.bytes().all(|b| b.is_ascii_digit()) {
            return Err("release must be a number".into());
        }
        if let Some(a) = &self.artifact {
            if self.runtime == Runtime::Container || !a.staged() {
                return Err("only native services and jobs fetch code (git or archive)".into());
            }
            if self.release.is_empty() {
                return Err("fetched code needs a release number".into());
            }
            a.validate()?;
        }
        if !valid_slug(&self.environment, 40) || !valid_slug(&self.name, 40) {
            return Err("environment and name must be lowercase letters, digits and dashes".into());
        }
        if self.env.len() > 500 {
            return Err("too many environment variables".into());
        }
        for (k, v) in &self.env {
            if !valid_env_key(k) {
                return Err(format!("invalid environment variable name `{k}`"));
            }
            if !clean_text(v, 32 * 1024) {
                return Err(format!("value of `{k}` may not contain line breaks"));
            }
        }
        for f in &self.env_files {
            if crate::find(f).is_none() {
                return Err(format!("unknown package env file `{f}`"));
            }
        }
        let mut seen = std::collections::HashSet::new();
        for d in &self.dependencies {
            let pkg = crate::find(&d.package).ok_or_else(|| format!("unknown runtime `{}`", d.package))?;
            if pkg.category != crate::Category::Runtimes {
                return Err(format!("`{}` is not a runtime", pkg.name));
            }
            if !seen.insert(d.package.as_str()) {
                return Err(format!("{} is listed twice", pkg.name));
            }
            crate::validate(&d.package, &d.version, "install", &d.params)?;
        }
        if self.dependencies.len() > 10 {
            return Err("at most 10 runtimes".into());
        }
        if self.cpu_millis > 256_000 || self.memory_mb > 1_048_576 {
            return Err("CPU or memory limit out of range".into());
        }
        let container_only = !self.entrypoint.is_empty()
            || !self.user.is_empty()
            || !self.hostname.is_empty()
            || !self.labels.is_empty()
            || !self.extra_hosts.is_empty()
            || self.healthcheck.is_some()
            || self.stop_timeout_secs > 0
            || self.read_only
            || self.log_max_mb > 0;
        if container_only && self.runtime != Runtime::Container {
            return Err("entrypoint, user, host name, labels, hosts, health check, stop timeout, read-only and log size apply to containers only".into());
        }
        if !clean_text(&self.entrypoint, 1024) {
            return Err("entrypoint may not contain line breaks".into());
        }
        let user_ok =
            self.user.len() <= 64 && self.user.bytes().all(|b| b.is_ascii_alphanumeric() || b"_-:.".contains(&b));
        if !self.user.is_empty() && !user_ok {
            return Err("user: a name, uid or uid:gid".into());
        }
        let host_ok =
            self.hostname.len() <= 63 && self.hostname.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
        if !self.hostname.is_empty() && !host_ok {
            return Err("host name: letters, digits and dashes".into());
        }
        if self.labels.len() > 50 {
            return Err("at most 50 labels".into());
        }
        for (k, v) in &self.labels {
            if k.is_empty()
                || k.len() > 128
                || k.starts_with("forge.")
                || !k.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-/".contains(&b))
            {
                return Err(format!("invalid label `{k}` (forge.* is reserved)"));
            }
            if !clean_text(v, 1024) {
                return Err(format!("label `{k}` may not contain line breaks"));
            }
        }
        for h in &self.extra_hosts {
            let ok = h.split_once(':').is_some_and(|(n, ip)| {
                !n.is_empty()
                    && n.len() <= 253
                    && n.bytes().all(|b| b.is_ascii_alphanumeric() || b".-".contains(&b))
                    && ip.parse::<std::net::Ipv4Addr>().is_ok()
            });
            if !ok {
                return Err(format!("invalid host entry `{h}` (name:ipv4)"));
            }
        }
        if let Some(hc) = &self.healthcheck {
            if hc.command.trim().is_empty() || !clean_text(&hc.command, 1024) {
                return Err("health check: a single-line command is required".into());
            }
            if !(1..=3600).contains(&hc.interval_secs)
                || !(1..=600).contains(&hc.timeout_secs)
                || !(1..=20).contains(&hc.retries)
                || hc.start_period_secs > 3600
            {
                return Err("health check timings out of range".into());
            }
        }
        if self.stop_timeout_secs > 3600 || self.log_max_mb > 10_240 {
            return Err("stop timeout or log size out of range".into());
        }
        match (self.runtime, self.listen_port) {
            (_, Some(0)) => return Err("the port must be between 1 and 65535".into()),
            (Runtime::Container, Some(_)) => {
                return Err("a listening port applies to native services only (containers publish ports)".into());
            }
            _ => {}
        }
        match self.runtime {
            Runtime::Systemd => {
                if self.command.trim().is_empty() || !clean_text(&self.command, 4096) {
                    return Err("a single-line command is required".into());
                }
                if !self.working_dir.is_empty() && !valid_abs_path(&self.working_dir) {
                    return Err("working directory must be an absolute path".into());
                }
                if !self.image.is_empty() || !self.ports.is_empty() || !self.volumes.is_empty() {
                    return Err("image, ports and volumes apply to containers only".into());
                }
            }
            Runtime::Container => {
                if !valid_image(&self.image) {
                    return Err("a valid image reference is required".into());
                }
                if let Some(p) = self.ports.iter().find(|p| !valid_port_map(p)) {
                    return Err(format!("invalid port mapping `{p}`"));
                }
                if let Some(v) = self.volumes.iter().find(|v| !valid_volume(v)) {
                    return Err(format!("invalid volume `{v}` (use /host/path:/container/path[:ro])"));
                }
                // The same host-mount rules as compose stacks: never the
                // Docker socket, system paths or Forge's own folders.
                for v in &self.volumes {
                    let host = v.split(':').next().unwrap_or_default();
                    if let Some(why) = crate::stacks::protected_host_path(host) {
                        return Err(format!("volume `{v}` is not allowed: {why}"));
                    }
                }
                if !self.command.is_empty() && !clean_text(&self.command, 4096) {
                    return Err("command may not contain line breaks".into());
                }
            }
        }
        Ok(())
    }

    /// `<ns>-<name>`: unit and container suffix.
    #[must_use]
    pub fn key(&self) -> String {
        format!("{}-{}", self.environment, self.name)
    }

    /// Where a release's fetched code lives: `<home>/releases/<release>`.
    #[must_use]
    pub fn release_dir(&self) -> String {
        format!("{}/releases/{}", self.home(), self.release)
    }

    /// `--<slot>` for a slot, empty for the single instance.
    #[must_use]
    pub fn slot_suffix(&self) -> String {
        if self.slot.is_empty() { String::new() } else { format!("--{}", self.slot) }
    }

    /// systemd unit (without suffix) or container name; slots add `--a`/`--b`.
    #[must_use]
    pub fn unit(&self) -> String {
        format!("forge-app-{}{}", self.key(), self.slot_suffix())
    }

    /// The GECOS comment that marks the app's own account as Forge's (D-081).
    /// The agent adopts an existing account only when it carries exactly this
    /// mark, so it never takes over a system or human account.
    #[must_use]
    pub fn user_mark(&self) -> String {
        app_user_mark(&self.environment, &self.name)
    }

    /// Names for the app's own system account, in order of preference
    /// (D-081): `<environment>-<app>`, readable in `ps`, `ls -l` and audit
    /// records, when it fits Linux's 32 characters; then the same shortened
    /// with a hash suffix, used when the readable name is too long or an
    /// account Forge did not create already has it. Slugs start with a
    /// letter and use `a-z`, `0-9` and `-`, so every name is a valid user name.
    #[must_use]
    pub fn user_candidates(&self) -> Vec<String> {
        app_user_candidates(&self.environment, &self.name)
    }

    /// The account name before D-081 (`fa-<app>-<hash>`). The next deploy
    /// renames it to the readable name and keeps its uid, so file ownership
    /// survives.
    #[must_use]
    pub fn legacy_user(&self) -> String {
        let name: String = self.name.chars().take(22).collect();
        format!("fa-{}-{}", name.trim_end_matches('-'), short_hash(&self.key()))
    }

    #[must_use]
    pub fn home(&self) -> String {
        app_home(&self.environment, &self.name)
    }

    #[must_use]
    pub fn env_path(&self) -> String {
        format!("/etc/forge/apps/{}/{}{}.env", self.environment, self.name, self.slot_suffix())
    }

    #[must_use]
    pub fn launcher(&self) -> String {
        format!("{}/.forge/{}", self.home(), self.launcher_file())
    }

    /// The launcher's file name in `<home>/.forge`.
    #[must_use]
    pub fn launcher_file(&self) -> String {
        format!("run{}.sh", self.slot_suffix())
    }

    /// The default `FORGE_LOG_URL` for this runtime (D-142): where the app
    /// sends Seq/CLEF logs, the node's local listener. A value the app sets
    /// itself wins.
    #[must_use]
    pub fn log_url(&self) -> &'static str {
        if self.runtime == Runtime::Container { LOG_URL_CONTAINER } else { LOG_URL_HOST }
    }

    /// `KEY=value` lines, valid for systemd `EnvironmentFile=` and `docker --env-file`.
    #[must_use]
    pub fn env_file(&self) -> String {
        let mut s = format!("# Managed by Orbyn Forge: {}/{}\n", self.environment, self.name);
        for (k, v) in &self.env {
            s.push_str(k);
            s.push('=');
            s.push_str(v);
            s.push('\n');
        }
        s
    }

    #[must_use]
    pub fn launcher_script(&self) -> String {
        format!(
            "#!/bin/sh\n# Managed by Orbyn Forge: {}/{}\n{}exec {}\n",
            self.environment,
            self.name,
            self.runtime_env(),
            self.command.trim()
        )
    }

    /// Shell lines that put the app's runtimes on `PATH` and set their usual
    /// variables. Versions are validated against the catalog, so they are safe
    /// to interpolate.
    fn runtime_env(&self) -> String {
        let mut s = String::new();
        for d in &self.dependencies {
            let v = &d.version;
            match d.package.as_str() {
                "dotnet" => s.push_str(&format!("DOTNET_ROOT=/opt/forge/pkgs/dotnet/{v}; export DOTNET_ROOT\nPATH=\"$DOTNET_ROOT:$PATH\"\n")),
                "go" => s.push_str(&format!("GOROOT=/opt/forge/pkgs/go/{v}; export GOROOT\nPATH=\"$GOROOT/bin:$PATH\"\n")),
                "java" => s.push_str("if [ -n \"${FORGE_PKG_JAVA_HOME:-}\" ]; then JAVA_HOME=\"$FORGE_PKG_JAVA_HOME\"; export JAVA_HOME; PATH=\"$JAVA_HOME/bin:$PATH\"; fi\n"),
                _ => {}
            }
        }
        if !self.runtime_links().is_empty() {
            s.push_str(&format!("PATH=\"{}/.forge/bin:$PATH\"\n", self.home()));
        }
        if !s.is_empty() {
            s.push_str("export PATH\n");
        }
        s
    }

    /// Commands that must resolve to a chosen runtime version, as
    /// `(name, target)` links under `<home>/.forge/bin` (e.g. `php` → `/usr/bin/php8.3`).
    #[must_use]
    pub fn runtime_links(&self) -> Vec<(String, String)> {
        self.dependencies
            .iter()
            .filter(|d| d.package == "php")
            .map(|d| ("php".to_owned(), format!("/usr/bin/php{}", d.version)))
            .collect()
    }

    /// Package env files to load: `env_files` plus every runtime dependency.
    #[must_use]
    pub fn pkg_env_files(&self) -> Vec<String> {
        let mut out = self.env_files.clone();
        for d in &self.dependencies {
            if !out.contains(&d.package) {
                out.push(d.package.clone());
            }
        }
        out
    }

    fn working_dir_or_home(&self) -> String {
        if self.working_dir.is_empty() { self.home() } else { self.working_dir.clone() }
    }

    /// The systemd service unit, running as `user` (the app's own account,
    /// resolved on the node).
    #[must_use]
    pub fn service_unit(&self, user: &str) -> String {
        let mut env_files = String::new();
        for f in &self.pkg_env_files() {
            env_files.push_str(&format!("EnvironmentFile=-/etc/forge/pkgs/{f}.env\n"));
        }
        let mut limits = String::new();
        if self.cpu_millis > 0 {
            limits.push_str(&format!("CPUQuota={}%\n", self.cpu_millis.div_ceil(10)));
        }
        if self.memory_mb > 0 {
            limits.push_str(&format!("MemoryMax={}M\n", self.memory_mb));
        }
        let restart = if self.restart { "Restart=always\nRestartSec=3\n" } else { "" };
        format!(
            "# Managed by Orbyn Forge\n[Unit]\nDescription=Orbyn Forge app {ns}/{name}\nAfter=network-online.target\nWants=network-online.target\n\n\
             [Service]\nUser={user}\nGroup={user}\nWorkingDirectory={wd}\nEnvironment={LOG_URL_ENV}={LOG_URL_HOST}\n{env_files}EnvironmentFile={env}\nExecStart={launcher}\n{restart}{limits}\
             NoNewPrivileges=true\nPrivateTmp=true\nProtectSystem=full\nSyslogIdentifier={unit}\n\n[Install]\nWantedBy=multi-user.target\n",
            ns = self.environment,
            name = self.name,
            wd = self.working_dir_or_home(),
            env = self.env_path(),
            launcher = self.launcher(),
            unit = self.unit(),
        )
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn svc() -> AppSpec {
        AppSpec {
            environment: "staging".into(),
            name: "api".into(),
            runtime: Runtime::Systemd,
            command: "/srv/forge/apps/staging/api/venv/bin/gunicorn app:app -b 127.0.0.1:8000".into(),
            working_dir: String::new(),
            image: String::new(),
            ports: vec![],
            volumes: vec![],
            env: vec![("DATABASE_URL".into(), "postgres://u:p@db/x".into()), ("WORKERS".into(), "4".into())],
            env_files: vec!["postgresql".into()],
            restart: true,
            dependencies: vec![],
            cpu_millis: 0,
            memory_mb: 0,
            entrypoint: String::new(),
            user: String::new(),
            hostname: String::new(),
            labels: vec![],
            extra_hosts: vec![],
            healthcheck: None,
            listen_port: None,
            health: None,
            stop_timeout_secs: 0,
            read_only: false,
            slot: String::new(),
            release: String::new(),
            artifact: None,
            swarm: None,
            log_max_mb: 0,
        }
    }

    #[test]
    fn debug_output_hides_environment_values() {
        let mut s = svc();
        s.env = vec![("DATABASE_PASSWORD".into(), "hunter2-very-secret".into()), ("PORT".into(), "8080".into())];
        let d = format!("{s:?}");
        assert!(!d.contains("hunter2-very-secret") && !d.contains("8080"), "{d}");
        assert!(d.contains("DATABASE_PASSWORD=***") && d.contains("PORT=***"), "{d}");
    }

    #[test]
    fn app_users_are_readable_and_fit_linux() {
        let s = svc();
        assert_eq!(s.user_candidates()[0], "staging-api", "readable first");
        assert_eq!(s.user_mark(), "Orbyn Forge app staging/api");
        assert_eq!(s.legacy_user(), format!("fa-api-{}", short_hash("staging-api")));
        let mut long = svc();
        long.environment = "production-europe-west".into();
        long.name = "billing-reconciliation-worker".into();
        let c = long.user_candidates();
        assert_eq!(c.len(), 1, "too long for the readable form: {c:?}");
        for s in [&svc(), &long] {
            for u in s.user_candidates() {
                assert!(u.len() <= APP_USER_MAX, "{u}");
                assert!(u.as_bytes()[0].is_ascii_lowercase() && !u.ends_with('-'), "{u}");
                assert!(u.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'), "{u}");
            }
        }
        assert!(c[0].starts_with("production-europe-west-bi-") && c[0].len() == APP_USER_MAX, "{c:?}");
        // The fallback differs from the readable name and is stable.
        let fb = &svc().user_candidates()[1];
        assert!(fb.starts_with("staging-api-") && fb.len() == "staging-api-".len() + 6, "{fb}");
        assert_eq!(&svc().user_candidates()[1], fb);
    }

    #[test]
    fn service_spec_renders_unit_env_and_launcher() {
        let s = svc();
        s.validate().unwrap();
        let unit = s.service_unit("staging-api");
        assert!(unit.contains("EnvironmentFile=-/etc/forge/pkgs/postgresql.env"));
        assert!(unit.contains("EnvironmentFile=/etc/forge/apps/staging/api.env"));
        let log = unit.find("Environment=FORGE_LOG_URL=http://127.0.0.1:5341").unwrap();
        assert!(log < unit.find("EnvironmentFile=/etc").unwrap(), "the app's own value wins");
        assert!(unit.contains("User=staging-api\nGroup=staging-api\n"));
        assert!(unit.contains("Restart=always"));
        assert_eq!(s.env_file().lines().nth(1), Some("DATABASE_URL=postgres://u:p@db/x"));
        assert!(s.launcher_script().ends_with("-b 127.0.0.1:8000\n"));
    }

    #[test]
    fn rejects_injection_and_bad_input() {
        let mut s = svc();
        s.env.push(("BAD".into(), "a\nExecStartPre=/bin/evil".into()));
        assert!(s.validate().is_err(), "newline in value");
        let mut s = svc();
        s.env.push(("1BAD".into(), "x".into()));
        assert!(s.validate().is_err());
        let mut s = svc();
        s.command = "run\nmore".into();
        assert!(s.validate().is_err());
        let mut s = svc();
        s.name = "../etc".into();
        assert!(s.validate().is_err());
        let mut s = svc();
        s.env_files = vec!["nope".into()];
        assert!(s.validate().is_err());
    }

    #[test]
    fn container_and_job_specs() {
        let mut c = svc();
        c.runtime = Runtime::Container;
        c.command = String::new();
        c.image = "ghcr.io/org/api:1.4".into();
        c.ports = vec!["8080:80".into(), "127.0.0.1:9000:9000/tcp".into()];
        c.volumes = vec!["/srv/forge/apps/staging/api/data:/data".into()];
        c.validate().unwrap();
        c.ports = vec!["80".into()];
        assert!(c.validate().is_err());
        c.ports = vec![];
        c.volumes = vec!["/../x:/data".into()];
        assert!(c.validate().is_err());
        // Host takeover through volumes is refused (security regression).
        for v in [
            "/:/host",
            "/var/run/docker.sock:/s",
            "/run/docker.sock:/s",
            "/var/lib/forge-agent:/a",
            "/etc:/e:ro",
            "/opt/forge/stacks/a/b:/x",
            "/var/lib/docker/volumes:/v",
            "//etc/./ssh:/x",
        ] {
            c.volumes = vec![v.into()];
            assert!(c.validate().is_err(), "{v}");
        }
        c.volumes = vec!["/srv/data/api:/data".into()];
        c.validate().unwrap();
    }

    #[test]
    fn a_listening_port_is_for_native_services_only() {
        let spec = |v: serde_json::Value| -> AppSpec { serde_json::from_value(v).unwrap() };
        let mut s = spec(serde_json::json!({
            "environment": "pay", "name": "api", "runtime": "systemd", "command": "python3 app.py", "listen_port": 8080
        }));
        s.validate().unwrap();
        assert_eq!(serde_json::to_value(&s).unwrap()["listen_port"], 8080);
        s.listen_port = Some(0);
        assert!(s.validate().is_err(), "port 0");
        s.listen_port = None;
        assert!(serde_json::to_value(&s).unwrap().get("listen_port").is_none(), "omitted when unset");
        let ctr = spec(serde_json::json!({
            "environment": "pay", "name": "web", "runtime": "container", "image": "nginx:1", "listen_port": 80
        }));
        assert!(ctr.validate().unwrap_err().contains("containers publish ports"));
    }
    #[test]
    fn dependencies_and_container_options_are_validated() {
        let base = || -> AppSpec {
            serde_json::from_value(serde_json::json!({
                "environment": "pay", "name": "api", "runtime": "systemd", "command": "python3 app.py"
            }))
            .unwrap()
        };
        let mut s = base();
        s.dependencies =
            vec![Dependency { package: "python".into(), version: "system".into(), params: Default::default() }];
        s.validate().unwrap();
        s.dependencies.push(s.dependencies[0].clone());
        assert!(s.validate().is_err(), "duplicate runtime");
        s.dependencies =
            vec![Dependency { package: "redis".into(), version: "distro".into(), params: Default::default() }];
        assert!(s.validate().is_err(), "not a runtime");
        s.dependencies = vec![Dependency {
            package: "dotnet".into(),
            version: "8.0".into(),
            params: [("flavor".to_owned(), "sdk".to_owned())].into(),
        }];
        s.validate().unwrap();

        let mut s = base();
        s.healthcheck = Some(HealthCheck {
            command: "true".into(),
            interval_secs: 30,
            timeout_secs: 5,
            retries: 3,
            start_period_secs: 0,
        });
        assert!(s.validate().is_err(), "health check is for containers");

        let mut c: AppSpec = serde_json::from_value(serde_json::json!({
            "environment": "shop", "name": "web", "runtime": "container", "image": "nginx:alpine",
            "cpu_millis": 500, "memory_mb": 256, "user": "101:101", "hostname": "web",
            "labels": [["team", "shop"]], "extra_hosts": ["db:10.64.1.5"],
            "healthcheck": {"command": "wget -qO- localhost"}
        }))
        .unwrap();
        c.validate().unwrap();
        c.labels = vec![("forge.app".into(), "x".into())];
        assert!(c.validate().is_err(), "reserved label");
        c.labels.clear();
        c.extra_hosts = vec!["db:not-an-ip".into()];
        assert!(c.validate().is_err());
    }

    #[test]
    fn launcher_puts_runtimes_on_path() {
        let mut s: AppSpec = serde_json::from_value(serde_json::json!({
            "environment": "pay", "name": "api", "runtime": "systemd", "command": "dotnet Api.dll",
            "dependencies": [
                {"package": "dotnet", "version": "8.0", "params": {"flavor": "aspnetcore"}},
                {"package": "php", "version": "8.3"}
            ]
        }))
        .unwrap();
        s.validate().unwrap();
        let l = s.launcher_script();
        assert!(l.contains("DOTNET_ROOT=/opt/forge/pkgs/dotnet/8.0"));
        assert!(l.contains("PATH=\"/srv/forge/apps/pay/api/.forge/bin:$PATH\""));
        assert!(l.trim_end().ends_with("exec dotnet Api.dll"));
        assert_eq!(s.runtime_links(), vec![("php".to_owned(), "/usr/bin/php8.3".to_owned())]);
        s.dependencies.clear();
        assert_eq!(s.launcher_script().lines().count(), 3, "no runtime lines without dependencies");
    }
}
