//! SSH certificates bound to one node (DECISIONS.md D-069).
//!
//! Forge's browser terminal signs a short-lived user certificate. Its only
//! principal used to be the login (`root`), so a certificate issued for one
//! node also opened every other node that trusts Forge's SSH CA. Now each
//! node accepts only `forge.<server_id>.<login>`:
//!
//! * the helper (root) writes this node's id to [`NODE_FILE`] from its pin
//!   and adds `AuthorizedPrincipalsCommand` to Forge's sshd configuration;
//! * sshd runs `forge-agent-helper ssh-principals <user>` (as `nobody`),
//!   which prints the one principal this node accepts for `<user>`;
//! * direct port tunnels (#35 phase 2) log in as [`TUNNEL_USER`] with a
//!   certificate whose key id (`%i`, signed by Forge's CA) names the one
//!   `address/port` it may forward to; the principals line for that user
//!   carries `restrict,port-forwarding,permitopen=..`, and a
//!   `Match User forge-tunnel` block in Forge's drop-in allows only local
//!   TCP forwarding (no Unix sockets, no listeners, no shell or SFTP), so
//!   sshd allows nothing but that one destination;
//! * the agent reports the setup in its facts, and Forge then signs
//!   certificates with that principal for this node.

/// This node's server id, for the principals command (root, 0644).
pub const NODE_FILE: &str = "/etc/ssh/forge_node_id";
/// Forge's drop-in, written by the installer.
pub const DROP_IN: &str = "/etc/ssh/sshd_config.d/50-forge.conf";
const MAIN_CONFIG: &str = "/etc/ssh/sshd_config";
const CA_FILE: &str = "/etc/ssh/forge_user_ca.pub";
const COMMAND: &str = "AuthorizedPrincipalsCommand /usr/local/bin/forge-agent-helper ssh-principals %u %i";
/// The line before direct tunnels; replaced on upgrade.
const OLD_COMMAND: &str = "AuthorizedPrincipalsCommand /usr/local/bin/forge-agent-helper ssh-principals %u";
/// The account direct tunnels log in as: no password, no shell.
pub const TUNNEL_USER: &str = "forge-tunnel";
/// Key ids of tunnel certificates: `forge-tunnel/<tunnel id>/<address>/<port>`.
pub const TUNNEL_KEY_PREFIX: &str = "forge-tunnel/";
/// Opens the sshd block that confines [`TUNNEL_USER`]; it ends the drop-in.
const TUNNEL_MATCH: &str = "Match User forge-tunnel";
const COMMAND_USER: &str = "AuthorizedPrincipalsCommandUser nobody";
const MARK: &str = "# Managed by Orbyn Forge (browser terminal)";

/// The principal Forge signs for `login` on node `server_id`.
#[must_use]
pub fn principal(server_id: &str, login: &str) -> String {
    format!("forge.{server_id}.{login}")
}

