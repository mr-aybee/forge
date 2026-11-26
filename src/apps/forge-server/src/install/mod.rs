//! Agent distribution and the node installer.
//!
//! One installer serves both onboarding paths:
//! * **manual** — `curl -fsSL <url>/install.sh | sudo FORGE_ENROLL_TOKEN=… sh`
//! * **assisted** — the `node.bootstrap` job uploads the same script (with
//!   the token embedded) over SSH and runs it with sudo.
//!
//! The script downloads the agent from the control plane, verifies its
//! SHA-256 against the value baked into the script, installs a dedicated
//! unprivileged `forge-agent` user, enrolls (the agent pins the CA through
//! the token) and starts the agent under systemd, or a respawn loop where
//! there is no systemd (containers).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use forge_secrets::crypto::sha256_hex;
use serde::{Deserialize, Serialize};

/// Binaries the control plane can hand out, keyed by `(os, arch, name)`.
#[derive(Debug, Clone, Default)]
pub struct AgentDist {
    dir: PathBuf,
    sums: HashMap<String, String>,
    /// The version in `MANIFEST.json`, when the folder has one.
    version: Option<String>,
}

/// `MANIFEST.json` in the agent folder: the version of the agent binaries
/// there and the SHA-256 of each. The control plane offers the agents as
/// this version (not its own) and never hands out a file whose hash does
/// not match. Written by the release package (`package.sh`), the
/// self-updater from the signed `SHA256SUMS`, and the dev scripts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DistManifest {
    pub version: String,
    /// File name → lower-case hex SHA-256.
    pub files: BTreeMap<String, String>,
}

/// File name of the manifest in `agent_dist_dir`.
pub const MANIFEST: &str = "MANIFEST.json";
/// Largest manifest read (bytes).
const MAX_MANIFEST: u64 = 64 * 1024;

impl DistManifest {
    /// Reads `<dir>/MANIFEST.json`: `Ok(None)` when there is none.
    ///
    /// # Errors
    /// When it cannot be read, is not valid JSON, or names an invalid version.
    pub fn read(dir: &Path) -> Result<Option<Self>, String> {
        use std::io::Read as _;
        let path = dir.join(MANIFEST);
        let f = match std::fs::File::open(&path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };

        let mut raw = Vec::new();
        f.take(MAX_MANIFEST).read_to_end(&mut raw).map_err(|e| format!("{}: {e}", path.display()))?;
        let m: Self = serde_json::from_slice(&raw).map_err(|e| format!("{}: {e}", path.display()))?;
        if !forge_catalog::versions::valid_build(&m.version) {
            return Err(format!("{}: `{}` is not a version", path.display(), m.version));
        }

        Ok(Some(m))
    }

    /// Writes `<dir>/MANIFEST.json` through a temporary file, fsync and a
    /// rename, so a reader sees the old or the new manifest, never half.
    ///
    /// # Errors
    /// When the file cannot be written.
    pub fn write(&self, dir: &Path) -> Result<(), String> {
        use std::io::Write as _;
        let raw = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        let tmp = dir.join(format!("{MANIFEST}.tmp"));
        let mut f = std::fs::File::create(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
        f.write_all(&raw).and_then(|()| f.sync_all()).map_err(|e| format!("{}: {e}", tmp.display()))?;
        drop(f);

        std::fs::rename(&tmp, dir.join(MANIFEST)).map_err(|e| format!("{}: {e}", dir.join(MANIFEST).display()))
    }
}

/// Supported Linux architectures (as `uname -m` reports them after normalising).
pub const LINUX_ARCHES: [&str; 2] = ["x86_64", "aarch64"];
/// Binaries handed to Linux nodes; `forge-dns` is optional (older packages
/// do not carry it, and a node installs it only when offered).
pub const BINARIES: [&str; 3] = ["forge-agent", "forge-agent-helper", "forge-dns"];
/// Windows architectures with an agent build (D-079); there is no helper on Windows.
pub const WINDOWS_ARCHES: [&str; 1] = ["x86_64"];
/// Release asset name of the Windows agent.
fn windows_file(arch: &str) -> String {
    format!("forge-agent-windows-{arch}.exe")
}

fn file_name(os: &str, arch: &str, bin: &str) -> String {
    format!("{bin}-{os}-{arch}")
}

impl AgentDist {
    /// Scans `dir` for `forge-agent-linux-x86_64` style files and hashes them.
    pub fn scan(dir: &Path) -> Self {
        let mut sums = HashMap::new();

        for arch in LINUX_ARCHES {
            for bin in BINARIES {
                let name = file_name("linux", arch, bin);
                if let Ok(bytes) = std::fs::read(dir.join(&name)) {
                    sums.insert(name, sha256_hex(&bytes));
                }
            }
        }

        for arch in WINDOWS_ARCHES {
            let name = windows_file(arch);
            if let Ok(bytes) = std::fs::read(dir.join(&name)) {
                sums.insert(name, sha256_hex(&bytes));
            }
        }

        let version = match DistManifest::read(dir) {
            Ok(Some(m)) => {
                // Only files the manifest lists, with its hash: a file
                // replaced by hand (or half copied) is not handed out as
                // this version.
                sums.retain(|name, sum| {
                    let ok = m.files.get(name).is_some_and(|want| want.eq_ignore_ascii_case(sum));
                    if !ok {
                        tracing::warn!(file = %name, version = %m.version, "agent binary does not match MANIFEST.json; not handed out");
                    }

                    ok
                });
                Some(m.version)
            }
            Ok(None) => {
                if !sums.is_empty() {
                    tracing::warn!(dir = %dir.display(), version = crate::VERSION, "no MANIFEST.json beside the agent binaries; offering them as this server's version");
                }

                None
            }
            Err(e) => {
                tracing::error!(error = %e, "unreadable agent MANIFEST.json; no agent binaries are handed out until it is fixed");
                sums.clear();
                None
            }
        };

        if sums.is_empty() {
            tracing::warn!(dir = %dir.display(), "no agent binaries found; node onboarding is unavailable until they are added");
        }

        Self { dir: dir.to_path_buf(), sums, version }
    }

