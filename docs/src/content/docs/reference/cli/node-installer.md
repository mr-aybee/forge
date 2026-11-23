---
title: Node installer
description: The install.sh (Linux) and install.ps1 (Windows) scripts the control plane serves to nodes, with their parameters, environment variables, steps, files, services, output and errors.
sidebar:
  order: 94
---

This page lists exactly what the node installers do. The control plane
generates the Linux script at `GET /install.sh`. It installs `forge-agent` and
`forge-agent-helper` on a Linux machine, enrolls the machine with a one-time
token, and starts both as services. For the task, see
[Add nodes](/docs/guides/add-nodes/). The Windows installer, `GET /install.ps1`,
is described in [Windows (install.ps1)](#windows-installps1).

The same script serves both onboarding paths:

| Path | How the script runs | Token |
| --- | --- | --- |
| Manual | You pipe `GET /install.sh` into `sudo sh` on the node | From `FORGE_ENROLL_TOKEN` |
| Assisted | The `node.bootstrap` job uploads the script over SSH to `/tmp/forge-install.XXXXXX/install.sh` and runs it with `sudo` in the background, writing its output to `out.log` and its exit code to `rc` in the same directory | Embedded in the script by the job |

**Requires:** Linux on x86_64 (`amd64`) or aarch64 (`arm64`), root (through
`sudo`), `sha256sum`, `useradd`, `runuser` or `su`, and `curl` or `wget`.
`?terminal_account_sudo=1` also needs `sudo` with `/etc/sudoers.d` and
`visudo`. systemd is optional. To create the token you need the `server.enroll`
action on the workspace (see [Cedar schema](/docs/reference/cedar-schema/)).

:::note
`GET /install.sh` is a POSIX shell script for Linux. Windows machines use
`GET /install.ps1`; see [Windows (install.ps1)](#windows-installps1).
:::

## Synopsis

```
printf 'Enrollment token: '; stty -echo; read -r FORGE_ENROLL_TOKEN; stty echo; echo; export FORGE_ENROLL_TOKEN
curl -fsSL '<forge-url>/install.sh' | sudo --preserve-env=FORGE_ENROLL_TOKEN sh
curl -fsSL '<forge-url>/install.sh?hostname=<node-name>' | sudo --preserve-env=FORGE_ENROLL_TOKEN sh
curl -fsSL '<forge-url>/install.sh?terminal_account=<account-name>&terminal_account_sudo=1' | sudo --preserve-env=FORGE_ENROLL_TOKEN sh
```

The script reads the token from `FORGE_ENROLL_TOKEN` only. The first line
reads it at a prompt without echo, so it stays out of shell history and of
the process list.

`<forge-url>` is `node_public_url`, or `public_url` when that is not set.
**Nodes → Add node → Manual** shows the command with the URL filled in, and
with `?terminal_account=` while **Create a terminal account** is on.

## The endpoint

`GET /install.sh` is public: it needs no session and holds no secrets. It
answers with `Content-Type: text/x-shellscript; charset=utf-8`.

| Query parameter | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `hostname` | string | none: the agent reports the machine's own hostname | No | Name the node enrolls under. Passed to `forge-agent enroll --hostname`. Accepted when it has at most 63 characters from `A–Z`, `a–z`, `0–9`, `-` and `.`; any other value is ignored without an error. An empty value counts as not given | `?hostname=<node-name>` |
| `terminal_account` | string | none: no account is created | No | Terminal account the script creates for the browser terminal (step 10a) and reports when it enrolls. 1–32 characters of `a-z`, `0-9`, `_` and `-`, starting with a letter or `_`; not a reserved system name (`root`, `nobody`, `forge-agent`, `daemon`, `bin`, `sys`, `sync`, `games`, `man`, `lp`, `mail`, `news`, `uucp`, `proxy`, `www-data`, `backup`, `list`, `irc`, `gnats`, `sshd`, `messagebus`, `admin`, `adm`, `sudo`, `wheel`, `docker`, `polkitd`, `halt`, `shutdown`, `operator`) and not starting with `systemd-`. Any other value answers `400` with no script. An empty value counts as not given | `?terminal_account=forge-ops` |
| `terminal_account_sudo` | `1` or `true` | off | No | Gives the terminal account passwordless `sudo`. Any other value counts as off. Ignored without `terminal_account` | `&terminal_account_sudo=1` |

:::note
The **Hostname** field under **Nodes → Add node → Manual** labels the token
while the node installs. It does not add `?hostname=` to the command.
:::

The control plane bakes these values into the script:

| Variable in the script | Value |
| --- | --- |
| `FORGE_URL` | `node_public_url` (or `public_url`), without a trailing `/`. Used for downloads and progress reports |
| `AGENT_URL` | `agent_public_url`. Passed to `forge-agent enroll --server` |
| `SSH_USER_CA` | The public key of Forge's SSH user CA, for the browser terminal |
| `TERMINAL_ACCOUNT` | `terminal_account`, or empty |
| `TERMINAL_ACCOUNT_SUDO` | `1` when `terminal_account_sudo` is on and an account is given, otherwise `0` |
| `AGENT_SHA`, `HELPER_SHA` | SHA-256 of `forge-agent-linux-<arch>` and `forge-agent-helper-linux-<arch>` in `agent_dist_dir`, per architecture. Empty when the file is missing |
| `FORGE_ENROLL_TOKEN` | Assisted path only: the job's single-use token. The manual script requires the variable from the environment instead |

See [Configuration](/docs/reference/configuration/) for `node_public_url`,
`agent_public_url` and `agent_dist_dir`.

## Environment variables

| Name | Default | Required | Description |
| --- | --- | --- | --- |
| `FORGE_ENROLL_TOKEN` | none | Yes (manual path) | One-time enrollment token from **Nodes → Add node → Manual** or `POST /api/v1/enrollment-tokens`. Used to enroll and to authenticate progress reports. Required on every run, also when the machine is already enrolled |

The script passes the token to `forge-agent enroll` through the environment,
and to `curl` through a `0600` header file. It never puts the token on a
command line.

## Examples

Run the command the console shows, then paste the token at the
`Enrollment token:` prompt:

```sh
printf 'Enrollment token: '; stty -echo; read -r FORGE_ENROLL_TOKEN; stty echo; echo; export FORGE_ENROLL_TOKEN; curl -fsSL 'http://<forge-host>:8080/install.sh' | sudo --preserve-env=FORGE_ENROLL_TOKEN sh; unset FORGE_ENROLL_TOKEN
```

Enroll under a chosen name (same prompt):

```sh
printf 'Enrollment token: '; stty -echo; read -r FORGE_ENROLL_TOKEN; stty echo; echo; export FORGE_ENROLL_TOKEN; curl -fsSL 'http://<forge-host>:8080/install.sh?hostname=<node-name>' | sudo --preserve-env=FORGE_ENROLL_TOKEN sh; unset FORGE_ENROLL_TOKEN
```

Create the terminal account `forge-ops` with passwordless `sudo` (same
prompt):

```sh
printf 'Enrollment token: '; stty -echo; read -r FORGE_ENROLL_TOKEN; stty echo; echo; export FORGE_ENROLL_TOKEN; curl -fsSL 'http://<forge-host>:8080/install.sh?terminal_account=forge-ops&terminal_account_sudo=1' | sudo --preserve-env=FORGE_ENROLL_TOKEN sh; unset FORGE_ENROLL_TOKEN
```

`sudo --preserve-env=<name>` needs `sudo` 1.8.21 or later (Debian 10,
Ubuntu 18.04, RHEL 8 and newer). As `root`, pipe into `sh` without `sudo`.

Read the script before you run it:

```sh
curl -fsSL http://<forge-host>:8080/install.sh -o forge-install.sh
less forge-install.sh
sudo -s
read -rs FORGE_ENROLL_TOKEN && export FORGE_ENROLL_TOKEN   # paste the token, then press Enter
sh forge-install.sh
unset FORGE_ENROLL_TOKEN
exit
```

On a machine without `curl`, fetch the script with `wget`. The install works,
but the console shows no progress, because progress reports need `curl`:

```sh
printf 'Enrollment token: '; stty -echo; read -r FORGE_ENROLL_TOKEN; stty echo; echo; export FORGE_ENROLL_TOKEN; wget -qO- 'http://<forge-host>:8080/install.sh' | sudo --preserve-env=FORGE_ENROLL_TOKEN sh; unset FORGE_ENROLL_TOKEN
```

## Steps

The script runs with `set -eu` and `umask 077`. It prints a `$ <command>`
line for each step and a `forge-install: <message>` line for each status.

| # | Printed line | What happens | When |
| --- | --- | --- | --- |
| 1 | `forge-install: installing forge-agent on <uname -n>` | Starts | Always |
| 2 | `$ id -u  # must be root` | Checks for root | Always |
| 3 | `$ uname -s -m` | Checks for Linux and maps `x86_64`/`amd64` to `x86_64`, `aarch64`/`arm64` to `aarch64` | Always |
| 4 | `forge-install: architecture <arch>` | Confirms an agent build exists for the architecture | Always |
| 5 | `$ curl -fsSL <forge-url>/agent/download/linux/<arch>/forge-agent` | Downloads the agent to a temporary directory (with `curl`, or `wget`) | Always |
| 6 | `$ sha256sum -c  # verify forge-agent` | Checks it against `AGENT_SHA` | Always |
| 7 | `$ install -m 0755 forge-agent /usr/local/bin/forge-agent` | Installs it | Always |
| 8 | `$ curl -fsSL <forge-url>/agent/download/linux/<arch>/forge-agent-helper`, `$ sha256sum -c  # forge-agent-helper`, `$ install -m 0755 forge-agent-helper /usr/local/bin/forge-agent-helper` | Same for the helper, checked against `HELPER_SHA` | The control plane has a helper build |
| 9 | `$ useradd --system forge-agent` | Creates the system user `forge-agent` (home `/var/lib/forge-agent`, shell `/usr/sbin/nologin`) | The user does not exist |
| 10 | (none) | Creates `/var/lib/forge-agent` (`0700`, owner `forge-agent`) | Always |
| 10a | `$ forge-agent-helper terminal-account <name>` (with ` --sudo` when asked) | Runs [`forge-agent-helper terminal-account`](/docs/reference/cli/forge-agent-helper/#forge-agent-helper-terminal-account) as root: creates or keeps the account and sets its sudo rule. On failure, or when the helper is not installed, prints `forge-install: terminal account <name> not created; the install continues` and goes on | `terminal_account` was given, also on a re-run |
| 11 | `$ forge-agent enroll --server <agent-url>` | Runs `forge-agent --state-dir /var/lib/forge-agent enroll` as `forge-agent`, with `--hostname` when the URL had `?hostname=`, and with `--terminal-account <name>` when step 10a succeeded, so Forge saves the account to the node's terminal logins. The agent pins the control plane's CA through the token and writes its identity, key, certificate and `agent.toml` | `/var/lib/forge-agent/identity.json` does not exist |
| 12 | `$ forge-agent-helper pin` | Runs `forge-agent-helper pin --agent-dir /var/lib/forge-agent` as root. The helper then accepts only commands signed with the key of this enrollment | After step 11, when the helper is installed |
| 11a | `forge-install: already enrolled; updating the agent in place` | Skips enrolling and pinning | `identity.json` exists |
| 13 | `$ trust the Forge SSH user CA (browser terminal)` | Sets up sshd for the browser terminal (see [SSH configuration](#ssh-configuration)) | `/etc/ssh/sshd_config` exists |
| 14 | (the agent's output) | Runs `forge-agent --state-dir /var/lib/forge-agent offer-commands` as `forge-agent`: adds command names that are new in this release to `allowed_commands`. Names you removed stay removed | `/var/lib/forge-agent/agent.toml` exists |
| 15 | `$ systemctl restart forge-agent-helper.service` | Writes and enables `forge-agent-helper.service`, then restarts it | systemd, helper installed |
| 16 | `$ write /etc/systemd/system/forge-agent.service`, `$ systemctl restart forge-agent.service` | Writes and enables `forge-agent.service`, then restarts it | systemd |
| 17 | `forge-install: server <server-id>` | Reports which Forge server record this machine is | `identity.json` has a `server_id` |
| 18 | `forge-install: forge-agent installed and running (systemd)` or `forge-install: forge-agent installed and running (no systemd; respawn loop)` | Done | Always |

### Without systemd

When `/run/systemd/system` does not exist, for example in a container, the
script supervises the processes with respawn loops instead of steps 15 and 16:

| Process | Command | Restart delay | Log |
| --- | --- | --- | --- |
| Helper (root) | `forge-agent-helper serve --agent-dir /var/lib/forge-agent` | 2 s | `/var/log/forge-agent-helper.log` |
| Agent (`forge-agent`) | `runuser -u forge-agent -- forge-agent --state-dir /var/lib/forge-agent run` | 5 s | `/var/log/forge-agent.log` |

It stops the running processes so the loops restart them on the new
binaries, and starts a loop only when none is running. The loops do not
survive a reboot: start them again from your container's entry point, or
run the installer again.

## SSH configuration

The browser terminal signs in to sshd with short-lived certificates from
Forge's SSH user CA. Step 13 does this:

1. Writes the CA public key to `/etc/ssh/forge_user_ca.pub` (`0644`).
2. When `/etc/ssh/sshd_config.d/` exists and `sshd_config` has a line
   starting with `Include /etc/ssh/sshd_config.d/*.conf`, writes
   `/etc/ssh/sshd_config.d/50-forge.conf` (`0644`). Otherwise it appends the
   block to `/etc/ssh/sshd_config`, unless the `TrustedUserCAKeys` line is
   already there.
3. Runs `sshd -t`. On success it reloads `ssh.service` or `sshd.service`, or
   sends `HUP` to the PID in `/run/sshd.pid`. On failure it prints
   `forge-install: sshd config test failed; terminal access not enabled` and
   does not reload sshd. The written files stay in place.

On a first install, the drop-in trusts the CA only:

```
# Managed by Orbyn Forge (browser terminal)
TrustedUserCAKeys /etc/ssh/forge_user_ca.pub
```

When the helper starts with a pinned enrollment, it writes the node's server
id to `/etc/ssh/forge_node_id` and adds the node-bound lines, so sshd accepts
only certificates issued for this node. A re-run keeps them:

```
# Managed by Orbyn Forge (browser terminal)
TrustedUserCAKeys /etc/ssh/forge_user_ca.pub
AuthorizedPrincipalsCommand /usr/local/bin/forge-agent-helper ssh-principals %u
AuthorizedPrincipalsCommandUser nobody
```

The script does not change `PermitRootLogin` or any other sshd setting. See
[Terminal: set up SSH access for a node](/docs/guides/terminal/#set-up-ssh-access-for-a-node).

## Files and services it creates

| Path or object | Owner | Mode | Contents |
| --- | --- | --- | --- |
| `/usr/local/bin/forge-agent` | root | `0755` | Agent |
| `/usr/local/bin/forge-agent-helper` | root | `0755` | Root helper |
| User `forge-agent` | | | System user, no login shell |
| `/var/lib/forge-agent/` | `forge-agent` | `0700` | Agent state: `identity.json`, keys, certificate, `agent.toml` |
| `/var/lib/forge-agent-helper/` | root | private | The helper's pin of Forge's command key |
| `/etc/ssh/forge_user_ca.pub` | root | `0644` | Forge's SSH user CA public key |
| `/etc/ssh/sshd_config.d/50-forge.conf` | root | `0644` | sshd drop-in (or a block appended to `/etc/ssh/sshd_config`) |
| User `<name>`, `/home/<name>` | | | Terminal account (`?terminal_account=`): own group, shell `/bin/bash` (or `/bin/sh`), no password |
| `/etc/sudoers.d/forge-<name>` | root | `0440` | `<name> ALL=(ALL:ALL) NOPASSWD: ALL` (`?terminal_account_sudo=1` only) |
| `/etc/systemd/system/forge-agent-helper.service` | root | `0600` | Helper unit (systemd, helper installed) |
| `/etc/systemd/system/forge-agent.service` | root | `0600` | Agent unit (systemd) |
| `/var/log/forge-agent.log`, `/var/log/forge-agent-helper.log` | root | `0600` | Respawn-loop logs (no systemd) |

The modes of new files under `/etc/systemd/system` and `/var/log` come from
the script's `umask 077`.

The units it writes:

```ini
# /etc/systemd/system/forge-agent-helper.service
[Unit]
Description=Orbyn Forge agent helper (root; fixed allowlist and catalog recipes)
Before=forge-agent.service

[Service]
ExecStart=/usr/local/bin/forge-agent-helper serve --agent-dir /var/lib/forge-agent
Restart=always
RestartSec=2

[Install]
WantedBy=multi-user.target
```

```ini
# /etc/systemd/system/forge-agent.service
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
```

See [Agents: privileged helper](/docs/concepts/agents/#privileged-helper) for why
the helper runs as root and the agent does not.

## Re-running and upgrading in place

The script is safe to re-run. On a machine that is already enrolled
(`/var/lib/forge-agent/identity.json` exists) it:

- downloads, verifies and replaces both binaries;
- skips `useradd` and `enroll`, and keeps the identity, keys and
  certificate;
- creates or keeps the terminal account when the URL has
  `?terminal_account=`. Forge does not save it, because the node does not
  enroll again; **Open terminal** lists it as **on node** once the agent
  reports its facts;
- runs `forge-agent-helper pin` when the helper has no pin yet
  (`/var/lib/forge-agent-helper/pin.json` is missing): a node installed before
  pinning existed, or one whose pin failed, is pinned now, as root;
- rewrites the sshd drop-in, keeping the node-bound lines;
- runs `offer-commands`, which adds only command names new in this release;
- rewrites the units and restarts both services.

It still needs `FORGE_ENROLL_TOKEN` to be set. A used or expired token
works for the install, because the token is not sent to enroll. Progress
reports are accepted until one hour after the token expires.

:::note
If `forge-agent-helper pin` fails after a fresh enrollment, the script stops.
Fix the cause and run the installer again: it pins on the already-enrolled
path.
:::

## Progress reports

Every printed `forge-install:` and `$` line is also sent to
`POST /api/v1/enroll/progress` with `Authorization: Bearer <token>`, so
**Nodes → Add node → Manual** (and the node's **installing** row) shows a
manual install live in the **Installer console**. When the script exits it
sends `installer finished`, or `installer failed (exit <code>)`. Errors are
sent as `error: <message>`.

Reporting is best effort: it needs `curl`, waits at most 5 seconds per line
and never fails the install. Command output, such as the agent's enroll
line, appears in your terminal only. The control plane masks the token in
stored lines.

## What it prints

A first install on a Debian node with systemd:

```
forge-install: installing forge-agent on <node-name>
$ id -u  # must be root
$ uname -s -m
forge-install: architecture x86_64
$ curl -fsSL http://<forge-host>:8080/agent/download/linux/x86_64/forge-agent
$ sha256sum -c  # verify forge-agent
$ install -m 0755 forge-agent /usr/local/bin/forge-agent
$ curl -fsSL http://<forge-host>:8080/agent/download/linux/x86_64/forge-agent-helper
$ sha256sum -c  # forge-agent-helper
$ install -m 0755 forge-agent-helper /usr/local/bin/forge-agent-helper
$ useradd --system forge-agent
$ forge-agent enroll --server https://<forge-host>:7443
<timestamp>  INFO forge_agent::enroll: enrolled server_id=<server-id>
enrolled as server <server-id>
$ forge-agent-helper pin
pinned Forge's command key for server <server-id>
$ trust the Forge SSH user CA (browser terminal)
no new commands
$ systemctl restart forge-agent-helper.service
$ write /etc/systemd/system/forge-agent.service
$ systemctl restart forge-agent.service
forge-install: server <server-id>
forge-install: forge-agent installed and running (systemd)
```

With `?terminal_account=forge-ops&terminal_account_sudo=1`, the lines
between `useradd` and `pin` read:

```
$ useradd --system forge-agent
$ forge-agent-helper terminal-account forge-ops --sudo
terminal account forge-ops: created, passwordless sudo
$ forge-agent enroll --server https://<forge-host>:7443
<timestamp>  INFO forge_agent::enroll: enrolled server_id=<server-id>
enrolled as server <server-id>
$ forge-agent-helper pin
```

When the account cannot be created, for example because a group of that
name exists, the install goes on:

```
$ forge-agent-helper terminal-account forge-ops
forge-agent-helper terminal-account: `forge-ops` is already a group on this node; choose another name
forge-install: terminal account forge-ops not created; the install continues
$ forge-agent enroll --server https://<forge-host>:7443
```

A re-run on an enrolled node, after an update that adds a command name:

```
forge-install: installing forge-agent on <node-name>
$ id -u  # must be root
$ uname -s -m
forge-install: architecture x86_64
$ curl -fsSL http://<forge-host>:8080/agent/download/linux/x86_64/forge-agent
$ sha256sum -c  # verify forge-agent
$ install -m 0755 forge-agent /usr/local/bin/forge-agent
$ curl -fsSL http://<forge-host>:8080/agent/download/linux/x86_64/forge-agent-helper
$ sha256sum -c  # forge-agent-helper
$ install -m 0755 forge-agent-helper /usr/local/bin/forge-agent-helper
forge-install: already enrolled; updating the agent in place
$ trust the Forge SSH user CA (browser terminal)
allowed: stack
$ systemctl restart forge-agent-helper.service
$ write /etc/systemd/system/forge-agent.service
$ systemctl restart forge-agent.service
forge-install: server <server-id>
forge-install: forge-agent installed and running (systemd)
```

`offer-commands` prints `no new commands`, or `allowed: <name>, <name>` with
the names it added.

## Exit codes and errors

Errors print `forge-install: error: <message>` to standard error.

| Exit code | Message | Cause | Fix |
| --- | --- | --- | --- |
| `0` | `forge-install: forge-agent installed and running (…)` | Installed and started | |
| `0` (nothing ran) | `curl: (22) The requested URL returned error: 400` | `terminal_account` breaks the naming rules, so the control plane sent no script and `sh` read nothing | Fix the name in the URL, see [The endpoint](#the-endpoint) |
| non-zero | `FORGE_ENROLL_TOKEN: set FORGE_ENROLL_TOKEN to the token shown in Orbyn Forge` (from the shell) | `FORGE_ENROLL_TOKEN` is unset or empty. Nothing was changed and nothing was reported | Set the token, see [Examples](#examples) |
| `1` | `run as root (e.g. with sudo)` | Not root | Pipe into `sudo sh` |
| `1` | `this installer is for Linux; see the Forge documentation for Windows nodes` | `uname -s` is not `Linux` | Run on Linux. For Windows, use [`install.ps1`](#windows-installps1) |
| `1` | `unsupported architecture <machine>` | Not x86_64 or aarch64 | Use a supported machine |
| `1` | `no agent build for <arch> on this control plane` | `forge-agent-linux-<arch>` is missing from `agent_dist_dir` | Put the build in `agent_dist_dir` (see [Configuration](/docs/reference/configuration/)) and restart the control plane |
| `1` | `need curl or wget` | Neither tool is installed | Install `curl` |
| `1` | `download failed: <url>` | The node cannot reach `<forge-url>`, or the build is missing | Check that the node reaches `node_public_url` |
| `1` | `checksum mismatch for <file>` | The download does not match the checksum in the script | Fetch the script again; if it persists, check for a proxy that rewrites downloads |
| `1` | `Error: <reason>` (from `forge-agent`) | Enrollment failed, for example a malformed, used or expired token, or the node cannot reach `<agent-url>` on TCP 7443 | Create a new token; open TCP 7443 from the node to the control plane |
| `1` | `forge-agent-helper pin: <reason>` | The helper could not pin the enrollment | See the caution in [Re-running and upgrading in place](#re-running-and-upgrading-in-place) |
| non-zero | output of the failing command | Another command failed (`set -e`), for example `useradd` or `systemctl restart` | Read the `$` line above the error |

On any exit other than `0`, the script reports `installer failed (exit
<code>)` to the console.

## Windows (install.ps1)

`install.ps1` installs `forge-agent.exe` on a Windows machine, enrolls the
machine with a one-time token and runs the agent as the `ForgeAgent` Windows
service. There is no helper on Windows: the service runs as LocalSystem and
does privileged work in its own process. For the task, see
[Add a Windows node with the installer](/docs/guides/add-nodes/#add-a-windows-node-with-the-installer).

**Requires:** Windows 10 version 1809, Windows Server 2019 or later (build
17763 or higher); 64-bit Windows on x86_64 (ARM64 is not supported);
Windows PowerShell 5.1 or PowerShell 7, 64-bit, run as Administrator. To
create the token you need the `server.enroll` action on the workspace (see
[Cedar schema](/docs/reference/cedar-schema/)).

:::caution
The `ForgeAgent` service runs as LocalSystem, by design. A compromised agent
process has full control of the machine. See
[Open security work](/docs/security/roadmap/#windows-agent-runs-as-localsystem)
for the reasons and what limits the risk.
:::

### Windows synopsis

```powershell
$t = Read-Host 'Enrollment token' -AsSecureString; $env:FORGE_ENROLL_TOKEN = [Net.NetworkCredential]::new('', $t).Password; Remove-Variable t; irm <forge-url>/install.ps1 | iex
irm <forge-url>/install.ps1 -OutFile install.ps1
powershell -ExecutionPolicy Bypass -File .\install.ps1 [-Uninstall [-Purge]] [-AllowHttp]
```

The first line is the command **Nodes → Add node → Manual** shows under
**Windows 10/11 or Server 2019+ (PowerShell as Administrator)**, and the
`install.windows` field of `POST /api/v1/enrollment-tokens`. It reads the
token masked, into `FORGE_ENROLL_TOKEN`, so it stays out of the PowerShell
history. `<forge-url>` is `node_public_url`, or `public_url` when that is not
set.

### The install.ps1 endpoint

`GET /install.ps1` is public: it needs no session and takes no query
parameters. It answers with `Content-Type: text/plain; charset=utf-8`. The
script never contains a token.

The control plane bakes these values into the script:

| Variable in the script | Value |
| --- | --- |
| `$ForgeUrl` | `node_public_url` (or `public_url`), without a trailing `/`. Used for the download and progress reports |
| `$AgentUrl` | `agent_public_url`. Passed to `forge-agent.exe enroll --server` |
| `$AgentSha` | SHA-256 of `forge-agent-windows-x86_64.exe` in `agent_dist_dir`, as hashed when the control plane started. Empty when the file is missing |

A `'` in a URL is doubled, so a value cannot break out of the PowerShell
string. The agent build is served at
`GET /agent/download/windows/x86_64/forge-agent`.

### Parameters and environment variables

Parameters work only when you run the script as a file (`-File`). Piped into
`iex`, the script takes no parameters; use the environment variables instead.
Parameter names are not case-sensitive.

| Parameter | Environment variable | Default | Description |
| --- | --- | --- | --- |
| (none) | `FORGE_ENROLL_TOKEN` | none | One-time enrollment token from **Nodes → Add node → Manual** or `POST /api/v1/enrollment-tokens`. Used to enroll and to authenticate progress reports. Required unless the machine is already enrolled (`identity.json` exists) |
| `-Uninstall` | `FORGE_UNINSTALL` | off | Removes the agent instead of installing it (see [Uninstalling](#uninstalling)). The variable counts only when it is exactly `1` |
| `-Purge` | `FORGE_PURGE` | off | With `-Uninstall`, also deletes `C:\ProgramData\OrbynForge`. The variable counts only when it is exactly `1`. Ignored without `-Uninstall` |
| `-AllowHttp` | `FORGE_ALLOW_HTTP` | off | Allows a control plane whose `$ForgeUrl` is plain `http://`. The variable counts only when it is exactly `1`. For test labs only: the agent download, and the progress reports that carry the token, then cross the network unencrypted |

When it ends, also after an error, the script removes `FORGE_ENROLL_TOKEN`,
`FORGE_UNINSTALL` and `FORGE_PURGE` from the PowerShell session it runs in.
Run with `-File`, that is the child `powershell` process, not your session.
It does not remove `FORGE_ALLOW_HTTP`; remove it with
`Remove-Item Env:FORGE_ALLOW_HTTP` when you are done.

### Install steps

The script runs with `$ErrorActionPreference = 'Stop'` and strict mode. It
first turns on TLS 1.2 for its own PowerShell process (Windows PowerShell 5.1
may offer older versions only); the machine's TLS settings do not change.
It prints a `$ <command>` line for each step and a `forge-install: <message>`
line for each status.

| # | Printed line | What happens | When |
| --- | --- | --- | --- |
| 1 | `$ check: Administrator, 64-bit, Windows 10 1809 / Server 2019 or later` | Checks that PowerShell runs as Administrator, that Windows is 64-bit and `PROCESSOR_ARCHITECTURE` is `AMD64`, and that the build is 17763 or higher | Always, also when uninstalling |
| 2 | (none) | Checks that `FORGE_ENROLL_TOKEN` is set or the machine is enrolled, that the control plane has a Windows build (`$AgentSha` is not empty), and that `$ForgeUrl` starts with `https://` or `-AllowHttp` is on | Install |
| 3 | `forge-install: installing forge-agent on <computer-name>` | Starts; creates `C:\Program Files\OrbynForge` if needed | Install |
| 4 | `$ download <forge-url>/agent/download/windows/x86_64/forge-agent` | Downloads the agent to `C:\Program Files\OrbynForge\forge-agent.exe.download`. Only Administrators can write to that folder; `%TEMP%` is not used | Install |
| 5 | `forge-install: checksum verified` | Checks the SHA-256 of `forge-agent.exe.download` against `$AgentSha` | Install |
| 6 | `$ sc.exe stop ForgeAgent` | Stops the running service and waits up to 30 seconds for it to stop | The service exists and is not stopped |
| 6a | `$ Stop-Process forge-agent  # did not stop within 30 s` | Clears the service's restart-on-failure actions, so the Service Control Manager does not start the old binary again, then ends the `forge-agent.exe` process that runs from `C:\Program Files\OrbynForge` (no other process) and waits up to 15 seconds. Step 10 sets the failure actions again | The service is still running after 30 seconds |
| 7 | `$ install C:\Program Files\OrbynForge\forge-agent.exe` | Moves the checked `forge-agent.exe.download` over `forge-agent.exe`, replacing any older copy | Install |
| 8 | `$ icacls C:\ProgramData\OrbynForge /setowner Administrators; Set-Acl: no inheritance, SYSTEM and Administrators full control only` | Locks `C:\ProgramData\OrbynForge` as described in [The data folder](#the-data-folder): creates it, or checks an existing folder and everything in it; makes Administrators the owner of the folder itself; and replaces its permissions with a new list that grants full control to SYSTEM and Administrators only, without inheritance, inherited by subfolders and files. Then creates `C:\ProgramData\OrbynForge\agent`. The groups are given by SID (`S-1-5-18`, `S-1-5-32-544`), so this works on any Windows language | Install. The line is not printed when the folder is already locked |
| 9a | `$ forge-agent.exe enroll --server <agent-url>  # token from $env:FORGE_ENROLL_TOKEN` | Runs `forge-agent.exe --state-dir C:\ProgramData\OrbynForge\agent enroll --server <agent-url>`. The token reaches the agent through the environment, never the command line. The agent pins the control plane's CA through the token and writes its identity, key, certificate and `agent.toml` | `identity.json` does not exist |
| 9b | `forge-install: already enrolled; keeping this node's identity` | Skips enrolling | `identity.json` exists |
| 10 | `$ forge-agent.exe service install  # ForgeAgent, automatic start, LocalSystem` | Runs [`forge-agent.exe --state-dir C:\ProgramData\OrbynForge\agent service install`](/docs/reference/cli/forge-agent/#forge-agent-service-install): creates the `ForgeAgent` service, or points the existing one at this binary, and starts it | Install |
| 11 | `forge-install: the ForgeAgent service is running; logs: C:\ProgramData\OrbynForge\agent\logs` | Done | Install |
| 12 | (none) | Deletes `C:\Program Files\OrbynForge\forge-agent.exe.download` if it is still there | After steps 4 to 11, whether they succeed or fail |
| 13 | (none) | Removes `FORGE_ENROLL_TOKEN`, `FORGE_UNINSTALL` and `FORGE_PURGE` from the session | Always, also after an error and after uninstalling |

The paths follow `%ProgramFiles%` and `%ProgramData%`; the table shows the
defaults. When a check in step 1 or 2 fails, nothing on the machine has
changed.

The download stays in `C:\Program Files\OrbynForge` from the checksum to the
install. A copy in `%TEMP%` could be replaced by any program the same user
runs, between the check and the copy, and the replacement would then run as
LocalSystem.

### The data folder

Any user can create folders in `C:\ProgramData`, so step 8 does not trust a
`C:\ProgramData\OrbynForge` that already exists:

1. If the folder is already locked, it is kept as is and step 8 prints
   nothing. Locked means: not a link, owned by SYSTEM, Administrators or the
   Administrator who runs the installer, inheritance removed, and access
   granted to SYSTEM and Administrators only.
2. Otherwise the installer checks the folder and everything in it, without
   following links. If an item is a link (junction, symbolic link or mount
   point), or is owned by an account other than SYSTEM, Administrators or
   the Administrator who runs the installer, it stops with an error and
   changes nothing. It never takes the folder over.
3. It runs `icacls C:\ProgramData\OrbynForge /setowner Administrators` on the
   folder itself (not `/T`), then sets a new access list with `Set-Acl`:
   inheritance removed, full control for SYSTEM and Administrators,
   inherited by subfolders and files. It does not use `icacls /grant:r`,
   because that keeps other accounts' entries.
4. It checks the folder and its contents again, for anything created while
   it ran.

:::caution[A folder the installer did not create is refused]
Someone who creates `C:\ProgramData\OrbynForge` before the first install
controls what is in it. Taking such a folder over would keep the links and
the permissions on the items inside, and could expose the agent's private
key or redirect what the LocalSystem service writes. When the installer
refuses a folder, find out who created it before you delete it.
:::

The agent locks the folder again on its own: `forge-agent.exe enroll`,
`forge-agent.exe service install` and every service start apply the same
SYSTEM and Administrators access (see
[`forge-agent service install`](/docs/reference/cli/forge-agent/#forge-agent-service-install)).

The script does not open firewall ports (the agent only connects out),
change the execution policy, or change `PATH` or other environment
variables. The Windows installer has no `hostname` or `terminal_account`
options: the node enrolls under the computer name, and no terminal account
is created.

### Files and service

| Path or object | Permissions | Contents |
| --- | --- | --- |
| `C:\Program Files\OrbynForge\forge-agent.exe` | Inherited from `C:\Program Files` | Agent |
| `C:\ProgramData\OrbynForge\` | Owner Administrators (or the SYSTEM, Administrators or Administrator owner of a folder that was already locked); full control for SYSTEM and Administrators only; inheritance removed. Subfolders and files inherit these permissions. See [The data folder](#the-data-folder) | All of the agent's data: enrollment and logs (`agent\`), and what the agent writes while it runs: WireGuard keys (`wireguard\`), package settings as `.env` files (`pkgs\`) and the file explorer's trash and revisions (`files\`) |
| `C:\ProgramData\OrbynForge\agent\` | As above | Agent state: `identity.json`, key, certificate, `agent.toml` |
| `C:\ProgramData\OrbynForge\agent\logs\forge-agent.log` | As above | Service log. At service start, a log larger than 10 MB is moved to `forge-agent.log.1` |
| Service `ForgeAgent` | | See below |

The `ForgeAgent` service:

| Setting | Value |
| --- | --- |
| Display name | `Orbyn Forge agent` |
| Description | `Connects this machine to Orbyn Forge and carries out the actions Forge authorizes.` |
| Command line | `"C:\Program Files\OrbynForge\forge-agent.exe" service run --state-dir C:\ProgramData\OrbynForge\agent` |
| Account | LocalSystem |
| Start type | Automatic |
| Type | Own process |
| On failure | Restart after 5 seconds, then 10 seconds, then 60 seconds for every later failure. The failure count resets after one day. The actions also apply when the agent stops with an error, not only on a crash |

See [`forge-agent service`](/docs/reference/cli/forge-agent/#forge-agent-service)
for the service subcommands.

### Re-running and upgrading

The script is safe to re-run. On a machine that is already enrolled
(`C:\ProgramData\OrbynForge\agent\identity.json` exists) it:

- downloads and verifies the agent, stops the service and replaces
  `forge-agent.exe`;
- keeps `C:\ProgramData\OrbynForge` as is when it is still locked, and
  checks and locks it again otherwise (see [The data folder](#the-data-folder));
- skips `enroll`, and keeps the identity, key and certificate;
- runs `service install`, which points the service at the new binary,
  resets its settings and failure actions, and starts it.

When it starts, the new agent adds the command names that are new in its
release to `allowed_commands`; names you removed stay removed.

A re-run needs no `FORGE_ENROLL_TOKEN`. When one is set, it is used only for
progress reports, which are accepted, used or not, until one hour after the
token expires. Windows agents do not self-update and are not part of
rollouts; re-running the installer is how you upgrade them.

### Uninstalling

With `-Uninstall` (or `FORGE_UNINSTALL=1`), after the checks of step 1:

| # | Printed line | What happens | When |
| --- | --- | --- | --- |
| 1 | `forge-install: removing forge-agent from <computer-name>` | Starts | Always |
| 2 | `$ sc.exe stop ForgeAgent` and, if needed, `$ Stop-Process forge-agent  # did not stop within 30 s` | Stops the service as in steps 6 and 6a of the install | The service exists and is not stopped |
| 3 | `$ forge-agent.exe service uninstall` | Runs [`forge-agent.exe service uninstall`](/docs/reference/cli/forge-agent/#forge-agent-service-uninstall): stops the service, waiting up to 30 seconds, and deletes it | `forge-agent.exe` exists |
| 3a | `$ sc.exe delete ForgeAgent` | Deletes the service without the binary | `forge-agent.exe` is missing but the service exists |
| 4 | `$ Remove-Item C:\Program Files\OrbynForge` | Deletes the program folder | The folder exists |
| 5 | `$ Remove-Item C:\ProgramData\OrbynForge  # -Purge: identity, keys, logs and agent data` | Deletes the data folder | `-Purge`, and the folder exists |
| 6 | `forge-install: forge-agent removed with its data; revoke the node in Forge as well` | Done | `-Purge` |
| 6 | `forge-install: forge-agent removed; its enrollment stays in C:\ProgramData\OrbynForge\agent (-Purge removes it)` | Done | No `-Purge` |

Uninstalling needs no token and no Windows build on the control plane, and
does not check for `https://`. It does not tell Forge: revoke the node in
Forge yourself (see [Revoke a node](/docs/guides/add-nodes/#revoke-a-node)).
Without `-Purge`, installing again later reuses the kept identity. If you
revoked the node, uninstall with `-Purge` before you install again, because
the control plane refuses a revoked identity.

### Progress reports from install.ps1

Every printed `forge-install:` and `$` line is also sent to
`POST /api/v1/enroll/progress` with `Authorization: Bearer <token>` and
`Content-Type: text/plain`, as described in
[Progress reports](#progress-reports), but without the `forge-install: `
prefix. Errors are sent as `error: <message>`. After the steps it sends
`installer finished`, or `installer failed` when a step from the download
(step 4) on failed. A failed check in step 1 or 2 sends only its
`error: <message>` line. It sends no exit code.

Reporting needs `FORGE_ENROLL_TOKEN`: without it, nothing is sent. Each
report waits at most 5 seconds, and a failed report never fails the install.
The agent's own output, such as its enroll line, appears in your PowerShell
window only.

### What install.ps1 prints

A first install:

```
$ check: Administrator, 64-bit, Windows 10 1809 / Server 2019 or later
forge-install: installing forge-agent on <computer-name>
$ download https://<forge-host>/agent/download/windows/x86_64/forge-agent
forge-install: checksum verified
$ install C:\Program Files\OrbynForge\forge-agent.exe
$ icacls C:\ProgramData\OrbynForge /setowner Administrators; Set-Acl: no inheritance, SYSTEM and Administrators full control only
$ forge-agent.exe enroll --server https://<forge-host>:7443  # token from $env:FORGE_ENROLL_TOKEN
<timestamp>  INFO forge_agent::enroll: enrolled server_id=<server-id>
enrolled as server <server-id>
$ forge-agent.exe service install  # ForgeAgent, automatic start, LocalSystem
service ForgeAgent installed and running (C:\Program Files\OrbynForge\forge-agent.exe)
forge-install: the ForgeAgent service is running; logs: C:\ProgramData\OrbynForge\agent\logs
```

A re-run on an enrolled machine whose data folder is still locked (no
`icacls` line):

```
$ check: Administrator, 64-bit, Windows 10 1809 / Server 2019 or later
forge-install: installing forge-agent on <computer-name>
$ download https://<forge-host>/agent/download/windows/x86_64/forge-agent
forge-install: checksum verified
$ sc.exe stop ForgeAgent
$ install C:\Program Files\OrbynForge\forge-agent.exe
forge-install: already enrolled; keeping this node's identity
$ forge-agent.exe service install  # ForgeAgent, automatic start, LocalSystem
service ForgeAgent installed and running (C:\Program Files\OrbynForge\forge-agent.exe)
forge-install: the ForgeAgent service is running; logs: C:\ProgramData\OrbynForge\agent\logs
```

### install.ps1 errors

An error prints `forge-install: error: <message>` in red, then stops the
script with the PowerShell error `forge-install: <message>`. Piped into
`iex`, the PowerShell window stays open; run with `-File`, `powershell`
exits with code `1`.

| Message | Cause | Fix |
| --- | --- | --- |
| `run PowerShell as Administrator` | PowerShell is not elevated | Start PowerShell with **Run as administrator** |
| `this installer needs 64-bit Windows on x86_64` | 32-bit Windows, ARM64 Windows, or the 32-bit **Windows PowerShell (x86)** | Use the 64-bit PowerShell on 64-bit x86_64 Windows |
| `Windows 10 1809 or Windows Server 2019 or later is needed (this is build <n>)` | Build older than 17763 | Upgrade Windows |
| `set $env:FORGE_ENROLL_TOKEN to the token shown in Orbyn Forge` | No token and the machine is not enrolled | Run the command from **Add node**, or set the variable as in the [synopsis](#windows-synopsis) |
| `this Forge server has no Windows agent build` | `forge-agent-windows-x86_64.exe` is not in `agent_dist_dir` | Put the build there and restart the control plane |
| `refusing a plain http:// server; set $env:FORGE_ALLOW_HTTP=1 only for a test lab` | `$ForgeUrl` is `http://` | Give the control plane an `https://` URL; in a test lab only, set `FORGE_ALLOW_HTTP` to `1` |
| `checksum mismatch for forge-agent.exe (expected <sha>, got <sha>)` | The build in `agent_dist_dir` was replaced after the control plane started, or a proxy rewrote the download | Restart the control plane after replacing builds; otherwise check for a proxy |
| `<path> is a link (reparse point); this installer did not create it. Inspect and remove <folder>, then run the installer again` | `C:\ProgramData\OrbynForge` existed but was not locked, and it, or an item in it, is a junction, symbolic link or mount point. `<path>` is that item | Find out who created the folder. Remove `<folder>` (`Remove-Item` deletes a link, not its target), then run the installer again |
| `<path> is owned by <SID>, not SYSTEM or Administrators; this installer did not create it. Inspect and remove <folder>, then run the installer again` | `C:\ProgramData\OrbynForge` existed but was not locked, and it, or an item in it, belongs to another account. `<SID>` is that account | Look up the account (`([Security.Principal.SecurityIdentifier]'<SID>').Translate([Security.Principal.NTAccount])`), find out why it created the folder, remove `<folder>`, then run the installer again |
| Either message above, ending in `created while the installer ran. Inspect and remove <folder>, then run the installer again` | A link or another account's item appeared in the folder while the installer locked it | Treat the machine as compromised until you know which program created it; then remove `<folder>` and run the installer again |
| `could not take ownership of <folder>` or `could not restrict access to <folder>` | `icacls /setowner` failed on `C:\ProgramData\OrbynForge`, or the folder was not locked after `Set-Acl` | Close programs that use the folder, then run again |
| `enrollment failed (is the token still valid?)` | The token is malformed, used or expired, or the machine cannot reach `<agent-url>` on TCP 7443. The agent's reason is printed above the error; `Error: could not restrict access to <dir> (icacls …)` means it could not lock the data folder | Create a new token; open TCP 7443 from the machine to the control plane. For the `icacls` error, see [`forge-agent enroll`](/docs/reference/cli/forge-agent/#notes) |
| `could not install or start the ForgeAgent service` | The Service Control Manager refused, the agent could not lock the data folder (`Error: could not restrict access to <dir> (icacls …)` above the error), or the agent stopped at start-up | Read the output above the error and `C:\ProgramData\OrbynForge\agent\logs\forge-agent.log` |
| `the ForgeAgent service did not stop` | The service was still running 15 seconds after the installer ended its process, for example because another program holds it | Run `sc.exe queryex ForgeAgent` to see its state and process id, end that process, and run the installer again |
| `could not remove the ForgeAgent service` | `service uninstall` failed while uninstalling | Read the output above the error, then run the installer again |
| PowerShell download error (for example `Unable to connect to the remote server`) | The machine cannot reach `<forge-url>` | Check that the machine reaches `node_public_url` |

## See also

- [Add nodes](/docs/guides/add-nodes/): assisted and manual onboarding, step by
  step.
- [Agents](/docs/concepts/agents/#enrollment): how enrollment, the link and the
  helper work.
- [Configuration](/docs/reference/configuration/): `node_public_url`,
  `agent_public_url`, `agent_dist_dir` and the agent's
  [`agent.toml`](/docs/reference/configuration/#agent-agenttoml).
- [Update agents](/docs/guides/update-agents/): roll out new agent builds without
  re-running the installer.
- [Hardening: agents and nodes](/docs/security/hardening/#agents-and-nodes).
- [Server installer](/docs/reference/cli/server-installer/): install the control
  plane.
