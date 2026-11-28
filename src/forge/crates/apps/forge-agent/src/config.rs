//! On-disk agent state: `agent.toml` (operator settings), `identity.json`
//! (assigned at enrollment), `agent.key`, `agent.crt`, `ca.crt`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Operator-controlled settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentConfig {
    /// Control plane agent endpoint, e.g. `https://forge.example.com:3843`.
    pub server_url: String,
    /// Commands this agent will execute. Anything else is refused locally,
    /// even if correctly signed.
    #[serde(default = "default_allowed")]
    pub allowed_commands: Vec<String>,
    /// Command kinds already offered by an install or update. A kind in this
    /// list but not in `allowed_commands` was removed by the operator, and
    /// updates leave it removed (D-070).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub offered_commands: Vec<String>,
    /// The analytics worker's agent listener, from enrollment (D-142);
    /// heartbeats keep it current.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub analytics_url: Option<String>,
}

fn default_allowed() -> Vec<String> {
    vec![
        "ping".into(),
        "collect_facts".into(),
        "service_status".into(),
        "package".into(),
        "container".into(),
        "app".into(),
        "site".into(),
        "dns".into(),
        "probe".into(),
        "ingress".into(),
        "fabric".into(),
        "files".into(),
        "access".into(),
        "cancel".into(),
        "processes".into(),
        "process_action".into(),
        "update".into(),
        "stack".into(),
        "terminal_account".into(),
        "names".into(),
        "tunnel".into(),
        "container_exec".into(),
        "docker_tls".into(),
        "swarm".into(),
        "docker".into(),
    ]
}

/// Kinds a new enrollment allows but an update never adds to an existing
/// `allowed_commands`: the operator opts in by adding them (D-114).
const OPT_IN: &[&str] = &["container_exec"];

impl AgentConfig {
    #[must_use]
    pub fn new(server_url: impl Into<String>) -> Self {
        Self {
            server_url: server_url.into(),
            allowed_commands: default_allowed(),
            offered_commands: default_allowed(),
            analytics_url: None,
        }
    }

    /// Allows command kinds this release adds, and only those: a kind that
    /// was offered before and is missing now was removed by the operator and
    /// stays removed. Returns the kinds added.
    ///
    /// A config from before this tracking has no `offered_commands`: every
    /// missing kind is added once (as installers always did) and from then
    /// on removals stick.
    pub fn offer_new_commands(&mut self) -> Vec<String> {
        let mut added = Vec::new();

        for kind in default_allowed() {
            if self.offered_commands.contains(&kind) {
                continue;
            }

            if !self.allowed_commands.contains(&kind) && !OPT_IN.contains(&kind.as_str()) {
                self.allowed_commands.push(kind.clone());
                added.push(kind.clone());
            }

            self.offered_commands.push(kind);
        }

        added
    }
}

/// Identity issued by the control plane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentIdentity {
    pub server_id: String,
    /// Base64url Ed25519 public key that signs commands.
    pub command_public_key: String,
    pub heartbeat_interval_secs: u32,
}

/// File locations inside the state directory.
#[derive(Debug, Clone)]
pub struct StatePaths {
    pub dir: PathBuf,
}

impl StatePaths {
    #[must_use]
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// Platform default: `/var/lib/forge-agent` or `%ProgramData%\OrbynForge\agent`.
    #[must_use]
    pub fn default_dir() -> PathBuf {
        if cfg!(windows) {
            let base = std::env::var_os("ProgramData").map_or_else(|| PathBuf::from(r"C:\ProgramData"), PathBuf::from);
            base.join("OrbynForge").join("agent")
        } else {
            PathBuf::from("/var/lib/forge-agent")
        }
    }

    #[must_use]
    pub fn config(&self) -> PathBuf {
        self.dir.join("agent.toml")
    }
    #[must_use]
    pub fn identity(&self) -> PathBuf {
        self.dir.join("identity.json")
    }
    #[must_use]
    pub fn key(&self) -> PathBuf {
        self.dir.join("agent.key")
    }
    #[must_use]
    pub fn cert(&self) -> PathBuf {
        self.dir.join("agent.crt")
    }
    #[must_use]
    pub fn ca(&self) -> PathBuf {
        self.dir.join("ca.crt")
    }

    /// The control-plane keys this agent pins (D-167); see `crate::pins`.
    #[must_use]
    pub fn pins(&self) -> PathBuf {
        crate::pins::path(&self.dir)
    }