    /// The version of the agents this control plane hands out: the one in
    /// `MANIFEST.json`, or this server's own version for a folder without
    /// one (packages older than the manifest).
    #[must_use]
    pub fn version(&self) -> &str {
        self.version.as_deref().unwrap_or(crate::VERSION)
    }

    /// Whether [`version`](Self::version) comes from `MANIFEST.json`.
    #[must_use]
    pub fn has_manifest(&self) -> bool {
        self.version.is_some()
    }

    #[must_use]
    pub fn available(&self) -> bool {
        !self.sums.is_empty()
    }

    /// Path and checksum of one binary, if present. Inputs are matched
    /// against fixed lists, so no path traversal is possible.
    #[must_use]
    pub fn binary(&self, os: &str, arch: &str, bin: &str) -> Option<(PathBuf, &str)> {
        if os == "windows" && WINDOWS_ARCHES.contains(&arch) && bin == "forge-agent" {
            let name = windows_file(arch);
            return self.sums.get(&name).map(|sum| (self.dir.join(&name), sum.as_str()));
        }

        if os != "linux" || !LINUX_ARCHES.contains(&arch) || !BINARIES.contains(&bin) {
            return None;
        }

        let name = file_name(os, arch, bin);

        self.sums.get(&name).map(|sum| (self.dir.join(&name), sum.as_str()))
    }

    /// Whether a Windows agent build is present.
    #[must_use]
    pub fn windows_available(&self) -> bool {
        WINDOWS_ARCHES.iter().any(|a| self.sums.contains_key(&windows_file(a)))
    }

    fn windows_sum(&self, arch: &str) -> &str {
        self.sums.get(&windows_file(arch)).map_or("", String::as_str)
    }

    fn sum(&self, arch: &str, bin: &str) -> &str {
        self.sums.get(&file_name("linux", arch, bin)).map_or("", String::as_str)
    }
}

/// Values baked into the script.
#[derive(Debug)]
pub struct InstallParams<'a> {
    /// API base URL as reachable from the node (downloads).
    pub forge_url: &'a str,
    /// Agent endpoint as reachable from the node.
    pub agent_url: &'a str,
    /// Embedded token (assisted path). `None` → read `FORGE_ENROLL_TOKEN`.
    pub token: Option<&'a str>,
    pub hostname: Option<&'a str>,
    /// Forge's SSH user-CA public key; sshd is told to trust it so browser
    /// terminal sessions can log in with short-lived certificates.
    pub ssh_user_ca: Option<&'a str>,
    /// Terminal account the helper creates (D-078), already validated.
    pub terminal_account: Option<&'a str>,
    /// Passwordless sudo for that account.
    pub terminal_account_sudo: bool,
}

fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Renders the POSIX shell installer.
#[must_use]
pub fn render_script(dist: &AgentDist, p: &InstallParams<'_>) -> String {
    let token_line = match p.token {
        Some(t) => format!("FORGE_ENROLL_TOKEN={}", sh_quote(t)),
        None => r#": "${FORGE_ENROLL_TOKEN:?set FORGE_ENROLL_TOKEN to the token shown in Orbyn Forge}""#.to_owned(),
    };

    let ssh_ca = sh_quote(p.ssh_user_ca.unwrap_or_default());
    let host_flag = p.hostname.map(|h| format!(" --hostname {}", sh_quote(h))).unwrap_or_default();
    let account = sh_quote(p.terminal_account.unwrap_or_default());
    let account_sudo = if p.terminal_account.is_some() && p.terminal_account_sudo { "1" } else { "0" };
    let sums = LINUX_ARCHES
        .iter()
        .map(|a| {
            format!(
                "    {a}) AGENT_SHA={}; HELPER_SHA={}; DNS_SHA={} ;;",
                sh_quote(dist.sum(a, "forge-agent")),
                sh_quote(dist.sum(a, "forge-agent-helper")),
                sh_quote(dist.sum(a, "forge-dns"))
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        r#"#!/bin/sh
# Orbyn Forge node installer. Installs forge-agent as an unprivileged service
# and enrolls this machine. Safe to re-run.
set -eu
umask 077
FORGE_URL={forge_url}
AGENT_URL={agent_url}
{token_line}
SSH_USER_CA={ssh_ca}
TERMINAL_ACCOUNT={account}
TERMINAL_ACCOUNT_SUDO={account_sudo}

# Console: every step is printed as a `$ command` line and also reported to
# Forge (authenticated by the enrollment token, sent from a 0600 header file,
# never on a command line), so the onboarding console shows manual installs
# too. Reporting is best effort and never fails the install.
TMP=$(mktemp -d)
printf 'Authorization: Bearer %s\n' "$FORGE_ENROLL_TOKEN" > "$TMP/report-header"
report() {{
  command -v curl >/dev/null 2>&1 || return 0
  printf '%s' "$1" | curl -fsS -m 5 -X POST -H @"$TMP/report-header" -H 'Content-Type: text/plain' \
    --data-binary @- "$FORGE_URL/api/v1/enroll/progress" >/dev/null 2>&1 || true
}}
say() {{ printf 'forge-install: %s\n' "$*"; report "$*"; }}
step() {{ printf '$ %s\n' "$*"; report "\$ $*"; }}
die() {{ printf 'forge-install: error: %s\n' "$*" >&2; report "error: $*"; exit 1; }}
finish() {{
  rc=$?
  if [ "$rc" -eq 0 ]; then report "installer finished"; else report "installer failed (exit $rc)"; fi
  rm -rf "$TMP"
}}
trap finish EXIT

say "installing forge-agent on $(uname -n)"
step "id -u  # must be root"
[ "$(id -u)" -eq 0 ] || die "run as root (e.g. with sudo)"
step "uname -s -m"
[ "$(uname -s)" = "Linux" ] || die "this installer is for Linux; see the Forge documentation for Windows nodes"

case "$(uname -m)" in
  x86_64|amd64) ARCH=x86_64 ;;
  aarch64|arm64) ARCH=aarch64 ;;
  *) die "unsupported architecture $(uname -m)" ;;
esac
case "$ARCH" in
{sums}
esac
[ -n "$AGENT_SHA" ] || die "no agent build for $ARCH on this control plane"
say "architecture $ARCH"

fetch() {{
  if command -v curl >/dev/null 2>&1; then curl -fsSL "$1" -o "$2" || die "download failed: $1";
  elif command -v wget >/dev/null 2>&1; then wget -qO "$2" "$1" || die "download failed: $1";
  else die "need curl or wget"; fi
}}
verify() {{ echo "$2  $1" | sha256sum -c - >/dev/null || die "checksum mismatch for $1"; }}

step "curl -fsSL $FORGE_URL/agent/download/linux/$ARCH/forge-agent"
fetch "$FORGE_URL/agent/download/linux/$ARCH/forge-agent" "$TMP/forge-agent"
step "sha256sum -c  # verify forge-agent"
verify "$TMP/forge-agent" "$AGENT_SHA"
step "install -m 0755 forge-agent /usr/local/bin/forge-agent"
install -m 0755 "$TMP/forge-agent" /usr/local/bin/forge-agent
if [ -n "$HELPER_SHA" ]; then
  step "curl -fsSL $FORGE_URL/agent/download/linux/$ARCH/forge-agent-helper"
  fetch "$FORGE_URL/agent/download/linux/$ARCH/forge-agent-helper" "$TMP/forge-agent-helper"
  step "sha256sum -c  # forge-agent-helper"
  verify "$TMP/forge-agent-helper" "$HELPER_SHA"
  step "install -m 0755 forge-agent-helper /usr/local/bin/forge-agent-helper"
  install -m 0755 "$TMP/forge-agent-helper" /usr/local/bin/forge-agent-helper
fi
# Cluster DNS (plans/native-mesh.md 2.4): installed when offered; the agent
# configures and starts it once the node is on a cluster network.
if [ -n "$DNS_SHA" ] && command -v systemctl >/dev/null 2>&1; then
  step "curl -fsSL $FORGE_URL/agent/download/linux/$ARCH/forge-dns"
  fetch "$FORGE_URL/agent/download/linux/$ARCH/forge-dns" "$TMP/forge-dns"
  verify "$TMP/forge-dns" "$DNS_SHA"
  install -m 0755 "$TMP/forge-dns" /usr/local/bin/forge-dns
  id forge-dns >/dev/null 2>&1 || useradd --system --no-create-home --home-dir /nonexistent --shell /usr/sbin/nologin forge-dns
  install -d -m 0755 /var/lib/forge-dns
  cat > /etc/systemd/system/forge-dns.service <<'UNIT'
[Unit]
Description=Orbyn Forge cluster DNS
After=network-online.target
Wants=network-online.target

[Service]
User=forge-dns
Group=forge-dns
DynamicUser=no
AmbientCapabilities=CAP_NET_BIND_SERVICE
CapabilityBoundingSet=CAP_NET_BIND_SERVICE
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
ReadOnlyPaths=/var/lib/forge-dns
EnvironmentFile=-/etc/forge/forge-dns.env
ExecStart=/usr/local/bin/forge-dns --file ${{FORGE_DNS_FILE}} $FORGE_DNS_ARGS
Restart=always
RestartSec=1

[Install]
WantedBy=multi-user.target
UNIT
  systemctl daemon-reload
fi
# SELinux (RHEL-likes): give the binaries their directory's default label.
command -v restorecon >/dev/null 2>&1 && restorecon -F /usr/local/bin/forge-agent* /usr/local/bin/forge-dns 2>/dev/null || true

if ! id forge-agent >/dev/null 2>&1; then
  step "useradd --system forge-agent"
  useradd --system --home-dir /var/lib/forge-agent --shell /usr/sbin/nologin forge-agent
fi
install -d -o forge-agent -g forge-agent -m 0700 /var/lib/forge-agent

# Terminal account (D-078): no password, login only with Forge's node-bound
# SSH certificates, sudo only when asked. Never fails the install.
ACCOUNT_FLAG=
if [ -n "$TERMINAL_ACCOUNT" ]; then
  ACCOUNT_SUDO=
  [ "$TERMINAL_ACCOUNT_SUDO" = 1 ] && ACCOUNT_SUDO=--sudo
  step "forge-agent-helper terminal-account $TERMINAL_ACCOUNT $ACCOUNT_SUDO"
  if [ -x /usr/local/bin/forge-agent-helper ] \
    && /usr/local/bin/forge-agent-helper terminal-account "$TERMINAL_ACCOUNT" $ACCOUNT_SUDO; then
    ACCOUNT_FLAG=1
  else
    say "terminal account $TERMINAL_ACCOUNT not created; the install continues"
  fi
fi

as_agent() {{
  if command -v runuser >/dev/null 2>&1; then runuser -u forge-agent -- "$@";
  else su -s /bin/sh forge-agent -c "$*"; fi
}}

# Which Forge server record this machine is (new or already enrolled), so
# onboarding a node that is already enrolled updates it instead of waiting
# for a new enrollment.
report_identity() {{
  SID=$(sed -n 's/.*"server_id"[[:space:]]*:[[:space:]]*"\([0-9a-fA-F-]*\)".*/\1/p' /var/lib/forge-agent/identity.json 2>/dev/null | head -1)
  [ -z "$SID" ] || say "server $SID"
}}

if [ -f /var/lib/forge-agent/identity.json ]; then
  say "already enrolled; updating the agent in place"
  # A node whose pin was never written (installed before D-068, or a failed
  # pin) is pinned now, as root, instead of on the helper's first use.
  if [ -x /usr/local/bin/forge-agent-helper ] && [ ! -f /var/lib/forge-agent-helper/pin.json ]; then
    /usr/local/bin/forge-agent-helper pin --agent-dir /var/lib/forge-agent
  fi
else
  step "forge-agent enroll --server $AGENT_URL"
  # The token reaches the agent through its environment (runuser and su keep
  # it without --login), never on a command line where `ps` would show it.
  export FORGE_ENROLL_TOKEN
  as_agent /usr/local/bin/forge-agent \
    --state-dir /var/lib/forge-agent enroll --server "$AGENT_URL"{host_flag} \
    ${{ACCOUNT_FLAG:+--terminal-account "$TERMINAL_ACCOUNT"}}
  unset FORGE_ENROLL_TOKEN
  # The root helper trusts only commands signed with the key of this fresh
  # enrollment (D-068): pin it as root before the agent ever runs.
  step "forge-agent-helper pin"
  if [ -x /usr/local/bin/forge-agent-helper ]; then
    /usr/local/bin/forge-agent-helper pin --agent-dir /var/lib/forge-agent
  fi
fi

# Browser terminal: trust Forge's SSH user CA. Certificates it signs are valid
# for minutes and name exactly one login; root login policy is unchanged.
if [ -n "$SSH_USER_CA" ] && [ -f /etc/ssh/sshd_config ]; then
  step "trust the Forge SSH user CA (browser terminal)"
  printf '%s\n' "$SSH_USER_CA" > /etc/ssh/forge_user_ca.pub
  chmod 0644 /etc/ssh/forge_user_ca.pub
  TRUST="TrustedUserCAKeys /etc/ssh/forge_user_ca.pub"
  if [ -d /etc/ssh/sshd_config.d ] && grep -qs '^Include /etc/ssh/sshd_config.d/\*\.conf' /etc/ssh/sshd_config; then
    # Keep node-bound certificates (D-069) when the helper already set them up.
    if [ -f /etc/ssh/forge_node_id ]; then
      printf '# Managed by Orbyn Forge (browser terminal)\n%s\n%s\n%s\n' "$TRUST" \
        "AuthorizedPrincipalsCommand /usr/local/bin/forge-agent-helper ssh-principals %u %i" \
        "AuthorizedPrincipalsCommandUser nobody" > /etc/ssh/sshd_config.d/50-forge.conf
    else
      printf '# Managed by Orbyn Forge (browser terminal)\n%s\n' "$TRUST" > /etc/ssh/sshd_config.d/50-forge.conf
    fi
    # Readable like sshd_config: the agent reports the setup from it.
    chmod 0644 /etc/ssh/sshd_config.d/50-forge.conf
  elif ! grep -qs "^$TRUST" /etc/ssh/sshd_config; then
    printf '\n# Managed by Orbyn Forge (browser terminal)\n%s\n' "$TRUST" >> /etc/ssh/sshd_config
  fi
  SSHD=$(command -v sshd || echo /usr/sbin/sshd)
  if "$SSHD" -t 2>/dev/null; then
    systemctl reload ssh 2>/dev/null || systemctl reload sshd 2>/dev/null \
      || {{ [ -f /run/sshd.pid ] && kill -HUP "$(cat /run/sshd.pid)"; }} || true
  else
    say "sshd config test failed; terminal access not enabled"
  fi
fi

# Upgrades: allow the command kinds this release adds. Kinds offered before
# and removed by the operator from `allowed_commands` stay removed (D-070).
if [ -f /var/lib/forge-agent/agent.toml ]; then
  OFFERED=$(as_agent /usr/local/bin/forge-agent --state-dir /var/lib/forge-agent offer-commands)
  case "$OFFERED" in
    "allowed: "*) say "command kinds this release adds, now allowed: ${{OFFERED#allowed: }}" ;;
  esac
