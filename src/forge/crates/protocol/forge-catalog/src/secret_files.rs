//! Secrets and configs as files for native services
//! (`design/plans/service-orchestration.md` step 3a, D-179).
//!
//! A service stores only [`FileRef`]s: which credential vault item to
//! mount, at which path, with which mode and owner. The server resolves
//! them into [`MountedFile`]s inside the deploy step (unsealing the vault
//! item there) and sends those only inside the signed agent command. The
//! agent writes each file under [`host_dir`] (a root-only folder per
//! service copy) and bind-mounts it read-only into a container, or points
//! a native service at the folder with [`DIR_ENV`].

use serde::{Deserialize, Serialize};

/// The stored service spec key that holds the [`FileRef`]s.
pub const SPEC_KEY: &str = "secret_files";
/// The variable that tells a native service where its files are.
pub const DIR_ENV: &str = "FORGE_SECRETS_DIR";
/// Where the agent keeps the files, one folder per service copy.
pub const HOST_ROOT: &str = "/var/lib/forge-agent/secrets";
/// Where a secret goes in a container unless its target says otherwise.
pub const SECRETS_MOUNT: &str = "/run/secrets";
/// Most files one service may mount.
pub const MAX_FILES: usize = 50;
/// Largest value of one file (a vault item holds up to 4096 bytes).
pub const MAX_VALUE_BYTES: usize = 4096;

/// A secret (default mode `0400`, under `/run/secrets`) or a config
/// (default mode `0444`, at its target).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    Secret,
    Config,
}

impl FileKind {
    /// The Compose section name (`secrets`, `configs`).
    #[must_use]
    pub fn section(self) -> &'static str {
        match self {
            Self::Secret => "secrets",
            Self::Config => "configs",
        }
    }

    /// The mode a file of this kind gets unless it sets one.
    #[must_use]
    pub fn default_mode(self) -> u32 {
        match self {
            Self::Secret => 0o400,
            Self::Config => 0o444,
        }
    }
}

/// One file a service mounts, as the service stores it: a reference to a
/// credential vault item by name, never its value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct FileRef {
    pub kind: FileKind,
    /// The name of the credential vault item that holds the value.
    pub item: String,
    /// The absolute path inside the container. Its last part is the file
    /// name in the native service's folder.
    pub target: String,
    /// Octal permission bits; absent: `0400` for a secret, `0444` for a config.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<u32>,
    /// Owner inside the container (default `0`, root).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<u32>,
    /// Group inside the container (default `0`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gid: Option<u32>,
}

impl FileRef {
    /// A reference with the default target: `/run/secrets/<name>` for a
    /// secret, `/<name>` for a config. A relative `target` of a secret is
    /// placed under `/run/secrets`, as Compose does.
    #[must_use]
    pub fn target_for(kind: FileKind, name: &str, target: Option<&str>) -> String {
        match (kind, target.map(str::trim).filter(|t| !t.is_empty())) {
            (_, Some(t)) if t.starts_with('/') => t.to_owned(),
            (FileKind::Secret, Some(t)) => format!("{SECRETS_MOUNT}/{t}"),
            (FileKind::Config, Some(t)) => format!("/{t}"),
            (FileKind::Secret, None) => format!("{SECRETS_MOUNT}/{name}"),
            (FileKind::Config, None) => format!("/{name}"),
        }
    }

    /// The mode the file gets.
    #[must_use]
    pub fn effective_mode(&self) -> u32 {
        self.mode.unwrap_or_else(|| self.kind.default_mode())
    }

    /// The file name in the service's folder: the target's last part.
    #[must_use]
    pub fn file_name(&self) -> &str {
        self.target.rsplit('/').next().unwrap_or_default()
    }

    /// Checks one reference.
    pub fn validate(&self) -> Result<(), String> {
        let item = self.item.trim();

        if item.is_empty() || item.chars().count() > 100 || item.chars().any(char::is_control) {
            return Err("the vault item name must be 1 to 100 characters".into());
        }

        check_target(&self.target)?;

        if self.effective_mode() & !0o777 != 0 {
            return Err(format!("`{}`: mode must be octal permission bits (at most 0777)", self.target));
        }

        Ok(())
    }
}

/// A container path for a file: absolute, plain parts, a valid file name,
/// not inside `/proc`, `/sys` or `/dev`.
fn check_target(target: &str) -> Result<(), String> {
    let bad = || format!("`{target}`: the target must be an absolute path of plain names");

    if !target.starts_with('/') || target.len() > 255 {
        return Err(bad());
    }

    let parts: Vec<&str> = target[1..].split('/').collect();

    for p in &parts {
        if p.is_empty()
            || *p == "."
            || *p == ".."
            || !p.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-@+".contains(&b))
        {
            return Err(bad());
        }
    }

    if matches!(parts.first(), Some(&("proc" | "sys" | "dev"))) {
        return Err(format!("`{target}`: files cannot be mounted under /proc, /sys or /dev"));
    }

    Ok(())
}