    /// The last link failure caused by an unpinned server key, for
    /// `forge-agent status`; removed once the link works again.
    #[must_use]
    pub fn link_error(&self) -> PathBuf {
        self.dir.join("link-error.txt")
    }

    pub fn load_config(&self) -> anyhow::Result<AgentConfig> {
        Ok(toml::from_str(&std::fs::read_to_string(self.config())?)?)
    }

    pub fn save_config(&self, c: &AgentConfig) -> anyhow::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        std::fs::write(self.config(), toml::to_string_pretty(c)?)?;

        Ok(())
    }

    pub fn load_identity(&self) -> anyhow::Result<AgentIdentity> {
        Ok(serde_json::from_str(&std::fs::read_to_string(self.identity())?)?)
    }

    pub fn save_identity(&self, id: &AgentIdentity) -> anyhow::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        std::fs::write(self.identity(), serde_json::to_vec_pretty(id)?)?;

        Ok(())
    }

    /// Writes key material with owner-only permissions.
    pub fn save_credentials(&self, key_pem: &str, cert_pem: &str) -> anyhow::Result<()> {
        forge_secrets::write_private(&self.key(), key_pem.as_bytes())?;
        std::fs::write(self.cert(), cert_pem)?;

        Ok(())
    }

    pub fn is_enrolled(&self) -> bool {
        [self.identity(), self.key(), self.cert(), self.ca()].iter().all(|p| Path::new(p).exists())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// Security review (D-070): an update re-added commands the operator had
    /// removed. Now only kinds never offered before are added.
    #[test]
    fn removed_commands_stay_removed_and_new_ones_arrive() {
        // Before tracking: whatever is missing is added once, then tracked.
        let mut old = AgentConfig {
            server_url: "x".into(),
            allowed_commands: vec!["ping".into()],
            offered_commands: vec![],
            analytics_url: None,
        };
        let added = old.offer_new_commands();
        assert!(added.contains(&"files".to_owned()));
        assert_eq!(old.offered_commands, default_allowed());

        // The operator removes `files`; the next update keeps it removed.
        old.allowed_commands.retain(|c| c != "files");
        assert!(old.offer_new_commands().is_empty());
        assert!(!old.allowed_commands.contains(&"files".to_owned()));

        // A kind added by a later release is offered once.
        old.offered_commands.retain(|c| c != "stack");
        old.allowed_commands.retain(|c| c != "stack");
        assert_eq!(old.offer_new_commands(), vec!["stack".to_owned()]);

        // Round trip through TOML keeps the record.
        let back: AgentConfig = toml::from_str(&toml::to_string_pretty(&old).unwrap()).unwrap();
        assert_eq!(back, old);

        // An old file without the field still parses.
        let legacy: AgentConfig = toml::from_str("server_url = \"x\"\nallowed_commands = [\"ping\"]\n").unwrap();
        assert!(legacy.offered_commands.is_empty());
    }

    /// D-114 review: container shells are on for new enrollments, but an
    /// update never turns them on for an existing agent.
    #[test]
    fn container_shells_are_opt_in_for_existing_agents() {
        let shell = "container_exec".to_owned();
        assert!(AgentConfig::new("x").allowed_commands.contains(&shell), "new enrollments");
        let mut old = AgentConfig::new("x");
        old.allowed_commands.retain(|c| c != "container_exec");
        old.offered_commands.retain(|c| c != "container_exec");
        assert!(old.offer_new_commands().is_empty());
        assert!(!old.allowed_commands.contains(&shell), "an update does not opt in");
        let mut legacy = AgentConfig {
            server_url: "x".into(),
            allowed_commands: vec![],
            offered_commands: vec![],
            analytics_url: None,
        };
        assert!(!legacy.offer_new_commands().contains(&shell));

        // `container` does not admit shells.
        let only_containers = crate::policy::LocalPolicy::new(&["container".into()]);
        let cmd = forge_proto::Command::ContainerExecOpen(forge_proto::messages::ContainerExecOpen::default());
        assert!(only_containers.check(&cmd).is_err());
        assert!(crate::policy::LocalPolicy::new(&[shell]).check(&cmd).is_ok());
    }
}

/// The agent's Windows data folder, `%ProgramData%\OrbynForge`: enrollment
/// (`agent\`), WireGuard keys (`wireguard\`), package settings (`pkgs\`)
/// and the file store (`files\`). install.ps1 creates it for SYSTEM and
/// Administrators only and `-Purge` removes it (D-079). Never a generic name
/// another product could own.
#[must_use]
pub fn windows_data_dir() -> PathBuf {
    std::env::var_os("ProgramData").map_or_else(|| PathBuf::from(r"C:\ProgramData"), PathBuf::from).join("OrbynForge")
}