fi

# The agent must stay up: a unit that exits at once (bad state, a pin it
# cannot read) would otherwise be reported as installed. Polls ~15 s.
agent_failed() {{
  say "$1"
  if [ -s /var/lib/forge-agent/link-error.txt ]; then
    say "last link error: $(cat /var/lib/forge-agent/link-error.txt)"
  fi
  if [ -n "$2" ]; then
    printf '%s\n' "--- last log lines ---" >&2
    sh -c "$2" >&2 2>/dev/null || true
  fi
  die "forge-agent is not running; fix the cause above and run the installer again"
}}

HELPER_CMD="/usr/local/bin/forge-agent-helper serve --agent-dir /var/lib/forge-agent"
if [ -d /run/systemd/system ]; then
  if [ -x /usr/local/bin/forge-agent-helper ]; then
    cat > /etc/systemd/system/forge-agent-helper.service <<'UNIT'
[Unit]
Description=Orbyn Forge agent helper (root; fixed allowlist and catalog recipes)
Before=forge-agent.service

[Service]
ExecStart=/usr/local/bin/forge-agent-helper serve --agent-dir /var/lib/forge-agent
Restart=always
RestartSec=2

[Install]
WantedBy=multi-user.target
UNIT
    systemctl daemon-reload
    systemctl enable forge-agent-helper.service >/dev/null 2>&1
    step "systemctl restart forge-agent-helper.service"
    systemctl restart forge-agent-helper.service
  fi
  step "write /etc/systemd/system/forge-agent.service"
  cat > /etc/systemd/system/forge-agent.service <<'UNIT'
