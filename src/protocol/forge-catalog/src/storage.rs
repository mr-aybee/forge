//! Object store operations on a node (RustFS first; Garage later behind the
//! same operations).
//!
//! Shared by the control plane (builds and validates the operation) and the
//! node (validates again, then runs the provider script). Every store is an
//! *instance* named by its slug, so several stores can share a node with
//! distinct services, containers, ports and data paths:
//!
//! | | Linux | Windows |
//! |---|---|---|
//! | Service / container | `forge-objstore-<slug>` | `forge-objstore-<slug>` |
//! | Binaries (native) | `/opt/forge/objstore/<slug>/<release>/` | `%ProgramData%\OrbynForge\objstore\<slug>\<release>\` |
//! | Settings (0600) | `/etc/forge/objstore/<slug>.env` | `%ProgramData%\OrbynForge\objstore\<slug>.env` |
//!
//! The data path belongs to the operator: it is never created on a missing
//! mount point's behalf and never deleted by `uninstall`.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

/// Providers Forge manages.
pub const PROVIDERS: &[&str] = &["rustfs"];

/// The Linux script (POSIX sh) for RustFS; prepended with [`crate::PRELUDE`].
pub const RUSTFS_SH: &str = include_str!("../recipes/objstore-rustfs.sh");
/// The Windows script (PowerShell) for RustFS.
pub const RUSTFS_PS1: &str = include_str!("../recipes/objstore-rustfs.ps1");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Runtime {
    /// The release binary as a system service (systemd or a Windows service).
    #[default]
    Native,
    /// A pinned container image on the node's Docker engine (Linux).
    Docker,
}

impl Runtime {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Docker => "docker",
        }
    }
}

/// What the node needs to run one store instance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoreSpec {
    pub provider: String,
    pub slug: String,
    /// `latest` or an exact release (`1.0.0-alpha.67`).
    pub version: String,
    pub runtime: Runtime,
    pub s3_port: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub console_port: Option<u16>,
    /// Exclusive data directories (absolute), one per drive. Standalone
    /// Windows stores have exactly one.
    pub data_paths: Vec<String>,
    /// Distributed (HA) stores: every drive of every member as
    /// `http://host:port/path`, identical on all members and in the same
    /// order. Empty for a standalone store.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub peers: Vec<String>,
    /// Refuse to start unless every data path is its own mount point (a
    /// dedicated volume), so a missing mount never fills the root filesystem.
    #[serde(default)]
    pub require_mount: bool,
    /// The App Store `rustfs` container (`forge-rustfs`, data in
    /// `/var/lib/forge/pkgs/rustfs`), adopted in place instead of recreated.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub adopted: bool,
    /// The store id, written as the ownership marker `.forge-objstore` into
    /// every data path. Data is deleted only where the marker matches. Empty
    /// for agents that predate markers (the field is then left out).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub owner: String,
}

/// The App Store's RustFS container, which a store can adopt.
pub const LEGACY_CONTAINER: &str = "forge-rustfs";
pub const LEGACY_DATA: &str = "/var/lib/forge/pkgs/rustfs";

/// Bootstrap credential for the instance. Travels only inside the signed
/// command (mTLS link) and the root helper's socket; never logged.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoreCredential {
    pub access_key: String,
    pub secret_key: String,
}

impl fmt::Debug for StoreCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StoreCredential").field("access_key", &self.access_key).field("secret_key", &"***").finish()
    }
}

/// One operation on one store instance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum StoreOp {
    /// Install (or repair) the instance and start it. Idempotent.
    Install {
        spec: StoreSpec,
        credential: StoreCredential,
    },
    Start {
        spec: StoreSpec,
    },
    Stop {
        spec: StoreSpec,
    },
    Restart {
        spec: StoreSpec,
    },
    /// Process state, release, and the data path's mount and capacity.
    Status {
        spec: StoreSpec,
    },
    /// Removes the service or container and binaries. Keeps the data unless
    /// `delete_data` is set (sent only when true, so older agents keep working).
    Uninstall {
        spec: StoreSpec,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        delete_data: bool,
        /// With `delete_data`: other stores' data paths on this node. A path
        /// equal to, inside or containing one of them is never deleted.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        protect: Vec<String>,
    },
    /// Takes over the App Store container without recreating it: checks it
    /// runs and reports its credential once (to be sealed in the vault).
    Adopt {
        spec: StoreSpec,
    },
}

