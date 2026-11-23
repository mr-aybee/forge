//! Terminal accounts (DECISIONS.md D-078).
//!
//! * **Discovery:** the accounts a browser terminal can log in as, reported
//!   in the agent's facts as `login_accounts`: `root` when sshd permits root
//!   logins with keys, and local users with uid ≥ 1000, a real login shell and
//!   an unlocked, unexpired account. The helper (root) reads `/etc/shadow` and
//!   `sshd -T`; without it the agent reports what `/etc/passwd` shows.
//! * **Creation:** the helper creates a terminal account: no password (`*`, so
//!   only Forge's node-bound SSH certificates can log in; D-069), a home
//!   directory, and `sudo` through a drop-in in `/etc/sudoers.d` only when
//!   asked, checked with `visudo -c` before it is put in place. Idempotent.

use std::collections::BTreeSet;

/// The agent's own unprivileged account; never a terminal account.
pub const AGENT_USER: &str = "forge-agent";
/// Prefix of Forge's sudoers drop-ins (`/etc/sudoers.d/forge-<name>`).
#[cfg(unix)]
const SUDOERS_PREFIX: &str = "forge-";
const SUDOERS_MARK: &str = "# Managed by Orbyn Forge (terminal account)";
/// First uid of regular users on Debian, Ubuntu and RHEL-likes.
const FIRST_UID: u32 = 1000;
/// `nobody` on most distributions.
const NOBODY_UID: u32 = 65534;
/// At most this many accounts are reported.
const MAX_REPORTED: usize = 64;

pub use forge_core::tags::valid_account_name;

/// One `/etc/passwd` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasswdEntry {
    pub name: String,
    pub uid: u32,
    pub shell: String,
}

/// Parses `/etc/passwd`; malformed lines are skipped.
#[must_use]
pub fn parse_passwd(text: &str) -> Vec<PasswdEntry> {
    text.lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| {
            let f: Vec<&str> = l.split(':').collect();
            if f.len() < 7 {
                return None;
            }
            Some(PasswdEntry { name: f[0].to_owned(), uid: f[2].parse().ok()?, shell: f[6].trim().to_owned() })
        })
        .collect()
}

/// Accounts `/etc/shadow` marks as locked (`!…`) or expired on `today`
/// (days since 1970-01-01).
#[must_use]
pub fn unusable_accounts(shadow: &str, today: i64) -> BTreeSet<String> {
    shadow
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split(':').collect();
            if f.len() < 2 {
                return None;
            }
            let locked = f[1].starts_with('!');
            let expired = f.get(7).and_then(|e| e.trim().parse::<i64>().ok()).is_some_and(|e| e <= today);
            (locked || expired).then(|| f[0].to_owned())
        })
        .collect()
}

/// Whether `sshd -T` output lets root log in with a key or certificate.
#[must_use]
pub fn root_login_allowed(sshd_t: &str) -> bool {
    sshd_t.lines().any(|l| {
        let mut w = l.split_whitespace();
        w.next().is_some_and(|k| k.eq_ignore_ascii_case("permitrootlogin"))
            && w.next().is_some_and(|v| matches!(v, "yes" | "prohibit-password" | "without-password"))
    })
}

/// A shell someone can log in with (not `nologin`, `false` or `sync`).
fn login_shell(shell: &str) -> bool {
    let base = shell.rsplit('/').next().unwrap_or_default();
    shell.starts_with('/') && !matches!(base, "" | "nologin" | "false" | "sync" | "halt" | "shutdown")
}

/// Accounts a terminal can log in as: `root` first (when allowed), then
/// regular users in `/etc/passwd` order. `shadow` is `None` when it cannot
/// be read (the unprivileged agent), so locks are not checked.
#[must_use]
pub fn discover(passwd: &str, shadow: Option<&str>, root_allowed: bool, today: i64) -> Vec<String> {
    let unusable = shadow.map(|s| unusable_accounts(s, today)).unwrap_or_default();
    let mut out = Vec::new();
    if root_allowed {
        out.push("root".to_owned());
    }
    for e in parse_passwd(passwd) {
        if e.uid >= FIRST_UID
            && e.uid != NOBODY_UID
            && e.name != AGENT_USER
            && login_shell(&e.shell)
            && !unusable.contains(&e.name)
            && forge_core::tags::valid_login(&e.name)
            && !out.contains(&e.name)
        {
            out.push(e.name);
        }
    }
    out.truncate(MAX_REPORTED);
    out
}