fn valid_user(u: &str) -> bool {
    !u.is_empty()
        && u.len() <= 32
        && !u.starts_with('-')
        && u.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

/// The destination a tunnel certificate's key id names: an address that is
/// not unspecified, multicast, broadcast or link-local (cloud metadata), and
/// a port. `None` for anything else.
#[must_use]
pub fn tunnel_destination(key_id: &str) -> Option<(std::net::IpAddr, u16)> {
    let rest = key_id.strip_prefix(TUNNEL_KEY_PREFIX)?;
    let mut parts = rest.split('/');
    let (tunnel, ip, port) = (parts.next()?, parts.next()?, parts.next()?);
    let tunnel_ok = !tunnel.is_empty() && tunnel.len() <= 64 && tunnel.chars().all(|c| c.is_ascii_hexdigit());
    if parts.next().is_some() || !tunnel_ok {
        return None;
    }
    let ip: std::net::IpAddr = ip.parse().ok()?;
    let port: u16 = port.parse().ok().filter(|p| *p != 0)?;
    let blocked = ip.is_unspecified()
        || ip.is_multicast()
        || match ip {
            std::net::IpAddr::V4(v4) => v4.is_link_local() || v4.is_broadcast(),
            std::net::IpAddr::V6(v6) => (v6.segments()[0] & 0xffc0) == 0xfe80,
        };
    (!blocked).then_some((ip, port))
}

/// What `ssh-principals <user> [<key id>]` prints, given the content of
/// [`NODE_FILE`]: the single principal this node accepts for `user`, or
/// nothing. For [`TUNNEL_USER`] the line carries the options that limit the
/// login to forwarding to the key id's destination, and a certificate
/// without a valid tunnel key id gets nothing.
#[must_use]
pub fn principals_for(node_file: &str, user: &str, key_id: &str) -> Option<String> {
    let id = node_file.trim();
    if !(valid_id(id) && valid_user(user)) {
        return None;
    }
    if user != TUNNEL_USER {
        return Some(principal(id, user));
    }
    let (ip, port) = tunnel_destination(key_id)?;
    let dest = match ip {
        std::net::IpAddr::V4(v4) => format!("{v4}:{port}"),
        std::net::IpAddr::V6(v6) => format!("[{v6}]:{port}"),
    };
    Some(format!("restrict,port-forwarding,permitopen=\"{dest}\" {}", principal(id, user)))
}

/// Whether sshd on this node is set up for node-bound certificates
/// (reported in the agent's facts).
#[must_use]
pub fn configured() -> bool {
    let has = |path: &str| {
        std::fs::read_to_string(path).is_ok_and(|s| s.lines().any(|l| l.trim() == COMMAND || l.trim() == OLD_COMMAND))
    };
    std::path::Path::new(NODE_FILE).exists() && (has(DROP_IN) || has(MAIN_CONFIG))
}

/// Whether direct tunnels can log in here (reported in the agent's facts):
/// Forge's drop-in passes the key id to the principals command and confines
/// the tunnel account, and the account exists.
#[must_use]
pub fn tunnel_ready() -> bool {
    let drop_in = std::fs::read_to_string(DROP_IN).unwrap_or_default();
    let has = |line: &str| drop_in.lines().any(|l| l.trim() == line);
    let account = std::fs::read_to_string("/etc/passwd")
        .is_ok_and(|p| p.lines().any(|l| l.split(':').next() == Some(TUNNEL_USER)));
    std::path::Path::new(NODE_FILE).exists() && has(COMMAND) && has(TUNNEL_MATCH) && account
}

/// Appends the block that confines [`TUNNEL_USER`] to Forge's drop-in text:
/// local TCP forwarding only (the principals line names the destination),
/// no Unix-socket forwarding, no listeners, no tty, and `shell` as the
/// forced command so neither a shell nor an SFTP subsystem runs. A `Match`
/// block lasts to the end of its file, so it goes last. `None` when the text
/// is not Forge's or already has it.
#[must_use]
pub fn with_tunnel_block(config: &str, shell: &str) -> Option<String> {
    if !config.lines().any(|l| l.trim() == MARK) || config.lines().any(|l| l.trim() == TUNNEL_MATCH) {
        return None;
    }
    let mut out = config.trim_end().to_owned();
    out.push_str("\n\n# Direct port tunnels: forwarding to one destination, nothing else\n");
    out.push_str(TUNNEL_MATCH);
    out.push('\n');
    for line in [
        "AllowTcpForwarding local",
        "AllowStreamLocalForwarding no",
        "PermitListen none",
        "X11Forwarding no",
        "AllowAgentForwarding no",
        "PermitTTY no",
    ] {
        out.push_str("    ");
        out.push_str(line);
        out.push('\n');
    }
    out.push_str("    ForceCommand ");
    out.push_str(shell);
    out.push('\n');
    Some(out)
}

/// Adds the principals lines to the Forge-managed sshd configuration text,
/// right after its `TrustedUserCAKeys` line. `None` when Forge does not
/// manage it or nothing needs to change.
#[must_use]
pub fn with_principals(config: &str) -> Option<String> {
    if config.lines().any(|l| l.trim() == COMMAND) {
        return None;
    }
    // Upgrade: the line without the key id becomes the current one.
    if config.lines().any(|l| l.trim() == OLD_COMMAND) {
        let mut out = String::with_capacity(config.len() + 4);
        for line in config.lines() {
            out.push_str(if line.trim() == OLD_COMMAND { COMMAND } else { line });
            out.push('\n');
        }
        return Some(out);
    }
    let trust = format!("TrustedUserCAKeys {CA_FILE}");
    let mut out = String::with_capacity(config.len() + 160);
    let mut done = false;
    let mut in_forge_block = false;
    for line in config.lines() {
        out.push_str(line);
        out.push('\n');
        if line.trim() == MARK {
            in_forge_block = true;
        }
        if !done && in_forge_block && line.trim() == trust {
            out.push_str(COMMAND);
            out.push('\n');
            out.push_str(COMMAND_USER);
            out.push('\n');
            done = true;
        }
    }
    done.then_some(out)
}

/// Sets up node-bound certificates when the browser terminal is enabled on
/// this node (Forge's CA is trusted). Root only (the helper). Returns whether
/// sshd was changed.
#[cfg(unix)]
pub async fn ensure(server_id: &str) -> Result<bool, String> {
    use std::os::unix::fs::PermissionsExt;
    if !std::path::Path::new(CA_FILE).exists() {
        return Ok(false);
    }
    if !valid_id(server_id) {
        return Err(format!("invalid server id `{server_id}`"));
    }
    let current = std::fs::read_to_string(NODE_FILE).unwrap_or_default();
    if current.trim() != server_id {
        std::fs::write(NODE_FILE, format!("{server_id}\n")).map_err(|e| format!("{NODE_FILE}: {e}"))?;
        std::fs::set_permissions(NODE_FILE, std::fs::Permissions::from_mode(0o644))
            .map_err(|e| format!("{NODE_FILE}: {e}"))?;
    }
    ensure_tunnel_user().await?;
    let path = if std::path::Path::new(DROP_IN).exists() { DROP_IN } else { MAIN_CONFIG };
    // The agent (unprivileged) reads this file to report the setup in its
    // facts (`configured`), so it must be world-readable like sshd_config;
    // the installer used to create the drop-in 0600. It holds no secrets.
    let readable =
        || std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644)).map_err(|e| format!("{path}: {e}"));
    let before = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    let mut after = with_principals(&before).unwrap_or_else(|| before.clone());
    // The tunnel account is confined only in Forge's own drop-in, where the
    // `Match` block can end the file; without it the node reports no
    // direct-tunnel support.
    if path == DROP_IN
        && let Some(with_block) = with_tunnel_block(&after, nologin())
    {
        after = with_block;
    }
    if after == before {
        readable()?;
        return Ok(false);
    }
    std::fs::write(path, &after).map_err(|e| format!("{path}: {e}"))?;
    readable()?;
    let sshd = ["/usr/sbin/sshd", "/usr/bin/sshd", "/sbin/sshd"].into_iter().find(|p| std::path::Path::new(p).exists());
    let ok = match sshd {
        Some(s) => tokio::process::Command::new(s).arg("-t").output().await.is_ok_and(|o| o.status.success()),
        None => false,
    };
    if !ok {
        // Never leave sshd unable to start.
        let _ = std::fs::write(path, &before);
        return Err("sshd rejected the node-bound certificate settings; left unchanged".into());
    }
    for unit in ["ssh", "sshd"] {
        if tokio::process::Command::new("systemctl")
            .args(["reload", unit])
            .output()
            .await
            .is_ok_and(|o| o.status.success())
        {
            break;
        }
    }
    Ok(true)
}

