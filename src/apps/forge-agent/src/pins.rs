//! The control-plane keys this node pins (D-167).
//!
//! The agent accepts the control plane only if the key in its certificate
//! (SHA-256 of the SubjectPublicKeyInfo) is in this set; names and
//! addresses are not checked, so the node reaches Forge by any address.
//!
//! **Where.** On Linux nodes set up by the installer (the default state
//! directory) the set lives in `/var/lib/forge-agent-pins/pins.toml`,
//! owned by root and readable by the agent user, which cannot change it.
//! Only the root helper writes it, for a key announcement Forge signed with
//! its command key ([`apply_signed`]), and `forge-agent pins` run as root.
//! Elsewhere (Windows, where the agent is LocalSystem; a non-default state
//! directory, as in tests) it is `pins.toml` in the state directory.
//!
//! **Rotation.** Forge announces its current and next key in every
//! heartbeat reply. New ones are added; a pinned key Forge no longer
//! announces is marked retired and dropped [`RETIRED_KEPT`] later, so the
//! node still reaches a server that was rolled back meanwhile. At most
//! [`MAX_PINS`] keys are kept; keys added by hand (`pins add`) are never
//! retired by an announcement.
//!
//! **Replay and revocation.** Each announcement carries a sequence the
//! server bumps on every change of its key set. The file keeps the last
//! applied sequence and a digest of its keys: an older announcement, or one
//! with the same sequence and other keys, is refused. Fingerprints Forge
//! revoked are unpinned at once (never merely retired), remembered, and
//! never pinned again by an announcement; `pins add` asks for `--force`.

use std::path::{Path, PathBuf};

use forge_proto::messages::{ServerKey, ServerKeys, SignedServerKeys};
use serde::{Deserialize, Serialize};

/// Most keys in a pin set.
pub const MAX_PINS: usize = 8;
/// Largest pin file read.
pub const MAX_FILE: u64 = 64 * 1024;
/// The root-owned folder of the pin file on Linux nodes.
pub const SYSTEM_DIR: &str = "/var/lib/forge-agent-pins";
/// The pin file's name.
pub const FILE: &str = "pins.toml";
/// How long a key Forge stopped announcing stays pinned (30 days).
pub const RETIRED_KEPT: std::time::Duration = std::time::Duration::from_secs(30 * 24 * 3600);
/// Label of keys added by an operator.
pub const LABEL_MANUAL: &str = "manual";
/// Most revoked fingerprints remembered.
pub const MAX_REVOKED: usize = 32;
/// The lock file writers of the pin file hold (next to it).
pub const LOCK_FILE: &str = "pins.lock";
/// Label of the key from the enrollment token.
pub const LABEL_ENROLLMENT: &str = "enrollment";
/// The pin set of an enrollment run as the agent user, staged in the state
/// directory until the root helper adopts it ([`adopt_staged`]).
pub const STAGED_FILE: &str = "pins.enrolled.toml";

/// What can go wrong with the pin set.
#[derive(Debug, thiserror::Error)]
pub enum PinError {
    #[error("not a key fingerprint: {0} (expected 64 hex digits, optionally prefixed sha256:)")]
    Fingerprint(String),
    #[error("the pin set is full ({MAX_PINS} keys); remove one first with 'forge-agent pins remove'")]
    Full,
    #[error("{0}: {1}")]
    Io(String, String),
    #[error("{0} is unreadable: {1}")]
    Parse(String, String),
    #[error("the key announcement was refused: {0}")]
    Announcement(String),
    #[error("key announcement {got} is older than the one applied ({applied}); ignored")]
    Stale { got: u64, applied: u64 },
    #[error("key announcement {0} names other keys than the one applied with the same sequence; ignored")]
    Replayed(u64),
    #[error("{0} was revoked by Forge; add it anyway with --force")]
    Revoked(String),
}

/// One pinned key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pin {
    /// SHA-256 of the key's SubjectPublicKeyInfo, 64 lower-case hex digits.
    pub sha256: String,
    /// `current`, `next`, `enrollment` or `manual` (or an operator's own).
    pub label: String,
    /// When it was added (ms since epoch).
    pub added_ms: i64,
    /// When Forge stopped announcing it; dropped [`RETIRED_KEPT`] later.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retired_ms: Option<i64>,
}

/// The pin file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PinSet {
    /// Sequence of the last applied announcement (0: none, or an older
    /// server).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub sequence: u64,
    /// Digest of that announcement's keys and revocations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub announcement_digest: Option<String>,
    /// Fingerprints Forge revoked; never pinned again by an announcement.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub revoked: Vec<String>,
    #[serde(default)]
    pub server_keys: Vec<Pin>,
}

