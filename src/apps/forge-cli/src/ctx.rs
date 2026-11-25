//! Local contexts: named connections to Forge deployments.
//!
//! Stored as JSON in the user's config directory (`FORGE_CONFIG_DIR`
//! overrides it). Writes are atomic (temp file + rename) under a lock file,
//! files are owner-only on Unix, and a command snapshots its context once at
//! start, so another process switching contexts cannot redirect it. No
//! secrets are stored here: tokens live in the credential store (`creds`).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::error::{CliError, Exit, Result};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub current: Option<String>,
    #[serde(default)]
    pub contexts: BTreeMap<String, Context>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Context {
    /// Canonical server URL, e.g. `https://forge.example.com`.
    pub server: String,
    /// The deployment's workspace id, learned at first login. Guards
    /// against a URL that now points somewhere else.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance_id: Option<String>,
    /// PEM file with an extra CA for on-premises servers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_cluster: Option<String>,
    /// Signed-in user (display only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// Local ports `forge tunnel open` picked for a target, so the same
    /// service lands on the same local port next time. Keyed
    /// `"<kind>:<target>@<node>:<remote port>"`; a free port is a property
    /// of this machine, so it belongs here and not on the server.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tunnel_ports: BTreeMap<String, u16>,
}

/// Context names: letters, digits, `.`, `_`, `-`; 1–64 characters.
pub fn valid_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));

    if ok {
        Ok(())
    } else {
        Err(CliError::usage(format!("invalid context name `{name}`: use letters, digits, '.', '_' or '-'")))
    }
}

/// Normalises and checks a server URL: https, or http only on loopback.
pub fn normalise_server(raw: &str) -> Result<String> {
    let url = url::Url::parse(raw.trim()).map_err(|_| CliError::usage(format!("invalid server URL `{raw}`")))?;
    let host = url.host_str().unwrap_or_default().to_owned();

    if !url.username().is_empty() || url.password().is_some() || url.query().is_some() || url.fragment().is_some() {
        return Err(CliError::usage("the server URL must not contain credentials, a query or a fragment"));
    }

    match url.scheme() {
        "https" => {}
        "http" if is_loopback(&host) || std::env::var_os("FORGE_ALLOW_HTTP").is_some() => {}
        "http" => {
            return Err(CliError::usage(
                "plain http is only allowed for localhost (set FORGE_ALLOW_HTTP=1 for a lab without TLS)",
            ));
        }
        s => return Err(CliError::usage(format!("unsupported URL scheme `{s}`"))),
    }

    Ok(url.as_str().trim_end_matches('/').to_owned())
}

fn is_loopback(host: &str) -> bool {
    host == "localhost"
        || host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
        || host.trim_matches(['[', ']']).parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// The CLI's config directory.
pub fn config_dir() -> Result<PathBuf> {
    if let Some(d) = std::env::var_os("FORGE_CONFIG_DIR") {
        return Ok(PathBuf::from(d));
    }

    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("OrbynForge"))
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Service Support/OrbynForge"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(|x| PathBuf::from(x).join("orbyn-forge"))
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config/orbyn-forge")))
    };

    base.map(|b| b.join("cli"))
        .ok_or_else(|| CliError::new(Exit::General, "cannot find a config directory (set FORGE_CONFIG_DIR)"))
}

fn ensure_dir(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Writes `data` to `path` atomically with owner-only permissions.
pub fn write_private(path: &Path, data: &[u8]) -> Result<()> {
    let dir = path.parent().ok_or_else(|| CliError::new(Exit::General, "bad config path"))?;
    ensure_dir(dir)?;
    let tmp =
        dir.join(format!(".{}.tmp-{}", path.file_name().and_then(|n| n.to_str()).unwrap_or("f"), std::process::id()));

    {
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp)?;
        std::io::Write::write_all(&mut f, data)?;
        f.sync_all()?;
    }

    fs::rename(&tmp, path).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })?;

    Ok(())
}

/// An exclusive lock on the config directory (lock file, removed on drop).
#[derive(Debug)]
pub struct Lock(PathBuf);

