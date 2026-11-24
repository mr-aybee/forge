---
title: forge-agent
description: Every forge-agent subcommand, flag, environment variable, state file and exit code.
sidebar:
  order: 91
---

`forge-agent` is the node agent. It enrolls a machine with a one-time token,
then holds one mutual-TLS connection to the control plane and runs the signed
commands it receives. This page lists every subcommand, flag, environment
variable and file. For how enrollment and the link work, see
[Agents](/docs/concepts/agents/).

```
Usage: forge-agent [OPTIONS] <COMMAND>
```

| Subcommand | Purpose | Normally run by |
| --- | --- | --- |
| [`enroll`](#forge-agent-enroll) | Enrolls this machine with a one-time token | The node installer, as the `forge-agent` user |
| [`run`](#forge-agent-run) | Runs the agent in the foreground | systemd (`forge-agent.service`), as the `forge-agent` user |
| [`status`](#forge-agent-status) | Prints the enrolled identity | You, as root or the `forge-agent` user |
| [`offer-commands`](#forge-agent-offer-commands) | Allows command kinds that this release adds | The node installer, as the `forge-agent` user; `run` does the same at start |
| `help` | Prints help for the agent or a subcommand | You |

:::note
On Windows the binary is `forge-agent.exe` and `--help` prints
`forge-agent.exe` in the usage line. The flags are the same.
:::

## Platforms

| | Linux (systemd distributions) | Windows 10 1809+ / Server 2019+ (x86_64) |
| --- | --- | --- |
| Install | The node installer (see [Add nodes](/docs/guides/add-nodes/#add-a-linux-node-with-the-installer-manual)) | `install.ps1` (see [Add a Windows node](/docs/guides/add-nodes/#add-a-windows-node-with-the-installer) and the [node installer](/docs/reference/cli/node-installer/#windows-installps1)) |
| Runs as | Unprivileged `forge-agent` user; root work goes to [`forge-agent-helper`](/docs/reference/cli/forge-agent-helper/) | LocalSystem, as the `ForgeAgent` service; privileged work runs in the agent's own process |
| Default state directory | `/var/lib/forge-agent` | `%ProgramData%\OrbynForge\agent` |
| Binary | `/usr/local/bin/forge-agent` | `C:\Program Files\OrbynForge\forge-agent.exe` |
| Service | `forge-agent.service` (systemd), written by the installer | `ForgeAgent` (automatic start, restarted after a failure), created by [`forge-agent service install`](#forge-agent-service-install) |
| Self-update | Yes (see [Update agents](/docs/guides/update-agents/)) | No; replace the binary by hand |

macOS is not supported.

## Global options

These options work before or after the subcommand.

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--state-dir <STATE_DIR>` | path | `/var/lib/forge-agent` (Linux), `%ProgramData%\OrbynForge\agent` (Windows) | No | State directory: identity, keys and `agent.toml`. Env: `FORGE_AGENT_STATE_DIR` | `--state-dir /var/lib/forge-agent` |
| `-h`, `--help` | flag | | No | Prints help | `forge-agent enroll --help` |
| `-V`, `--version` | flag | | No | Prints `forge-agent <version>` | `forge-agent --version` |

### Environment variables

| Variable | Used by | Description |
| --- | --- | --- |
| `FORGE_AGENT_STATE_DIR` | all subcommands | Same as `--state-dir`. The flag wins over the variable |
| `FORGE_SERVER` | `enroll` | Same as `--server` |
| `FORGE_ENROLL_TOKEN` | `enroll` | Same as `--token`. `--help` never prints its value |
| `RUST_LOG` | all subcommands | Log filter, for example `debug` or `forge_agent=debug`. Default `info`. Logs go to standard output |

## State directory

`enroll` creates these files. `run` and `status` read them.

| File | Contents | Mode (Linux) |
| --- | --- | --- |
| `agent.toml` | Operator settings: `server_url`, `allowed_commands`, `offered_commands`. See [Configuration](/docs/reference/configuration/#agent-agenttoml) | `0600`, owner `forge-agent` |
| `identity.json` | `server_id`, `command_public_key`, `heartbeat_interval_secs`, issued by the control plane | `0600`, owner `forge-agent` |
| `agent.key` | The agent's private key, generated on the node. It never leaves the node | `0600`, owner `forge-agent` |
| `agent.crt` | The agent's client certificate (renewed automatically) | `0600`, owner `forge-agent` |
| `ca.crt` | The control plane's CA, checked against the fingerprint in the token | `0600`, owner `forge-agent` |

The installer creates the directory as `0700`, owned by `forge-agent`.

### Windows data folder

On Windows the agent locks `%ProgramData%\OrbynForge` itself, and the state
directory too when it lies outside that folder. It does this at
[`enroll`](#forge-agent-enroll), at
[`service install`](#forge-agent-service-install) and each time
[`service run`](#forge-agent-service-run) starts. Without it, a hand-run
`enroll` would write `agent.key` into a folder that inherits the read access
`C:\ProgramData` gives to Users.

| Step | What happens |
| --- | --- |
| 1 | Refuses a folder that is a link (junction, symbolic link or mount point): `<dir> is a link (reparse point); remove it and run again` |
| 2 | Creates the folder if it does not exist |
| 3 | Runs `icacls <dir> /inheritance:r /grant:r *S-1-5-18:(OI)(CI)F *S-1-5-32-544:(OI)(CI)F /Q`: removes inherited access and grants full control to SYSTEM and Administrators, inherited by subfolders and files |
| 4 | Runs `icacls <dir> /remove:g *S-1-1-0 *S-1-5-11 *S-1-5-32-545 *S-1-3-0 *S-1-3-4 /Q`: removes grants to Everyone, Authenticated Users, Users, CREATOR OWNER and OWNER RIGHTS |

When `icacls` fails, the error is
`could not restrict access to <dir> (icacls <arguments>): <icacls error>; run as Administrator`.
`enroll` and `service install` stop with exit code `1`; `service run` logs a
warning and starts anyway.

The agent does not change the folder's owner and does not check the items
inside it. The installer does both before the agent runs (see
[Node installer: the data folder](/docs/reference/cli/node-installer/#the-data-folder)).

:::caution
`agent.key` is the node's credential. Do not copy it off the node or into a
support ticket. If you think it leaked, revoke the node and enroll it again
(see [Revoke a node](/docs/guides/add-nodes/#revoke-a-node)).
:::

## Exit codes

| Code | Meaning |
| --- | --- |
| `0` | Success |
| `1` | The subcommand failed. The error is printed to standard error as `Error: <message>` |
| `2` | Usage error: unknown subcommand, or a missing or invalid flag |

---

## forge-agent enroll

Enrolls this machine with a one-time token. The agent fetches the control
plane's CA, checks it against the fingerprint in the token, generates its key,
and sends a certificate request. It then writes the
[state directory](#state-directory).

**Who runs it:** the node installer, as the `forge-agent` user
(`runuser -u forge-agent`), right after it installs the binaries. On Linux the
installer then runs [`forge-agent-helper pin`](/docs/reference/cli/forge-agent-helper/#forge-agent-helper-pin)
as root. You run it by hand only to enroll a Windows node, or to re-enroll a
Linux node. Creating the token needs the `server.enroll` action
(`platform_admin`, `workspace_admin` or `operator`).

### Synopsis

```
Usage: forge-agent enroll [OPTIONS] --server <SERVER> --token <TOKEN>
```

### Options

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--server <SERVER>` | URL | | Yes | Agent endpoint of the control plane (the agent listener, port `7443` by default). Env: `FORGE_SERVER` | `--server https://<forge-host>:7443` |
| `--token <TOKEN>` | string | | Yes | One-time enrollment token. Prefer `FORGE_ENROLL_TOKEN` so the token stays out of shell history and `ps` | `FORGE_ENROLL_TOKEN=<token>` |
| `--hostname <HOSTNAME>` | string | the OS hostname | No | Hostname reported to Forge instead of the detected one | `--hostname <node-name>` |
| `--terminal-account <NAME>` | string | none | No | Terminal account the installer created on this machine. Forge saves it to the node's terminal logins when it is a valid login name (1–32 characters of `A-Z a-z 0-9 _ . -`, not starting with `-`), not saved yet, and the list holds fewer than 32 accounts, audited as `server.logins.update` by the person who created the token. It does not create the account; [`forge-agent-helper terminal-account`](/docs/reference/cli/forge-agent-helper/#forge-agent-helper-terminal-account) does | `--terminal-account forge-ops` |
| `--state-dir <STATE_DIR>` | path | see [Global options](#global-options) | No | Where to write the identity, keys and `agent.toml` | `--state-dir /var/lib/forge-agent` |

The token has the form `<secret>.<sha256 of the CA>`. Tokens are single use
and expire after 1–1440 minutes.

### Examples

Enroll a Linux node by hand, as the `forge-agent` user. `read -rs` keeps the
token out of shell history:

```sh
read -rs FORGE_ENROLL_TOKEN && export FORGE_ENROLL_TOKEN
sudo --preserve-env=FORGE_ENROLL_TOKEN runuser -u forge-agent -- \
  /usr/local/bin/forge-agent --state-dir /var/lib/forge-agent \
  enroll --server https://<forge-host>:7443
unset FORGE_ENROLL_TOKEN
```

Enroll a Windows node (PowerShell as Administrator, in the folder that holds
`forge-agent.exe`). This is the command the UI shows:

```powershell
$t = Read-Host 'Enrollment token' -AsSecureString; $env:FORGE_ENROLL_TOKEN = [Net.NetworkCredential]::new('', $t).Password; Remove-Variable t; .\forge-agent.exe enroll --server https://<forge-host>:7443; Remove-Item Env:FORGE_ENROLL_TOKEN
```

The token is read masked at the `Enrollment token` prompt, so it stays out
of the PowerShell history.

:::note
[`service install`](#forge-agent-service-install) refuses a
`forge-agent.exe` outside `%ProgramFiles%`. To run the enrolled agent as a
service, keep the binary in `C:\Program Files\OrbynForge` before you enroll.
Releases do not include the Windows agent yet; build it from source.
`install.ps1` is the supported way to set up a Windows node: it downloads
the binary there, checks it, locks the data folder and enrolls (see the
[node installer](/docs/reference/cli/node-installer/#windows-installps1)).
:::

Report the terminal account the installer created, so Forge offers it on
**Open terminal**:

```sh
forge-agent enroll --server https://<forge-host>:7443 --terminal-account forge-ops
```

Report a different hostname:

```sh
forge-agent enroll --server https://<forge-host>:7443 --hostname <node-name>
```

### Sample output

Reconstructed from the code (`main.rs`, `enroll.rs`). The log line goes to
standard output, followed by the result:

```text
<timestamp>  INFO forge_agent::enroll: enrolled server_id=<server-id>
enrolled as server <server-id>
```

A missing flag (real output, exit code `2`):

```text
error: the following required arguments were not provided:
  --server <SERVER>
  --token <TOKEN>

Usage: forge-agent enroll --server <SERVER> --token <TOKEN>

For more information, try '--help'.
```

### Notes

:::caution
`enroll` overwrites the state directory, including `agent.toml`. Any command
names you removed from `allowed_commands` come back. Re-apply your
restrictions after a re-enrollment (see
[Restrict what an agent may do](/docs/guides/add-nodes/#restrict-what-an-agent-may-do)).
:::

- On Linux, after a manual re-enrollment, pin the new identity for the
  helper as root, then restart both services. Until you do, the helper keeps
  trusting the old pin and refuses commands for the new node id:

  ```sh
  sudo /usr/local/bin/forge-agent-helper pin --agent-dir /var/lib/forge-agent
  sudo systemctl restart forge-agent-helper forge-agent
  ```

- `malformed enrollment token` means the token was cut off or is not a Forge
  token. A CA fingerprint error means the agent reached a different control
  plane than the one that issued the token.
- On Windows, `enroll` first locks `%ProgramData%\OrbynForge` (see
  [Windows data folder](#windows-data-folder)). If that fails, it stops
  before it contacts the control plane, so the token stays unused:

  ```text
  Error: could not restrict access to C:\ProgramData\OrbynForge (icacls /inheritance:r /grant:r *S-1-5-18:(OI)(CI)F *S-1-5-32-544:(OI)(CI)F /Q): <icacls error>; run as Administrator
  ```

  Run PowerShell as Administrator and enroll again. A folder that is a link
  fails with `Error: C:\ProgramData\OrbynForge is a link (reparse point); remove it and run again`.

---

## forge-agent run

Runs the agent in the foreground. It loads `agent.toml`, `identity.json` and
its certificate, connects to the control plane, and long-polls for signed
commands until it is stopped.

**Who runs it:** systemd, through `forge-agent.service`, as the `forge-agent`
user. On Linux hosts without systemd (containers) the installer starts it in a
respawn loop that logs to `/var/log/forge-agent.log`. On Linux, run it by hand
only to debug. On Windows the `ForgeAgent` service runs the agent through
[`forge-agent service run`](#forge-agent-service-run); run `run` by hand,
as Administrator, only to debug.

### Synopsis

```
Usage: forge-agent run [OPTIONS]
```

### Options

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--state-dir <STATE_DIR>` | path | see [Global options](#global-options) | No | State directory to load | `--state-dir /var/lib/forge-agent` |

`run` has no other flags. Settings come from `agent.toml`.

### The systemd unit

The installer writes `/etc/systemd/system/forge-agent.service`:

```ini
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

### Examples

Restart the agent after you edit `agent.toml`:

```sh
sudo systemctl restart forge-agent
```

Run it in the foreground with debug logs, after you stop the service.
Press **Ctrl+C** to stop:

```sh
sudo systemctl stop forge-agent
sudo runuser -u forge-agent -- env RUST_LOG=debug \
  /usr/local/bin/forge-agent --state-dir /var/lib/forge-agent run
sudo systemctl start forge-agent
```

Follow the agent's log:

```sh
journalctl -u forge-agent -f
```

### Sample output

Real output from a Linux node (`journalctl -u forge-agent`), with ids
replaced by placeholders:

```text
Started forge-agent.service - Orbyn Forge agent.
<timestamp>  INFO forge_agent: agent starting server_id="<server-id>"
<timestamp>  INFO forge_agent::agent: self-update confirmed version="<version>"
<timestamp>  INFO forge_agent::agent: executing command op=<operation-id> command="fabric" principal=user:<user-id>
<timestamp>  WARN forge_agent::agent: heartbeat failed error=Connection refused (os error 111) retry_in=2.345937579s
<timestamp>  INFO forge_agent::agent: reconnected to control plane
```

`heartbeat failed` followed by `reconnected to control plane` is the normal
pattern while the control plane restarts. The agent retries with exponential
backoff and full jitter: each wait is random, up to a limit that grows from
1 s to 60 s.

A state directory that was never enrolled (real output, exit code `1`):

```text
Error: reading agent.toml

Caused by:
    No such file or directory (os error 2)
```

---

## forge-agent status

Prints the enrolled identity from `identity.json` as JSON. It does not contact
the control plane, so it works while the node is offline.

**Who runs it:** you, as root or as the `forge-agent` user. Other users cannot
read the state directory.

### Synopsis

```
Usage: forge-agent status [OPTIONS]
```

### Options

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--state-dir <STATE_DIR>` | path | see [Global options](#global-options) | No | State directory to read | `--state-dir /var/lib/forge-agent` |

### Output fields

| Field | Type | Description |
| --- | --- | --- |
| `server_id` | string (UUID) | The node's id in Forge. It matches the id in the node page URL and in the agent's certificate |
| `command_public_key` | string (base64url) | Forge's Ed25519 public key. The agent accepts only commands signed with it |
| `heartbeat_interval_secs` | integer, seconds | Long-poll hold time the control plane announced at enrollment |

### Examples

```sh
sudo runuser -u forge-agent -- /usr/local/bin/forge-agent --state-dir /var/lib/forge-agent status
```

Print only the node id (needs `jq`):

```sh
sudo /usr/local/bin/forge-agent --state-dir /var/lib/forge-agent status | jq -r .server_id
```

### Sample output

Real output from a Linux node, with the id and key replaced:

```json
{
  "server_id": "<server-id>",
  "command_public_key": "<command-public-key>",
  "heartbeat_interval_secs": 25
}
```

A state directory without `identity.json` (real output, exit code `1`):

```text
Error: No such file or directory (os error 2)
```

---

## forge-agent offer-commands

Adds to `allowed_commands` the command kinds that this release adds and that
were never offered before. Kinds that you removed stay removed. It records
every kind it has offered in `offered_commands`.

**Who runs it:** the node installer, as the `forge-agent` user, each time it
runs on an enrolled node. `forge-agent run` does the same at every start, so a
node that updates itself gets the new kinds too; it logs
`new command kinds allowed added=…` when it adds any.

### Synopsis

```
Usage: forge-agent offer-commands [OPTIONS]
```

### Options

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--state-dir <STATE_DIR>` | path | see [Global options](#global-options) | No | State directory whose `agent.toml` to update | `--state-dir /var/lib/forge-agent` |

### How it decides

| Kind is in `offered_commands` | Kind is in `allowed_commands` | Result |
| --- | --- | --- |
| Yes | Yes | Unchanged |
| Yes | No | Unchanged: you removed it, so it stays removed |
| No | No | Added to both lists, and printed |
| No | Yes | Added to `offered_commands` only |

An `agent.toml` without `offered_commands` gets every missing kind once.
From then on, removals stick.

`container_exec` is the exception: it is opt-in. `offer-commands` records it
in `offered_commands` but never adds it to `allowed_commands`; add it
yourself to allow [container shells](/docs/concepts/container-shells/) on an
existing node. New enrollments allow it.

### Examples

```sh
sudo runuser -u forge-agent -- /usr/local/bin/forge-agent --state-dir /var/lib/forge-agent offer-commands
sudo systemctl restart forge-agent
```

The agent reads `agent.toml` only when it starts. Restart it after you run
`offer-commands` by hand.

### Sample output

Reconstructed from the code (`main.rs`). Nothing to add:

```text
no new commands
```

Kinds added:

```text
allowed: <command-kind>, <command-kind>
```

## forge-agent service

The Windows service. Windows only: on Linux each subcommand exits `1` with
"`forge-agent service` is for Windows; on Linux the installer sets up the
forge-agent systemd unit". The Windows installer (`install.ps1`) runs
`service install`; you use these commands by hand only to repair or remove
an installation.

:::caution
The service runs as LocalSystem, so a compromised agent process has full
control of that machine. Linux splits an unprivileged agent from a privileged
helper; the same split for Windows is planned (see the
[security roadmap](/docs/security/roadmap/)).
:::

### forge-agent service install

Registers the `ForgeAgent` service for this binary and starts it. The service
starts automatically with Windows, runs as LocalSystem, and the Service
Control Manager restarts it 5 seconds, 10 seconds and then 60 seconds after a
failure (the count resets after a day). If the service exists, `install`
points it at this binary and these options, which is how an upgrade works.
Run it as Administrator.

Before it registers the service, it:

1. Refuses a binary outside `%ProgramFiles%` (by default `C:\Program Files`).
   The service runs the binary as LocalSystem. In a folder other accounts
   can write, such as `Downloads`, `%TEMP%` or a share, any program those
   accounts run could replace it and take over the machine.
2. Locks `%ProgramData%\OrbynForge`, and the state directory when it lies
   elsewhere (see [Windows data folder](#windows-data-folder)).

#### Synopsis

```
Usage: forge-agent service install [OPTIONS]
```

#### Options

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--state-dir <STATE_DIR>` | path | see [Global options](#global-options) | No | State directory the service uses; it is written into the service's command line | `--state-dir C:\ProgramData\OrbynForge\agent` |

#### Example

```powershell
& 'C:\Program Files\OrbynForge\forge-agent.exe' --state-dir C:\ProgramData\OrbynForge\agent service install
```

#### Sample output

```text
service ForgeAgent installed and running (C:\Program Files\OrbynForge\forge-agent.exe)
```

A binary outside `%ProgramFiles%` (exit code `1`, nothing is changed):

```text
Error: C:\Users\<user>\Downloads\forge-agent.exe is not under C:\Program Files: the service runs it as LocalSystem, and a folder other accounts can write would let them replace it. Copy it to C:\Program Files\OrbynForge, or use install.ps1
```

| Exit code | Meaning |
| --- | --- |
| `0` | Installed and started |
| `1` | The binary is not under `%ProgramFiles%` (`<exe> is not under <ProgramFiles>: …`) |
| `1` | The data folder could not be locked (`could not restrict access to <dir> (icacls …): …; run as Administrator`, or `<dir> is a link (reparse point); remove it and run again`) |
| `1` | Not Administrator, or the Service Control Manager refused (the message says why) |

### forge-agent service uninstall

Stops the `ForgeAgent` service (waiting up to 30 seconds) and deletes it. The
binary and the enrollment in the state directory stay: run the installer with
`-Uninstall -Purge` to remove everything. Run it as Administrator.

#### Synopsis

```
Usage: forge-agent service uninstall [OPTIONS]
```

#### Options

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--state-dir <STATE_DIR>` | path | see [Global options](#global-options) | No | Not used by `uninstall` | |

#### Sample output

```text
service ForgeAgent removed
```

When no service is installed it prints `service ForgeAgent is not installed`
and exits `0`. Exit codes: `0`; `1` not Administrator or the service could
not be removed.

### forge-agent service run

The service's entry point: only the Service Control Manager starts it. It
runs the agent like [`run`](#forge-agent-run) until the service is stopped or
Windows shuts down, and logs to `logs\forge-agent.log` in the state
directory. At start-up, a log larger than 10 MB is moved to
`forge-agent.log.1`. Started from a console, it fails, because only the SCM
can host a service.

At each start it locks the data folder again (see
[Windows data folder](#windows-data-folder)), in case its permissions were
loosened. If that fails, it logs
`could not restrict access to the data folder` with the error as a warning
and starts anyway.

On a stop or shutdown request, the service reports **Stop pending** at
once and the agent ends its current exchange with the control plane. If the
agent has not finished after 15 seconds, the service reports **Stopped**
anyway and the process exits, ending any command still running.

#### Synopsis

```
Usage: forge-agent service run [OPTIONS]
```

#### Options

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--state-dir <STATE_DIR>` | path | see [Global options](#global-options) | No | State directory to load (set by `service install`) | `--state-dir C:\ProgramData\OrbynForge\agent` |

Exit codes: the service reports `0` when it stopped normally, and
`ERROR_SERVICE_SPECIFIC_ERROR` (1066) when the agent stopped with an error,
which triggers the restart actions.

## See also

- [Agents](/docs/concepts/agents/): enrollment, the link, signed commands and the
  privileged helper.
- [Add nodes](/docs/guides/add-nodes/): the installer that runs these commands.
- [Configuration: `agent.toml`](/docs/reference/configuration/#agent-agenttoml):
  every key and the default `allowed_commands`.
- [forge-agent-helper](/docs/reference/cli/forge-agent-helper/): the root helper
  on Linux.
- [Agent protocol](/docs/reference/agent-protocol/): the wire messages.