#[allow(clippy::trivially_copy_pass_by_ref)] // reason: serde's skip_serializing_if signature
const fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// What an announcement says, independent of when it was signed.
fn announcement_digest(keys: &ServerKeys) -> String {
    let mut k: Vec<String> =
        keys.keys.iter().map(|k| format!("{}:{}", k.sha256.to_ascii_lowercase(), k.label)).collect();
    k.sort();
    let mut r: Vec<String> = keys.revoked.iter().map(|f| f.to_ascii_lowercase()).collect();
    r.sort();

    forge_secrets::crypto::sha256_hex(format!("{}|{}", k.join(","), r.join(",")).as_bytes())
}

fn canonical(fp: &str) -> Result<String, PinError> {
    forge_pki::parse_fingerprint(fp).ok_or_else(|| PinError::Fingerprint(fp.chars().take(80).collect()))
}

fn clean_label(label: &str) -> String {
    let l: String = label.chars().filter(|c| !c.is_control()).take(40).collect();
    if l.trim().is_empty() { LABEL_MANUAL.to_owned() } else { l.trim().to_owned() }
}

impl PinSet {
    /// The pinned fingerprints.
    #[must_use]
    pub fn fingerprints(&self) -> Vec<String> {
        self.server_keys.iter().map(|p| p.sha256.clone()).collect()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.server_keys.is_empty()
    }

    #[must_use]
    pub fn contains(&self, fp: &str) -> bool {
        self.server_keys.iter().any(|p| p.sha256 == fp)
    }

    /// Whether Forge revoked `fp` (any form).
    #[must_use]
    pub fn is_revoked(&self, fp: &str) -> bool {
        canonical(fp).is_ok_and(|fp| self.revoked.contains(&fp))
    }

    /// [`Self::add`] by an operator: a revoked key needs `force`, which
    /// also forgets the revocation.
    pub fn add_checked(&mut self, fp: &str, label: &str, now_ms: i64, force: bool) -> Result<bool, PinError> {
        let c = canonical(fp)?;
        if self.revoked.contains(&c) {
            if !force {
                return Err(PinError::Revoked(forge_pki::short_fingerprint(&c)));
            }

            self.revoked.retain(|r| *r != c);
        }

        self.add(&c, label, now_ms)
    }

    /// Applies a verified announcement: refuses a replay (older sequence,
    /// or the same sequence with other content), drops revoked keys, then
    /// [`Self::apply_announcement`]. Returns whether the set changed.
    pub fn apply_verified(&mut self, keys: &ServerKeys, now_ms: i64) -> Result<bool, PinError> {
        let digest = announcement_digest(keys);

        if keys.sequence < self.sequence {
            return Err(PinError::Stale { got: keys.sequence, applied: self.sequence });
        }

        if keys.sequence == self.sequence && keys.sequence > 0 {
            if self.announcement_digest.as_deref().is_some_and(|d| d != digest) {
                return Err(PinError::Replayed(keys.sequence));
            }

            if self.announcement_digest.is_some() {
                return Ok(false);
            }
        }

        let mut changed = false;

        for r in &keys.revoked {
            let r = canonical(r)?;
            let before = self.server_keys.len();
            self.server_keys.retain(|p| p.sha256 != r);
            changed |= self.server_keys.len() != before;

            if !self.revoked.contains(&r) {
                self.revoked.push(r);
                changed = true;
            }
        }

        if self.revoked.len() > MAX_REVOKED {
            let extra = self.revoked.len() - MAX_REVOKED;
            self.revoked.drain(..extra);
        }

        let allowed: Vec<ServerKey> = keys
            .keys
            .iter()
            .filter(|k| canonical(&k.sha256).is_ok_and(|fp| !self.revoked.contains(&fp)))
            .cloned()
            .collect();
        changed |= self.apply_announcement(&allowed, now_ms)?;

        if keys.sequence > 0 && (self.sequence != keys.sequence || self.announcement_digest.as_deref() != Some(&digest))
        {
            self.sequence = keys.sequence;
            self.announcement_digest = Some(digest);
            changed = true;
        }

        Ok(changed)
    }

