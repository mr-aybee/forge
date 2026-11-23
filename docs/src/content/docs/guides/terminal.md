---
title: Browser terminal (SSH)
description: Open SSH sessions to any node from the console or the CLI, with short-lived certificates.
sidebar:
  order: 3
---

This guide shows you how to open an SSH session to a node from the console or
the `forge` CLI. Forge signs a five-minute certificate for each session, so no
password or key is stored.

## Before you begin

- You need `terminal.open` on the node, its cluster, its environment or the
  workspace. The **Operator**, **Admin** and **Platform admin** roles hold
  it. `terminal.open` needs an MFA-verified session when the server requires
  MFA ([Multi-factor authentication](/docs/guides/mfa/)).
- The account you log in as must exist on the node. To add one, see
  [Create a terminal account on a node](#create-a-terminal-account-on-a-node).
- The node has an SSH endpoint and trusts Forge's SSH CA. Assisted
  onboarding and the node installer set both up on Linux. Otherwise see
  [Set up SSH access for a node](#set-up-ssh-access-for-a-node).
- **Windows nodes** need the OpenSSH Server feature with
  `TrustedUserCAKeys` set in `%ProgramData%\ssh\sshd_config`:

  ```powershell
  Add-WindowsCapability -Online -Name OpenSSH.Server~~~~0.0.1.0
  ```

## Open a terminal

1. Open a session in one of these ways:
   - **Network → Terminal**, then pick the node.
   - On a node, select **Open terminal**.
   - Press <kbd>Ctrl</kbd>+<kbd>K</kbd> and type the node's name.
2. In **Open terminal**, select an account, or type one and select
   **Connect as `<login>`**. The list shows the node's saved accounts first,
   then the accounts the node reports, then the ones you used recently in
   this browser, marked **saved**, **on node** or **recent** (see
   [Which accounts the list offers](#which-accounts-the-list-offers)). Arrow
   keys and <kbd>Enter</kbd> pick from the keyboard.
3. Optional: tick **Save `<login>` for this node (admins)** so everyone sees
   the account next time. Saving needs `server.manage` on the node. Without
   it the session still opens and a message says the account was not saved.
4. If your session is not MFA-verified, confirm in **Verify it's you**.

### Start a session from the terminal panel

In the terminal panel, select **New session** (<kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>T</kbd>):

1. Choose **Node** or **Container**.
2. Pick the **Node**. For **Container**, only nodes whose agent is online are
   listed.
3. Pick the **Account** or the **Container**:
   - **Node**: the **Account** list shows the node's saved accounts, the
     accounts it reports and the ones you used recently. It preselects the
     account you used last. Choose **Other account…** to type one.
   - **Container**: pick a running container. The shell runs as the
     container's own user (the image's `USER`, often `root`), as
     `docker exec` does.
4. Select **Connect**.

Login names are 1 to 32 characters of letters, digits, `_`, `.` and `-`, and
do not start with `-`. A node holds at most 32 saved accounts. Changes are
audited as `server.logins.update`. To replace the list through the API, send
`PUT /api/v1/servers/<node-id>/logins` with `{"logins": ["deploy", "ops"]}`.

:::caution
The saved list is a convenience, not a grant. Opening the session still needs
`terminal.open` on the node (and MFA), and the account must exist on the node.
:::

### Which accounts the list offers

| Marker | Source | When it appears |
| --- | --- | --- |
| **saved** | The node's saved list in Forge | Saved under **Open terminal**, through `PUT /servers/{id}/logins`, or automatically: the SSH user of an assisted onboarding, the account the installer created, and an account created through the API |
| **on node** | The node's facts (`login_accounts`) | The agent reports the account and it is not saved yet |
| **recent** | This browser | You opened a session as this account on this node before |

An account that is both saved or on the node and recent shows
**saved · recent** or **on node · recent**.

The agent reports these accounts on a Linux node:

- `root`, first, when `sshd -T` shows `PermitRootLogin` as `yes`,
  `prohibit-password` or `without-password`;
- local users with a uid of 1000 or higher (not `65534`), other than
  `forge-agent`, whose shell is a login shell (not `nologin`, `false`,
  `sync`, `halt` or `shutdown`) and whose account is neither locked (`!` in
  `/etc/shadow`) nor expired.

The root helper reads `/etc/shadow` and runs `sshd -T`. When the helper does
not answer, the agent reads `/etc/passwd` alone: `root` is then not listed,
and locked or expired accounts are not filtered out. A node reports at most
64 accounts. Windows nodes report none.

The agent reports its facts when it connects and once a day, so an account
added on the node shows up after the next report.

## Create a terminal account on a node

A terminal account is a local Linux user that Forge creates for the browser
terminal. Assisted onboarding and the node installer can create one when you
add the node (see [Add nodes](/docs/guides/add-nodes/)). This task creates one on
a node that is already enrolled.

### Before you begin

- You need `server.command` on the node, with an MFA-verified session. The
  **Operator**, **Admin** and **Platform admin** roles hold it.
- The node runs Linux, its agent is online and allows the
  `terminal_account` command kind (it does by default; see
  [`agent.toml`](/docs/reference/configuration/#agent-agenttoml)).
- Pick a name: 1 to 32 characters of `a-z`, `0-9`, `_` and `-`, starting
  with a letter or `_`. Forge refuses system names such as `root`,
  `nobody`, `forge-agent`, `daemon`, `www-data`, `sshd`, `admin`, `sudo`,
  `wheel` and `docker`, and any name starting with `systemd-`.

### Steps

1. Sign in to the CLI with an MFA-verified session (`forge auth step-up`
   when the server asks).
2. Send the request. Set `"sudo": true` only when the account needs
   passwordless `sudo`:

   ```sh
   forge api request POST /api/v1/servers/<node-id>/terminal-account --body '{"name": "forge-ops", "sudo": false}' --yes
   ```

   The node's root helper creates the account, or keeps it when it already
   exists as a regular user, and Forge adds it to the node's saved list.

The answer is `{"name", "sudo", "created", "logins"}`. `created` is `false`
when the account existed already. See the
[REST API reference](/docs/reference/rest-api/#servers) for every status code.

What the node does:

| Item | Value |
| --- | --- |
| User | Created with `useradd --create-home --user-group`, shell `/bin/bash` (or `/bin/sh` when there is no bash), comment `Orbyn Forge terminal account` |
| Password | None. The password field is `*`, so only Forge's node-bound SSH certificates log in |
| Sudo | With `"sudo": true`: `/etc/sudoers.d/forge-<name>` holds `<name> ALL=(ALL:ALL) NOPASSWD: ALL`. Forge checks the file with `visudo -c` before it moves it into place. With `"sudo": false`: Forge removes that file when it carries Forge's marker line |
| Existing user | A regular user (uid 1000 or higher) with that name is kept as it is; only the sudo file changes. A system user or an existing group with that name is refused |

Call it again with `"sudo": false` to take passwordless `sudo` away from an
account Forge created. Forge never deletes the user.

:::caution[A default account is a login surface]
Every node with a terminal account has one more account that can log in.
Forge keeps it narrow: the account has no password and logs in only with a
five-minute certificate for this node, issued to someone who holds
`terminal.open`. Allow `sudo` only when the account needs it: with `sudo`,
anyone who can open a terminal as this account is root on the node. Remove
accounts you no longer use with `userdel -r <name>` and
`rm /etc/sudoers.d/forge-<name>` on the node.
:::

### Verify

- The answer lists the account in `logins`, and **Open terminal** shows it
  as **saved**.
- On the node, `getent passwd forge-ops` prints the user, and
  `sudo -l -U forge-ops` shows the `NOPASSWD: ALL` rule when you allowed
  `sudo`.
- **Operations → Audit log** shows `server.terminal_account.create` and,
  when the list changed, `server.logins.update`.

## Work in the terminal panel

Sessions open in a panel docked at the bottom of the console. Sessions stay
connected while you move between pages.

- Drag the panel's top edge to resize it. Use **Maximize panel**, or
  <kbd>Ctrl</kbd>+<kbd>`</kbd> to collapse or expand it.
- Each session has its own tab with its connection status.
  <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>T</kbd> opens a new one; a
  middle-click on a tab closes it.
- **Open in a separate window** moves a session into its own browser window
  without reconnecting. The main window keeps the SSH connection and relays
  it, and the new window starts with the existing scrollback. **Back to
  panel**, or closing the window, returns the session to the dock. If the
  main window is closed or reloaded, the session ends; the pop-out then
  offers **Reconnect here**, which opens a fresh session of its own.

| Feature | How |
| --- | --- |
| Copy / paste | <kbd>Ctrl</kbd>+<kbd>C</kbd> copies when text is selected, otherwise it interrupts. <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>C</kbd> always copies. <kbd>Ctrl</kbd>+<kbd>V</kbd>, <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>V</kbd> and <kbd>Shift</kbd>+<kbd>Insert</kbd> paste, also on consoles served over plain HTTP. <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>A</kbd> selects all. Right-click opens a menu with **Copy**, **Paste**, **Select all** and **Clear screen**; over plain HTTP the browser blocks **Paste** from the menu, so use <kbd>Ctrl</kbd>+<kbd>V</kbd>. Copy-on-select is in **Terminal settings**. OSC 52 clipboard is supported |
| Find | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>F</kbd> searches the 10,000-line scrollback |
| Rendering | WebGL (falls back to DOM), 256 colours and true colour, Unicode 11 widths, clickable links |
| Full-screen programs | `vim`, `htop`, `tmux`, `mc` and others, with live resize |
| Themes | Ocean, Frost (light), Classic, Midnight; font size 10–24 px in **Terminal settings** |
| Log consoles | Log tabs and job, container and stack output match the console's light or dark theme when the page opens. **Log consoles** in **Terminal settings**, or the sun/moon button in a log tab, switches only the log consoles until you reload the page or change the console theme. Lines are coloured by the app's ANSI colours, or by level: errors red, warnings amber, debug dimmed |
| Transcript | **Save transcript** saves the scrollback as a `.log` file |
| Status bar | Connection state, login@node, size, round-trip latency |

Sessions end on `exit`, after 30 minutes without input, after 12 hours in
total, within 30 seconds of your sign-in ending (sign-out, expiry, a
password change, or the API key being deleted) or your `terminal.open`
permission being removed, within a second of being revoked
(`forge sessions revoke`) or of the API key that opened them being revoked,
or when the tab closes.

## Open a session from the command line

The `forge` CLI opens the same sessions, with the same checks, from your own
terminal on Linux, macOS or Windows:

```sh
forge nodes terminal <node> --login <login>
forge nodes exec <node> --login <login> -- systemctl status nginx
```

`terminal` gives an interactive shell; resize works, and the session ends
when the shell exits. `exec` runs one command without a terminal: stdout and
stderr stay apart, piped stdin reaches the command, and
`--propagate-exit-code` makes `forge` exit with the command's status. With
`-o json`, `exec` prints `{stdout, stderr, exit_code}`. Both need an
MFA-verified CLI session: when the server asks, the CLI prompts for a
one-time code (`forge auth step-up`). See the [CLI reference](/docs/reference/cli/)
for every flag.

`forge sessions list` shows your open sessions from the console and the CLI;
`forge sessions revoke <id>` ends one within a second. Administrators with
`user.manage` see and end everyone's with `--all`.

:::caution
`exec` writes the command line to the audit log (`terminal.exec`). Pass
secrets on stdin, never as arguments.
:::

## Set up SSH access for a node

Each node has an *SSH endpoint* (host and port as reachable from Forge) and a
pinned host key. Assisted onboarding records both. For other nodes:

1. Open the node and select the **SSH / Terminal** tab.
2. Under **SSH access**, select **Set up SSH access**.
3. Enter the **Host or IP as reachable from Forge** and the **Port**.
4. Under **Confirm the host key**, compare the fingerprint with the output of
   this command on the node, then save:

   ```sh
   ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub
   ```

5. If the node was not set up by the installer, open
   **Trust Forge's SSH CA manually**, copy the key to
   `/etc/ssh/forge_user_ca.pub` and set
   `TrustedUserCAKeys /etc/ssh/forge_user_ca.pub` in `sshd_config`. Or re-run
   the node installer, which does this for you.

Saving an endpoint needs `server.manage` and `server.command` on the node,
with an MFA-verified session (**Operator** or higher). Forge refuses the
endpoint when:

- the host key is pinned for another node;
- the `host:port` is another node's SSH endpoint;
- the host is another node's agent address (unless both nodes share it,
  behind the same NAT);
- the node's agent is online and reports host keys, and the key is not one
  of them.

Refusals are audited as denied `server.ssh_endpoint` events.

:::caution
The endpoint decides where root terminals for this node connect. Treat a
change like running a command on the node.
:::

### What the installer changes on a Linux node

The installer writes the CA key to `/etc/ssh/forge_user_ca.pub`, adds
`TrustedUserCAKeys` (in `/etc/ssh/sshd_config.d/50-forge.conf` when that
directory is included), checks the config with `sshd -t`, then reloads sshd.
When the agent's root helper starts, it adds these lines to the same
Forge-managed block and writes the node's id to `/etc/ssh/forge_node_id`:

```text
AuthorizedPrincipalsCommand /usr/local/bin/forge-agent-helper ssh-principals %u
AuthorizedPrincipalsCommandUser nobody
```

:::caution
`AuthorizedPrincipalsCommand` applies to every user certificate sshd accepts.
If the node also trusts your own SSH CA, its certificates are refused from
then on. Remove the two lines to go back, or keep your CA on other nodes.
:::

## How it stays safe

- No stored passwords or keys. For each session Forge creates a fresh
  key and signs a certificate valid for five minutes for the login you
  chose. The node's `sshd` trusts the CA (`TrustedUserCAKeys`); nothing
  long-lived reaches it.
- Bound to one node. The certificate's only principal is
  `forge.<server-id>.<login>`, and each Linux node accepts only its own id
  (`AuthorizedPrincipalsCommand`), so a certificate for one node does not
  open another. A node whose agent has not set this up yet gets the bare
  login, which such older nodes all accept, so Forge refuses `root` there
  (`409` `agent_update_required`) until you update the agent.
- Pinned host key. Forge connects only if the node presents the host key
  confirmed at onboarding or under **SSH access**.
- Same-origin, CSRF-checked WebSocket. Cookies alone cannot open a
  shell. The CLI sends its token in the `Authorization` header, never in the
  URL, and a request with both a token and a cookie is refused.
- Audited. `terminal.open` and `terminal.close` (duration, bytes in and
  out, reason) are written to the audit log. Root login stays whatever the
  node's `sshd_config` allows (`PermitRootLogin no` keeps it off).

## Verify

- The session tab shows the connected state and your shell prompt.
- The status bar shows `login@node` and a round-trip latency.
- **Operations → Audit log** shows a `terminal.open` entry for your session.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| `409` `agent_update_required` when signing in as `root` | The node's agent is older and does not bind terminal certificates to the node, so a `root` certificate would open every such node | Update the agent under **Administration → Forge updates → Agents**, or sign in with another account |
| **SSH access** says "No SSH endpoint yet" | The node was added with the manual installer | [Set up SSH access for a node](#set-up-ssh-access-for-a-node) |
| Authentication fails for a valid account | The node does not trust Forge's SSH CA | Re-run the node installer, or follow **Trust Forge's SSH CA manually** |
| Your own SSH CA's certificates stop working on a node | `AuthorizedPrincipalsCommand` applies to every certificate | Remove the two lines, or keep your CA on other nodes |
| Forge refuses to connect because of the host key | The node's host key changed since it was pinned | Check why it changed; then set up SSH access again and confirm the new key |
| "Not saved for this node" | You lack `server.manage` on the node | The session still opens; ask an administrator to save the account |
| Session closes after a while | 30 minutes without input, or 12 hours in total | Open a new session |
| An account on the node is missing from **on node** | Its uid is below 1000, its shell is `nologin` or `false`, it is locked or expired, or the agent has not reported its facts since the account was added | Type the login and select **Connect as `<login>`**, or save it for the node |
| `root` is not listed | sshd does not allow root logins with keys, or the helper did not answer and the agent read `/etc/passwd` only | Type `root` if sshd allows it; check `systemctl status forge-agent-helper` |
| `409` `terminal accounts need a Linux node` | The node is not Linux | Not supported on Windows nodes |
| `409` `the node refused: …` | The name is a system user or an existing group on the node, `sudo` is not installed (`/etc/sudoers.d` is missing), `visudo` rejected the rule, or the agent's allowlist lacks `terminal_account` | Read the reason; pick another name, install `sudo`, or add `terminal_account` to `allowed_commands` |
| `504` `the node did not answer in time` | The node is offline, or did not answer within 60 seconds. The signed command stays valid for two minutes, so an agent that reconnects in that time still runs it | Check the agent and helper services on the node, then send the request again; it is safe to repeat |

## Next steps

- [Multi-factor authentication](/docs/guides/mfa/)
- [Roles and access](/docs/guides/roles-and-access/)
- [Add nodes](/docs/guides/add-nodes/)
- [Sign in to the CLI on a server or over SSH](/docs/guides/cli-headless-login/)