impl StoreOp {
    #[must_use]
    pub const fn spec(&self) -> &StoreSpec {
        match self {
            Self::Install { spec, .. }
            | Self::Start { spec }
            | Self::Stop { spec }
            | Self::Restart { spec }
            | Self::Status { spec }
            | Self::Uninstall { spec, .. }
            | Self::Adopt { spec } => spec,
        }
    }

    #[must_use]
    pub const fn action(&self) -> &'static str {
        match self {
            Self::Install { .. } => "install",
            Self::Start { .. } => "start",
            Self::Stop { .. } => "stop",
            Self::Restart { .. } => "restart",
            Self::Status { .. } => "status",
            Self::Uninstall { .. } => "uninstall",
            Self::Adopt { .. } => "adopt",
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        self.spec().validate()?;
        match self {
            Self::Adopt { spec } if !spec.adopted => return Err("adopt needs an adopted spec".into()),
            Self::Install { spec, .. } if spec.adopted => {
                return Err("an adopted App Store container is kept as it is; it cannot be reinstalled".into());
            }
            _ => {}
        }
        if let Self::Uninstall { protect, .. } = self {
            for p in protect {
                if p.contains('|') || valid_data_path(p).is_err() && p != LEGACY_DATA {
                    return Err(format!("protected path `{p}` is not a data path"));
                }
            }
        }
        if let Self::Install { credential, .. } = self {
            credential.validate()?;
        }
        Ok(())
    }

    /// Variables for the provider script. Includes the credential for
    /// `install`; callers must never log the result.
    #[must_use]
    pub fn env(&self) -> BTreeMap<String, String> {
        let s = self.spec();
        let mut env = BTreeMap::from([
            // The prelude expects a package id and version.
            ("FORGE_PKG_ID".to_owned(), format!("objstore-{}", s.slug)),
            ("FORGE_PKG_VERSION".to_owned(), s.version.clone()),
            ("FORGE_ACTION".to_owned(), self.action().to_owned()),
            ("FORGE_OS_SLUG".to_owned(), s.slug.clone()),
            ("FORGE_OS_VERSION".to_owned(), s.version.clone()),
            ("FORGE_OS_RUNTIME".to_owned(), s.runtime.as_str().to_owned()),
            ("FORGE_OS_S3_PORT".to_owned(), s.s3_port.to_string()),
            ("FORGE_OS_CONSOLE_PORT".to_owned(), s.console_port.map(|p| p.to_string()).unwrap_or_default()),
            // Windows (standalone) uses the first path; Linux paths have no spaces.
            ("FORGE_OS_DATA".to_owned(), s.data_paths.first().cloned().unwrap_or_default()),
            ("FORGE_OS_DATA_ALL".to_owned(), s.data_paths.join(" ")),
            ("FORGE_OS_VOLUMES".to_owned(), s.peers.join(" ")),
            ("FORGE_OS_REQUIRE_MOUNT".to_owned(), if s.require_mount { "1" } else { "0" }.to_owned()),
            ("FORGE_OS_ADOPTED".to_owned(), if s.adopted { "1" } else { "0" }.to_owned()),
            ("FORGE_OS_OWNER".to_owned(), s.owner.clone()),
            (
                "FORGE_OS_DELETE_DATA".to_owned(),
                if matches!(self, Self::Uninstall { delete_data: true, .. }) { "1" } else { "0" }.to_owned(),
            ),
        ]);
        if let Self::Uninstall { protect, .. } = self {
            env.insert("FORGE_OS_PROTECT".to_owned(), protect.join("|"));
        }
        if let Self::Install { credential, .. } = self {
            env.insert("FORGE_OS_ACCESS_KEY".to_owned(), credential.access_key.clone());
            env.insert("FORGE_OS_SECRET_KEY".to_owned(), credential.secret_key.clone());
        }
        env
    }

    /// The Linux script for this operation's provider (prelude + provider).
    #[must_use]
    pub fn linux_script(&self) -> String {
        format!("{}\n# ---- object store: rustfs ----\n{RUSTFS_SH}\n", crate::PRELUDE)
    }
}

/// Drives (data paths) one member may have.
pub const MAX_DRIVES_PER_NODE: usize = 16;
/// RustFS distributed layouts Forge sets up: at least 4 drives in total
/// (the documented minimum layout), at most 64.
pub const MIN_DISTRIBUTED_DRIVES: usize = 4;
pub const MAX_DISTRIBUTED_DRIVES: usize = 64;

/// Hostnames and IP addresses members reach each other on.
#[must_use]
pub fn valid_host(h: &str) -> bool {
    !h.is_empty()
        && h.len() <= 253
        && h.as_bytes()[0].is_ascii_alphanumeric()
        && h.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-'))
}