    /// Adds a key (or renames it and makes it active again). Returns
    /// whether the set changed.
    pub fn add(&mut self, fp: &str, label: &str, now_ms: i64) -> Result<bool, PinError> {
        let fp = canonical(fp)?;
        let label = clean_label(label);

        if let Some(p) = self.server_keys.iter_mut().find(|p| p.sha256 == fp) {
            let changed = p.label != label || p.retired_ms.is_some();
            p.label = label;
            p.retired_ms = None;

            return Ok(changed);
        }

        self.make_room()?;
        self.server_keys.push(Pin { sha256: fp, label, added_ms: now_ms, retired_ms: None });

        Ok(true)
    }

    /// Removes a key. Returns whether it was pinned.
    pub fn remove(&mut self, fp: &str) -> Result<bool, PinError> {
        let fp = canonical(fp)?;
        let before = self.server_keys.len();
        self.server_keys.retain(|p| p.sha256 != fp);

        Ok(self.server_keys.len() != before)
    }

    /// Drops the oldest retired key when the set is full.
    fn make_room(&mut self) -> Result<(), PinError> {
        if self.server_keys.len() < MAX_PINS {
            return Ok(());
        }

        let oldest = self
            .server_keys
            .iter()
            .enumerate()
            .filter_map(|(i, p)| p.retired_ms.map(|r| (r, i)))
            .min()
            .map(|(_, i)| i)
            .ok_or(PinError::Full)?;
        self.server_keys.remove(oldest);

        Ok(())
    }

    /// Applies the keys Forge announced (already verified): adds new ones,
    /// retires announced keys it no longer names, drops keys retired longer
    /// than [`RETIRED_KEPT`]. Keys added by hand are left alone. Returns
    /// whether the set changed.
    pub fn apply_announcement(&mut self, keys: &[ServerKey], now_ms: i64) -> Result<bool, PinError> {
        let announced: Vec<(String, String)> = keys
            .iter()
            .map(|k| canonical(&k.sha256).map(|fp| (fp, clean_label(&k.label))))
            .collect::<Result<_, _>>()?;

        if announced.is_empty() {
            return Ok(false);
        }

        let mut changed = false;

        for (fp, label) in &announced {
            match self.server_keys.iter_mut().find(|p| &p.sha256 == fp) {
                Some(p) => {
                    let relabel = p.label != *label && p.label != LABEL_MANUAL;
                    if relabel || p.retired_ms.is_some() {
                        if relabel {
                            p.label.clone_from(label);
                        }

                        p.retired_ms = None;
                        changed = true;
                    }
                }
                None => {
                    self.make_room()?;
                    self.server_keys.push(Pin {
                        sha256: fp.clone(),
                        label: label.clone(),
                        added_ms: now_ms,
                        retired_ms: None,
                    });
                    changed = true;
                }
            }
        }

        let kept = i64::try_from(RETIRED_KEPT.as_millis()).unwrap_or(i64::MAX);

        for p in &mut self.server_keys {
            if p.retired_ms.is_none() && p.label != LABEL_MANUAL && !announced.iter().any(|(fp, _)| *fp == p.sha256) {
                p.retired_ms = Some(now_ms);
                changed = true;
            }
        }

        let before = self.server_keys.len();
        self.server_keys.retain(|p| p.retired_ms.is_none_or(|r| now_ms - r < kept));

        Ok(changed || self.server_keys.len() != before)
    }

    pub fn parse(text: &str, origin: &str) -> Result<Self, PinError> {
        let set: Self = toml::from_str(text).map_err(|e| PinError::Parse(origin.into(), e.to_string()))?;
        if set.server_keys.len() > MAX_PINS {
            return Err(PinError::Parse(origin.into(), format!("more than {MAX_PINS} keys")));
        }

        for p in &set.server_keys {
            if forge_pki::parse_fingerprint(&p.sha256).as_deref() != Some(p.sha256.as_str()) {
                return Err(PinError::Parse(origin.into(), format!("bad fingerprint {}", p.sha256)));
            }
        }

        Ok(set)
    }

    pub fn to_toml(&self) -> Result<String, PinError> {
        toml::to_string_pretty(self).map_err(|e| PinError::Parse(FILE.into(), e.to_string()))
    }
}

/// Where the pin file of the agent with state directory `state_dir` lives.
#[must_use]
pub fn path(state_dir: &Path) -> PathBuf {
    if root_owned(state_dir) { Path::new(SYSTEM_DIR).join(FILE) } else { state_dir.join(FILE) }
}

/// Whether the pins of `state_dir` live in the root-owned [`SYSTEM_DIR`]:
/// on Linux and macOS for the default state directory, the installer's.
#[must_use]
pub fn root_owned(state_dir: &Path) -> bool {
    cfg!(unix) && state_dir == crate::config::StatePaths::default_dir()
}