impl Lock {
    pub fn acquire(dir: &Path) -> Result<Self> {
        ensure_dir(dir)?;
        let path = dir.join(".lock");
        let deadline = Instant::now() + Duration::from_secs(10);

        loop {
            match fs::OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(_) => return Ok(Self(path)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    // A crashed process can leave the file behind: break locks older than 30 s.
                    let stale = fs::metadata(&path)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.elapsed().ok())
                        .is_some_and(|age| age > Duration::from_secs(30));

                    if stale {
                        let _ = fs::remove_file(&path);
                        continue;
                    }

                    if Instant::now() >= deadline {
                        return Err(CliError::new(
                            Exit::Timeout,
                            format!("config is locked by another forge process ({})", path.display()),
                        ));
                    }

                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => return Err(e.into()),
            }
        }
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

impl Config {
    fn path(dir: &Path) -> PathBuf {
        dir.join("config.json")
    }

    pub fn load_from(dir: &Path) -> Result<Self> {
        match fs::read(Self::path(dir)) {
            Ok(b) => Ok(serde_json::from_slice(&b)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn load() -> Result<Self> {
        Self::load_from(&config_dir()?)
    }

    /// Re-reads, applies `f` and saves, all under the lock.
    pub fn update<T>(f: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        let dir = config_dir()?;
        let _lock = Lock::acquire(&dir)?;
        let mut cfg = Self::load_from(&dir)?;
        let out = f(&mut cfg)?;
        write_private(&Self::path(&dir), &serde_json::to_vec_pretty(&cfg)?)?;

        Ok(out)
    }

    pub fn get(&self, name: &str) -> Result<&Context> {
        self.contexts
            .get(name)
            .ok_or_else(|| CliError::new(Exit::NotFound, format!("no context `{name}` (see `forge contexts list`)")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_urls_are_checked() {
        assert_eq!(normalise_server("https://forge.example.com/").ok().as_deref(), Some("https://forge.example.com"));
        assert!(normalise_server("http://localhost:8080").is_ok());
        assert!(normalise_server("http://127.0.0.1:8080").is_ok());
        assert!(normalise_server("ftp://x").is_err());
        assert!(normalise_server("https://user:pw@forge.example.com").is_err());
        assert!(normalise_server("https://forge.example.com/?next=x").is_err());
    }

    #[test]
    fn names_are_checked() {
        assert!(valid_name("company-a").is_ok());
        assert!(valid_name("").is_err());
        assert!(valid_name("a b").is_err());
        assert!(valid_name("../x").is_err());
    }

    #[test]
    fn config_round_trips_atomically() {
        let dir = tempfile::tempdir().ok();
        let Some(dir) = dir else { return };
        let mut cfg = Config::default();
        cfg.contexts.insert("a".into(), Context { server: "https://a".into(), ..Context::default() });
        cfg.current = Some("a".into());
        let path = Config::path(dir.path());
        assert!(write_private(&path, &serde_json::to_vec(&cfg).unwrap_or_default()).is_ok());
        let back = Config::load_from(dir.path()).unwrap_or_default();
        assert_eq!(back.current.as_deref(), Some("a"));
        assert_eq!(back.contexts["a"].server, "https://a");
        let lock = Lock::acquire(dir.path());
        assert!(lock.is_ok());
    }

    #[test]
    fn remembered_tunnel_ports_round_trip_and_stay_out_when_empty() {
        let mut c = Context { server: "https://a".into(), ..Context::default() };
        let plain = serde_json::to_string(&c).unwrap_or_default();
        assert!(!plain.contains("tunnel_ports"), "no empty map in the file");
        c.tunnel_ports.insert("app:site@lab-b:8080".into(), 18080);
        let text = serde_json::to_string(&c).unwrap_or_default();
        let back: Context = serde_json::from_str(&text).unwrap_or_default();
        assert_eq!(back.tunnel_ports.get("app:site@lab-b:8080"), Some(&18080));
        assert_eq!(back, c);
    }
}