[Unit]
Description=Orbyn Forge agent
After=network-online.target
Wants=network-online.target

[Service]
User=forge-agent
ExecStart=/usr/local/bin/forge-agent --state-dir /var/lib/forge-agent run
Restart=always
RestartSec=5
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/var/lib/forge-agent

[Install]
WantedBy=multi-user.target
UNIT
  systemctl daemon-reload
  systemctl enable forge-agent.service >/dev/null 2>&1
  step "systemctl restart forge-agent.service"
  systemctl restart forge-agent.service
  step "systemctl is-active forge-agent.service  # stays up for 15 s"
  # NRestarts restarts from 0 on an explicit restart; any crash raises it.
  i=0
  while [ "$i" -lt 15 ]; do
    sleep 1
    i=$((i + 1))
    STATE=$(systemctl is-active forge-agent.service 2>/dev/null || true)
    RESTARTS=$(systemctl show -p NRestarts --value forge-agent.service 2>/dev/null || echo 0)
    if [ "$STATE" != active ] || [ "${{RESTARTS:-0}}" != 0 ]; then
      agent_failed "forge-agent.service did not stay running (state $STATE, restarts ${{RESTARTS:-0}})" \
        "command -v journalctl >/dev/null && journalctl -u forge-agent.service -n 15 --no-pager"
    fi
  done
  report_identity
  say "forge-agent installed and running (systemd)"
else
  # No systemd (e.g. a container): supervise with respawn loops. Running
  # processes are stopped so the loops restart them on the new binaries.
  if [ -x /usr/local/bin/forge-agent-helper ]; then
    pkill -x -f "$HELPER_CMD" >/dev/null 2>&1 || true
    if ! pgrep -f "while true; do $HELPER_CMD" >/dev/null 2>&1; then
      nohup setsid sh -c "while true; do $HELPER_CMD; sleep 2; done" >>/var/log/forge-agent-helper.log 2>&1 </dev/null &
    fi
  fi
  pkill -x -f "/usr/local/bin/forge-agent --state-dir /var/lib/forge-agent run" >/dev/null 2>&1 || true
  if ! pgrep -f "while true; do runuser -u forge-agent" >/dev/null 2>&1; then
    nohup setsid sh -c 'while true; do runuser -u forge-agent -- /usr/local/bin/forge-agent --state-dir /var/lib/forge-agent run; sleep 5; done' \
      >>/var/log/forge-agent.log 2>&1 </dev/null &
  fi
  step "pgrep forge-agent  # stays up for 15 s"
  i=0
  while [ "$i" -lt 15 ]; do
    sleep 1
    i=$((i + 1))
    if ! pgrep -x -f "/usr/local/bin/forge-agent --state-dir /var/lib/forge-agent run" >/dev/null 2>&1; then
      agent_failed "forge-agent did not stay running" "tail -n 15 /var/log/forge-agent.log"
    fi
  done
  report_identity
  say "forge-agent installed and running (no systemd; respawn loop)"