/// The staged pin set of an enrollment in `state_dir` ([`STAGED_FILE`]).
#[must_use]
pub fn staged_path(state_dir: &Path) -> PathBuf {
    state_dir.join(STAGED_FILE)
}

/// Where an enrollment writes its pin set: the pin file itself, unless it
/// lives in the root-owned [`SYSTEM_DIR`] and the enrollment does not run
/// as root (the installer enrolls as the agent user). The set is then
/// staged in the state directory for `forge-agent-helper pin` to adopt.
#[must_use]
pub fn enrollment_target(state_dir: &Path, is_root: bool) -> PathBuf {
    if root_owned(state_dir) && !is_root { staged_path(state_dir) } else { path(state_dir) }
}

/// What [`adopt_staged`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Adoption {
    /// No staged set.
    NothingStaged,
    /// The staged set became the pin file; it holds this many keys.
    Adopted(usize),
    /// A pin file already existed and was kept; the staged set was dropped.
    KeptExisting,
}

/// `forge-agent-helper pin` (root, right after a fresh enrollment, the
/// moment it also trusts `identity.json`, D-068): turns the staged pin set
/// at `staged` into the pin file at `system`, only when no pin file exists
/// yet. An existing set is never replaced from the agent-writable staged
/// file. The staged file is checked like the pin file (size, keys,
/// fingerprints) and removed once handled; an invalid one is left in place
/// and reported. Blocking.
pub fn adopt_staged(staged: &Path, system: &Path) -> Result<Adoption, PinError> {
    let shown = staged.display().to_string();

    match std::fs::symlink_metadata(staged) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Adoption::NothingStaged),
        Err(e) => return Err(PinError::Io(shown, e.to_string())),
        Ok(_) => {}
    }

    let _lock = lock(system)?;

    let outcome = match std::fs::symlink_metadata(system) {
        Ok(_) => Adoption::KeptExisting,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let set = load(staged)?;
            if !set.is_empty() {
                save(system, &set)?;
            }

            Adoption::Adopted(set.server_keys.len())
        }
        Err(e) => return Err(PinError::Io(system.display().to_string(), e.to_string())),
    };

    std::fs::remove_file(staged).map_err(|e| PinError::Io(shown, e.to_string()))?;

    Ok(outcome)
}

/// Fails when the pins of `state_dir` live in the root-owned
/// [`SYSTEM_DIR`], that file is missing, and an enrollment's set is still
/// staged: the agent would otherwise start without pins and fall back to
/// the CA and host name. Blocking.
pub fn check_adopted(state_dir: &Path) -> Result<(), String> {
    check_adopted_at(&path(state_dir), &staged_path(state_dir), root_owned(state_dir))
}

fn check_adopted_at(system: &Path, staged: &Path, root_owned: bool) -> Result<(), String> {
    if root_owned && !system.exists() && staged.exists() {
        return Err(format!(
            "the pins of this enrollment are staged in {} but {} does not exist; run 'forge-agent-helper pin' as root to adopt them",
            staged.display(),
            system.display()
        ));
    }

    Ok(())
}

/// Reads the pin set; a missing file is an empty set. Blocking.
pub fn load(path: &Path) -> Result<PinSet, PinError> {
    let shown = path.display().to_string();

    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(PinSet::default()),
        Err(e) => return Err(PinError::Io(shown, e.to_string())),
        Ok(m) if !m.is_file() => return Err(PinError::Io(shown, "not a regular file".into())),
        Ok(m) if m.len() > MAX_FILE => return Err(PinError::Io(shown, format!("larger than {MAX_FILE} bytes"))),
        Ok(_) => {}
    }

    let raw = crate::self_update::read_nofollow(path, MAX_FILE)
        .ok_or_else(|| PinError::Io(shown.clone(), "not readable".into()))?;

    PinSet::parse(&String::from_utf8_lossy(&raw), &shown)
}

/// Writes the pin set (readable by everyone, writable by its owner),
/// creating its folder. Blocking.
pub fn save(path: &Path, set: &PinSet) -> Result<(), PinError> {
    let shown = path.display().to_string();

    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| PinError::Io(dir.display().to_string(), e.to_string()))?;
        #[cfg(unix)]
        if dir == Path::new(SYSTEM_DIR) {
            use std::os::unix::fs::PermissionsExt;
            let meta = std::fs::symlink_metadata(dir).map_err(|e| PinError::Io(SYSTEM_DIR.into(), e.to_string()))?;
            if !meta.is_dir() {
                return Err(PinError::Io(SYSTEM_DIR.into(), "not a directory".into()));
            }

            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755))
                .map_err(|e| PinError::Io(SYSTEM_DIR.into(), e.to_string()))?;
        }
    }

    crate::self_update::write_nofollow(path, set.to_toml()?.as_bytes()).map_err(|e| PinError::Io(shown, e))
}