/// Builds one drive entry of a distributed layout.
#[must_use]
pub fn peer(host: &str, port: u16, path: &str) -> String {
    format!("http://{host}:{port}{path}")
}

/// Splits `http://host:port/path`, checking host, port and path.
pub fn parse_peer(peer: &str, port: u16) -> Result<(&str, &str), String> {
    let bad = || format!("drive `{peer}`: http://host:{port}/absolute/path");
    let rest = peer.strip_prefix("http://").ok_or_else(bad)?;
    let (authority, _) = rest.split_once('/').ok_or_else(bad)?;
    let path = &rest[authority.len()..];
    let (host, p) = authority.rsplit_once(':').ok_or_else(bad)?;
    if !valid_host(host) || p != port.to_string() || !path.starts_with('/') {
        return Err(bad());
    }
    valid_data_path(path)?;
    Ok((host, path))
}

/// Instance names: `[a-z][a-z0-9-]{0,30}`, no trailing dash.
#[must_use]
pub fn valid_slug(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 31
        && s.as_bytes()[0].is_ascii_lowercase()
        && !s.ends_with('-')
        && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Releases: `latest` or `[0-9A-Za-z.-]{1,40}` starting with a digit.
#[must_use]
pub fn valid_version(s: &str) -> bool {
    s == "latest"
        || (!s.is_empty()
            && s.len() <= 40
            && s.as_bytes()[0].is_ascii_digit()
            && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-')))
}

/// System locations a data path may not be, or sit inside.
const LINUX_RESERVED: &[&str] = &[
    "/bin",
    "/boot",
    "/dev",
    "/etc",
    "/lib",
    "/lib64",
    "/proc",
    "/run",
    "/sbin",
    "/sys",
    "/usr",
    "/opt/forge",
    "/var/lib/forge",
    "/root",
    "/tmp",
];

/// Absolute data paths. Linux: `/srv/...` style, `[A-Za-z0-9/_.-]`, no `..`,
/// not `/` and not inside system directories. Windows: `D:\data\...`.
pub fn valid_data_path(p: &str) -> Result<(), String> {
    let bad = || format!("data path `{p}`: an absolute directory such as /srv/objstore or D:\\objstore");
    if p.len() < 2 || p.len() > 200 || p.split(['/', '\\']).any(|c| c == "..") {
        return Err(bad());
    }
    if p.starts_with('/') {
        if !p.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'_' | b'.' | b'-')) || p.contains("//") {
            return Err(bad());
        }
        let trimmed = p.trim_end_matches('/');
        if trimmed.is_empty()
            || LINUX_RESERVED.iter().any(|r| trimmed == *r || trimmed.starts_with(&format!("{r}/")))
            || matches!(trimmed, "/home" | "/var" | "/opt" | "/srv" | "/mnt" | "/media" | "/data")
        {
            return Err(format!("data path `{p}` is a system directory; use a dedicated directory or mount"));
        }
        return Ok(());
    }
    // Windows: X:\dir[\dir...]
    let b = p.as_bytes();
    if b.len() >= 4
        && b[0].is_ascii_alphabetic()
        && b[1] == b':'
        && b[2] == b'\\'
        && p[3..].bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'\\' | b'_' | b'.' | b'-' | b' '))
        && !p.contains("\\\\")
    {
        let lower = p.to_ascii_lowercase();
        if ["\\windows", "\\program files", "\\programdata", "\\users"].iter().any(|r| lower[2..].starts_with(r)) {
            return Err(format!("data path `{p}` is a system directory; use a dedicated directory or volume"));
        }
        return Ok(());
    }
    Err(bad())
}

