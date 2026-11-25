---
title: forge-agent-helper
description: Every forge-agent-helper mode, its arguments, the one-shot request format, the files it keeps and its exit codes.
sidebar:
  order: 92
---

`forge-agent-helper` is the agent's root helper on Linux. It runs the
privileged work that the unprivileged agent may not do itself, and only the
work that Forge signed. This page lists every mode, argument, file and exit
code. For why the helper exists and how it decides what to trust, see
[Privileged helper](/docs/concepts/agents/#privileged-helper).

:::note
You normally never run the helper by hand. The installer, systemd and sshd
run it. The exceptions are [`pin`](#forge-agent-helper-pin), after you re-enroll a node by
hand, [`terminal-account`](#forge-agent-helper-terminal-account), to add a
terminal account yourself, and the read-only
[`ssh-principals <user>`](#forge-agent-helper-ssh-principals) check.
:::

The helper has no `--help` or `--version` flag. It reads its first argument:

| Mode | Purpose | Run by | Runs as |
| --- | --- | --- | --- |
| [`serve`](#forge-agent-helper-serve) | Serves signed requests from the agent on a Unix socket | systemd (`forge-agent-helper.service`) | root |
| [`pin`](#forge-agent-helper-pin) | Pins Forge's command key and this node's id | The installer, after a fresh enrollment | root |
| [`terminal-account <NAME> [--sudo]`](#forge-agent-helper-terminal-account) | Creates or keeps a terminal account for the browser terminal | The installer, before it enrolls | root |
| [`ssh-principals <user>`](#forge-agent-helper-ssh-principals) | Prints the one SSH principal this node accepts for `<user>` | sshd (`AuthorizedPrincipalsCommand`) | `nobody` |
| [`supervise <spec>`](#forge-agent-helper-supervise) | Keeps a native app running where there is no systemd | The helper itself | root |
| [`file-worker <uid> <gid> <base> <store>`](#forge-agent-helper-file-worker) | Runs one file explorer operation on a native app's folder | The helper itself | the app's account |
| [`health-worker <uid> <gid> <base> <timeout-secs>`](#forge-agent-helper-health-worker) | Runs one command health check of a native app, confined with Landlock, in a cgroup of its own | The helper itself | the app's account |
| [one-shot](#one-shot-request) (no mode) | Runs one JSON request read from standard input | Nobody in normal operation | the caller |

## Platforms

| | Linux | Windows |
| --- | --- | --- |
| Installed | Yes, as `/usr/local/bin/forge-agent-helper` | No. The agent runs as LocalSystem and does privileged work in its own process |
| `serve` | Yes | Not supported: prints `forge-agent-helper: serve is not supported on this OS yet` and exits `2` |
| `supervise`, `pin`, `terminal-account` | Yes | Not supported |
| `file-worker` | Yes | Not supported |
| `health-worker` | Yes, on kernels that enforce Landlock ABI 3 or later (6.2 or later, Landlock enabled), on nodes with cgroup v2 and its `pids` and `memory` controllers. On an older kernel, for example RHEL 8, it starts no command. On a node with cgroup v1 or a hybrid layout, the helper does not start it | Not supported |

The helper supports the same Linux distributions as
[`forge-agent`](/docs/reference/cli/forge-agent/#platforms).

## Files

| Path | Contents | Owner, mode |
| --- | --- | --- |
| `/var/lib/forge-agent-helper/` | The helper's own folder. The agent user cannot read it | root, `0700` |
| `/var/lib/forge-agent-helper/pin.json` | The pin: `server_id`, `command_public_key` and `source` (`installer` or `first-use`) | root |
| `/var/lib/forge-agent-helper/seen.json` | Recent operation ids, so a replayed command is refused after a restart too. Rewritten every 1024 commands | root |
| `/var/lib/forge-agent-helper/seen.log` | Operation ids admitted since the last rewrite of `seen.json`, one per line | root |
| `/var/lib/forge-agent-helper/update-*.json` | Self-update markers, used for automatic rollback | root |
| `/var/lib/forge-agent-helper/health/<environment>--<app>.json` | The enabled command health checks of one app, their policy revision, when Forge issued the policy and when the helper last received it, as Forge signed them. Written by a signed policy push or deploy, also for a policy without command checks, so an older signed policy can't bring checks back. Removed when the app is removed | root |
| `/sys/fs/cgroup/forge-health/` | One cgroup per running command check of a native app, `<environment>--<app>--<purpose>--<check>`, with `pids.max` 64 and `memory.max` 256 MiB. Created for the run and removed after it | root |
| `/run/forge-agent-helper.sock` | The socket `serve` listens on | root, group of the agent user, `0660` |
| `/etc/ssh/forge_node_id` | This node's id, read by `ssh-principals` | root, `0644` |
| `/var/lib/forge-agent/identity.json` | The agent's identity. `pin` reads it | `forge-agent`, `0600` |

## Exit codes

| Code | Meaning |
| --- | --- |
| `0` | Success. For `ssh-principals`, always `0`, even when it prints nothing |
| `1` | The mode failed (`pin`, `terminal-account`, `serve`, `supervise`), `file-worker` or `health-worker` could not write its answer, or a one-shot request ran and failed (`"ok": false`) |
| `2` | A one-shot request was rejected or refused before it ran, no request arrived, `supervise` has no spec, or `serve` runs on an unsupported OS |

---

## forge-agent-helper serve

Listens on `/run/forge-agent-helper.sock` and runs the agent's requests. At
start it loads the pin, sets up node-bound SSH certificates when the browser
terminal is enabled on the node, and starts the self-update rollback
watchdog.

**Who runs it:** systemd, as root, through `forge-agent-helper.service`. On
hosts without systemd, the installer starts it in a respawn loop that logs to
`/var/log/forge-agent-helper.log`.

### Synopsis

```
forge-agent-helper serve [--agent-dir <DIR>]
```

### Arguments

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--agent-dir <DIR>` | path | `/var/lib/forge-agent` | No | The agent's state directory. Its owner is the only non-root user allowed on the socket. `pin` on first use reads `identity.json` from it | `--agent-dir /var/lib/forge-agent` |

### What it accepts

- **Callers:** root, and the user that owns `--agent-dir` (the `forge-agent`
  user). It checks the caller's uid on each connection. Others get
  `caller is not the forge-agent user`.
- **Requests:** one JSON line per connection, at most 4 MiB.
- **Trust:** with a pin, only commands signed by Forge for this node, within
  their lifetime and not replayed. The one exception is the agent confirming
  that it came back after a self-update. Without a pin, the helper copies one
  from the agent's `identity.json` the first time it starts (`first-use`).
- **Cancel:** when the agent closes the connection, the helper stops the
  running work and its process group.

While a request runs, the helper streams `{"progress":"<line>"}` lines,
followed by one [response](#response) line.

### The systemd unit

The installer writes `/etc/systemd/system/forge-agent-helper.service`:

```ini
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

### Examples

Check the service:

```sh
systemctl status forge-agent-helper
```

Read its startup lines:

```sh
journalctl -u forge-agent-helper -n 20 --no-pager
```

Restart it, for example after [`pin`](#forge-agent-helper-pin):

```sh
sudo systemctl restart forge-agent-helper
```

### Sample output

Real output from a Linux node (`journalctl -u forge-agent-helper`), with the
host and id replaced:

```text
<timestamp> <node-name> systemd[1]: Started forge-agent-helper.service - Orbyn Forge agent helper (root; fixed allowlist and catalog recipes).
<timestamp> <node-name> forge-agent-helper[15845]: forge-agent-helper: verifying commands for server <server-id> (pin: first-use)
<timestamp> <node-name> forge-agent-helper[15845]: forge-agent-helper: serving on /run/forge-agent-helper.sock for uid 988
```

Other lines it logs, from the code:

| Line | Meaning |
| --- | --- |
| `pinned Forge's command key on first use (server <server-id>)` | No pin existed; the helper pinned the agent's current identity |
| `no pin yet (<reason>); requests are not verified` | No pin, and `identity.json` was not readable. Enroll the node |
| `no pin; requests are not verified (enroll the node)` | The helper runs without a pin |
| `sshd now accepts only certificates for this node` | The helper added `AuthorizedPrincipalsCommand` and reloaded sshd |
| `node-bound SSH certificates not set up: <reason>` | sshd rejected the change; the helper left sshd as it was |
| `the agent cancelled the operation` | The agent closed the connection; the work was stopped |
| `cannot save recent operation ids: <reason>` | `seen.json` could not be rewritten; ids keep going to `seen.log` and the rewrite is tried again |
| `cannot record an operation id: <reason>` | `seen.log` could not be written, for example because the disk is full |

---

## forge-agent-helper pin

Pins Forge's command key and this node's id from the agent's
`identity.json` into `/var/lib/forge-agent-helper/pin.json`, with
`source: installer`. It replaces an existing pin.

It also adopts the control-plane key pins of that enrollment. The installer
enrolls as the `forge-agent` user, which cannot write the root-owned
`/var/lib/forge-agent-pins/pins.toml`, so enrollment stages the set in
`/var/lib/forge-agent/pins.enrolled.toml`. `pin` checks the staged file (at
most 8 keys, valid fingerprints), makes it the pin file when none exists yet
and removes it. It never replaces an existing pin file from the staged one.

**Who runs it:** the installer, as root, right after a fresh enrollment. You
run it as root after you re-enroll a node by hand (see
[`forge-agent enroll`](/docs/reference/cli/forge-agent/#forge-agent-enroll)).

### Synopsis

```
forge-agent-helper pin [--agent-dir <DIR>]
```

### Arguments

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--agent-dir <DIR>` | path | `/var/lib/forge-agent` | No | The agent's state directory to read `identity.json` from | `--agent-dir /var/lib/forge-agent` |

### Examples

Pin after a manual re-enrollment, then restart the helper so the running
service loads the new pin:

```sh
sudo /usr/local/bin/forge-agent-helper pin --agent-dir /var/lib/forge-agent
sudo systemctl restart forge-agent-helper forge-agent
```

Check what is pinned:

```sh
sudo cat /var/lib/forge-agent-helper/pin.json
```

### Sample output

Reconstructed from the code (`forge-agent-helper.rs`, `helper_trust.rs`).
Success (exit code `0`):

```text
pinned Forge's command key for server <server-id>
```

The node is not enrolled (exit code `1`):

```text
forge-agent-helper pin: /var/lib/forge-agent/identity.json: not readable (is the node enrolled?)
```

:::caution
`pin` trusts whatever `identity.json` holds when you run it. Run it only right
after an enrollment you started yourself. A pin made on first use trusts a
file the agent user owns; if you doubt a node, revoke it and enroll it again.
See [Privileged helper](/docs/concepts/agents/#privileged-helper).
:::

---

## forge-agent-helper terminal-account

Creates a terminal account for the browser terminal, or keeps it when a
regular user of that name exists, and sets its passwordless `sudo` rule as
asked. It is safe to run again.

**Who runs it:** the node installer, as root, before it enrolls the node,
when the install URL has `?terminal_account=` (see
[Node installer](/docs/reference/cli/node-installer/#steps)). You can run it as
root to add an account yourself; Forge then lists it as **on node** in
**Open terminal** once the agent reports its facts. Through Forge, the same
work runs as the signed command kind `terminal_account`
(`POST /api/v1/servers/{id}/terminal-account`, `server.command` with MFA).

### Synopsis

```
forge-agent-helper terminal-account <NAME> [--sudo]
```

### Arguments

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<NAME>` | string | | Yes | The account. 1–32 characters of `a-z`, `0-9`, `_` and `-`, starting with a letter or `_`. Reserved system names (`root`, `nobody`, `forge-agent`, `daemon`, `www-data`, `sshd`, `admin`, `sudo`, `wheel`, `docker` and others) and names starting with `systemd-` are refused. So is a user with a uid below 1000 (or `65534`), and an existing group of that name | `forge-ops` |
| `--sudo` | flag | off | No | Writes `/etc/sudoers.d/forge-<NAME>` with `<NAME> ALL=(ALL:ALL) NOPASSWD: ALL`. Without it, the helper removes that file when it starts with Forge's marker line | `--sudo` |

### What it does

1. Checks the name, `/etc/passwd` and `/etc/group`.
2. When the user does not exist: runs
   `useradd --create-home --shell /bin/bash --comment "Orbyn Forge terminal account" --user-group <NAME>`
   (`/bin/sh` when `/bin/bash` is missing), then `usermod -p '*' <NAME>`.
   `*` means no password, not a locked account, so certificate logins work
   also where sshd does not use PAM.
3. With `--sudo`: writes the rule to a draft file `/etc/sudoers.d/.forge-<NAME>.new`
   (`0440`; sudo ignores names with a dot), checks it with
   `visudo -c -q -f`, then renames it to `/etc/sudoers.d/forge-<NAME>`. When
   `visudo` rejects it, the draft is removed and nothing changes.
4. Without `--sudo`: removes `/etc/sudoers.d/forge-<NAME>` when its first
   line is `# Managed by Orbyn Forge (terminal account)`. A file without that
   line is left alone.

The drop-in it writes:

```text
# Managed by Orbyn Forge (terminal account)
forge-ops ALL=(ALL:ALL) NOPASSWD: ALL
```

### Examples

Create `forge-ops` without `sudo`:

```sh
sudo /usr/local/bin/forge-agent-helper terminal-account forge-ops
```

Create it, or keep it, with passwordless `sudo`:

```sh
sudo /usr/local/bin/forge-agent-helper terminal-account forge-ops --sudo
```

Take `sudo` away again:

```sh
sudo /usr/local/bin/forge-agent-helper terminal-account forge-ops
```

### Sample output

Reconstructed from the code (`forge-agent-helper.rs`, `terminal_accounts.rs`).
A new account (exit code `0`):

```text
terminal account forge-ops: created, passwordless sudo
```

An existing account, without `sudo` (exit code `0`):

```text
terminal account forge-ops: kept, no sudo
```

Errors (exit code `1`):

```text
forge-agent-helper terminal-account: run as root
forge-agent-helper terminal-account: `Ops` is not allowed: use 1–32 of a-z, 0-9, `_` and `-`, starting with a letter or `_`, and not a system name
forge-agent-helper terminal-account: `backup` is not allowed: use 1–32 of a-z, 0-9, `_` and `-`, starting with a letter or `_`, and not a system name
forge-agent-helper terminal-account: `staff` is already a group on this node; choose another name
forge-agent-helper terminal-account: sudo is not installed on this node (/etc/sudoers.d is missing)
forge-agent-helper terminal-account: the sudo rule was rejected, nothing changed: visudo failed: <reason>
```

### Exit codes

| Code | Meaning |
| --- | --- |
| `0` | The account exists and its sudo rule is as asked |
| `1` | Not run as root, the name was refused, or `useradd`, `usermod`, writing the sudo rule or `visudo` failed. The reason is on standard error |

:::caution
With `--sudo`, anyone who can open a browser terminal as this account is
root on the node. The account has no password and accepts only Forge's
node-bound certificates, issued to people with `terminal.open`. When
`--sudo` fails after the user was created (for example, no `sudo` on the
node), the user stays, without `sudo`.
:::

---

## forge-agent-helper ssh-principals

Prints the one SSH certificate principal this node accepts for `<user>`:
`forge.<server-id>.<user>`. sshd calls it for each certificate login, so a
browser-terminal certificate for one node does not open another.

**Who runs it:** sshd, as `nobody`, through these lines in
`/etc/ssh/sshd_config.d/50-forge.conf` (or in `/etc/ssh/sshd_config`):

```text
AuthorizedPrincipalsCommand /usr/local/bin/forge-agent-helper ssh-principals %u
AuthorizedPrincipalsCommandUser nobody
```

You can run it to check the setup. It reads only `/etc/ssh/forge_node_id`
and changes nothing.

### Synopsis

```
forge-agent-helper ssh-principals <user>
```

### Arguments

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<user>` | string | | Yes | The login sshd checks. 1–32 characters from `A-Z a-z 0-9 _ . -`, not starting with `-` | `root` |

It prints nothing when `<user>` is not valid, or when `/etc/ssh/forge_node_id`
is missing or does not hold a valid id. sshd then accepts no Forge
certificate for that login.

### Examples

Run it the way sshd does:

```sh
sudo runuser -u nobody -- /usr/local/bin/forge-agent-helper ssh-principals root
```

Compare the result with the node id the agent reports:

```sh
cat /etc/ssh/forge_node_id
sudo /usr/local/bin/forge-agent --state-dir /var/lib/forge-agent status
```

### Sample output

Real output from a Linux node, with the id replaced (exit code `0`):

```text
forge.<server-id>.root
```

An invalid login prints nothing (real, exit code `0`).

See [Terminal](/docs/guides/terminal/#how-it-stays-safe) for how certificates are
bound to a node, and
[What the installer changes on a Linux node](/docs/guides/terminal/#what-the-installer-changes-on-a-linux-node) for
the sshd setup.

---

## forge-agent-helper supervise

Keeps a native app running on a node without systemd, and restarts it 3 s
after it exits when its spec asks for restarts. Each exit is appended to the
app's log as `forge: process exited with status <code>`.

**Who runs it:** the helper itself, as root, in its own process group, once
per app. It writes the supervisor's pid to `/run/forge/apps/<app-key>.pid`.
Nodes with systemd run apps as systemd units instead.

### Synopsis

```
forge-agent-helper supervise <spec>
```

### Arguments

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<spec>` | path | | Yes | The app's JSON spec, written by the helper | `/etc/forge/apps/<environment>/<app>.json` |

Without `<spec>` it exits `2` and prints nothing. A spec that cannot be read
or is not valid exits `1` with `forge-agent-helper supervise: <reason>`.

### Example

List running supervisors:

```sh
pgrep -af 'forge-agent-helper supervise'
```

---

## forge-agent-helper file-worker

Runs one file explorer operation on a native app's folder as that app's own
account, then exits. The helper uses it for every operation on an
[app root](/docs/concepts/file-explorer/#app-roots), so the work never runs as
root.

**Who runs it:** the helper itself, once per operation, with the app's uid
and gid, an empty environment and `/` as its working directory. You never
run it by hand: it reads a request that only the helper prepares, and the
helper ignores anything it says beyond one bounded answer.

### Synopsis

```
forge-agent-helper file-worker <uid> <gid> <base> <store>
```

### Arguments

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<uid>` | integer | | Yes | The app account's user id. `0` is refused | `998` |
| `<gid>` | integer | | Yes | The app account's primary group id. `0` is refused | `997` |
| `<base>` | path | | Yes | The app's folder, the only folder the operation may touch | `/srv/forge/apps/<environment>/<app>` |
| `<store>` | path | | Yes | The app's trash and revision store | `/var/lib/forge-app-store/<environment>/<app>` |
| standard input | JSON | | Yes | One file operation, at most 16 MiB. Its root must be `{"kind":"dir","base":"<base>"}`; an owner change is refused | |

The helper finds `<uid>` and `<gid>` itself from the app's account on the
node, never from the request. See
[How an app root runs](/docs/concepts/file-explorer/#how-an-app-root-runs).

### What it checks

Before it reads the request, the worker:

1. Reads `/proc/self/status` and refuses to run unless the real, effective,
   saved and filesystem user ids are all `<uid>`, the four group ids are all
   `<gid>`, and it holds no supplementary group other than `<gid>`.
2. Lowers its resource limits: no core dumps, 512 open files, 660 s of CPU
   time and files of at most 2 GiB.

Then it reads the request and refuses it when its root is not `<base>` or
when it asks to change an owner. The operation runs with the same path rules
as any other root, and the kernel's permissions for the app's account apply
on top.

The helper treats the worker as untrusted. It reads at most 48 MiB of
answer, accepts only the exact answer format below, stops the worker after
120 s (600 s for a search), runs at most 8 workers at once on a node, and
never acts as root on anything the worker returns.

### Output

One JSON line on standard output. Reconstructed from the code
(`app_worker.rs`). Success, here for a listing of an empty `/public`:

```json
{"ok":true,"data":{"path":"/public","offset":0,"total":2,"truncated":false,"skipped_names":0,"entries":[]}}
```

Failure:

```json
{"ok":false,"error":"refused: the worker does not run as the app's account"}
```

`data` is the same result as the operation returns on any other root. The
helper adds `as_reported`, the account it started the worker as, before it
passes `data` on. Errors the worker can print:

| Error | Meaning |
| --- | --- |
| `usage: file-worker <uid> <gid> <base> <store>` | Not exactly four arguments |
| `bad uid`, `bad gid` | `<uid>` or `<gid>` is not a number |
| `refused: a worker never runs as root` | `<uid>` or `<gid>` is `0` |
| `refused: the worker does not run as the app's account` | One of its user or group ids is not the app's |
| `refused: the worker holds other groups` | It holds a supplementary group |
| `the request is too large` | Standard input is over 16 MiB |
| `invalid request: <reason>` | Standard input is not a valid file operation |
| `refused: not the app's folder` | The request's root is not `<base>`, or it changes an owner |

### Exit codes

| Code | Meaning |
| --- | --- |
| `0` | The worker printed its answer, whether `ok` is `true` or `false` |
| `1` | The worker could not write its answer |

### Example

See running workers, for example while a long search runs:

```sh
pgrep -af 'forge-agent-helper file-worker'
```

---

## forge-agent-helper health-worker

Runs one command health check (an `exec` or `shell` probe) of a native app
as that app's own account, confined with Landlock, then exits. The helper
uses it for every command check of a **Service** service, so the
command never runs as root, as the agent's user or as the helper's user.
Checks of container apps do not use it; the helper runs those in the
container through the Docker Engine API. See
[How command checks run](/docs/reference/health-policy/#how-command-checks-run).

**Who runs it:** the helper itself, once per run of a check, with the app's
uid and gid, no supplementary groups, an empty environment and `/` as its
working directory. You never run it by hand: it reads a command that only
the helper prepares, from a policy Forge signed. The agent names the check
to run; it cannot supply the command.

### How the helper contains it

Before it starts anything as the app's account, the helper:

1. Creates a cgroup v2 for the run,
   `/sys/fs/cgroup/forge-health/<environment>--<app>--<purpose>--<check>`,
   with these limits:

   | File | Value | Meaning |
   | --- | --- | --- |
   | `pids.max` | `64` | Processes and threads, the worker included |
   | `memory.max` | 256 MiB | Memory of the whole run |
   | `memory.swap.max` | `0` | No swap, where the kernel has the file |
   | `memory.oom.group` | `1` | An out-of-memory kill takes the whole run, where the kernel has the file |

   A cgroup left by a run that was cut short is emptied and used again.
2. Starts the worker and moves it into the cgroup.
3. Only then writes the command to the worker's standard input. The worker
   starts nothing before it has read its command.

When the run ends, when its timeout passes, or when the agent closes the
connection, the helper writes `cgroup.kill`, so the kernel kills everything
in the cgroup, and removes the cgroup. A process that started a new
session, a new process group or a daemon is still in the cgroup.

A node without cgroup v2, or without the `pids` and `memory` controllers
(cgroup v1 and hybrid layouts), runs no native command check: the helper
does not start the worker and refuses the check with `not_contained`.

### Synopsis

```
forge-agent-helper health-worker <uid> <gid> <base> <timeout-secs>
```

### Arguments

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<uid>` | integer | | Yes | The app account's user id. `0` is refused | `998` |
| `<gid>` | integer | | Yes | The app account's primary group id. `0` is refused | `997` |
| `<base>` | path | | Yes | The app's folder. The command starts in it and may read and execute beneath it | `/srv/forge/apps/<environment>/<app>` |
| `<timeout-secs>` | integer, seconds | | Yes | The check's `timeout_secs`. Sets the worker's CPU time limit to this plus 5 s. The helper enforces the timeout itself | `2` |
| standard input | JSON | | Yes | One command, at most 512 KiB: `{"argv": [...], "stdin": "<script>" or null, "account": "<app account>"}`. Unknown fields are refused | |

The helper finds `<uid>` and `<gid>` itself from the app's account on the
node, never from the request.

### What it does

Before it reads the command, the worker:

1. Reads `/proc/self/status` and refuses to run unless the real, effective,
   saved and filesystem user ids are all `<uid>`, the four group ids are all
   `<gid>`, and it holds no supplementary group other than `<gid>`.
2. Lowers its resource limits: no core dumps, 256 open files,
   `<timeout-secs>` + 5 s of CPU time and files of at most 16 MiB.
3. Confines itself with Landlock. The rules bind the worker and every
   process it starts:

   | Access | Where |
   | --- | --- |
   | Read and execute | Beneath `/usr`, `/bin`, `/sbin`, `/lib`, `/lib32`, `/lib64`, `/etc`, `/opt`, `/proc`, `/sys` and `<base>` |
   | Read | `/dev/zero`, `/dev/random`, `/dev/urandom` |
   | Write | `/dev/null` only |
   | Create, write, append to, truncate, rename or delete a file | Nowhere |

   Every filesystem right through Landlock ABI 3, truncation included, is a
   hard requirement: a kernel without them is an error, never a weaker run.
   Landlock also sets `no_new_privs`, so setuid programs gain nothing.
   Where the kernel has them, the worker adds control of device ioctls
   (ABI 5) and stops signals and abstract UNIX sockets at its own processes
   (ABI 6).

   Landlock does not cover everything. The command can still change the
   mode and timestamps of files the app's account owns, connect to UNIX
   sockets by path, and use the network: TCP is not restricted.
4. Stops unless the kernel's Landlock ABI is 3 or more and the rules are
   enforced (`confined` is `full` or `partial`). It starts no command and
   answers `not_confined`. Release builds have no way to run a command
   unconfined.

Then it reads the command and starts `argv` directly, never through a shell,
with `<base>` as the working directory and this environment only:

| Variable | Value |
| --- | --- |
| `PATH` | `/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin` |
| `HOME` | `<base>` |
| `USER`, `LOGNAME` | The app's account |
| `LANG` | `C.UTF-8` |

For a `shell` probe, `argv` is the interpreter alone (`sh` or `bash`) and
`stdin` holds the script, so the script text is not in the node's process
list. For an `exec` probe, standard input is empty. The worker reads the
command's standard output and standard error to the end and keeps the last
512 bytes.

The helper treats the worker as untrusted. It reads at most 16 KiB of
answer, accepts only the answer format below, and does not believe a result
from a worker whose `abi` is below 3 or whose `confined` is not `full` or
`partial`. It kills the run's cgroup at the timeout and after a normal end.
A command that exits but leaves a process holding its output open has not
finished: the worker keeps reading, and the run ends at the timeout.

Limits the helper keeps:

| Limit | Value |
| --- | --- |
| Command checks running at once on a node | 4 |
| Command checks of one app running at once | 2 |
| Runs of one check at once | 1 |
| Shortest time between two starts of one check | 0.9 s |
| Wait for a free place | Up to 2 s, then the check is refused with `throttled` |
| Age of the stored policy | A check does not run from a policy the helper has not received for 30 minutes (`policy_expired`) |

### Output

One JSON line on standard output. Reconstructed from the code
(`health_exec.rs`). A command that exited with code `1`; `output` is the end
of its output as a list of bytes, here `down` and a newline:

```json
{"confined":"full","abi":6,"outcome":{"kind":"exited","code":1,"output":[100,111,119,110,10]}}
```

A kernel that does not enforce Landlock ABI 3, here one with ABI 1:

```json
{"confined":"error: <reason>","abi":1,"outcome":{"kind":"not_confined"}}
```

A refusal:

```json
{"confined":"not attempted","abi":0,"outcome":{"kind":"refused","reason":"refused: the worker does not run as the app's account"}}
```

| Field | Values |
| --- | --- |
| `confined` | `full`, `partial` (the kernel enforces some of the rules), `none`, `error: <reason>`, or `not attempted` when the worker stopped earlier |
| `abi` | integer. The kernel's Landlock ABI; `0` when it has none, or when the worker stopped before it asked |
| `outcome.kind` | `exited` (`code`, `output`), `signaled` (`signal`), `start_failed` (`detail`), `not_confined`, or `refused` (`reason`) |

The helper turns the answer into the check's result. By default no output
leaves the helper: a failed run reports its exit code only. When the check
has `show_output: true`, the last 160 bytes of a failed run's output leave
the helper, as printable ASCII. A passing run's output is always dropped.

Reasons a `refused` outcome can carry:

| Reason | Meaning |
| --- | --- |
| `usage: health-worker <uid> <gid> <base> <timeout-secs>` | Not exactly four arguments |
| `bad uid`, `bad gid`, `bad timeout` | The argument is not a number |
| `refused: a worker never runs as root` | `<uid>` or `<gid>` is `0` |
| `refused: the worker does not run as the app's account` | One of its user or group ids is not the app's |
| `refused: the worker holds other groups` | It holds a supplementary group |
| `the command is too large` | Standard input is over 512 KiB |
| `invalid command: <reason>` | Standard input is not a valid command |

### Exit codes

| Code | Meaning |
| --- | --- |
| `0` | The worker printed its answer, whatever the outcome |
| `1` | The worker could not write its answer |

### Example

See running workers:

```sh
pgrep -af 'forge-agent-helper health-worker'
```

Check that the node can run native command checks:

```sh
uname -r
cat /sys/kernel/security/lsm
cat /sys/fs/cgroup/cgroup.controllers
```

The kernel must be 6.2 or later, the first list must contain `landlock`,
and the second must contain `pids` and `memory`. On a node without
cgroup v2 the last file does not exist.

See the cgroups of running checks:

```sh
ls /sys/fs/cgroup/forge-health/
```

---

## One-shot request

With no mode, the helper reads one JSON request line from standard input,
runs it and prints one [response](#response) line. It runs with the caller's
own rights.

**Who runs it:** nobody in normal operation. The agent talks to
[`serve`](#forge-agent-helper-serve) instead.

### Synopsis

```
forge-agent-helper [--agent-dir <DIR>] < request.json
```

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--agent-dir <DIR>` | path | `/var/lib/forge-agent` | No | Accepted for symmetry with `serve`; a one-shot run never pins | `--agent-dir /var/lib/forge-agent` |
| standard input | JSON, one line | | Yes | The request, at most 4 MiB | `{"op":"service_status","name":"ssh"}` |

### Refused once pinned

When the helper can read a pin (as root, on any node enrolled with the
installer), it refuses every request that is not a command signed by Forge.
Forge's signed commands reach the helper only through the agent, so a
one-shot request typed by hand is always refused on a pinned node.

Run as any other user, the helper cannot read its root-only folder, so it
finds no pin and runs the request with that user's own rights. That gives the
caller nothing they could not already do.

Real output from a pinned Linux node, run as root (exit code `2`):

```sh
echo '{"op":"service_status","name":"ssh"}' | sudo /usr/local/bin/forge-agent-helper
```

```json
{"ok":false,"error":"refused: privileged requests must be commands signed by Forge (D-068)"}
```

A request that is not valid JSON (real output, exit code `2`):

```json
{"ok":false,"error":"rejected: expected ident at line 1 column 2"}
```

An argument that fails validation (real output, exit code `2`):

```json
{"ok":false,"error":"rejected: invalid service name `bad name;`"}
```

### Request operations

The `op` field selects one of a closed set. There is no generic "run this"
request and no shell. Every argument is validated before anything runs.

| `op` | Fields | What it does |
| --- | --- | --- |
| `service_status` | `name` | Reads a service's state |
| `restart_service` | `name` | Restarts a service |
| `package` | `package`, `version`, `action`, `params` | Runs a catalog recipe compiled into the helper |
| `container` | `container` (`kind`: `info`, `list`, `images`, `inspect`, `start`, `stop`, `restart`, `remove`, `logs`, `stats`, `pull`) | One Docker Engine operation |
| `app` | `action`, `spec`, `tail`, `cursor` | Manages a service |
| `site` | `action` (`apply`, `remove`, `status`, `renew`), `spec` | Manages an Nginx site |
| `dns` | `dns` | One PowerDNS operation |
| `ingress` | `ingress` | One ingress operation |
| `fabric` | `fabric` | One cluster-network operation |
| `storage` | `storage` | One object-store operation |
| `files` | `files` | One file explorer operation, confined to approved roots. On an app root it runs through [`file-worker`](#forge-agent-helper-file-worker) as the app's account |
| `access` | `access` | One remote-access gateway operation |
| `stack` | `stack` | One compose-stack operation |
| `update_agent` | `update` | Installs a new agent and helper |
| `confirm_update` | none | The new agent came back; keeps the update. Allowed unsigned |
| `login_accounts` | none | Lists the accounts the browser terminal can log in as, for the agent's facts. Read-only; allowed unsigned |
| `container_state` | `name` (a `forge-app-*` container name) | Whether a Forge app container runs, for its default health check: returns only `{id, running, status, health}`. Read-only; allowed unsigned. Other names are refused |
| `container_telemetry` | `telemetry` (`{enabled, exclude, ack}`: `exclude` lists container names or `key=value` labels, at most 64; `ack` is the `seq` the server stored, `0` for none) | Returns `{batch, samples}`: the pending batch of container log lines (`{seq, lines, dropped}`) and the latest per-minute samples, once. `ack` lets the helper move the per-container cursors and drop the stored batch. Read-only; allowed unsigned: its settings can only make collection smaller. See [Container log collection](/docs/concepts/container-logs/) |
| `health_policy` | `health` (the app's health policy, its revision and runtime), `issued_at_ms` (epoch ms; when Forge issued the signed command) | Replaces the command health checks the helper holds for one app. Derived only from Forge's signed app command `health`. The helper fills in `issued_at_ms` from the verified command, never from the agent, and refuses a policy issued before the one it holds: `the helper holds a newer health policy for this service` |
| `health_check` | `check` (`{environment, app, purpose, check, revision}`) | Runs one command health check by reference, from the signed policy the helper holds. It carries no command, arguments or script. Allowed unsigned. Refused when the helper has not received the policy for 30 minutes. A native app's check runs through [`health-worker`](#forge-agent-helper-health-worker), in a cgroup of its own. A container app's check runs only in a container that carries Forge's label for the app |
| `capabilities` | none | What this helper can do, for the agent's facts: `{container_exec}` is `true` only under `serve` with a pin. Read-only; allowed unsigned |
| `terminal_account` | `name`, `sudo` | Creates or keeps a terminal account, as [`terminal-account`](#forge-agent-helper-terminal-account) does |
| `signed` | `command` (base64url protobuf `SignedCommand`) | A command exactly as Forge signed it. The helper verifies it and derives the request itself. A signed `ContainerExecOpen` starts a container shell (Docker exec with a TTY, as the image's default user) only on a pinned helper (`refused: container shells need a pinned helper` otherwise). The helper checks the command's expiry and enforces its idle and lifetime limits and the limit of eight shells itself. It answers one response line, then the connection carries shell frames until the shell exits or the agent hangs up. There is no plain `op` for it. See [Container shells](/docs/concepts/container-shells/) |

### Response

| Field | Type | Present | Description |
| --- | --- | --- | --- |
| `ok` | boolean | always | Whether the request succeeded |
| `error` | string | on failure | Why it failed, was rejected or was refused |
| `state` | string | `service_status` | `running`, `stopped`, `starting`, `stopping`, `failed`, `not_found` or `unknown` |
| `output` | string | `package` | Tail of the recipe's output |
| `details` | object | `package` | The package's env file, with secret values redacted |
| `data` | JSON | most other requests | Structured result |

## See also

- [Privileged helper](/docs/concepts/agents/#privileged-helper): why the helper
  trusts only signed commands, and how the pin is made.
- [forge-agent](/docs/reference/cli/forge-agent/): enrollment and the agent's state
  directory.
- [Add nodes](/docs/guides/add-nodes/): the installer that sets up both services.
- [Terminal](/docs/guides/terminal/): node-bound SSH certificates.
- [Update agents](/docs/guides/update-agents/): self-update and rollback.