/// The lock file next to the pin file at `path`.
fn lock_path(path: &Path) -> PathBuf {
    path.with_file_name(LOCK_FILE)
}

/// An exclusive lock on the pin file, held while it lives. Every writer
/// that loads, changes and saves the set (the root helper, the agent where
/// it owns the file, `forge-agent pins`) holds it, so none overwrites
/// another's change.
#[derive(Debug)]
pub struct PinLock {
    _file: std::fs::File,
}

/// Takes the pin file's lock, waiting for another writer. Blocking.
pub fn lock(path: &Path) -> Result<PinLock, PinError> {
    let lp = lock_path(path);
    let shown = lp.display().to_string();

    if let Some(dir) = lp.parent() {
        std::fs::create_dir_all(dir).map_err(|e| PinError::Io(dir.display().to_string(), e.to_string()))?;
    }

    let mut opts = std::fs::OpenOptions::new();
    opts.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o644).custom_flags(i32::try_from(rustix::fs::OFlags::NOFOLLOW.bits()).unwrap_or(0));
    }
    let f = opts.open(&lp).map_err(|e| PinError::Io(shown.clone(), e.to_string()))?;
    f.lock().map_err(|e| PinError::Io(shown, e.to_string()))?;

    Ok(PinLock { _file: f })
}

/// Loads the set under [`lock`], lets `change` modify it, and saves it when
/// `change` reports a change. Returns what `change` returned. Blocking.
pub fn update<T>(
    path: &Path,
    change: impl FnOnce(&mut PinSet) -> Result<(bool, T), PinError>,
) -> Result<(bool, T), PinError> {
    let _lock = lock(path)?;
    let mut set = load(path)?;
    let (changed, out) = change(&mut set)?;
    if changed {
        save(path, &set)?;
    }

    Ok((changed, out))
}

/// `forge-agent pins reset`: moves an unreadable pin file aside to
/// `pins.toml.bad-<now_ms>`, leaving no pins, under [`lock`]. Returns the
/// backup, or `None` when the file is missing or readable (nothing to
/// reset). Blocking.
pub fn reset(path: &Path, now_ms: i64) -> Result<Option<PathBuf>, PinError> {
    let _lock = lock(path)?;

    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(PinError::Io(path.display().to_string(), e.to_string())),
        Ok(_) => {}
    }

    if load(path).is_ok() {
        return Ok(None);
    }

    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| FILE.to_owned());
    let backup = path.with_file_name(format!("{name}.bad-{now_ms}"));
    std::fs::rename(path, &backup).map_err(|e| PinError::Io(path.display().to_string(), e.to_string()))?;

    Ok(Some(backup))
}

/// Checks a key announcement against Forge's command key and this node's
/// id, then applies it to the pin file at `path`. Used by the root helper
/// and, where the agent owns the file, by the agent itself. Blocking.
pub fn apply_signed(
    path: &Path,
    signed: &SignedServerKeys,
    command_public_key: &[u8],
    server_id: &str,
    now: forge_core::Timestamp,
) -> Result<bool, PinError> {
    let keys = forge_proto::verify_server_keys(command_public_key, signed, server_id, now)
        .map_err(|e| PinError::Announcement(e.to_string()))?;
    Ok(update(path, |set| Ok((set.apply_verified(&keys, now.0)?, ())))?.0)
}

/// What to log when the pin file exists but cannot be used: the agent
/// then refuses to connect rather than fall back to the CA and host name.
#[must_use]
pub fn unreadable_message(e: &PinError, path: &Path) -> String {
    format!(
        "the pin file {} cannot be used ({e}); refusing to connect to the control plane. Run 'forge-agent pins reset' then 'forge-agent pins fetch', or remove {}",
        path.display(),
        path.display()
    )
}

/// Part of every [`mismatch_message`], to recognise it in an error chain.
pub const MISMATCH_MARK: &str = "is not pinned (pinned:";