/// Creates [`TUNNEL_USER`]: a system account without home or shell. Its
/// password field is `*` (no password, but not "locked", which sshd refuses
/// outright without PAM); only Forge's tunnel certificates log in, and the
/// principals line limits them to forwarding.
#[cfg(unix)]
fn nologin() -> &'static str {
    ["/usr/sbin/nologin", "/sbin/nologin", "/bin/false"]
        .into_iter()
        .find(|p| std::path::Path::new(p).exists())
        .unwrap_or("/bin/false")
}

#[cfg(unix)]
async fn ensure_tunnel_user() -> Result<(), String> {
    let exists = std::fs::read_to_string("/etc/passwd")
        .map_err(|e| format!("/etc/passwd: {e}"))?
        .lines()
        .any(|l| l.split(':').next() == Some(TUNNEL_USER));
    if exists {
        return Ok(());
    }
    let shell = nologin();
    let out = tokio::process::Command::new("useradd")
        .args(["--system", "--no-create-home", "--home-dir", "/nonexistent", "--shell", shell, "-p", "*", TUNNEL_USER])
        .output()
        .await
        .map_err(|e| format!("useradd: {e}"))?;
    if !out.status.success() {
        return Err(format!("useradd {TUNNEL_USER}: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// Security review (D-069): a certificate for node A must not open node B.
    #[test]
    fn each_node_accepts_only_its_own_principal() {
        let a = "01a1d3c4-8057-76f1-a2dd-df3f6ebd2625";
        let b = "01a1d3c4-8250-754a-988c-8474d92c6a86";
        let cert_for_a = principal(a, "root");
        assert_eq!(principals_for(&format!("{a}\n"), "root", "").as_deref(), Some(cert_for_a.as_str()));
        assert_ne!(principals_for(b, "root", "").as_deref(), Some(cert_for_a.as_str()), "B refuses A's certificate");
        assert_ne!(principals_for(a, "admin", "").as_deref(), Some(cert_for_a.as_str()), "and another login");
        assert!(principals_for(a, "-oProxy", "").is_none() && principals_for("x;y", "root", "").is_none());
        assert!(principals_for("", "root", "").is_none());
    }

    #[test]
    fn principals_go_into_the_forge_block_once() {
        let drop_in = format!("{MARK}\nTrustedUserCAKeys {CA_FILE}\n");
        let new = with_principals(&drop_in).unwrap();
        assert!(new.contains(COMMAND) && new.contains(COMMAND_USER));
        assert!(with_principals(&new).is_none(), "idempotent");
        // Appended to the main config: only inside Forge's block.
        let main = format!("Port 22\nTrustedUserCAKeys /etc/ssh/other_ca.pub\n\n{MARK}\nTrustedUserCAKeys {CA_FILE}\n");
        let out = with_principals(&main).unwrap();
        assert_eq!(out.matches(COMMAND).count(), 1);
        assert!(out.find(COMMAND).unwrap() > out.find(MARK).unwrap());
        // Not Forge-managed: untouched.
        assert!(with_principals("Port 22\n").is_none());
    }

    /// Security review (#35 phase 2): a tunnel certificate opens exactly one
    /// destination, only as the tunnel account, and nothing else.
    #[test]
    fn tunnel_logins_forward_to_one_destination_only() {
        let a = "01a1d3c4-8057-76f1-a2dd-df3f6ebd2625";
        let line = principals_for(a, TUNNEL_USER, "forge-tunnel/ab12/127.0.0.1/5432").unwrap();
        assert_eq!(line, format!("restrict,port-forwarding,permitopen=\"127.0.0.1:5432\" forge.{a}.forge-tunnel"));
        let v6 = principals_for(a, TUNNEL_USER, "forge-tunnel/ab12/::1/22").unwrap();
        assert!(v6.contains("permitopen=\"[::1]:22\""), "{v6}");
        // Not a tunnel certificate (a terminal one), or a bad destination: nothing.
        for bad in [
            "",
            "terminal:alice",
            "forge-tunnel/ab12/127.0.0.1/0",
            "forge-tunnel/ab12/169.254.169.254/80",
            "forge-tunnel/ab12/0.0.0.0/80",
            "forge-tunnel/ab12/fe80::1/80",
            "forge-tunnel/ab12/224.0.0.1/80",
            "forge-tunnel/ab12/db.example.com/5432",
            "forge-tunnel/ab12/127.0.0.1/5432/x",
            "forge-tunnel/zz\"/127.0.0.1/5432",
            "forge-tunnel//127.0.0.1/5432",
        ] {
            assert!(principals_for(a, TUNNEL_USER, bad).is_none(), "{bad}");
        }
        // Other logins ignore the key id: a tunnel certificate's principal
        // (`forge.<id>.forge-tunnel`) never matches theirs.
        assert_eq!(principals_for(a, "root", "forge-tunnel/ab12/127.0.0.1/22").unwrap(), principal(a, "root"));
    }

    /// Security review (#35 phase 2): `port-forwarding` alone would also
    /// allow Unix sockets, listeners and SFTP; the block confines the account.
    #[test]
    fn the_tunnel_account_is_confined_once_at_the_end() {
        let drop_in = format!("{MARK}\nTrustedUserCAKeys {CA_FILE}\n{COMMAND}\n{COMMAND_USER}\n");
        let out = with_tunnel_block(&drop_in, "/usr/sbin/nologin").unwrap();
        let tail = &out[out.find(TUNNEL_MATCH).unwrap()..];
        for want in [
            "AllowTcpForwarding local",
            "AllowStreamLocalForwarding no",
            "PermitListen none",
            "PermitTTY no",
            "ForceCommand /usr/sbin/nologin",
        ] {
            assert!(tail.contains(want), "{want}");
        }
        assert!(out.find(COMMAND).unwrap() < out.find(TUNNEL_MATCH).unwrap(), "global lines stay before the Match");
        assert!(with_tunnel_block(&out, "/usr/sbin/nologin").is_none(), "idempotent");
        assert!(with_principals(&out).is_none(), "principals already there");
        assert!(with_tunnel_block("Port 22\n", "/bin/false").is_none(), "not Forge's file");
    }

    #[test]
    fn the_old_principals_line_is_upgraded() {
        let old = format!("{MARK}\nTrustedUserCAKeys {CA_FILE}\n{OLD_COMMAND}\n{COMMAND_USER}\n");
        let new = with_principals(&old).unwrap();
        assert_eq!(new.matches(COMMAND).count(), 1);
        assert!(!new.lines().any(|l| l.trim() == OLD_COMMAND));
        assert!(with_principals(&new).is_none());
    }
}