/// Checks a requested terminal account against this node's users and
/// groups. `Ok(true)` when the account already exists as a regular user (it
/// is kept as it is), `Ok(false)` when it will be created.
pub fn check_account(name: &str, passwd: &str, group: &str) -> Result<bool, String> {
    if !valid_account_name(name) {
        return Err(format!(
            "`{name}` is not allowed: use 1–32 of a-z, 0-9, `_` and `-`, starting with a letter or `_`, and not a system name"
        ));
    }
    if let Some(e) = parse_passwd(passwd).into_iter().find(|e| e.name == name) {
        if e.uid < FIRST_UID || e.uid == NOBODY_UID {
            return Err(format!("`{name}` is a system account on this node (uid {})", e.uid));
        }
        return Ok(true);
    }
    if group.lines().any(|l| l.split(':').next() == Some(name)) {
        return Err(format!("`{name}` is already a group on this node; choose another name"));
    }
    Ok(false)
}

/// The sudoers drop-in that gives `name` passwordless sudo.
#[must_use]
pub fn sudoers_entry(name: &str) -> String {
    format!("{SUDOERS_MARK}\n{name} ALL=(ALL:ALL) NOPASSWD: ALL\n")
}

#[cfg(unix)]
fn today() -> i64 {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    i64::try_from(secs / 86_400).unwrap_or(i64::MAX)
}

#[cfg(unix)]
fn sshd() -> Option<&'static str> {
    ["/usr/sbin/sshd", "/usr/bin/sshd", "/sbin/sshd"].into_iter().find(|p| std::path::Path::new(p).exists())
}

/// Discovery with root's view (the helper): locks, expiry and sshd's root
/// login policy are checked.
#[cfg(unix)]
pub async fn gather() -> Vec<String> {
    let passwd = std::fs::read_to_string("/etc/passwd").unwrap_or_default();
    let shadow = std::fs::read_to_string("/etc/shadow").ok();
    let root = match sshd() {
        Some(s) => tokio::process::Command::new(s)
            .arg("-T")
            .output()
            .await
            .is_ok_and(|o| o.status.success() && root_login_allowed(&String::from_utf8_lossy(&o.stdout))),
        None => false,
    };
    discover(&passwd, shadow.as_deref(), root, today())
}

/// Discovery from `/etc/passwd` alone (the unprivileged agent, when the
/// helper is unavailable): no root, no lock check.
#[must_use]
pub fn gather_unprivileged() -> Vec<String> {
    discover(&std::fs::read_to_string("/etc/passwd").unwrap_or_default(), None, false, 0)
}