fi
"#,
        forge_url = sh_quote(p.forge_url.trim_end_matches('/')),
        agent_url = sh_quote(p.agent_url),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dist() -> AgentDist {
        let mut d = AgentDist::default();
        d.sums.insert("forge-agent-linux-x86_64".into(), "ab".repeat(32));

        d
    }

    #[test]
    fn manual_script_requires_env_token() {
        let s = render_script(
            &dist(),
            &InstallParams {
                forge_url: "https://f/",
                agent_url: "https://f:7443",
                token: None,
                hostname: None,
                ssh_user_ca: Some("ssh-ed25519 AAAAC3 ca"),
                terminal_account: None,
                terminal_account_sudo: false,
            },
        );
        assert!(s.contains(r#"${FORGE_ENROLL_TOKEN:?"#));
        assert!(s.contains("FORGE_URL='https://f'"));
        assert!(s.contains(&"ab".repeat(32)));
        assert!(s.contains("sha256sum -c"));
        assert!(s.contains("SSH_USER_CA='ssh-ed25519 AAAAC3 ca'"));
        assert!(s.contains("TrustedUserCAKeys /etc/ssh/forge_user_ca.pub"));

        // D-070: updates offer new command kinds through the agent, which
        // keeps the operator's removals; no blind re-adding.
        assert!(s.contains("forge-agent --state-dir /var/lib/forge-agent offer-commands"));
        assert!(!s.contains("for CMD in"), "the installer no longer re-adds removed commands");

        // D-068: a fresh enrollment is pinned as root for the helper.
        let enroll = s.find("enroll --server");

        // The last pin is the fresh-install one; the first is the re-run branch.
        let pin = s.rfind("forge-agent-helper pin --agent-dir /var/lib/forge-agent");
        assert!(matches!((enroll, pin), (Some(e), Some(p)) if p > e), "pinned right after enrolling");

        // A re-run pins a node that has no pin yet (old node, or a failed pin).
        assert!(s.contains("[ ! -f /var/lib/forge-agent-helper/pin.json ]"));

        // D-069: re-running keeps node-bound certificates, and the drop-in is
        // readable by the agent that reports it.
        assert!(s.contains("ssh-principals %u %i") && s.contains("chmod 0644 /etc/ssh/sshd_config.d/50-forge.conf"));
    }

    #[test]
    fn the_installer_confirms_the_agent_stays_running() {
        let s = render_script(
            &dist(),
            &InstallParams {
                forge_url: "https://f",
                agent_url: "https://f:7443",
                token: None,
                hostname: None,
                ssh_user_ca: Some("ssh-ed25519 AAAAC3 ca"),
                terminal_account: None,
                terminal_account_sudo: false,
            },
        );

        // systemd: a crash-looping unit fails the install with its journal.
        let restart = s.find("systemctl restart forge-agent.service\n").unwrap_or(usize::MAX);
        let check = s.find("systemctl show -p NRestarts --value forge-agent.service");
        let ok = s.find("installed and running (systemd)");
        assert!(matches!((check, ok), (Some(c), Some(o)) if restart < c && c < o), "checked before success");
        assert!(s.contains("journalctl -u forge-agent.service -n 15 --no-pager"));
        assert!(s.contains("die \"forge-agent is not running"));

        // No systemd: the respawned process must stay up too.
        assert!(s.contains("tail -n 15 /var/log/forge-agent.log"));

        // offer-commands output is not echoed raw ("no new commands").
        assert!(s.contains("OFFERED=$(as_agent"));

        // The rendered script parses as POSIX sh where a shell is present.
        if let Ok(out) = std::process::Command::new("sh").arg("-n").arg("-c").arg(&s).output() {
            assert!(out.status.success(), "sh -n: {}", String::from_utf8_lossy(&out.stderr));
        }
    }

    #[test]
    fn embedded_values_are_shell_quoted() {
        let s = render_script(
            &dist(),
            &InstallParams {
                forge_url: "https://f",
                agent_url: "https://f:7443",
                token: Some("t'; rm -rf /"),
                hostname: Some("a b"),
                ssh_user_ca: None,
                terminal_account: Some("forge-ops"),
                terminal_account_sudo: true,
            },
        );
        assert!(s.contains(r"FORGE_ENROLL_TOKEN='t'\''; rm -rf /'"));
        assert!(s.contains("--hostname 'a b'"));

        // D-078: the account is created as root before enrolling, and the
        // agent reports it only when it was created.
        assert!(s.contains("TERMINAL_ACCOUNT='forge-ops'\nTERMINAL_ACCOUNT_SUDO=1\n"));
        let create = s.find("/usr/local/bin/forge-agent-helper terminal-account \"$TERMINAL_ACCOUNT\" $ACCOUNT_SUDO");
        let enroll = s.find("${ACCOUNT_FLAG:+--terminal-account \"$TERMINAL_ACCOUNT\"}");
        assert!(matches!((create, enroll), (Some(c), Some(e)) if c < e), "created before enrolling");
    }

    #[test]
    fn enrollment_token_never_appears_on_a_command_line() {
        for token in [Some("secret-token"), None] {
            let s = render_script(
                &dist(),
                &InstallParams {
                    forge_url: "https://f",
                    agent_url: "https://f:7443",
                    token,
                    hostname: None,
                    ssh_user_ca: None,
                    terminal_account: None,
                    terminal_account_sudo: false,
                },
            );
            assert!(s.contains("export FORGE_ENROLL_TOKEN\n"));

            // The only uses: the assignment / required check, the 0600 header
            // file, export and unset. No `VAR=value cmd` or `env VAR=…` argv.
            for line in s.lines().filter(|l| l.contains("FORGE_ENROLL_TOKEN")) {
                let l = line.trim();
                let ok = l.starts_with("FORGE_ENROLL_TOKEN=")
                    || l.starts_with(r#": "${FORGE_ENROLL_TOKEN:?"#)
                    || l.starts_with("printf 'Authorization: Bearer %s\\n' \"$FORGE_ENROLL_TOKEN\" > ")
                    || l == "export FORGE_ENROLL_TOKEN"
                    || l == "unset FORGE_ENROLL_TOKEN"
                    || l.starts_with('#');
                assert!(ok, "token on a command line: {l}");
            }

            assert!(!s.contains("env FORGE_ENROLL_TOKEN"));
        }
    }

    #[test]
    fn binary_lookup_rejects_unknown_inputs() {
        let d = dist();
        assert!(d.binary("linux", "x86_64", "forge-agent").is_some());
        assert!(d.binary("linux", "../../etc", "forge-agent").is_none());
        assert!(d.binary("windows", "x86_64", "forge-agent").is_none());
        assert!(d.binary("linux", "x86_64", "passwd").is_none());
    }
}

/// The Windows installer template (`GET /install.ps1`).
const INSTALL_PS1: &str = include_str!("install.ps1");

/// A PowerShell single-quoted string body. PowerShell ends such a string at
/// `'` and also at the typographic quotes U+2018, U+2019, U+201A and U+201B;
/// each is doubled, which PowerShell reads as one literal quote.
fn ps_quote_body(s: &str) -> String {
    let mut out = String::with_capacity(s.len());

    for c in s.chars() {
        if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}') {
            out.push(c);
        }

        out.push(c);
    }

    out
}