impl StoreSpec {
    pub fn validate(&self) -> Result<(), String> {
        if !PROVIDERS.contains(&self.provider.as_str()) {
            return Err(format!("unsupported object store provider `{}`", self.provider));
        }
        if !valid_slug(&self.slug) {
            return Err(format!("instance name `{}`: lowercase letters, digits and -, up to 31", self.slug));
        }
        if self.owner.len() > 64 || !self.owner.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
            return Err("owner: the store id".into());
        }
        if !valid_version(&self.version) {
            return Err(format!("release `{}`: `latest` or an exact release such as 1.0.0-alpha.67", self.version));
        }
        if self.s3_port < 1024 {
            return Err("S3 port: 1024-65535".into());
        }
        if self.console_port.is_some_and(|c| c < 1024 || c == self.s3_port) {
            return Err("console port: 1024-65535, different from the S3 port".into());
        }
        if self.data_paths.is_empty() || self.data_paths.len() > MAX_DRIVES_PER_NODE {
            return Err(format!("1 to {MAX_DRIVES_PER_NODE} data paths per node"));
        }
        if self.adopted {
            // Exactly the App Store container's layout.
            if self.runtime != Runtime::Docker || self.data_paths != [LEGACY_DATA] || !self.peers.is_empty() {
                return Err(format!("an adopted store is the {LEGACY_CONTAINER} container with data in {LEGACY_DATA}"));
            }
            return Ok(());
        }
        for p in &self.data_paths {
            valid_data_path(p)?;
        }
        let distinct: std::collections::BTreeSet<&str> =
            self.data_paths.iter().map(|p| p.trim_end_matches(['/', '\\'])).collect();
        if distinct.len() != self.data_paths.len() {
            return Err("each data path may be listed once".into());
        }
        let windows = !self.data_paths[0].starts_with('/');
        if windows && (self.data_paths.len() > 1 || self.runtime == Runtime::Docker || !self.peers.is_empty()) {
            return Err("Windows nodes run a standalone native store with one data path".into());
        }
        if !self.peers.is_empty() {
            self.validate_peers()?;
        }
        Ok(())
    }

    /// Distributed layout: at least [`MIN_DISTRIBUTED_DRIVES`] drives, each
    /// `http://host:<s3_port>/path` once, and this member's own paths among them.
    fn validate_peers(&self) -> Result<(), String> {
        if self.peers.len() < MIN_DISTRIBUTED_DRIVES || self.peers.len() > MAX_DISTRIBUTED_DRIVES {
            return Err(format!(
                "a distributed store needs {MIN_DISTRIBUTED_DRIVES} to {MAX_DISTRIBUTED_DRIVES} drives in total"
            ));
        }
        let mut seen = std::collections::BTreeSet::new();
        for peer in &self.peers {
            let (host, path) = parse_peer(peer, self.s3_port)?;
            if !seen.insert((host, path)) {
                return Err(format!("drive `{peer}` is listed twice"));
            }
        }
        if !self
            .data_paths
            .iter()
            .all(|p| self.peers.iter().any(|peer| peer.ends_with(&format!(":{}{p}", self.s3_port))))
        {
            return Err("every data path of this member must be one of the cluster's drives".into());
        }
        Ok(())
    }

    /// Service or container name on the node.
    #[must_use]
    pub fn unit(&self) -> String {
        if self.adopted { LEGACY_CONTAINER.to_owned() } else { format!("forge-objstore-{}", self.slug) }
    }
}

impl StoreCredential {
    pub fn validate(&self) -> Result<(), String> {
        let ok = |s: &str, min: usize| (min..=128).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric());
        if !ok(&self.access_key, 3) || !ok(&self.secret_key, 8) {
            return Err("access key (3+) and secret key (8+): letters and digits, up to 128".into());
        }
        Ok(())
    }
}

/// What the node reports after an operation (last line of the script's
/// output: `forge: objstore: {json}`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoreReport {
    /// `running`, `stopped`, `not-installed`.
    pub state: String,
    /// Release the binary or image reports.
    #[serde(default)]
    pub version: String,
    /// Whether the data path is its own mount point.
    #[serde(default)]
    pub dedicated: Option<bool>,
    #[serde(default)]
    pub capacity_bytes: Option<u64>,
    #[serde(default)]
    pub free_bytes: Option<u64>,
}

/// Marker in front of the report line.
pub const REPORT_PREFIX: &str = "forge: objstore: ";
/// Marker of the credential line `adopt` prints. The agent removes that line
/// from the output it returns, so it never reaches job history.
pub const CREDENTIAL_PREFIX: &str = "forge: objstore-credential: ";

/// Takes the adopted credential out of a script's output: returns it and the
/// output without that line.
#[must_use]
pub fn take_credential(output: &str) -> (Option<StoreCredential>, String) {
    let mut cred = None;
    let mut rest = String::with_capacity(output.len());
    for line in output.lines() {
        match line.trim().strip_prefix(CREDENTIAL_PREFIX) {
            Some(json) => cred = serde_json::from_str(json).ok().or(cred),
            None => {
                rest.push_str(line);
                rest.push('\n');
            }
        }
    }
    (cred.filter(|c: &StoreCredential| c.validate().is_ok()), rest)
}