#[cfg(unix)]
async fn run(program: &str, args: &[&str]) -> Result<(), String> {
    let out = tokio::process::Command::new(program).args(args).output().await.map_err(|e| format!("{program}: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!("{program} failed: {}", String::from_utf8_lossy(&out.stderr).trim()))
    }
}

/// Creates (or keeps) terminal account `name` and sets its sudo drop-in to
/// `sudo`. Root only (the helper). Returns `{name, created, sudo}`.
#[cfg(unix)]
pub async fn ensure(name: &str, sudo: bool) -> Result<serde_json::Value, String> {
    use std::os::unix::fs::PermissionsExt;
    let passwd = std::fs::read_to_string("/etc/passwd").map_err(|e| format!("/etc/passwd: {e}"))?;
    let group = std::fs::read_to_string("/etc/group").unwrap_or_default();
    let existed = check_account(name, &passwd, &group)?;
    if !existed {
        let shell = if std::path::Path::new("/bin/bash").exists() { "/bin/bash" } else { "/bin/sh" };
        run(
            "useradd",
            &["--create-home", "--shell", shell, "--comment", "Orbyn Forge terminal account", "--user-group", name],
        )
        .await?;
        // No password. `*` (not `!`) is "no password" rather than "locked",
        // so certificate logins also work where sshd does not use PAM.
        run("usermod", &["-p", "*", name]).await?;
    }
    let dir = std::path::Path::new("/etc/sudoers.d");
    let file = dir.join(format!("{SUDOERS_PREFIX}{name}"));
    if sudo {
        if !dir.is_dir() {
            return Err("sudo is not installed on this node (/etc/sudoers.d is missing)".into());
        }
        // sudo ignores files whose name contains a dot, so the draft is inert.
        let draft = dir.join(format!(".{SUDOERS_PREFIX}{name}.new"));
        std::fs::write(&draft, sudoers_entry(name)).map_err(|e| format!("{}: {e}", draft.display()))?;
        std::fs::set_permissions(&draft, std::fs::Permissions::from_mode(0o440))
            .map_err(|e| format!("{}: {e}", draft.display()))?;
        let draft_s = draft.to_string_lossy().into_owned();
        if let Err(e) = run("visudo", &["-c", "-q", "-f", &draft_s]).await {
            let _ = std::fs::remove_file(&draft);
            return Err(format!("the sudo rule was rejected, nothing changed: {e}"));
        }
        std::fs::rename(&draft, &file).map_err(|e| format!("{}: {e}", file.display()))?;
    } else if std::fs::read_to_string(&file).is_ok_and(|s| s.starts_with(SUDOERS_MARK)) {
        std::fs::remove_file(&file).map_err(|e| format!("{}: {e}", file.display()))?;
    }
    Ok(serde_json::json!({ "name": name, "created": !existed, "sudo": sudo }))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const PASSWD: &str = "\
root:x:0:0:root:/root:/bin/bash
daemon:x:1:1:daemon:/usr/sbin:/usr/sbin/nologin
forge-agent:x:998:998::/var/lib/forge-agent:/usr/sbin/nologin
nobody:x:65534:65534:nobody:/nonexistent:/usr/sbin/nologin
alice:x:1000:1000:Alice:/home/alice:/bin/bash
bob:x:1001:1001::/home/bob:/bin/zsh
svc:x:1002:1002::/home/svc:/usr/sbin/nologin
carol:x:1003:1003::/home/carol:/bin/false
dave:x:1004:1004::/home/dave:/bin/bash
old:x:1005:1005::/home/old:/bin/bash
broken line
";
    const SHADOW: &str = "\
root:*:19000:0:99999:7:::
alice:$y$j9T$abc:19000:0:99999:7:::
bob:*:19000:0:99999:7:::
dave:!$y$j9T$abc:19000:0:99999:7:::
old:$y$j9T$abc:19000:0:99999:7::19500:
";

    #[test]
    fn discovery_keeps_regular_unlocked_users_with_a_shell() {
        assert_eq!(discover(PASSWD, Some(SHADOW), true, 20_000), ["root", "alice", "bob"]);
        // Without root's view: no root, no lock or expiry check.
        assert_eq!(discover(PASSWD, None, false, 20_000), ["alice", "bob", "dave", "old"]);
        // Not expired yet.
        assert!(discover(PASSWD, Some(SHADOW), false, 19_000).contains(&"old".to_owned()));
    }

    #[test]
    fn root_login_policy_from_sshd_t() {
        assert!(root_login_allowed("port 22\npermitrootlogin prohibit-password\n"));
        assert!(root_login_allowed("permitrootlogin yes"));
        assert!(root_login_allowed("permitrootlogin without-password"));
        assert!(!root_login_allowed("permitrootlogin no"));
        assert!(!root_login_allowed("permitrootlogin forced-commands-only"));
        assert!(!root_login_allowed(""));
    }

    #[test]
    fn account_names_are_validated() {
        for ok in ["forge-ops", "_ops", "ops2", "a"] {
            assert!(valid_account_name(ok), "{ok}");
        }
        for bad in
            ["", "Ops", "1ops", "-ops", "ops.x", "ops$", "root", "forge-agent", "nobody", "systemd-x", &"a".repeat(33)]
        {
            assert!(!valid_account_name(bad), "{bad}");
        }
    }

    #[test]
    fn requested_accounts_never_collide_with_system_users_or_groups() {
        let group = "root:x:0:\nsudo:x:27:alice\nstaff:x:50:\n";
        assert_eq!(check_account("forge-ops", PASSWD, group), Ok(false));
        assert_eq!(check_account("alice", PASSWD, group), Ok(true));
        assert!(check_account("forge-agent", PASSWD, group).is_err());
        assert!(check_account("daemon", PASSWD, group).is_err());
        assert!(check_account("staff", PASSWD, group).unwrap_err().contains("group"));
        let sys = "backupd:x:120:120::/:/bin/bash\n";
        assert!(check_account("backupd", sys, "").unwrap_err().contains("system account"));
    }

    #[test]
    fn sudoers_entry_is_one_rule() {
        assert_eq!(
            sudoers_entry("forge-ops"),
            "# Managed by Orbyn Forge (terminal account)\nforge-ops ALL=(ALL:ALL) NOPASSWD: ALL\n"
        );
    }
}
