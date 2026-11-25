---
title: Add nodes
description: Onboard Linux and Windows machines with an SSH login (assisted) or a one-line installer (manual).
sidebar:
  order: 2
---

A node is any machine that runs the Forge agent. This guide shows you how to
add one. Every way ends with the same result: the agent runs as a service,
holds its own certificate and manages the node from then on. On Linux the
service runs under an unprivileged `forge-agent` user; on Windows it is the
`ForgeAgent` service, running as LocalSystem.

| | Assisted (SSH login) | Manual (one-line installer) |
| --- | --- | --- |
| You provide | Host, SSH login of a **sudo-capable user** (password or key) | Nothing: you run one command on the machine |
| Root login needed | No | No: Linux uses `sudo`; Windows needs PowerShell as Administrator |
| Cedar action | `node.onboard` (needs an MFA-verified session) | `server.enroll` |
| Platforms | Linux | Linux (`install.sh`); Windows (`install.ps1`) only with an agent you build from source |

For how agents connect and why they dial out, see [Agents](/docs/concepts/agents/).

## Before you begin

- You need the **Operator**, **Admin** or **Platform admin** role on the
  workspace or on the target cluster. Assisted onboarding uses
  `node.onboard`, which needs an MFA-verified session when the server requires
  MFA ([Multi-factor authentication](/docs/guides/mfa/)). Manual onboarding uses
  `server.enroll`, which does not.
- The machine reaches the control plane on some address (to download the
  installer and agent), its agent port, TCP `3843` by default, and, when
  analytics is on, the analytics worker's upload port, TCP `3832` by
  default. See [Network ports](/docs/reference/network-ports/).
- For assisted onboarding: the control plane reaches the machine's SSH port,
  and the machine runs Linux with a user that can run `sudo`.