impl StoreReport {
    /// Finds the last report line in a script's output.
    #[must_use]
    pub fn parse(output: &str) -> Option<Self> {
        output
            .lines()
            .rev()
            .find_map(|l| l.trim().strip_prefix(REPORT_PREFIX))
            .and_then(|j| serde_json::from_str(j).ok())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// Issue #1: a distributed install waited 15 minutes in silence. The
    /// wait now fails at once when the engine keeps crashing, shows its log
    /// without the credential, and names the members it waits for.
    #[cfg(unix)]
    #[test]
    fn the_install_wait_fails_fast_and_never_prints_the_credential() {
        let start = RUSTFS_SH.find("# ------------------------------------------------------------ start-up").unwrap();
        let end = RUSTFS_SH.find("# ---------------------------------------------------------------- actions").unwrap();
        let script = format!(
            r#"log() {{ printf 'forge: %s
' "$*"; }}
die() {{ printf 'forge: error: %s
' "$*"; exit 1; }}
{}
systemctl() {{ echo 4; }}
journalctl() {{ echo "started with secret s3cr3tvalue"; echo "cannot bind the port"; }}
RUNTIME=native; UNIT=x; SLUG=test-store; S3_PORT=1; FORGE_OS_SECRET_KEY=s3cr3tvalue
VOLUMES="http://10.0.0.1:9000/d1 http://10.0.0.1:9000/d2 http://10.0.0.2:9000/d1"
echo "members: $(member_addrs | tr '
' ' ')"
wait_store 900
"#,
            &RUSTFS_SH[start..end]
        );
        let out = std::process::Command::new("sh").arg("-c").arg(&script).output().unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(!out.status.success(), "{text}");
        assert!(text.contains("members: 10.0.0.1:9000 10.0.0.2:9000"), "{text}");
        assert!(text.contains("RustFS keeps stopping on this node (restarted 4 times)"), "{text}");
        assert!(text.contains("cannot bind the port") && !text.contains("s3cr3tvalue"), "{text}");
    }

    /// D-079: Windows recipes keep Forge's files under `%ProgramData%\OrbynForge`.
    /// The generic `%ProgramData%\Forge` appears only where an older install
    /// is moved or its empty folders are removed.
    #[test]
    fn windows_recipes_use_only_the_branded_data_folder() {
        for (name, script) in [("_winget.ps1", crate::WINGET_RUNNER), ("objstore-rustfs.ps1", RUSTFS_PS1)] {
            assert!(script.contains("OrbynForge"), "{name}");
            for line in script.lines() {
                let generic = line.contains("ProgramData \"Forge") || line.contains("ProgramData 'Forge");
                let legacy = line.contains("Legacy") || line.contains("Join-Path $env:ProgramData 'Forge')");
                assert!(!generic || legacy, "{name}: generic folder outside the legacy move: {line}");
            }
        }
    }

    /// The node is the failure unit: drives of one member on one disk give a
    /// warning and the RustFS disk-check bypass, never a refusal.
    #[test]
    fn shared_disk_within_a_node_warns_instead_of_failing() {
        assert!(!RUSTFS_SH.contains("must be on separate disks"));
        assert!(RUSTFS_SH.contains("RUSTFS_UNSAFE_BYPASS_DISK_CHECK=true"));
        let start = RUSTFS_SH.find("SHARED_DISK=0").unwrap();
        let end = start + RUSTFS_SH[start..].find("\n}\n").unwrap() + 3;
        let func = &RUSTFS_SH[start..end];
        let base = std::env::temp_dir().join(format!("forge-objstore-shared-{}", std::process::id()));
        let (d1, d2) = (base.join("disk1"), base.join("disk2"));
        std::fs::create_dir_all(&d1).unwrap();
        std::fs::create_dir_all(&d2).unwrap();
        let script = format!(
            "log() {{ printf 'forge: %s\n' \"$*\"; }}\ndie() {{ echo die; exit 1; }}\n{func}\ncheck_distinct_disks\necho \"shared=$SHARED_DISK\"\n"
        );
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(&script)
            .env("VOLUMES", "http://a:9000/x")
            .env("DATA_ALL", format!("{} {}", d1.display(), d2.display()))
            .output();
        let _ = std::fs::remove_dir_all(&base);
        let Ok(out) = out else { return }; // no POSIX shell on this host
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success(), "{text}");
        assert!(text.contains("shared=1"), "{text}");
        assert!(text.contains("disk2 shares a device with") && text.contains("fault tolerance is per node"), "{text}");
    }

    /// Runs the recipe's ownership helpers with `DATA_ALL` = `paths` and the
    /// given owner and protected paths; returns the output, or `None` when
    /// the host has no POSIX shell.
    fn run_delete(paths: &[&std::path::Path], owner: &str, protect: &str) -> Option<String> {
        let start = RUSTFS_SH.find("# ------------------------------------------------------------ ownership").unwrap();
        let end = RUSTFS_SH.find("ensure_user() {").unwrap();
        let script = format!(
            "log() {{ printf 'forge: %s\n' \"$*\"; }}\nis_mount() {{ return 1; }}\n{}\ndelete_data_paths\n",
            &RUSTFS_SH[start..end]
        );
        let all: Vec<String> = paths.iter().map(|p| sh_path(p)).collect();
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(&script)
            .env("DATA_ALL", all.join(" "))
            .env("FORGE_OS_OWNER", owner)
            .env("FORGE_OS_PROTECT", protect)
            .output()
            .ok()?;
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        Some(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// The path as the shell sees it (Git Bash maps a `D:` drive to `/d`).
    fn sh_path(p: &std::path::Path) -> String {
        let s = p.to_string_lossy().replace(char::from(92), "/");
        match s.as_bytes() {
            [d, b':', ..] if d.is_ascii_alphabetic() => format!("/{}{}", (*d as char).to_ascii_lowercase(), &s[2..]),
            _ => s,
        }
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("forge-objstore-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        // The shell sees the canonical path (no symlinked temp parents).
        let d = std::fs::canonicalize(&d).unwrap();
        #[cfg(windows)]
        let d = std::path::PathBuf::from(d.to_string_lossy().trim_start_matches(r"\\?\").to_owned());
        d
    }

    #[test]
    fn delete_data_needs_the_matching_marker() {
        let base = scratch("marker");
        let (mine, other, bare) = (base.join("mine"), base.join("other"), base.join("bare"));
        for d in [&mine, &other, &bare] {
            std::fs::create_dir_all(d.join("bucket")).unwrap();
        }
        std::fs::write(mine.join(".forge-objstore"), "store-1\n").unwrap();
        std::fs::write(other.join(".forge-objstore"), "store-2\n").unwrap();
        let Some(out) = run_delete(&[&mine, &other, &bare], "store-1", "") else { return };
        let gone = !mine.exists();
        let kept = other.exists() && bare.exists();
        let _ = std::fs::remove_dir_all(&base);
        // Git Bash paths differ from Windows paths; the behaviour is what counts.
        assert!(gone, "matching marker deletes: {out}");
        assert!(kept, "{out}");
        assert_eq!(out.matches("not owned by this store").count(), 2, "{out}");
    }

    #[test]
    fn delete_data_refuses_nested_and_overlapping_paths() {
        let base = scratch("nested");
        let outer = base.join("outer");
        let inner = outer.join("inner");
        std::fs::create_dir_all(&inner).unwrap();
        std::fs::write(outer.join(".forge-objstore"), "store-1\n").unwrap();
        std::fs::write(inner.join(".forge-objstore"), "store-2\n").unwrap();
        let Some(out) = run_delete(&[&outer], "store-1", "") else { return };
        assert!(outer.exists() && out.contains("contains data of another store"), "{out}");
        // A path the server lists as another store's is refused even without a nested marker.
        std::fs::remove_file(inner.join(".forge-objstore")).unwrap();
        let protect = sh_path(&inner);
        let out = run_delete(&[&outer], "store-1", &format!("/srv/x|{protect}")).unwrap();
        let kept = outer.exists();
        let _ = std::fs::remove_dir_all(&base);
        assert!(kept && out.contains("overlaps"), "{out}");
    }

    #[cfg(unix)]
    #[test]
    fn delete_data_skips_symlinks() {
        let base = scratch("link");
        let real = base.join("real");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(real.join(".forge-objstore"), "store-1\n").unwrap();
        let link = base.join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let parent_link = base.join("plink");
        std::os::unix::fs::symlink(&base, &parent_link).unwrap();
        let Some(out) = run_delete(&[&link, &parent_link.join("real")], "store-1", "") else { return };
        let kept = real.exists();
        let _ = std::fs::remove_dir_all(&base);
        assert!(kept, "{out}");
        assert!(out.contains("is a symlink") && out.contains("parent directory is a symlink"), "{out}");
    }

    /// Windows: a junction planted inside the data directory is removed as a
    /// link; its target survives. Windows PowerShell 5.1 by absolute path,
    /// without an inherited PSModulePath.
    #[cfg(windows)]
    #[test]
    fn windows_delete_data_never_follows_a_junction() {
        let base = scratch("junction");
        let data = base.join("data");
        let target = base.join("target");
        std::fs::create_dir_all(data.join("bucket")).unwrap();
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("keep.txt"), "keep").unwrap();
        std::fs::write(data.join(".forge-objstore"), "store-1\r\n").unwrap();
        let start = RUSTFS_PS1.find("$Marker = '.forge-objstore'").unwrap();
        let end = RUSTFS_PS1.find("function Uninstall-Store {").unwrap();
        let script = format!(
            "$ErrorActionPreference = 'Stop'\r\nfunction Log($m) {{ Write-Output \"forge: $m\" }}\r\n$Data = '{data}'\r\n{funcs}\r\n\
             & cmd.exe /d /c mklink /J \"{data}\\link\" \"{target}\" | Out-Null\r\n\
             if (-not (Test-Path '{data}\\link')) {{ 'no junction'; exit 0 }}\r\n\
             Remove-OwnedData\r\n",
            data = data.display(),
            target = target.display(),
            funcs = &RUSTFS_PS1[start..end],
        );
        let file = base.join("t.ps1");
        std::fs::write(&file, script).unwrap();
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
        let exe = std::path::Path::new(&root).join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
        let out = std::process::Command::new(exe)
            .env_remove("PSModulePath")
            .env("FORGE_OS_OWNER", "store-1")
            .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File"])
            .arg(&file)
            .output();
        let Ok(out) = out else { return };
        let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        let survived = target.join("keep.txt").exists();
        let deleted = !data.exists();
        let _ = std::fs::remove_dir_all(&base);
        assert!(out.status.success(), "{text}");
        assert!(survived, "the junction's target was deleted: {text}");
        assert!(text.contains("no junction") || deleted, "{text}");
    }
    fn spec() -> StoreSpec {
        StoreSpec {
            provider: "rustfs".into(),
            slug: "media".into(),
            version: "latest".into(),
            runtime: Runtime::Native,
            s3_port: 9000,
            console_port: Some(9001),
            data_paths: vec!["/srv/objstore/media".into()],
            peers: Vec::new(),
            require_mount: false,
            adopted: false,
            owner: String::new(),
        }
    }

    #[test]
    fn scripts_have_unix_line_endings() {
        assert!(!RUSTFS_SH.contains('\r'));
    }

    #[test]
    fn validates_specs() {
        assert!(spec().validate().is_ok());
        for bad in ["", "Media", "-a", "a-", "a b", "a;id", &"a".repeat(32)] {
            assert!(StoreSpec { slug: bad.into(), ..spec() }.validate().is_err(), "{bad}");
        }
        for bad in ["/", "/etc", "/etc/x", "/var/lib/forge/x", "/srv/../etc", "/srv/a b", "srv", "/srv", "/tmp/x"] {
            assert!(StoreSpec { data_paths: vec![bad.into()], ..spec() }.validate().is_err(), "{bad}");
        }
        let win = |p: &str| vec![p.to_owned()];
        assert!(StoreSpec { data_paths: win("D:\\objstore\\media"), ..spec() }.validate().is_ok());
        assert!(StoreSpec { data_paths: win("C:\\Windows\\x"), ..spec() }.validate().is_err());
        assert!(StoreSpec { data_paths: win("D:\\objstore"), runtime: Runtime::Docker, ..spec() }.validate().is_err());
        assert!(
            StoreSpec { data_paths: vec!["D:\\a".into(), "E:\\b".into()], ..spec() }.validate().is_err(),
            "Windows: one data path"
        );
        assert!(
            StoreSpec { data_paths: vec!["/srv/a".into(), "/srv/a/".into()], ..spec() }.validate().is_err(),
            "duplicate paths"
        );
        assert!(StoreSpec { version: "1.0.0-alpha.67".into(), ..spec() }.validate().is_ok());
        assert!(StoreSpec { version: "1.0; rm".into(), ..spec() }.validate().is_err());
        assert!(StoreSpec { console_port: Some(9000), ..spec() }.validate().is_err());
        assert!(StoreSpec { provider: "minio".into(), ..spec() }.validate().is_err());
    }

    #[test]
    fn validates_distributed_layouts() {
        let paths = vec!["/srv/objstore/d0".to_owned(), "/srv/objstore/d1".to_owned()];
        let peers: Vec<String> =
            ["node-a", "node-b"].iter().flat_map(|h| paths.iter().map(move |p| peer(h, 9000, p))).collect();
        let ha = StoreSpec { data_paths: paths.clone(), peers: peers.clone(), ..spec() };
        assert!(ha.validate().is_ok());
        assert!(StoreSpec { runtime: Runtime::Docker, ..ha.clone() }.validate().is_ok());
        assert!(StoreSpec { peers: peers[..3].to_vec(), ..ha.clone() }.validate().is_err(), "4 drives minimum");
        let mut dup = peers.clone();
        dup[3] = dup[0].clone();
        assert!(StoreSpec { peers: dup, ..ha.clone() }.validate().is_err());
        let mut wrong_port = peers.clone();
        wrong_port[1] = peer("node-a", 9100, "/srv/objstore/d1");
        assert!(StoreSpec { peers: wrong_port, ..ha.clone() }.validate().is_err(), "one port everywhere");
        let mut inject = peers.clone();
        inject[0] = "http://node-a:9000/srv/x http://evil:9000/y".into();
        assert!(StoreSpec { peers: inject, ..ha.clone() }.validate().is_err());
        assert!(
            StoreSpec { data_paths: vec!["/srv/objstore/other".into()], ..ha.clone() }.validate().is_err(),
            "own paths must be in the layout"
        );
        let env = StoreOp::Status { spec: ha }.env();
        assert_eq!(env["FORGE_OS_VOLUMES"], peers.join(" "));
        assert_eq!(env["FORGE_OS_DATA_ALL"], "/srv/objstore/d0 /srv/objstore/d1");
    }

    #[test]
    fn credential_is_validated_and_redacted() {
        let c = StoreCredential { access_key: "forgeadmin".into(), secret_key: "s3cr3tvalue1".into() };
        assert!(c.validate().is_ok());
        assert!(!format!("{c:?}").contains("s3cr3t"));
        let op = StoreOp::Install { spec: spec(), credential: c.clone() };
        assert!(!format!("{op:?}").contains("s3cr3t"));
        assert!(StoreCredential { secret_key: "a'b\"c$d e".into(), ..c }.validate().is_err());
    }

    #[test]
    fn adoption_is_limited_to_the_app_store_container() {
        let legacy = StoreSpec {
            slug: "rustfs".into(),
            runtime: Runtime::Docker,
            data_paths: vec![LEGACY_DATA.into()],
            adopted: true,
            ..spec()
        };
        assert!(legacy.validate().is_ok());
        assert_eq!(legacy.unit(), "forge-rustfs");
        assert!(StoreOp::Adopt { spec: legacy.clone() }.validate().is_ok());
        assert_eq!(StoreOp::Status { spec: legacy.clone() }.env()["FORGE_OS_ADOPTED"], "1");
        assert!(StoreOp::Adopt { spec: spec() }.validate().is_err(), "only adopted specs");
        let cred = StoreCredential { access_key: "forgeadmin".into(), secret_key: "s3cr3tvalue1".into() };
        assert!(StoreOp::Install { spec: legacy.clone(), credential: cred }.validate().is_err(), "never recreated");
        assert!(StoreSpec { runtime: Runtime::Native, ..legacy.clone() }.validate().is_err());
        assert!(StoreSpec { data_paths: vec!["/srv/other".into()], ..legacy }.validate().is_err());
        // Without the flag, the App Store data path stays forbidden.
        assert!(StoreSpec { data_paths: vec![LEGACY_DATA.into()], ..spec() }.validate().is_err());
    }

    #[test]
    fn rejects_unknown_fields_and_ops() {
        assert!(serde_json::from_str::<StoreOp>(r#"{"op":"exec","spec":{}}"#).is_err());
        let mut v = serde_json::to_value(StoreOp::Status { spec: spec() }).unwrap();
        v["spec"]["extra"] = serde_json::json!(1);
        assert!(serde_json::from_value::<StoreOp>(v).is_err());
    }

    #[test]
    fn parses_report() {
        let out = "x\nforge: objstore: {\"state\":\"running\",\"version\":\"1.0.0\",\"dedicated\":true}\n";
        let r = StoreReport::parse(out).unwrap();
        assert_eq!(r.state, "running");
        assert_eq!(r.dedicated, Some(true));
    }

    #[test]
    fn adopted_credential_is_taken_out_of_the_output() {
        let out = "forge: checking\nforge: objstore-credential: {\"access_key\":\"abcAKey1\",\"secret_key\":\"s3cr3tvalue1\"}\nforge: objstore: {\"state\":\"running\"}\n";
        let (cred, rest) = take_credential(out);
        assert_eq!(cred.unwrap().access_key, "abcAKey1");
        assert!(!rest.contains("s3cr3t") && rest.contains("forge: objstore: "));
        let (none, _) = take_credential("forge: objstore-credential: {\"access_key\":\"a b\",\"secret_key\":\"x\"}\n");
        assert!(none.is_none(), "malformed keys are dropped");
    }
}
