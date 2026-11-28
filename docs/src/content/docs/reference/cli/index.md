---
title: forge CLI
description: Install the forge command-line client, sign in, and look up every command, flag, output format and exit code.
sidebar:
  order: 0
---

`forge` is the command-line client for Orbyn Forge. It does what the console
does, through the same API, permissions, MFA and audit log, for people,
scripts, CI pipelines and AI agents that run shell commands. This page covers
installing it, signing in, the flags every command takes, output formats and
exit codes. Each command group has its own page; see the
[command map](#command-map).

## Install

Each release publishes the CLI as one binary per platform. Every file is
listed in the signed `SHA256SUMS`:

| Release asset | Platform |
| --- | --- |
| `forge-cli-x86_64-unknown-linux-musl` | Linux x86_64, any distribution (static, no runtime libraries) |
| `forge-cli-aarch64-unknown-linux-musl` | Linux arm64, any distribution (static, no runtime libraries) |
| `forge-cli-x86_64-pc-windows-msvc.exe` | Windows 10 / 11, Windows Server 2019 or later, x86_64 |

Releases include no macOS binary. On macOS, and on any other platform, build
it from source with `cargo build --release -p forge-cli` in `src/forge`.
The binary is `src/forge/target/release/forge` (`forge.exe` on Windows).

### Install on Linux

On arm64, use `aarch64` instead of `x86_64`:

```sh
curl -fLO https://github.com/orbyntech/forge/releases/latest/download/forge-cli-x86_64-unknown-linux-musl
curl -fLO https://github.com/orbyntech/forge/releases/latest/download/SHA256SUMS
grep ' forge-cli-x86_64-unknown-linux-musl$' SHA256SUMS | sha256sum -c
sudo install -m 0755 forge-cli-x86_64-unknown-linux-musl /usr/local/bin/forge
forge --version
```

### Install on Windows

In PowerShell, as the user who runs the CLI:

```powershell
$f = "forge-cli-x86_64-pc-windows-msvc.exe"
$base = "https://github.com/orbyntech/forge/releases/latest/download"
Invoke-WebRequest "$base/$f" -OutFile $f
Invoke-WebRequest "$base/SHA256SUMS" -OutFile SHA256SUMS
$want = ((Select-String -Path SHA256SUMS -Pattern " $([regex]::Escape($f))$").Line -split '\s+')[0]
if ((Get-FileHash $f -Algorithm SHA256).Hash -ne $want) { throw "checksum mismatch for $f" }
$dir = "$env:LOCALAPPDATA\Programs\OrbynForge"
New-Item -ItemType Directory -Force $dir | Out-Null
Move-Item -Force $f "$dir\forge.exe"
$path = [Environment]::GetEnvironmentVariable("Path", "User")
if (($path -split ';') -notcontains $dir) { [Environment]::SetEnvironmentVariable("Path", "$path;$dir", "User") }
& "$dir\forge.exe" --version
```

Open a new terminal to pick up the changed `Path`.

:::caution
The Windows binary is not code-signed. A file downloaded with a browser
carries a download mark, and SmartScreen then warns before the first start.
Check the checksum, then run `Unblock-File <file>`. Files downloaded with
`Invoke-WebRequest` carry no mark.
:::

To keep it current, run [`forge update`](/docs/reference/cli/update/).

## Sign in

1. Add a context, a named connection to one Forge server:

   ```sh
   forge contexts add prod --server https://forge.example.com
   ```

2. Sign in. The browser opens Forge's sign-in page; MFA and SSO work as in
   the console:

   ```sh
   forge auth login
   ```

   On a machine without a browser, use a one-time code you approve from
   another device: `forge auth login --device`. See
   [Sign in from a headless machine](/docs/guides/cli-headless-login/).

3. Check who you are:

   ```sh
   forge auth whoami
   ```

Contexts are on the [`forge contexts`](/docs/reference/cli/contexts/) page and
sign-in on the [`forge auth`](/docs/reference/cli/auth/) page. For one-off scripts
you can skip contexts: set `FORGE_SERVER` and `FORGE_TOKEN` (a personal API
key from [`forge api-keys`](/docs/reference/cli/api-keys/)).

### Where the CLI keeps its session

| OS | Session tokens | Settings (`FORGE_CONFIG_DIR` overrides) |
| --- | --- | --- |
| Linux | Secret Service (GNOME Keyring, KWallet) | `$XDG_CONFIG_HOME/orbyn-forge/cli`, default `~/.config/orbyn-forge/cli` |
| Windows | Windows Credential Manager | `%APPDATA%\OrbynForge\cli` |
| macOS | login Keychain | `~/Library/Application Support/OrbynForge/cli` |

Where no credential store is available (headless servers, containers), or
with `FORGE_CREDENTIAL_STORE=file`, the CLI stores the tokens in an
owner-only file in the settings directory instead.

:::caution
The fallback file is protected by file permissions only, not encrypted.
Anyone with root or your account on that machine can read the tokens.
:::

## Command map

| Page | Commands | For |
| --- | --- | --- |
| [contexts](/docs/reference/cli/contexts/) | `add`, `list`, `use`, `current`, `update`, `remove` | Connection profiles |
| [auth](/docs/reference/cli/auth/) | `login`, `step-up`, `logout`, `status`, `whoami` | Sign-in, MFA step-up |
| [nodes](/docs/reference/cli/nodes/) | `list`, `get`, `tags`, `command`, `rename`, `revoke`, `enroll-token`, `metrics`, `processes`, `kill`, `priority`, `connectivity`, `logins`, `ssh`, `terminal`, `exec` | Nodes, shells and commands on them |
| [nodes files](/docs/reference/cli/nodes-files/) | `roots`, `ls`, `stat`, `cat`, `download`, `upload`, `save`, `mkdir`, `mv`, `cp`, `rm`, `trash`, `search`, `chmod`, `chown`, `acl`, `compress`, `extract`, `archive`, `bulk`, `sftp` … | Files on nodes |
| [agents](/docs/reference/cli/agents/) | `versions`, `update`, `auto-update` | Agent versions and updates |
| [jobs](/docs/reference/cli/jobs/) | `list`, `get`, `steps`, `wait`, `watch`, `cancel` | Durable jobs |
| [audit](/docs/reference/cli/audit/) | `list`, `verify` | The audit log |
| [stacks](/docs/reference/cli/stacks/) | `list`, `show`, `create`, `update`, `deploy`, `rollback`, `down`, `service`, `logs` … | Compose stacks |
| [docker](/docs/reference/cli/docker/) | `endpoint show/enable/disable/check`, `catalog`, `api` | A node's Docker Engine API over mutual TLS |
| [deploy](/docs/reference/cli/deploy/) | `validate`, `plan`, `apply`, `status`, `history`, `releases`, `release`, `slots`, `redeploy`, `rollback`, `pause`, `resume`, `abort`, `approve`, `reject` | Releases and rollouts of services from `forge.deploy.yaml` |
| [projects](/docs/reference/cli/projects/) | `list`, `show`, `create`, `update`, `delete` | Projects: the products whose environments hold your services |
| [services](/docs/reference/cli/services/) | `list`, `show`, `health`, `create`, `adopt`, `update`, `place`, `deploy`, `action`, `logs`, `remove`, `delete`, `files …` | Services |
| [plugins](/docs/reference/cli/plugins/) | `list`, `keygen`, `sign`, `install`, `enable`, `disable`, `pin`, `unpin`, `remove` | Plugins |
| [approvals](/docs/reference/cli/approvals/) | `list`, `get`, `deny`, `approve` | Requests from AI connectors |
| [users](/docs/reference/cli/users/) | `list`, `create`, `reset-passkeys` | Accounts |
| [access](/docs/reference/cli/access/) | `list`, `grant`, `revoke`, `catalog`, `profiles …`, `assignments …` | Role bindings, access profiles and assignments |
| [api-keys](/docs/reference/cli/api-keys/) | `list`, `create`, `delete` | Personal API keys |
| [service-accounts](/docs/reference/cli/service-accounts/) | `list`, `create`, `disable`, `enable`, `tokens`, `token-create`, `token-revoke` | Non-person identities for automation |
| [connectors](/docs/reference/cli/connectors/) | `list`, `revoke`, `clients`, `revoke-client` | AI apps connected through MCP |
| [settings](/docs/reference/cli/settings/) | `list`, `set`, `unset`, `server`, `public-address` | Workspace settings and the server's public address |
| [tenants and environments](/docs/reference/cli/tenants-environments/) | `tenants list/create`, `environments list/create/move` | Tenancy |
| [clusters](/docs/reference/cli/clusters/) | `list`, `get`, `create`, `update`, `delete`, `live`, `routing`, `ingress get/set/apply/remove` | Clusters and their ingress profile |
| [sites and certificates](/docs/reference/cli/sites-certificates/) | `sites …`, `certificates …` | Domains, ingress, TLS |
| [tunnel](/docs/reference/cli/tunnel/) | `open`, `list`, `close`, `ports`, `grants …` | Port tunnels to services, databases and SSH |
| [vault](/docs/reference/cli/vault/) | `list`, `add`, `update`, `reveal`, `delete` | The password vault |
| [system](/docs/reference/cli/system/) | `update`, `check`, `apply` | Server updates |
| [packages](/docs/reference/cli/packages/) | `catalog`, `installed`, `install`, `updates`, `update` … | The App Store |
| [sessions](/docs/reference/cli/sessions/) | `list`, `revoke` | Open terminal and exec sessions |
| [api](/docs/reference/cli/api/) | `operations`, `describe`, `call`, `request` | Any API operation |
| [parity](/docs/reference/cli/parity/) | `parity` | How the CLI covers the API |
| [update](/docs/reference/cli/update/) | `update` | Updating the CLI itself |

This page also covers [`forge version`](#forge-version),
[`forge capabilities`](#forge-capabilities),
[`forge completion`](#forge-completion) and
[`forge commands`](#forge-commands).

### Server-side binaries

`forge-server`, `forge-agent`, `forge-agent-helper`, the installers and
`fleet-sim` run on servers and nodes, not on your workstation:
[forge-server](/docs/reference/cli/forge-server/),
[forge-agent](/docs/reference/cli/forge-agent/),
[forge-agent-helper](/docs/reference/cli/forge-agent-helper/),
[server installer](/docs/reference/cli/server-installer/),
[node installer](/docs/reference/cli/node-installer/),
[fleet-sim](/docs/reference/cli/fleet-sim/).

## Global flags

These flags work with every `forge` command, before or after the command
name.

| Flag | Env | Type | Default | Description |
| --- | --- | --- | --- | --- |
| `--context`, `-C <name>` | `FORGE_CONTEXT` | string | the current context | Context (connection profile) for this command only |
| `--server <url>` | `FORGE_SERVER` | URL | the context's server | Server URL. With `FORGE_TOKEN`, targets a server without a context. Must match the context when both are given |
| `--token <token>` | `FORGE_TOKEN` | string | none | Bearer token for this command; never stored. Needs `--server` or a context. Prefer the environment variable to keep it out of shell history |
| `--output`, `-o <format>` | | `table`, `json`, `yaml` or `xml` | `table` | `json`, `yaml` and `xml` print the same versioned envelope for scripts; see [Output](#output) |
| `--color <when>` | `NO_COLOR` | `auto`, `always` or `never` | `auto` | Colour for status words, table headers and the `error:` prefix. `auto` colours only on a terminal and never when `NO_COLOR` is set, `TERM=dumb`, output is piped or the format is not `table` |
| `--no-pager` | | flag | off | Print long output directly instead of opening a pager. See [Paging](#paging) |
| `--non-interactive` | | flag | off | Never prompt. A command that would ask fails with a typed error instead |
| `--expected-instance-id <id>` | | id | none | Refuse to run unless the server's workspace id matches (exit `7`, `instance_mismatch`) |
| `--help`, `-h` | | flag | | Print the command's help |
| `--version`, `-V` | | flag | | Print the CLI version |

Example: run one command against another context and get JSON.

```sh
forge -C staging -o json nodes list
```

## Output

With the default `--output table`, list commands print aligned columns and
other commands print readable text or key/value lists: one `key  value` line
per field, nested objects as indented sections, lists of flat objects as small
tables, empty fields left out, and epoch-millisecond timestamps as local
date-times. The default output is never raw JSON. Prompts, progress and
human-readable errors go to stderr, so stdout holds only data.

On a terminal, status words are coloured: green for healthy states
(`online`, `succeeded`, `active`, `deployed`), yellow for states in progress
(`pending`, `queued`, `running`), red for failures (`offline`, `failed`,
`denied`). Use `--color never` or `NO_COLOR=1` to turn colour off.

With `-o json`, stdout always holds exactly one JSON document, indented on a
terminal and on one line when piped:

```json
{ "schema": "forge.cli/v1", "result": { "…": "…" } }
```

On failure the exit code is non-zero and stdout holds the error instead:

```json
{
  "schema": "forge.cli/v1",
  "error": {
    "code": "not_found",
    "class": "not_found",
    "message": "no node named `web-09` that you can see",
    "retryable": false,
    "request_id": null,
    "exit_code": 6
  }
}
```

| Field | Description |
| --- | --- |
| `schema` | Always `forge.cli/v1`. A breaking change to the envelope gets a new value |
| `result` | The command's data: the API's answer, or what the command reports |
| `error.code` | The server's problem title (such as `step_up_required`), or the class below |
| `error.class` | The exit class: `error`, `invalid_input`, `unauthenticated`, `permission_denied`, `mfa_required`, `not_found`, `conflict`, `timeout`, `transport`, `job_failed` |
| `error.message` | What went wrong, for people |
| `error.retryable` | `true` when trying again later can succeed (timeouts, `429`, `502`–`504`) |
| `error.request_id` | The server's request id, for its logs |
| `error.exit_code` | The process exit code |

`-o yaml` prints the same envelope as a YAML document (starting with `---`).
`-o xml` prints it as XML: `<forge schema="forge.cli/v1">` holds `<result>` or
`<error>`; object fields become elements, list items `<item>` elements, a
`null` an empty element with `nil="true"`, and a key that is not a valid XML
name becomes `<entry key="…">`.

```sh
forge -o yaml nodes get <node>
forge -o xml jobs list --limit 5
```

## Paging

List commands print one page of rows: `--limit` rows (default `50`) of page
`--page` (default `1`). `--all` prints every row (same as `--limit 0`); on
commands where `--all` already means "everyone's" (`approvals list`,
`connectors list`, `sessions list`), use `--limit 0`.
For people, a footer under the table says where you are and what comes next:

```text
showing 1–50 of 312 · next: --page 2 · every row: --all
```

`jobs list`, `alerts alarms` and `audit list` page on the server; `audit list`
also takes the cursor `--before <seq>`. The other lists fetch every row and
cut the page in the CLI. Machine formats print only the page's rows, without
a footer.

On a terminal, output longer than the screen opens in a pager:
`$FORGE_PAGER`, else `$PAGER`, else `less` (run with `LESS=FRX` unless you set
`LESS`) on Linux and macOS, and a built-in pager on Windows (Space: next page,
Enter: next line, `q`: quit). `PAGER=cat` or `--no-pager` prints directly.
Pipes and machine formats never page.

## Exit codes

| Code | JSON `class` | Meaning |
| --- | --- | --- |
| `0` | | Success |
| `1` | `error` | Any other error. Also when you answer anything but `yes` at a confirmation prompt (`cancelled`) |
| `2` | `invalid_input` | Bad arguments or a `400`/`422` answer. Also `ambiguous` (a name matches more than one item), `invalid_body` (the body does not match the operation's schema), and a change without `--yes` when no prompt is possible |
| `3` | `unauthenticated` | Not signed in, the session ended, a wrong one-time code, or a device sign-in that was denied |
| `4` | `permission_denied` | Cedar denied the action |
| `5` | `mfa_required` | The action needs MFA, a step-up or a fresh code. The JSON `code` is the server's title, such as `step_up_required` |
| `6` | `not_found` | The item does not exist or you cannot see it |
| `7` | `conflict` | The item exists, changed, or the job already finished |
| `8` | `timeout` | A wait ran out (`--wait`, `jobs wait`, `jobs watch`, device sign-in). Jobs keep running |
| `9` | `transport` | The server is unreachable or answered `502`/`503` |
| `10` | `job_failed` | A job you waited for failed or was cancelled. The JSON `code` is `job_failed` or `job_cancelled` |

`forge nodes exec --propagate-exit-code` exits with the remote command's
status instead.

## Confirmation prompts

Commands that change something ask for confirmation and describe what they
will do first. Type `yes` to go on. `--yes` skips the prompt; with
`--non-interactive`, or without a terminal, a command that would ask fails
with exit `2` unless you pass `--yes`. `--yes` only skips the question: it
never bypasses permissions or MFA.

## Names and ids

Where a command takes a node, cluster, stack, site or similar, you can give
its id or its name. A name must match exactly one item you can see; when it
matches several, the command exits with `2` (`ambiguous`) and lists the ids.
It never picks the first match.

## Retries and idempotency

Every change the CLI sends carries a random `Idempotency-Key` (see the
[REST API](/docs/reference/rest-api/#idempotency-key)), so the server runs it at
most once. After a network error, a `429`, `502`, `503` or `504`, or
`idempotency_in_progress`, the CLI sends the same request with the same key
again, up to 3 times. It waits 0.5 to 4 seconds between tries, or as long as
the server's `Retry-After` says (at most 30 seconds). Each retry prints a
line on stderr. Reads are not retried. A retry never runs a change twice: the
server answers it with the stored result.

[`forge api call` and `forge api request`](/docs/reference/cli/api/) take
`--idempotency-key <key>` to set the key yourself.

## Environment variables

| Variable | Description |
| --- | --- |
| `FORGE_CONTEXT` | Same as `--context` |
| `FORGE_SERVER` | Same as `--server` |
| `FORGE_TOKEN` | Same as `--token`: a bearer token or personal API key, never stored |
| `FORGE_CONFIG_DIR` | Directory for contexts and settings, instead of the OS default |
| `FORGE_CREDENTIAL_STORE` | `file` stores tokens in an owner-only file instead of the OS credential store |
| `FORGE_ALLOW_HTTP` | Any value allows plain `http://` server URLs that are not on the loopback address. For test labs without TLS only |
| `FORGE_UPDATE_FEED` | Release feed for [`forge update`](/docs/reference/cli/update/), for a mirror |
| `FORGE_UPDATE_PUBLIC_KEY` | Release signing key for that mirror |
| `FORGE_URL` | Deprecated: use `FORGE_SERVER` or a context |

:::caution
`FORGE_ALLOW_HTTP` sends your tokens unencrypted. Never set it for a server
reached over a network you do not control.
:::

## Other commands

### forge version

Prints the CLI's version and, when a context or server is set, the server's
version and API versions. Needs no sign-in.

**Synopsis**

```text
forge version
```

**Arguments and flags**

This command takes no arguments.

**Examples**

```sh
# Which CLI and server versions am I using?
forge version
# The same as JSON, for scripts
forge -o json version
```

**Output**

```text
$ forge version
client  0.1.0
server  0.1.1-dev.20261113.214755 (build 07391d2)
```

```json
$ forge -o json version
{
  "schema": "forge.cli/v1",
  "result": {
    "client": { "version": "0.1.0" },
    "server": {
      "product": "orbyn-forge",
      "version": "0.1.1-dev.20261113.214755",
      "build": "07391d2",
      "api_versions": ["v1"],
      "agent_protocol": "forge.agent.v1",
      "plugin_api": "0.1.0"
    }
  }
}
```

Without a context or server, or when the server cannot be reached, the
table shows `server  (not connected)` and JSON has `"server": null`.

Exit codes: `0`, also when the server cannot be reached.

### forge capabilities

Prints the server's feature flags, including extension points that exist
but are switched off. Needs no sign-in.

**Synopsis**

```text
forge capabilities
```

**Arguments and flags**

This command takes no arguments.

**Examples**

```sh
# Does this server have the MCP endpoint enabled?
forge -o json capabilities
```

**Output**

```json
$ forge -o json capabilities
{
  "schema": "forge.cli/v1",
  "result": {
    "capabilities": [
      { "id": "auth.local", "enabled": true, "version": 1 },
      { "id": "auth.totp", "enabled": true, "version": 1 },
      { "id": "security.mfa_required", "enabled": false, "version": 1 },
      { "id": "auth.oidc", "enabled": false, "version": 1 },
      { "id": "auth.passkeys", "enabled": true, "version": 1 },
      { "id": "rbac.cedar", "enabled": true, "version": 1 },
      "…"
    ]
  }
}
```

The table output prints the same document as a key/value list. Each entry
has the feature `id`, whether it is `enabled` on this server, and its
`version`. Disabled entries are extension points that exist but are off.

Exit codes: `0`; `9` when the server cannot be reached.

### forge completion

Prints a shell completion script. Completion never contacts the server.

**Synopsis**

```text
forge completion <SHELL>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<shell>` | `bash`, `zsh`, `fish`, `powershell` or `elvish` | — | yes | The shell to write the script for | `bash` |

**Examples**

```sh
# Bash: load completions in every new shell
forge completion bash | sudo tee /etc/bash_completion.d/forge > /dev/null
# Zsh: write the script to a directory on $fpath
forge completion zsh > "${fpath[1]}/_forge"
# Fish
forge completion fish > ~/.config/fish/completions/forge.fish
```

```powershell
# PowerShell: add to your profile
forge completion powershell | Out-String | Add-Content $PROFILE
```

**Output**

The script on stdout. Exit codes: `0`; `2` for an unknown shell.

### forge commands

Describes every command and argument as JSON, for scripts and AI agents that
need the command tree without parsing `--help`. Needs no sign-in.

**Synopsis**

```text
forge commands
```

**Arguments and flags**

This command takes no arguments. The output is always JSON.

**Examples**

```sh
# The names of all top-level commands
forge commands | jq -r '.result.subcommands[].name'
```

**Output**

```json
{
  "schema": "forge.cli/v1",
  "result": {
    "name": "forge",
    "about": "Orbyn Forge CLI",
    "arguments": [
      { "name": "context", "long": "context", "short": "C", "positional": false, "required": false, "global": true,
        "help": "Context (connection profile) to use for this command only.", "values": [] }
    ],
    "subcommands": [
      { "name": "contexts", "about": "Manage local connection profiles (contexts).", "arguments": [], "subcommands": [ "…" ] }
    ]
  }
}
```

Exit codes: `0`.

## Old links

The command-line reference used to be one page. Links to its sections
still land here; each section moved:

| Old section | Now |
| --- | --- |
| <span id="forge"></span>`forge` | this page |
| <span id="installsh-mfa-options"></span>`install.sh MFA options` | [server-installer](/docs/reference/cli/server-installer/) |
| <span id="forge-server"></span>`forge-server` | [forge-server](/docs/reference/cli/forge-server/) |
| <span id="forge-agent"></span>`forge-agent` | [forge-agent](/docs/reference/cli/forge-agent/) |
| <span id="forge-agent-helper"></span>`forge-agent-helper` | [forge-agent-helper](/docs/reference/cli/forge-agent-helper/) |
| <span id="install-on-linux"></span>`Install on Linux` | [install-on-linux](#install-on-linux) |
| <span id="install-on-windows"></span>`Install on Windows` | [install-on-windows](#install-on-windows) |
| <span id="install-on-macos"></span>`Install on macOS` | [install](#install) (build from source) |
| <span id="update-the-cli"></span>`Update the CLI` | [update](/docs/reference/cli/update/) |
| <span id="retries"></span>`Retries` | [retries-and-idempotency](#retries-and-idempotency) |
| <span id="forge-contexts"></span>`forge contexts` | [contexts](/docs/reference/cli/contexts/) |
| <span id="forge-auth"></span>`forge auth` | [auth](/docs/reference/cli/auth/) |
| <span id="forge-nodes"></span>`forge nodes` | [nodes](/docs/reference/cli/nodes/) |
| <span id="forge-agents"></span>`forge agents` | [agents](/docs/reference/cli/agents/) |
| <span id="forge-nodes-files"></span>`forge nodes files` | [nodes-files](/docs/reference/cli/nodes-files/) |
| <span id="forge-jobs"></span>`forge jobs` | [jobs](/docs/reference/cli/jobs/) |
| <span id="forge-stacks"></span>`forge stacks` | [stacks](/docs/reference/cli/stacks/) |
| `forge projects` | [projects](/docs/reference/cli/projects/) |
| <span id="forge-apps"></span>`forge services` | [services](/docs/reference/cli/services/) |
| <span id="forge-audit"></span>`forge audit` | [audit](/docs/reference/cli/audit/) |
| <span id="forge-plugins"></span>`forge plugins` | [plugins](/docs/reference/cli/plugins/) |
| <span id="forge-approvals"></span>`forge approvals` | [approvals](/docs/reference/cli/approvals/) |
| <span id="forge-users"></span>`forge users` | [users](/docs/reference/cli/users/) |
| <span id="forge-access"></span>`forge access` | [access](/docs/reference/cli/access/) |
| <span id="forge-api-keys"></span>`forge api-keys` | [api-keys](/docs/reference/cli/api-keys/) |
| <span id="forge-service-accounts"></span>`forge service-accounts` | [service-accounts](/docs/reference/cli/service-accounts/) |
| <span id="forge-connectors"></span>`forge connectors` | [connectors](/docs/reference/cli/connectors/) |
| <span id="forge-settings"></span>`forge settings` | [settings](/docs/reference/cli/settings/) |
| <span id="forge-tenants-and-forge-environments"></span>`forge tenants and forge environments` | [tenants-environments](/docs/reference/cli/tenants-environments/) |
| <span id="forge-clusters"></span>`forge clusters` | [clusters](/docs/reference/cli/clusters/) |
| <span id="forge-sites-and-forge-certificates"></span>`forge sites and forge certificates` | [sites-certificates](/docs/reference/cli/sites-certificates/) |
| <span id="forge-tunnel"></span>`forge tunnel` | [tunnel](/docs/reference/cli/tunnel/) |
| <span id="forge-vault"></span>`forge vault` | [vault](/docs/reference/cli/vault/) |
| <span id="forge-system"></span>`forge system` | [system](/docs/reference/cli/system/) |
| <span id="forge-packages"></span>`forge packages` | [packages](/docs/reference/cli/packages/) |
| <span id="forge-sessions"></span>`forge sessions` | [sessions](/docs/reference/cli/sessions/) |
| <span id="forge-api"></span>`forge api` | [api](/docs/reference/cli/api/) |
| <span id="forge-parity"></span>`forge parity` | [parity](/docs/reference/cli/parity/) |
| <span id="fleet-sim"></span>`fleet-sim` | [fleet-sim](/docs/reference/cli/fleet-sim/) |