/// Limits a Windows data folder to SYSTEM and Administrators (D-079), the
/// same ACL install.ps1 sets, so keys written by a hand-run `enroll` are
/// never readable by other users through the inherited `%ProgramData%` ACL.
/// Creates the folder if needed, removes inherited access and any grant to
/// Everyone, Authenticated Users, Users, CREATOR OWNER or OWNER RIGHTS, and
/// refuses a folder that is a link (junction, symlink, mount point).
/// install.ps1 also refuses a folder another account planted beforehand.
/// Needs Administrator.
#[cfg(windows)]
pub fn lock_windows_dir(dir: &Path) -> anyhow::Result<()> {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

    if std::fs::symlink_metadata(dir).is_ok_and(|m| m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0) {
        anyhow::bail!("{} is a link (reparse point); remove it and run again", dir.display());
    }

    std::fs::create_dir_all(dir)?;
    let icacls = |args: &[&str]| -> anyhow::Result<()> {
        let out = std::process::Command::new("icacls.exe").arg(dir).args(args).output()?;
        if out.status.success() {
            Ok(())
        } else {
            anyhow::bail!(
                "could not restrict access to {} (icacls {}): {}; run as Administrator",
                dir.display(),
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            )
        }
    };
    icacls(&["/inheritance:r", "/grant:r", "*S-1-5-18:(OI)(CI)F", "*S-1-5-32-544:(OI)(CI)F", "/Q"])?;

    icacls(&["/remove:g", "*S-1-1-0", "*S-1-5-11", "*S-1-5-32-545", "*S-1-3-0", "*S-1-3-4", "/Q"])
}

/// Locks the agent's Windows data folders: `%ProgramData%\OrbynForge` and
/// the state directory when it lies elsewhere.
#[cfg(windows)]
pub fn lock_windows_data(state_dir: &Path) -> anyhow::Result<()> {
    let data = windows_data_dir();
    lock_windows_dir(&data)?;

    if !state_dir.starts_with(&data) {
        lock_windows_dir(state_dir)?;
    }

    Ok(())
}

#[cfg(test)]
mod windows_data_dir_tests {
    #[test]
    fn the_windows_data_folder_is_branded() {
        let d = super::windows_data_dir();
        assert!(d.ends_with("OrbynForge"), "{}", d.display());
    }

    #[cfg(windows)]
    #[test]
    fn the_data_folder_is_locked_and_a_link_is_refused() {
        let tmp = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let dir = tmp.path().join("OrbynForge");
        let icacls = |args: &[&str]| {
            std::process::Command::new("icacls.exe").arg(&dir).args(args).output().unwrap_or_else(|e| panic!("{e}"))
        };

        if let Err(e) = super::lock_windows_dir(&dir) {
            panic!("{e}");
        }

        // No inherited entries remain, and only SYSTEM and Administrators are granted.
        let sids = icacls(&["/save", &tmp.path().join("acl.txt").display().to_string()]);
        assert!(sids.status.success());
        let raw = std::fs::read(tmp.path().join("acl.txt")).unwrap_or_default();
        let text: String = raw.chunks(2).filter_map(|c| c.first().copied()).map(char::from).collect();
        let sddl = text.lines().nth(1).unwrap_or_default().to_owned();
        assert!(sddl.contains("D:P"), "protected DACL: {sddl}");
        assert!(!sddl.contains(";BU)") && !sddl.contains(";WD)") && !sddl.contains(";AU)"), "{sddl}");
        assert!(sddl.contains(";SY)") && sddl.contains(";BA)"), "{sddl}");

        // Give access back so the temp folder can be removed.
        let _ = icacls(&["/reset", "/T", "/C", "/Q"]);

        let target = tmp.path().join("elsewhere");
        let link = tmp.path().join("junction");
        std::fs::create_dir_all(&target).unwrap_or_else(|e| panic!("{e}"));
        let made = std::process::Command::new("cmd.exe")
            .args(["/c", "mklink", "/J"])
            .arg(&link)
            .arg(&target)
            .output()
            .is_ok_and(|o| o.status.success());

        if made {
            let err = super::lock_windows_dir(&link).err().map(|e| e.to_string()).unwrap_or_default();
            assert!(err.contains("reparse point"), "{err}");
        }
    }
}