- The control plane has agent builds to serve. A release package ships them
  in `dist/` next to the server. Otherwise put `forge-agent-linux-<arch>` and
  `forge-agent-helper-linux-<arch>` (`x86_64`, `aarch64`) with a `SHA256SUMS`
  file in `agent_dist_dir` (default `<data_dir>/dist`). Releases ship Linux
  agents only; for Windows nodes, add your own `forge-agent-windows-x86_64.exe`
  build (see [Add a Windows node](#add-a-windows-node-with-the-installer)).
  Until they exist, **Add a node**
  warns and onboarding fails with a clear error.

### Which address nodes use

The command on **Add node** uses the address your browser is on. The node
downloads the installer from that address and connects its agent to the same
host on port `3843`. Open the console on an address the node can reach
before you copy the command.

For example, with a control plane on two networks:

| Control plane address | Open the console on | Nodes on that network enroll against |
| --- | --- | --- |
| `192.0.2.10` (office LAN) | `http://192.0.2.10:3880` | `https://192.0.2.10:3843` |
| `10.0.0.5` (datacenter LAN) | `http://10.0.0.5:3880` | `https://10.0.0.5:3843` |

You can also edit the host in the copied command. To send every node to one
address regardless of where you are, set `node_public_url` in
[`forge.toml`](/docs/reference/configuration/).

## Add a node with its SSH login (assisted)

1. Open **Overview** and select **Add node**. From a cluster, open
   **Nodes** and select **Add new node**; the cluster is then preselected.
2. Select the **Assisted (SSH login)** tab.
3. Enter the **Host or IP** and **SSH port**, then select **Check host**.
   Forge connects without logging in and shows the server's SSH host-key
   fingerprint.
4. Compare the fingerprint on the machine:

   ```sh
   ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub
   ```

   If it matches, tick **This fingerprint matches my server**. Forge refuses
   any other key from now on, which protects you against a
   man-in-the-middle.
5. Under **Login for a sudo-capable account**, enter the **Username**, pick
   **Password** or **SSH private key** under **Authentication**, and fill in
   the secret. Enter a **Sudo password** if it differs from the login
   password, or if you use a key and the user has no passwordless sudo.
6. Under **Placement**, pick the **Cluster** and **Environment**.
7. Under **Terminal access**, choose the account for the
   [browser terminal](/docs/guides/terminal/):
   - **Create a terminal account** is on by default. Leave it on to have
     the installer create a local user named in **Account name** (default
     `forge-ops`). Turn it off to create none.
   - Turn on **Allow sudo** only when that account needs passwordless
     `sudo`. It is off by default.

   The SSH **Username** you entered is also saved as a terminal account for
   the node, whatever you choose here.
8. Leave **Keep the credential for repairs** cleared unless you need it.
9. Select **Install agent and join**.

The page then follows the `node.bootstrap` job: its steps with durations, a
red error box if it fails, and the **Install console**. The console shows
every command Forge runs on the machine as a `$ …` line, followed by its
output, live. **Cancel job** stops the install: steps already done stay done,
and the running step is stopped.

You can leave the page. Until the node enrolls, its cluster's **Nodes** list
shows it as **installing** (or **failed**). Select the row to open the same
console.

| Step | What happens |
| --- | --- |
| `connect` | SSH with the pinned host key; checks OS, architecture, sudo and systemd |
| `install` | Creates a single-use enrollment token, uploads the installer, runs it with sudo |
| `await_online` | Waits for the new agent to connect, then assigns cluster and environment and saves the SSH user as a terminal login for the node (audited as `server.logins.update`) |
| `cleanup` | Deletes the credential from the vault (unless you chose to keep it) |

:::note[How the credential is protected]
- It is sealed with envelope encryption (AES-256-GCM, a data key per secret)
  in the credential vault. The API never returns it and it is never logged.
- The job payload holds only the vault id. Each step reads the credential
  and never writes it to job state.
- It is deleted after onboarding, or after one hour at most.
- **Keep the credential for repairs** stores it until you delete it under
  **Administration → Credential vault**.
- After onboarding, Forge no longer uses SSH for management: the node is
  managed through its agent's mTLS certificate.
:::

API equivalent:

```http
POST /api/v1/nodes/probe    {"host": "203.0.113.10", "port": 22}
POST /api/v1/nodes/onboard
{"host": "203.0.113.10", "port": 22, "username": "<user>",
 "host_key_fingerprint": "SHA256:…",
 "auth": {"type": "password", "password": "<password>"},
 "cluster_id": "<cluster-id>", "keep_credential": false,
 "terminal_account": "forge-ops", "terminal_account_sudo": false}
```

Through the API, no terminal account is created unless you send
`terminal_account`.

## Add a Linux node with the installer (manual)

1. Open **Overview** and select **Add node**, then select the
   **Manual (one-line installer)** tab.
2. Pick the **Cluster** and **Environment**. Optionally enter a
   **Hostname (optional)**: it labels the node while it installs.
3. Set **Valid for (minutes)** (1–1440, default 60), then select
   **Generate token**.
4. Choose the terminal account. **Create a terminal account** is on by
   default, with **Account name** `forge-ops` and **Allow sudo** off. Turn
   **Allow sudo** on only when the account needs passwordless `sudo`, or turn
   **Create a terminal account** off to create none. The Linux command on
   the page follows your choice.
5. Copy the Linux command and run it on the machine as a user who can use
   `sudo`:

   ```sh
   printf 'Enrollment token: '; stty -echo; read -r FORGE_ENROLL_TOKEN; stty echo; echo; export FORGE_ENROLL_TOKEN; curl -fsSL 'https://<forge-host>/install.sh?terminal_account=forge-ops' | sudo --preserve-env=FORGE_ENROLL_TOKEN sh; unset FORGE_ENROLL_TOKEN
   ```

6. At the `Enrollment token:` prompt, paste the token (copy it with the
   button next to it on the page) and press Enter. Then enter your `sudo`
   password if asked.

:::note[Why the command asks for the token]
The token is typed at a prompt, so it is not echoed, not in shell history
and not on any command line that other users can see in `ps`. It reaches the
installer through the environment (`sudo --preserve-env`). This needs
`sudo` 1.8.21 or later (Debian 10, Ubuntu 18.04, RHEL 8 and newer). As
`root`, leave out `sudo --preserve-env=FORGE_ENROLL_TOKEN`.
:::

The installer downloads the agent from the control plane, **verifies its
SHA-256** against the value baked into the script, creates the `forge-agent`
user, creates the terminal account you chose, enrolls (the token pins the
control plane's CA) and starts the service. It uses systemd, or a respawn
loop on hosts without systemd (containers).

While the installer runs, it reports each step to the control plane with its
token. **Follow the install** shows those lines in the **Installer console**,
and the cluster's **Nodes** list shows the machine as **installing**.

You can also create the token from the API or the
[`forge` CLI](/docs/reference/cli/). All fields are optional:

```http
POST /api/v1/enrollment-tokens
{"ttl_minutes": 60, "cluster_id": "<cluster-id>", "hostname": "<node-name>", "environment_id": "<environment-id>"}
```

```sh
forge nodes enroll-token --cluster <cluster> --hostname <node-name>
```

The token does not carry the terminal account; the installer URL does. Add
`?terminal_account=<name>`, and `&terminal_account_sudo=1` for `sudo`, to
`/install.sh` (see [Node installer](/docs/reference/cli/node-installer/#the-endpoint)).

## The terminal account

Both onboarding paths create the terminal account the same way, as root,
before the agent enrolls:

- The account has a home directory and no password. It logs in only
  through the browser terminal, with Forge's short-lived certificates for
  this node.
- With **Allow sudo**, `/etc/sudoers.d/forge-<name>` gives it passwordless
  `sudo`. Forge checks the rule with `visudo -c` before it puts it in place.
- A regular user with that name that already exists is kept as it is.
- The agent reports the account when it enrolls, and Forge saves it to the
  node's terminal logins (audited as `server.logins.update`).
- If the account cannot be created, the install goes on without it and
  prints `forge-install: terminal account <name> not created; the install
  continues`. Nothing is saved.
- On a node that is already enrolled, a re-run creates or keeps the account
  but does not save it: it shows as **on node** in **Open terminal**, once
  the agent reports it.
- Assisted onboarding also saves the SSH **Username** you logged in with
  when the node comes online, audited the same way.

Account names are 1 to 32 characters of `a-z`, `0-9`, `_` and `-`, starting
with a letter or `_`, and not a system name such as `root` or `forge-agent`.
Terminal accounts need a Linux node. For the details, and to add an account
to a node later, see
[Create a terminal account on a node](/docs/guides/terminal/#create-a-terminal-account-on-a-node).

:::caution
A terminal account is one more account that can log in on every node you
add with it. It has no password, and only someone with `terminal.open` on
the node gets a certificate for it. With **Allow sudo**, that person is root
on the node. Leave **Allow sudo** off unless the account needs it.
:::

## Add a Windows node with the installer

The Windows installer, `install.ps1`, downloads the agent, checks its
SHA-256, installs it as the `ForgeAgent` service and enrolls the machine. You
run one PowerShell command on the machine. For every step it performs, see
[Node installer: Windows](/docs/reference/cli/node-installer/#windows-installps1).

:::note
Releases do not include a Windows agent yet. Windows nodes work only with an
agent you build from source, and such builds are not verified. Build it with
`cargo build --release -p forge-agent --target x86_64-pc-windows-msvc` on a
Windows machine, copy `target/x86_64-pc-windows-msvc/release/forge-agent.exe`
into `agent_dist_dir` as `forge-agent-windows-x86_64.exe`, add it to
`MANIFEST.json` with `python3 scripts/dev/dist_manifest.py <dir> <version>`,
and restart the control plane.
:::

Before you begin, check the machine:

- Windows 10 version 1809 or later, Windows 11, or Windows Server 2019 or
  later (build 17763 or higher).
- 64-bit Windows on x86_64. ARM64 Windows is not supported.
- Windows PowerShell 5.1 or PowerShell 7, started with **Run as
  administrator**. Use the 64-bit PowerShell, not **Windows PowerShell
  (x86)**.
- The control plane has `forge-agent-windows-x86_64.exe` in
  `agent_dist_dir` (see [Before you begin](#before-you-begin)).
- The control plane's public URL uses `https://`. The installer refuses a
  plain `http://` server unless you allow it, which is for test labs only
  (see [Troubleshooting](#troubleshooting)).

:::caution[The Windows agent runs as LocalSystem]
On Linux the agent runs as an unprivileged user and hands root work to a
separate helper. On Windows the `ForgeAgent` service runs as LocalSystem, so
anyone who takes over the agent process controls the whole machine. Add only
machines where you accept that. This is a deliberate design choice; see
[Open security work](/docs/security/roadmap/#windows-agent-runs-as-localsystem)
for why and what limits the risk.
:::

1. Open **Overview** and select **Add node**, then select the
   **Manual (one-line installer)** tab.
2. Pick the **Cluster** and **Environment**. Optionally enter a
   **Hostname (optional)**: it labels the node while it installs.
3. Set **Valid for (minutes)** (1–1440, default 60), then select
   **Generate token**. The **Terminal access** settings apply to the Linux
   command only; the Windows installer creates no terminal account.
4. Copy the command under **Windows 10/11 or Server 2019+ (PowerShell as
   Administrator)** and run it in PowerShell as Administrator on the
   machine:

   ```powershell
   $t = Read-Host 'Enrollment token' -AsSecureString; $env:FORGE_ENROLL_TOKEN = [Net.NetworkCredential]::new('', $t).Password; Remove-Variable t; irm <forge-url>/install.ps1 | iex
   ```

5. At the `Enrollment token` prompt, paste the token (copy it with the
   button next to it on the page) and press Enter.

The input is masked and stays out of the PowerShell history. The installer
passes the token to the agent through the environment and removes
`FORGE_ENROLL_TOKEN` from the session when it ends, also after an error. It
prints each step as a `$ …` line and reports it to the control
plane, so **Follow the install** shows the same lines in the **Installer
console**. It ends with `forge-install: the ForgeAgent service is running;
logs: C:\ProgramData\OrbynForge\agent\logs`.

The installer changes only the `ForgeAgent` service,
`C:\Program Files\OrbynForge` and `C:\ProgramData\OrbynForge` (enrollment,
logs, WireGuard keys, package settings and file store). It downloads the
agent straight into `C:\Program Files\OrbynForge`, not `%TEMP%`. It makes
Administrators the owner of the data folder, removes inherited permissions
and gives full control to SYSTEM and Administrators only. It adds no
firewall rules (the agent only connects out), and it does not change the
execution policy, `PATH` or other environment variables.

:::caution[An existing data folder must be clean]
Any user can create folders in `C:\ProgramData`. If
`C:\ProgramData\OrbynForge` exists before the first install and holds a link
or another account's files, the installer stops instead of taking the folder
over, because whoever created it could then read the agent's private key or
redirect what the LocalSystem service writes. Find out who created the folder
before you remove it. See
[Node installer: the data folder](/docs/reference/cli/node-installer/#the-data-folder).
:::

### Read the script before you run it

To inspect the installer first, download it, read it, then run it with the
token in the environment:

```powershell
irm <forge-url>/install.ps1 -OutFile install.ps1
notepad .\install.ps1
$t = Read-Host 'Enrollment token' -AsSecureString; $env:FORGE_ENROLL_TOKEN = [Net.NetworkCredential]::new('', $t).Password; Remove-Variable t
powershell -ExecutionPolicy Bypass -File .\install.ps1
Remove-Item Env:FORGE_ENROLL_TOKEN
```

`-ExecutionPolicy Bypass` applies to that one `powershell` process; the
machine's policy does not change. Run this way, the script clears the token
only in its own `powershell` process, so the last line removes it from your
session too.

### Upgrade a Windows node

Run the installer again, as Administrator. On an enrolled machine
(`C:\ProgramData\OrbynForge\agent\identity.json` exists) it downloads and
checks the new agent, stops the service, replaces
`C:\Program Files\OrbynForge\forge-agent.exe` and starts the service again.
It keeps the node's identity, keys and certificate, and does not enroll
again.

A re-run needs no token:

```powershell
irm <forge-url>/install.ps1 | iex
```

Without a token the installer cannot report progress, so only your
PowerShell window shows it. To follow it in the **Installer console**, run
the command from the **Add node** page with a new token instead.

:::note
Windows nodes do not take part in agent self-update or rollouts. The
**Update** button stays disabled for them; upgrade them with the installer
(see [Update agents](/docs/guides/update-agents/)).
:::

### Remove the agent from a Windows node

1. In PowerShell as Administrator, run the installer with `-Uninstall`:

   ```powershell
   irm <forge-url>/install.ps1 -OutFile install.ps1
   powershell -ExecutionPolicy Bypass -File .\install.ps1 -Uninstall
   ```

   This stops and deletes the `ForgeAgent` service and removes
   `C:\Program Files\OrbynForge`. The enrollment stays in
   `C:\ProgramData\OrbynForge\agent`, so installing again later brings
   back the same node.

2. To remove the enrollment, keys, logs and the agent's data as well, add
   `-Purge`:

   ```powershell
   powershell -ExecutionPolicy Bypass -File .\install.ps1 -Uninstall -Purge
   ```

   This deletes `C:\ProgramData\OrbynForge`.

3. In Forge, [remove the node](#remove-a-node). Removing the agent does not
   remove the node from Forge.

When you pipe the script into `iex`, it cannot take parameters. Set
`FORGE_UNINSTALL` (and `FORGE_PURGE`) to `1` instead. The script removes both
variables from the session when it ends:

```powershell
$env:FORGE_UNINSTALL = '1'; $env:FORGE_PURGE = '1'; irm <forge-url>/install.ps1 | iex
```

## Restrict what an agent may do

On a Linux node, edit `/var/lib/forge-agent/agent.toml` (Windows:
`C:\ProgramData\OrbynForge\agent\agent.toml`) and restart the agent
(`sudo systemctl restart forge-agent` on Linux,
`Restart-Service ForgeAgent` on Windows):

```toml
allowed_commands = ["ping", "collect_facts"]
```

The agent refuses anything not listed, even if it is correctly signed.
Re-running the installer or updating Forge keeps your removals; it adds only
command names that are new in that release.

## Revoke a node

Agent certificates last 24 hours and renew on their own after two thirds of
that. To cut a node off for good, open the node and select **Revoke**, type
the node's hostname and, if you have an authenticator app, a fresh
**Authenticator code**, then select **Revoke node**. The agent is refused on
its next request. This needs `server.manage` (**Operator** or higher).

A revoked node stays in Forge with its history. To take it out completely,
[remove it](#remove-a-node).

## Remove a node

Remove a node when you take a machine out of the fleet, or to enroll it
again from scratch. This needs `server.manage` (**Operator** or higher).

1. Open the node and select **Remove**.
2. Type the node's hostname and, if you have an authenticator app, a fresh
   **Authenticator code**.
3. Select **Remove node**.

From the CLI, run `forge nodes remove <node>` (see
[`forge nodes remove`](/docs/reference/cli/nodes/#forge-nodes-remove)).

Forge revokes the node's certificate, then deletes the node and everything it
recorded for it: services, stack parts, deploy slots, file roots and grants,
tunnels, saved logins, SSH and Docker settings, metrics history, alarms, its
DNS name and tags. Queued jobs for the node are cancelled. A stack that used
the node as its default node moves to another of its nodes. A stack that ran
only on this node is deleted.

:::caution
Removing cannot be undone. Forge does not touch the machine: containers and
files on it stay until you remove them, for example with the
[Windows uninstall steps](#remove-the-agent-from-a-windows-node). The audit log keeps the `server.remove`
record.
:::

To bring the machine back, enroll it again with a new token. It joins as a
fresh node, with the same host name and DNS name and no history.

The control node cannot be removed.

## Verify

- The node appears in its cluster's **Nodes** list with status **online**,
  usually within seconds of the installer finishing.
- Open the node and select **Ping**. The job finishes with `succeeded`.
- On a Linux node, `systemctl status forge-agent` shows the service as
  active.
- On a Windows node, `Get-Service ForgeAgent` shows `Running`, and
  `C:\ProgramData\OrbynForge\agent\logs\forge-agent.log` has an
  `agent starting` line.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| Red `error:` or `installer failed (exit N)` line in the console | A command on the machine failed | Read the `$ …` command above the error and its output; fix the cause on the machine, then start again |
| **failed** row under **Nodes** | The last onboarding attempt failed | Select the row: the console and error stay available for a day |
| **Installer console** stays empty | The command has not run yet, or the machine cannot reach the control plane | Run the command; check that the machine reaches the control plane's URL |
| "No agent builds are published on this control plane yet" | `agent_dist_dir` has no agent builds | Add the builds and `SHA256SUMS` (see [Before you begin](#before-you-begin)) |
| **MFA required** on the **Assisted (SSH login)** tab | Your session is not MFA-verified | Enrol MFA, sign out and sign in with your code, or use the manual installer |
| `forge-install: terminal account <name> not created; the install continues` | The name is a system user or an existing group on the machine, `sudo` is missing while **Allow sudo** is on, or the helper build is missing | Read the `forge-agent-helper terminal-account:` line above it. After the install, create the account as in [Create a terminal account on a node](/docs/guides/terminal/#create-a-terminal-account-on-a-node) |
| `400` `terminal account <name> is not allowed` | The name breaks the naming rules or is a system name | Use 1–32 of `a-z`, `0-9`, `_`, `-`, starting with a letter or `_` |
| Node installs but never comes **online** | TCP `3843` is blocked between node and control plane, or the node cannot reach the address you copied the command from | Open the agent port; copy the command again from an address the node reaches (see [Which address nodes use](#which-address-nodes-use)) |
| Node goes offline with `server key … is not pinned` in its log | The control plane's key changed while the node missed the announcement | Run `forge-server agent-key show` on the server and `forge-agent pins add <fingerprint>` on the node. See [The key mismatch error](/docs/reference/cli/forge-agent/#the-key-mismatch-error) |
| Windows: `forge-install: error: run PowerShell as Administrator` | PowerShell is not elevated | Start PowerShell with **Run as administrator** and run the command again |
| Windows: `this installer needs 64-bit Windows on x86_64` | 32-bit Windows, ARM64 Windows, or the 32-bit **Windows PowerShell (x86)** | Use the 64-bit PowerShell on 64-bit x86_64 Windows. ARM64 is not supported |
| Windows: `Windows 10 1809 or Windows Server 2019 or later is needed (this is build <n>)` | The Windows build is older than 17763 | Upgrade Windows; older versions are not supported |
| Windows: `set $env:FORGE_ENROLL_TOKEN to the token shown in Orbyn Forge` | No token in the session and the machine is not enrolled | Run the command from the **Add node** page and paste the token at the prompt |
| Windows: `this Forge server has no Windows agent build` | `forge-agent-windows-x86_64.exe` is missing from `agent_dist_dir`. Releases do not include it | Build it from source, add it and restart the control plane |
| Windows: `refusing a plain http:// server; set $env:FORGE_ALLOW_HTTP=1 only for a test lab` | The control plane's public URL is `http://` | Give the control plane an `https://` URL. In a test lab only, run `$env:FORGE_ALLOW_HTTP = '1'` first (it must be exactly `1`), then remove it with `Remove-Item Env:FORGE_ALLOW_HTTP`; the script does not clear it |
| Windows: `checksum mismatch for forge-agent.exe (expected …, got …)` | The download differs from the checksum baked into the script. The control plane hashes the builds when it starts, so a build replaced in `agent_dist_dir` while it runs causes this; so does a proxy that rewrites downloads | Restart the control plane after you replace a build, then run the command again; if it persists, check for a proxy |
| Windows: `<path> is a link (reparse point); this installer did not create it. Inspect and remove <folder>, then run the installer again` | `C:\ProgramData\OrbynForge` existed before the install, and it or an item in it is a junction, symbolic link or mount point | Find out who created the folder, remove it, then run the command again |
| Windows: `<path> is owned by <SID>, not SYSTEM or Administrators; this installer did not create it. …` | `C:\ProgramData\OrbynForge` existed before the install, and another account owns it or an item in it | Find out which account `<SID>` is and why it created the folder, remove the folder, then run the command again |
| Windows: either message above, ending in `created while the installer ran. …` | A link or another account's item appeared in the data folder while the installer locked it | Find the program that created it before you trust the machine; then remove the folder and run the command again |
| Windows: `could not take ownership of <folder>` or `could not restrict access to <folder>` | `icacls /setowner` failed on `C:\ProgramData\OrbynForge`, or the folder was still not locked after `Set-Acl`, for example because a program holds it open | Close programs that use the folder, then run the command again |
| Windows: `Error: could not restrict access to <dir> (icacls …): …; run as Administrator` above `enrollment failed` or `could not install or start the ForgeAgent service` | The agent could not lock its data folder | Run PowerShell as Administrator; see [Windows data folder](/docs/reference/cli/forge-agent/#windows-data-folder) |
| Windows: `enrollment failed (is the token still valid?)` | The token is used, expired or mistyped, or the machine cannot reach the agent port | Generate a new token and run again; check that TCP `3843` to the control plane is open. The output line above the error has the agent's reason |
| Windows: `could not install or start the ForgeAgent service`, or `Get-Service ForgeAgent` shows `Stopped` | The agent stops at start-up, for example because it cannot load its state | Read `C:\ProgramData\OrbynForge\agent\logs\forge-agent.log`; fix the cause, then run `Start-Service ForgeAgent` or the installer again |

## Next steps

- [Update agents](/docs/guides/update-agents/) to bring nodes to the control plane's version.
- [Node installer](/docs/reference/cli/node-installer/) for every step, file and message of `install.sh` and `install.ps1`.
- [Node overview, storage and connectivity](/docs/guides/node-monitoring/)
- [Tag nodes](/docs/guides/tags/) to find and group them.
- [Configuration reference](/docs/reference/configuration/) for `agent_dist_dir`, `agent_hostnames` and `node_public_url`.