/// Checks a service's references: each valid, at most [`MAX_FILES`], and
/// no two with the same target or the same file name (a native service
/// sees them side by side in one folder).
pub fn validate_refs(refs: &[FileRef]) -> Result<(), String> {
    if refs.len() > MAX_FILES {
        return Err(format!("at most {MAX_FILES} secret and config files"));
    }

    let mut names = std::collections::HashSet::new();

    for r in refs {
        r.validate()?;

        if !names.insert(r.file_name().to_owned()) {
            return Err(format!("two secret or config files are named `{}`", r.file_name()));
        }
    }

    Ok(())
}

/// One file as the agent receives it, inside the signed command only.
///
/// `Debug` never prints the value.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MountedFile {
    /// The file name in the service's folder.
    pub name: String,
    /// The absolute path inside the container.
    pub target: String,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    /// The secret or config value.
    pub value: String,
}

impl std::fmt::Debug for MountedFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MountedFile")
            .field("name", &self.name)
            .field("target", &self.target)
            .field("mode", &format_args!("{:o}", self.mode))
            .field("uid", &self.uid)
            .field("gid", &self.gid)
            .field("value", &"***")
            .finish()
    }
}

impl MountedFile {
    /// The file for `r` with `value`.
    #[must_use]
    pub fn new(r: &FileRef, value: String) -> Self {
        Self {
            name: r.file_name().to_owned(),
            target: r.target.clone(),
            mode: r.effective_mode(),
            uid: r.uid.unwrap_or(0),
            gid: r.gid.unwrap_or(0),
            value,
        }
    }
}

/// Checks the files of a node spec (the agent checks them again).
pub fn validate_files(files: &[MountedFile]) -> Result<(), String> {
    if files.len() > MAX_FILES {
        return Err(format!("at most {MAX_FILES} secret and config files"));
    }

    let mut names = std::collections::HashSet::new();

    for f in files {
        check_target(&f.target)?;

        if f.target.rsplit('/').next() != Some(f.name.as_str()) || !names.insert(f.name.as_str()) {
            return Err(format!("`{}`: file names must be unique and match the target", f.target));
        }

        if f.mode & !0o777 != 0 {
            return Err(format!("`{}`: mode must be at most 0777", f.target));
        }

        if f.value.len() > MAX_VALUE_BYTES {
            return Err(format!("`{}`: values are at most {MAX_VALUE_BYTES} bytes", f.target));
        }
    }

    Ok(())
}

/// The folder the agent keeps one service copy's files in:
/// `/var/lib/forge-agent/secrets/<environment>/<name>[--<slot>]`.
#[must_use]
pub fn host_dir(environment: &str, name: &str, slot_suffix: &str) -> String {
    format!("{HOST_ROOT}/{environment}/{name}{slot_suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(kind: FileKind, target: &str) -> FileRef {
        FileRef { kind, item: "db password".into(), target: target.into(), mode: None, uid: None, gid: None }
    }

    #[test]
    fn default_targets_follow_compose() {
        assert_eq!(FileRef::target_for(FileKind::Secret, "db", None), "/run/secrets/db");
        assert_eq!(FileRef::target_for(FileKind::Secret, "db", Some("pw")), "/run/secrets/pw");
        assert_eq!(FileRef::target_for(FileKind::Config, "nginx", None), "/nginx");
        assert_eq!(
            FileRef::target_for(FileKind::Config, "nginx", Some("/etc/nginx/nginx.conf")),
            "/etc/nginx/nginx.conf"
        );
        assert_eq!(r(FileKind::Secret, "/run/secrets/db").effective_mode(), 0o400);
        assert_eq!(r(FileKind::Config, "/etc/app.conf").effective_mode(), 0o444);
    }

    #[test]
    fn bad_targets_modes_and_clashes_are_refused() {
        for t in ["relative", "/run/../etc/shadow", "/proc/x", "/a//b", "/a b"] {
            assert!(r(FileKind::Secret, t).validate().is_err(), "{t}");
        }

        let mut m = r(FileKind::Secret, "/run/secrets/db");
        m.mode = Some(0o4755);
        assert!(m.validate().is_err());
        assert!(validate_refs(&[r(FileKind::Secret, "/run/secrets/db"), r(FileKind::Config, "/etc/db")]).is_err());
        assert!(validate_refs(&[r(FileKind::Secret, "/run/secrets/db"), r(FileKind::Config, "/etc/app.conf")]).is_ok());
    }

    #[test]
    fn debug_never_prints_the_value() {
        let f = MountedFile::new(&r(FileKind::Secret, "/run/secrets/db"), "hunter2".into());
        let shown = format!("{f:?}");
        assert!(!shown.contains("hunter2"));
        assert!(shown.contains("400"));
        assert!(validate_files(&[f]).is_ok());
    }
}