/// The message for a server whose key is not pinned.
#[must_use]
pub fn mismatch_message(presented: &str, pinned: &[String], endpoint: &str) -> String {
    let pinned: Vec<String> = pinned.iter().map(|p| forge_pki::short_fingerprint(p)).collect();
    format!(
        "server key {} is not pinned (pinned: {}). If the server's key changed, run 'forge-server agent-key show' on the server and 'forge-agent pins add <fingerprint>' on this node. If it did not, something else is answering on {endpoint}.",
        forge_pki::short_fingerprint(presented),
        if pinned.is_empty() { "none".to_owned() } else { pinned.join(", ") },
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // reason: tests
mod tests {
    use forge_core::Timestamp;

    use super::*;

    const DAY: i64 = 24 * 3_600_000;

    fn fp(n: u8) -> String {
        format!("{n:02x}").repeat(32)
    }

    fn key(n: u8, label: &str) -> ServerKey {
        ServerKey { sha256: fp(n), label: label.into() }
    }

    fn staged_set(dir: &Path, n: u8) -> PathBuf {
        let mut set = PinSet::default();
        set.add(&fp(n), LABEL_ENROLLMENT, 1).unwrap();
        let p = staged_path(dir);
        save(&p, &set).unwrap();

        p
    }

    #[test]
    fn enrollment_as_the_agent_user_stages_the_root_owned_set() {
        let default = crate::config::StatePaths::default_dir();

        if cfg!(unix) {
            assert_eq!(enrollment_target(&default, false), staged_path(&default));
            assert_eq!(enrollment_target(&default, true), Path::new(SYSTEM_DIR).join(FILE));
        }

        let other = Path::new("/srv/agent-state");
        assert_eq!(enrollment_target(other, false), other.join(FILE));
        assert_eq!(enrollment_target(other, true), other.join(FILE));
    }

    #[test]
    fn adopt_staged_moves_a_fresh_set_into_place() {
        let state = tempfile::tempdir().unwrap();
        let sys = tempfile::tempdir().unwrap();
        let system = sys.path().join("pins").join(FILE);
        let staged = staged_set(state.path(), 1);
        assert_eq!(adopt_staged(&staged, &system).unwrap(), Adoption::Adopted(1));
        assert_eq!(load(&system).unwrap().fingerprints(), vec![fp(1)]);
        assert!(!staged.exists(), "the staged file is removed");
        assert_eq!(adopt_staged(&staged, &system).unwrap(), Adoption::NothingStaged);
    }

    #[test]
    fn adopt_staged_never_replaces_an_existing_set() {
        let state = tempfile::tempdir().unwrap();
        let sys = tempfile::tempdir().unwrap();
        let system = sys.path().join(FILE);
        let mut existing = PinSet::default();
        existing.add(&fp(2), "current", 1).unwrap();
        save(&system, &existing).unwrap();
        let staged = staged_set(state.path(), 3);
        assert_eq!(adopt_staged(&staged, &system).unwrap(), Adoption::KeptExisting);
        assert_eq!(load(&system).unwrap().fingerprints(), vec![fp(2)]);
        assert!(!staged.exists());
    }

    #[test]
    fn adopt_staged_refuses_an_invalid_set() {
        let state = tempfile::tempdir().unwrap();
        let sys = tempfile::tempdir().unwrap();
        let system = sys.path().join(FILE);
        let staged = staged_path(state.path());
        std::fs::write(&staged, "[[server_keys]]\nsha256 = \"nothex\"\nlabel = \"x\"\nadded_ms = 1\n").unwrap();
        assert!(matches!(adopt_staged(&staged, &system), Err(PinError::Parse(..))));
        assert!(!system.exists(), "nothing is pinned from an invalid set");
        let too_many: String = (0..=u8::try_from(MAX_PINS).unwrap())
            .map(|i| format!("[[server_keys]]\nsha256 = \"{}\"\nlabel = \"x\"\nadded_ms = 1\n", fp(i)))
            .collect();
        std::fs::write(&staged, too_many).unwrap();
        assert!(matches!(adopt_staged(&staged, &system), Err(PinError::Parse(..))));
        assert!(!system.exists());
    }

    #[test]
    fn the_agent_refuses_to_start_with_unadopted_pins() {
        let state = tempfile::tempdir().unwrap();
        let system = state.path().join("system").join(FILE);
        let staged = staged_set(state.path(), 4);
        assert!(check_adopted_at(&system, &staged, true).unwrap_err().contains("forge-agent-helper pin"));
        assert!(check_adopted_at(&system, &staged, false).is_ok(), "not root-owned: the staged file is ignored");
        adopt_staged(&staged, &system).unwrap();
        assert!(check_adopted_at(&system, &staged, true).is_ok());
    }

    #[test]
    fn concurrent_writers_lose_no_change_and_never_corrupt_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE);
        let n = u8::try_from(MAX_PINS).unwrap();
        std::thread::scope(|s| {
            for i in 0..n {
                let path = &path;
                s.spawn(move || {
                    for _ in 0..20 {
                        update(path, |set| {
                            set.add(&fp(i), "manual", 1)?;
                            Ok((true, ()))
                        })
                        .unwrap();
                        load(path).unwrap();
                    }
                });
            }
        });
        let set = load(&path).unwrap();
        assert_eq!(set.server_keys.len(), MAX_PINS, "every writer's key is kept");
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[test]
    fn reset_moves_only_an_unreadable_file_aside() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE);
        assert!(reset(&path, 5).unwrap().is_none(), "missing: nothing to reset");
        update(&path, |set| Ok((set.add(&fp(1), "manual", 1)?, ()))).unwrap();
        assert!(reset(&path, 5).unwrap().is_none(), "readable: kept");
        assert!(load(&path).unwrap().contains(&fp(1)));
        std::fs::write(&path, "server_keys = 7").unwrap();
        let e = load(&path).unwrap_err();
        let m = unreadable_message(&e, &path);
        assert!(m.contains("forge-agent pins reset") && m.contains("pins fetch"), "{m}");
        let backup = reset(&path, 5).unwrap().unwrap();
        assert_eq!(backup, dir.path().join("pins.toml.bad-5"));
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), "server_keys = 7");
        assert!(load(&path).unwrap().is_empty());
    }

    #[test]
    fn announcements_add_retire_and_prune() {
        let mut set = PinSet::default();
        assert!(set.apply_announcement(&[key(1, "current"), key(2, "next")], 0).unwrap());
        assert!(!set.apply_announcement(&[key(1, "current"), key(2, "next")], 10).unwrap(), "no change");

        // The server switched: 2 is current, 3 is next, 1 is retired but kept.
        assert!(set.apply_announcement(&[key(2, "current"), key(3, "next")], DAY).unwrap());
        assert_eq!(set.fingerprints(), [fp(1), fp(2), fp(3)]);
        assert_eq!(set.server_keys[0].retired_ms, Some(DAY));

        // A key added by hand is never retired by an announcement.
        set.add(&fp(9), "", DAY).unwrap();

        // 30 days after retiring, 1 is dropped.
        set.apply_announcement(&[key(2, "current"), key(3, "next")], DAY + 31 * DAY).unwrap();
        assert_eq!(set.fingerprints(), [fp(2), fp(3), fp(9)]);
        assert_eq!(set.server_keys[2].label, LABEL_MANUAL);
    }

    #[test]
    fn the_set_holds_at_most_eight_keys() {
        let mut set = PinSet::default();

        for n in 1..=8 {
            set.add(&fp(n), "manual", 0).unwrap();
        }

        assert!(matches!(set.add(&fp(9), "x", 0), Err(PinError::Full)));
        assert!(matches!(set.apply_announcement(&[key(9, "next")], 0), Err(PinError::Full)));

        // A retired key makes room.
        let mut set = PinSet::default();
        set.apply_announcement(&[key(1, "current")], 0).unwrap();

        for n in 2..=8 {
            set.add(&fp(n), "manual", 0).unwrap();
        }

        set.apply_announcement(&[key(2, "current")], 1).unwrap();
        assert!(set.add(&fp(9), "x", 2).unwrap());
        assert!(!set.contains(&fp(1)) && set.contains(&fp(9)));

        // A file with more than eight is refused.
        let mut big = PinSet::default();

        for n in 1..=9 {
            big.server_keys.push(Pin { sha256: fp(n), label: "x".into(), added_ms: 0, retired_ms: None });
        }

        assert!(PinSet::parse(&big.to_toml().unwrap(), "t").is_err());
    }

    #[test]
    fn fingerprints_and_files_are_checked() {
        let mut set = PinSet::default();
        assert!(set.add("sha256:nothex", "x", 0).is_err());
        assert!(set.add(&format!("sha256:{}", fp(10).to_uppercase()), "x", 0).unwrap());
        assert_eq!(set.fingerprints(), [fp(10)]);
        assert!(set.remove(&fp(10)).unwrap() && set.is_empty());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE);
        assert!(load(&path).unwrap().is_empty(), "missing: empty");
        set.add(&fp(1), "x", 0).unwrap();
        save(&path, &set).unwrap();
        assert_eq!(load(&path).unwrap(), set);
        std::fs::write(&path, vec![b'#'; usize::try_from(MAX_FILE).unwrap() + 1]).unwrap();
        assert!(load(&path).is_err(), "too large");
        std::fs::write(&path, "server_keys = [{ sha256 = \"zz\", label = \"x\", added_ms = 0 }]").unwrap();
        assert!(load(&path).is_err(), "bad fingerprint");
    }

    #[test]
    fn only_a_signed_announcement_for_this_node_is_applied() {
        let forge = forge_proto::CommandSigner::generate().unwrap();
        let now = Timestamp(1_800_000_000_000);
        let keys = forge_proto::messages::ServerKeys {
            server_id: "srv-1".into(),
            issued_at_ms: now.0,
            keys: vec![key(1, "current"), key(2, "next")],
            sequence: 0,
            revoked: vec![],
        };
        let signed = forge.sign_server_keys(&keys);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE);
        let attacker = forge_proto::CommandSigner::generate().unwrap();
        assert!(apply_signed(&path, &attacker.sign_server_keys(&keys), &forge.public_key(), "srv-1", now).is_err());
        assert!(apply_signed(&path, &signed, &forge.public_key(), "srv-2", now).is_err());
        assert!(load(&path).unwrap().is_empty());
        assert!(apply_signed(&path, &signed, &forge.public_key(), "srv-1", now).unwrap());
        assert_eq!(load(&path).unwrap().fingerprints(), [fp(1), fp(2)]);
    }

    fn announced(sequence: u64, keys: Vec<ServerKey>, revoked: &[u8]) -> ServerKeys {
        ServerKeys {
            server_id: "srv-1".into(),
            issued_at_ms: 0,
            keys,
            sequence,
            revoked: revoked.iter().map(|n| fp(*n)).collect(),
        }
    }

    #[test]
    fn older_or_conflicting_announcements_are_refused() {
        let mut set = PinSet::default();
        assert!(set.apply_verified(&announced(2, vec![key(1, "current"), key(2, "next")], &[]), 0).unwrap());
        assert_eq!(set.sequence, 2);

        // The same announcement again (a later heartbeat): nothing to do.
        assert!(!set.apply_verified(&announced(2, vec![key(2, "next"), key(1, "current")], &[]), 5).unwrap());

        // Same sequence, other keys: a forged or replayed set.
        assert!(matches!(
            set.apply_verified(&announced(2, vec![key(7, "current")], &[]), 6),
            Err(PinError::Replayed(2))
        ));

        // Older: refused, also an old server's unsequenced one.
        assert!(matches!(
            set.apply_verified(&announced(1, vec![key(7, "current")], &[]), 7),
            Err(PinError::Stale { .. })
        ));
        assert!(set.apply_verified(&announced(0, vec![key(7, "current")], &[]), 7).is_err());
        assert_eq!(set.fingerprints(), [fp(1), fp(2)]);

        // Newer: applied.
        assert!(set.apply_verified(&announced(3, vec![key(2, "current"), key(3, "next")], &[]), 8).unwrap());

        // The file round-trips with its sequence.
        let again = PinSet::parse(&set.to_toml().unwrap(), "t").unwrap();
        assert_eq!(again, set);
    }

    #[test]
    fn revoked_keys_are_removed_at_once_and_never_readded() {
        let mut set = PinSet::default();
        set.apply_verified(&announced(1, vec![key(1, "current"), key(2, "next")], &[]), 0).unwrap();
        set.add(&fp(1), "manual", 0).unwrap();

        // Revoking 1: gone now, not retired; even a hand-added pin.
        assert!(set.apply_verified(&announced(2, vec![key(2, "current"), key(3, "next")], &[1]), 1).unwrap());
        assert_eq!(set.fingerprints(), [fp(2), fp(3)]);
        assert!(set.is_revoked(&fp(1)));

        // A later announcement naming it again does not pin it.
        set.apply_verified(&announced(3, vec![key(1, "current"), key(3, "next")], &[]), 2).unwrap();
        assert!(!set.contains(&fp(1)));

        // By hand only with force.
        assert!(matches!(set.add_checked(&fp(1), "x", 3, false), Err(PinError::Revoked(_))));
        assert!(set.add_checked(&fp(1), "x", 3, true).unwrap());
        assert!(set.contains(&fp(1)) && !set.is_revoked(&fp(1)));
    }

    #[test]
    fn the_mismatch_message_says_what_to_do() {
        let m = mismatch_message(&fp(0xab), &[fp(1)], "192.0.2.10:3843");
        assert!(m.starts_with("server key sha256:abababab…abab is not pinned (pinned: sha256:01010101…0101)"), "{m}");
        assert!(m.contains("forge-agent pins add <fingerprint>") && m.ends_with("answering on 192.0.2.10:3843."));
    }
}