/// Renders the Windows (PowerShell) installer (D-079): the URLs and the
/// agent's SHA-256 are baked in; the token never is (the script reads
/// `$env:FORGE_ENROLL_TOKEN`).
#[must_use]
pub fn render_ps1(dist: &AgentDist, forge_url: &str, agent_url: &str) -> String {
    INSTALL_PS1
        .replace("{{FORGE_URL}}", &ps_quote_body(forge_url.trim_end_matches('/')))
        .replace("{{AGENT_URL}}", &ps_quote_body(agent_url))
        .replace("{{AGENT_SHA}}", dist.windows_sum("x86_64"))
}

#[cfg(test)]
mod manifest_tests {
    use super::*;

    fn dist_with(files: &[(&str, &[u8])]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));

        for (name, body) in files {
            std::fs::write(dir.path().join(name), body).unwrap_or_else(|e| panic!("{e}"));
        }

        dir
    }

    #[test]
    fn the_manifest_names_the_version_and_vets_every_file() {
        let dir = dist_with(&[
            ("forge-agent-linux-x86_64", b"agent"),
            ("forge-agent-helper-linux-x86_64", b"helper"),
            ("forge-agent-linux-aarch64", b"replaced by hand"),
        ]);
        let m = DistManifest {
            version: "1.2.3".into(),
            files: [
                ("forge-agent-linux-x86_64".to_owned(), sha256_hex(b"agent")),
                ("forge-agent-helper-linux-x86_64".to_owned(), sha256_hex(b"helper").to_uppercase()),
                ("forge-agent-linux-aarch64".to_owned(), sha256_hex(b"the released one")),
            ]
            .into(),
        };
        m.write(dir.path()).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(DistManifest::read(dir.path()).ok().flatten().as_ref(), Some(&m));
        let dist = AgentDist::scan(dir.path());
        assert_eq!(dist.version(), "1.2.3");
        assert!(dist.has_manifest());
        assert!(dist.binary("linux", "x86_64", "forge-agent").is_some());
        assert!(dist.binary("linux", "x86_64", "forge-agent-helper").is_some(), "hex case does not matter");
        assert!(dist.binary("linux", "aarch64", "forge-agent").is_none(), "a mismatching file is not handed out");
    }

    #[test]
    fn a_file_the_manifest_does_not_list_is_not_handed_out() {
        let dir = dist_with(&[("forge-agent-linux-x86_64", b"agent"), ("forge-agent-helper-linux-x86_64", b"x")]);
        DistManifest {
            version: "1.2.3".into(),
            files: [("forge-agent-linux-x86_64".to_owned(), sha256_hex(b"agent"))].into(),
        }
        .write(dir.path())
        .unwrap_or_else(|e| panic!("{e}"));
        let dist = AgentDist::scan(dir.path());
        assert!(dist.binary("linux", "x86_64", "forge-agent").is_some());
        assert!(dist.binary("linux", "x86_64", "forge-agent-helper").is_none());
    }

    #[test]
    fn without_a_manifest_the_agents_are_this_servers_version() {
        let dir = dist_with(&[("forge-agent-linux-x86_64", b"agent")]);
        let dist = AgentDist::scan(dir.path());
        assert_eq!(dist.version(), crate::VERSION);
        assert!(!dist.has_manifest());
        assert!(dist.binary("linux", "x86_64", "forge-agent").is_some());
    }

    #[test]
    fn a_broken_manifest_hands_out_nothing() {
        for bad in [&b"{not json"[..], br#"{"version": "1.0; rm -rf /", "files": {}}"#] {
            let dir = dist_with(&[("forge-agent-linux-x86_64", b"agent"), (MANIFEST, bad)]);
            assert!(DistManifest::read(dir.path()).is_err());
            let dist = AgentDist::scan(dir.path());
            assert!(!dist.available(), "nothing is offered from an unreadable manifest");
        }
    }
}

#[cfg(test)]
mod windows_tests {
    use super::*;

    #[test]
    fn the_windows_agent_is_served_and_baked_into_install_ps1() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(dir.path().join("forge-agent-windows-x86_64.exe"), b"MZ fake agent").unwrap_or_default();
        let dist = AgentDist::scan(dir.path());
        assert!(dist.windows_available());
        let (path, sum) = dist.binary("windows", "x86_64", "forge-agent").unwrap_or_else(|| panic!("no windows build"));
        assert!(path.ends_with("forge-agent-windows-x86_64.exe"));
        assert_eq!(sum, sha256_hex(b"MZ fake agent"));
        assert!(dist.binary("windows", "x86_64", "forge-agent-helper").is_none(), "no helper on Windows");
        assert!(dist.binary("windows", "aarch64", "forge-agent").is_none());
        assert!(dist.binary("windows", "../x86_64", "forge-agent").is_none());

        let ps1 = render_ps1(&dist, "https://forge.example.com/", "https://forge.example.com:7443");
        assert!(ps1.contains("$ForgeUrl = 'https://forge.example.com'"), "trailing slash dropped");
        assert!(ps1.contains("$AgentUrl = 'https://forge.example.com:7443'"));
        assert!(ps1.contains(&format!("$AgentSha = '{sum}'")));
        assert!(!ps1.contains("{{"), "every placeholder filled");

        // Only Forge's own branded folders: never a generic name such as
        // ProgramData\Forge that another product could own (D-079).
        assert!(!ps1.contains("'Forge'") && !ps1.contains("ProgramData\\Forge"), "generic folder in install.ps1");

        // A quote in a URL cannot break out of the PowerShell string.
        let evil = render_ps1(&dist, "https://x/'; Remove-Item C:\\ ;'", "https://x");
        assert!(evil.contains("$ForgeUrl = 'https://x/''; Remove-Item C:\\ ;'''"), "{}", &evil[..200]);

        // PowerShell also closes a single-quoted string at typographic quotes.
        let smart = render_ps1(&dist, "https://x/\u{2019}; calc; \u{2018}", "https://x\u{201B}");
        assert!(smart.contains("$ForgeUrl = 'https://x/\u{2019}\u{2019}; calc; \u{2018}\u{2018}'"));
        assert!(smart.contains("$AgentUrl = 'https://x\u{201B}\u{201B}'"));

        // The URL is substituted into the quoted assignment only, never into
        // a comment line, where a newline would start a command.
        let multi = render_ps1(&dist, "https://x\nRemove-Item C:\\", "https://x");
        assert_eq!(multi.matches("Remove-Item C:\\").count(), 1);
        assert!(multi.contains("$ForgeUrl = 'https://x\nRemove-Item C:\\'"));

        // The agent is staged in the install folder (Administrators only),
        // never in %TEMP%, where this user's programs could swap it after the
        // checksum (it then runs as LocalSystem).
        assert!(!ps1.contains("GetTempPath") && !ps1.contains("$env:TEMP"), "agent staged in a user-writable folder");
        let hash =
            ps1.find("Get-FileHash -LiteralPath $staged").unwrap_or_else(|| panic!("no checksum of the staged file"));
        let install = ps1.find("Move-Item -Force -LiteralPath $staged").unwrap_or_else(|| panic!("no move into place"));
        assert!(hash < install, "checksum before install");

        // The data folder gets a fresh ACL (icacls /grant:r keeps other
        // accounts' entries) after a check for links and foreign owners.
        assert!(ps1.contains("Set-Acl -LiteralPath $Path -AclObject $acl"));
        assert!(!ps1.contains("/inheritance:r /grant:r"), "icacls /grant:r keeps other accounts' entries");
    }

    /// Runs `script` after the installer's function definitions (no install
    /// steps) in Windows PowerShell; `None` where it is not installed.
    #[cfg(windows)]
    fn with_install_functions(script: &str) -> Option<std::process::Output> {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let ps1 = render_ps1(&AgentDist::scan(dir.path()), "https://f", "https://f:7443");

        // Everything before the main `try {` block: settings and functions.
        let head = &ps1[..ps1.find("\ntry {\n").unwrap_or_else(|| panic!("no main block"))];
        let file = dir.path().join("t.ps1");
        std::fs::write(&file, format!("{head}\n{script}\n")).unwrap_or_else(|e| panic!("{e}"));

        // Windows PowerShell 5.1 by absolute path, without an inherited
        // PSModulePath: under a PowerShell 7 parent (GitHub's `pwsh` steps)
        // 5.1 would load pwsh's module versions and fail on Get-Acl.
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
        let exe = std::path::Path::new(&root).join(r"System32\WindowsPowerShell\v1.0\powershell.exe");

        std::process::Command::new(exe)
            .env_remove("PSModulePath")
            .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File"])
            .arg(&file)
            .output()
            .ok()
    }

    #[cfg(windows)]
    #[test]
    fn install_ps1_refuses_a_planted_link_in_the_data_folder() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let root = dir.path().join("OrbynForge");
        let elsewhere = dir.path().join("elsewhere");
        std::fs::create_dir_all(root.join("logs")).unwrap_or_else(|e| panic!("{e}"));
        std::fs::create_dir_all(&elsewhere).unwrap_or_else(|e| panic!("{e}"));
        let script = format!(
            "$r = '{root}'\n\
             if (Find-Untrusted $r) {{ 'clean tree flagged'; exit 2 }}\n\
             if (Test-Locked $r) {{ 'inherited ACL counted as locked'; exit 3 }}\n\
             & cmd.exe /c mklink /J \"$r\\agent\" \"{elsewhere}\" | Out-Null\n\
             if (-not (Test-Path \"$r\\agent\")) {{ 'no junction'; exit 0 }}\n\
             $why = Find-Untrusted $r\n\
             if ($why -notlike '*agent is a link*') {{ \"junction not found: $why\"; exit 4 }}\n\
             'ok'",
            root = root.display(),
            elsewhere = elsewhere.display()
        );
        let Some(out) = with_install_functions(&script) else { return };
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success(), "{text}{}", String::from_utf8_lossy(&out.stderr));
    }

    #[test]
    fn install_ps1_parses_as_powershell() {
        // Checked with the PowerShell parser where one is installed (Windows
        // runners, or pwsh on Linux); skipped elsewhere.
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let file = dir.path().join("install.ps1");
        std::fs::write(&file, render_ps1(&AgentDist::scan(dir.path()), "https://f", "https://f:7443"))
            .unwrap_or_default();
        let check = format!(
            "$e=$null; [void][System.Management.Automation.Language.Parser]::ParseFile('{}', [ref]$null, [ref]$e); if ($e) {{ $e | ForEach-Object {{ $_.ToString() }}; exit 1 }}",
            file.display()
        );

        for shell in ["pwsh", "powershell"] {
            if let Ok(out) =
                std::process::Command::new(shell).args(["-NoProfile", "-NonInteractive", "-Command", &check]).output()
            {
                assert!(out.status.success(), "{shell}: {}", String::from_utf8_lossy(&out.stdout));
                return;
            }
        }
    }
}
